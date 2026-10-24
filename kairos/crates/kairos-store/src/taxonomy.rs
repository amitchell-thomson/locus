//! The locus taxonomy and registry, as kairos sees them (ADR-008, spec K3). On the server they are
//! read straight from the checkout; on the Mac from kairosd (`GET /api/taxonomy`), keeping the last
//! copy fetched in the cache directory for when kairosd cannot be reached, and failing that the
//! files in the Mac's own checkout. Never a fourth copy kept by hand.

use crate::client::Daemon;
use kairos_core::config::Config;
use locus_contract::taxonomy::{self, Shared};
use std::path::PathBuf;

/// Where the last copy fetched from kairosd is kept.
pub fn cache_path(cfg: &Config) -> PathBuf {
    cfg.cache_dir().join("taxonomy.json")
}

/// Where this copy came from, for `kairos doctor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum From {
    Daemon,
    Cache,
    Checkout,
}

/// The taxonomy: kairosd's, else the cached copy, else this checkout's files.
pub fn get(cfg: &Config) -> Option<(Shared, From)> {
    if let Ok(d) = Daemon::from_config(cfg)
        && let Ok(Ok(s)) = d.get::<Shared>("/api/taxonomy")
    {
        if let Ok(text) = serde_json::to_string(&s) {
            let _ = std::fs::create_dir_all(cfg.cache_dir());
            let _ = std::fs::write(cache_path(cfg), text);
        }
        return Some((s, From::Daemon));
    }
    if let Ok(text) = std::fs::read_to_string(cache_path(cfg))
        && let Ok(s) = serde_json::from_str::<Shared>(&text)
    {
        return Some((s, From::Cache));
    }
    taxonomy::load(&taxonomy::root()).ok().map(|s| (s, From::Checkout))
}

/// The server's own read: the checkout's files, no network.
pub fn local() -> Result<Shared, taxonomy::Error> {
    taxonomy::load(&taxonomy::root())
}
