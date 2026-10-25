//! What was used (plan R9): the log itself is locus-contract's, shared with kairos (L8); this
//! is the report read at the start of each design round (`--use-report`).

pub use locus_contract::uselog::{Entry, UseLog, default_path};
use std::collections::BTreeMap;

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
