//! What was used (plan R9): every screen opened and key pressed, one JSON line each, local only.
//! Read at the start of each design round with `--use-report`, so a round starts from what Alec
//! actually reached for rather than what he remembers reaching for.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
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

/// `$LOCUS_TUI_USE_LOG`, else `$XDG_STATE_HOME/locus/tui-use.jsonl`, else `~/.local/state/...`.
pub fn default_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("LOCUS_TUI_USE_LOG") {
        return Some(PathBuf::from(p));
    }
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(state.join("locus/tui-use.jsonl"))
}

/// Appends entries; a log that cannot be written is switched off, never fatal (R5: the window
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
        let ok = (|| -> Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let mut f = OpenOptions::new().create(true).append(true).open(path)?;
            writeln!(f, "{}", serde_json::to_string(&entry)?)?;
            Ok(())
        })();
        if ok.is_err() {
            self.path = None;
        }
    }
}

/// Counts per screen, and per key within each screen, most used first.
pub fn report(text: &str) -> String {
    let mut screens: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    let mut bad = 0;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        match serde_json::from_str::<Entry>(line) {
            Ok(e) => *screens.entry(e.screen).or_default().entry(e.key).or_default() += 1,
            Err(_) => bad += 1,
        }
    }
    let mut rows: Vec<_> = screens.into_iter().collect();
    rows.sort_by_key(|(_, keys)| std::cmp::Reverse(keys.values().sum::<usize>()));
    let mut out = String::new();
    for (screen, keys) in rows {
        let opens = keys.get("open").copied().unwrap_or(0);
        let total: usize = keys.values().sum();
        out.push_str(&format!("{screen:<10} opened {opens:>4}   keys {:>5}\n", total - opens));
        let mut ks: Vec<_> = keys.into_iter().filter(|(k, _)| k != "open").collect();
        ks.sort_by_key(|(k, n)| (std::cmp::Reverse(*n), k.clone()));
        for (k, n) in ks {
            out.push_str(&format!("    {k:<8} {n:>5}\n"));
        }
    }
    if out.is_empty() {
        out.push_str("no use recorded yet\n");
    }
    if bad > 0 {
        out.push_str(&format!("({bad} unreadable line(s) skipped)\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_reports() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/use.jsonl");
        let mut log = UseLog::new(Some(path.clone()));
        log.record("home", "open");
        log.record("home", "p");
        log.record("home", "p");
        log.record("project", "open");
        let r = report(&std::fs::read_to_string(&path).unwrap());
        assert!(r.starts_with("home       opened    1   keys     2"), "{r}");
        assert!(r.contains("    p            2"), "{r}");
    }

    #[test]
    fn report_survives_garbage() {
        assert!(report("not json\n").contains("1 unreadable"));
    }
}
