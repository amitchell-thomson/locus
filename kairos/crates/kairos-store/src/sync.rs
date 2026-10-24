//! Running the configured full sync (`sync.full_cmd`), shared by `kairos sync`,
//! the TUI and kairosd's sync job.
//!
//! vdirsyncer refuses to sync a collection whose storage was "completely
//! emptied" since the last run. When the emptied side is iCloud
//! (`<pair>_remote`), iCloud is the source of truth and following it only
//! deletes local copies, so we retry that one collection with
//! `--force-delete`. A *locally* emptied collection is left alone: that would
//! delete from iCloud, and only kairosd's own writes may decide that.

use chrono::{DateTime, Utc};
use kairos_core::config::Config;
use std::path::PathBuf;

#[derive(Debug, Default)]
pub struct FullSync {
    pub ok: bool,
    /// Last stderr line of a failed run.
    pub error: Option<String>,
    /// `pair/collection`s that were emptied on iCloud and followed.
    pub followed_empty: Vec<String>,
}

/// `pair/collection`s whose remote side vdirsyncer reports as emptied.
pub fn remote_emptied(stderr: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in stderr.lines() {
        let Some(rest) = line.split("Storage \"").nth(1) else {
            continue;
        };
        let Some((storage, tail)) = rest.split_once('"') else {
            continue;
        };
        if !tail.trim_start().starts_with("was completely emptied") {
            continue;
        }
        let Some((name, coll)) = storage.split_once('/') else {
            continue;
        };
        let Some(pair) = name.strip_suffix("_remote") else {
            continue;
        };
        let pc = format!("{pair}/{coll}");
        if !out.contains(&pc) {
            out.push(pc);
        }
    }
    out
}

fn stamp_path(cfg: &Config) -> PathBuf {
    cfg.data_dir().join("last-sync")
}

/// When `run_full` last succeeded (calendars that just haven't changed are
/// not stale; a sync that hasn't succeeded is).
pub fn last_success(cfg: &Config) -> Option<DateTime<Utc>> {
    let s = std::fs::read_to_string(stamp_path(cfg)).ok()?;
    DateTime::parse_from_rfc3339(s.trim())
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

fn record_success(cfg: &Config) {
    let p = stamp_path(cfg);
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(p, Utc::now().to_rfc3339());
}

fn sh(cmd: &str) -> std::io::Result<std::process::Output> {
    std::process::Command::new("sh").arg("-c").arg(cmd).output()
}

fn last_line(b: &[u8]) -> String {
    String::from_utf8_lossy(b).lines().last().unwrap_or("").to_string()
}

/// Fetch kairosd's sidecar (e.g. Family `blocking` flags) into the local
/// copy the cache reads. Writes only when it changed, so the cache isn't
/// rebuilt for nothing.
pub fn pull_sidecar(cfg: &Config) -> bool {
    let Ok(d) = crate::client::Daemon::from_config(cfg) else {
        return false;
    };
    let Ok(Ok(rows)) = d.sidecar() else {
        return false;
    };
    let side: crate::load::Sidecar = rows.into_iter().map(|(s, u, m)| ((s, u), m)).collect();
    if crate::load::read_sidecar_json(cfg) == side {
        return true;
    }
    crate::load::write_sidecar_json(cfg, &side).is_ok()
}

pub fn run_full(cfg: &Config) -> FullSync {
    let cmd = cfg.sync.full_cmd.trim();
    if cmd.is_empty() {
        return FullSync {
            ok: true,
            ..Default::default()
        };
    }
    let mut res = FullSync::default();
    match sh(cmd) {
        Ok(o) if o.status.success() => res.ok = true,
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr).into_owned();
            let emptied = remote_emptied(&stderr);
            let per = cfg.sync.collection_cmd.trim();
            if emptied.is_empty() || per.is_empty() {
                res.error = Some(last_line(&o.stderr));
                return res;
            }
            for pc in emptied {
                let c = per.replace("{collection}", &format!("--force-delete {pc}"));
                if matches!(sh(&c), Ok(o) if o.status.success()) {
                    res.followed_empty.push(pc);
                }
            }
            // Re-run so the rest of the command (e.g. metasync) happens too.
            match sh(cmd) {
                Ok(o) if o.status.success() => res.ok = true,
                Ok(o) => res.error = Some(last_line(&o.stderr)),
                Err(e) => res.error = Some(e.to_string()),
            }
        }
        Err(e) => res.error = Some(e.to_string()),
    }
    if res.ok {
        record_success(cfg);
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_remote_emptied_collections_are_followed() {
        let stderr = "Syncing icloud/AAA\n\
error: icloud/BBB: Storage \"icloud_remote/BBB\" was completely emptied. If you want to delete ALL entries on BOTH sides,then use `vdirsyncer sync --force-delete icloud/BBB`.\n\
error: icloud/CCC: Storage \"icloud_local/CCC\" was completely emptied. If you want …\n\
error: icloud/BBB: Storage \"icloud_remote/BBB\" was completely emptied.\n\
error: Unknown error occurred: 401\n";
        assert_eq!(remote_emptied(stderr), vec!["icloud/BBB".to_string()]);
        assert!(remote_emptied("").is_empty());
    }
}
