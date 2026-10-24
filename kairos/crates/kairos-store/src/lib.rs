//! `kairos-store`: everything that touches disk or the network for reading.
//!
//! - [`vdir`]: vdirsyncer collections (one `.ics` per item) and atomic writes;
//! - [`load`]: config sources → [`kairos_core::view::LoadedSource`];
//! - [`cache`]: the precomputed instance index behind `next`/`today`/TUI;
//! - [`db`]: SQLite (sidecar metadata, tasks, changesets, check-ins);
//! - [`feeds`]: read-only `.ics` URL fetcher with conditional GET;
//! - [`sync`]: running `sync.full_cmd` (follows collections emptied on iCloud).

pub mod cache;
pub mod client;
pub mod db;
pub mod feeds;
pub mod load;
pub mod sync;
pub mod vdir;

use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("database: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("iCalendar: {0}")]
    Ics(#[from] kairos_core::ics::IcsError),
    #[error("HTTP: {0}")]
    Http(String),
    #[error("{0}")]
    Missing(String),
    #[error("command failed: {0}")]
    Command(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

pub(crate) fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> StoreError + '_ {
    move |source| StoreError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Run a shell command (`*_cmd` config fields) and return trimmed stdout.
pub fn run_secret_cmd(cmd: &str) -> Result<String> {
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .output()
        .map_err(|e| StoreError::Command(format!("{cmd}: {e}")))?;
    if !out.status.success() {
        // Never echo stdout (it may be a secret); stderr is fine.
        return Err(StoreError::Command(format!(
            "{cmd}: exit {} {}",
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
