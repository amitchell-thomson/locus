//! Fixture sets: a fixed, realistic snapshot that every screen runs on until its backend exists
//! (plan D). A set is a directory of JSON files, one per contract, under
//! `crates/locus-tui/fixtures/<name>/`.

use anyhow::{Context, Result};
use locus_contract::{Manifest, Registry};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Fixtures {
    pub dir: PathBuf,
    pub manifest: Manifest,
    pub registry: Registry,
}

/// Where the repo's fixture sets live, found from the crate so it works from any directory.
pub fn sets_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

/// A set by name (`default`, `empty`) or by path.
pub fn resolve(set: &str) -> PathBuf {
    let p = Path::new(set);
    if p.is_dir() {
        p.to_path_buf()
    } else {
        sets_dir().join(set)
    }
}

impl Fixtures {
    pub fn load(dir: &Path) -> Result<Fixtures> {
        let manifest = read(dir, "manifest.json")?;
        let registry = read(dir, "registry.json")?;
        Ok(Fixtures {
            dir: dir.to_path_buf(),
            manifest,
            registry,
        })
    }
}

fn read<T: serde::de::DeserializeOwned>(dir: &Path, file: &str) -> Result<T> {
    let path = dir.join(file);
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}
