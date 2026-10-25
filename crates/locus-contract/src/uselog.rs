//! What was used (plan R9): every screen opened and key pressed, one JSON line each, local only,
//! in one log for every app in the locus window (L8: kairos writes here too, its screens named
//! `calendar:week`, `calendar:day`, …). Read at the start of each design round with
//! `locus-tui --use-report`, so a round starts from what was actually reached for.

use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub ts: String,
    pub screen: String,
    /// The key pressed, or `open` when the screen was opened.
    pub key: String,
}

/// `$LOCUS_TUI_USE_LOG`, else `tui-use.jsonl` in locus's state directory.
pub fn default_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("LOCUS_TUI_USE_LOG") {
        return Some(PathBuf::from(p));
    }
    crate::link::state_dir().map(|d| d.join("tui-use.jsonl"))
}

/// Appends entries; a log that cannot be written is switched off, never fatal (R5: a window
/// must not die over its own bookkeeping).
#[derive(Debug, Default)]
pub struct UseLog {
    path: Option<PathBuf>,
}

impl UseLog {
    pub fn new(path: Option<PathBuf>) -> UseLog {
        UseLog { path }
    }

    pub fn off() -> UseLog {
        UseLog { path: None }
    }

    pub fn record(&mut self, screen: &str, key: &str) {
        let Some(path) = &self.path else { return };
        let entry = Entry {
            ts: chrono::Local::now().to_rfc3339(),
            screen: screen.into(),
            key: key.into(),
        };
        let ok = (|| -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let mut f = OpenOptions::new().create(true).append(true).open(path)?;
            writeln!(f, "{}", serde_json::to_string(&entry).map_err(std::io::Error::other)?)?;
            Ok(())
        })();
        if ok.is_err() {
            self.path = None;
        }
    }
}
