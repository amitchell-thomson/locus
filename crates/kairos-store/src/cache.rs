//! The instance index: expanded instances for a window around today, stored
//! as JSON and reused while the vdir is unchanged. This is what keeps
//! `kairos next --tmux` under 20 ms.

use crate::load::{self, Sidecar, fingerprint};
use crate::{Result, io_err, vdir};
use chrono::{DateTime, Duration, NaiveTime, Utc};
use kairos_core::config::{Config, Overlay, SourceKind};
use kairos_core::model::Instance;
use kairos_core::time::resolve_local;
use kairos_core::view;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const VERSION: u32 = 1;
/// Days before today kept in the index.
pub const BACK_DAYS: i64 = 14;
/// Days after today kept in the index.
pub const AHEAD_DAYS: i64 = 56;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SourceSummary {
    pub id: String,
    pub kind: SourceKind,
    pub overlay: Overlay,
    pub key: Option<String>,
    pub events: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Index {
    pub version: u32,
    pub fingerprint: Vec<(String, i64)>,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub built_at: DateTime<Utc>,
    pub sources: Vec<SourceSummary>,
    pub missing: Vec<(String, String)>,
    pub warnings: Vec<String>,
    /// Sorted by start.
    pub instances: Vec<Instance>,
}

impl Index {
    pub fn kind_of(&self, id: &str) -> Option<SourceKind> {
        self.sources.iter().find(|s| s.id == id).map(|s| s.kind)
    }

    pub fn covers(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> bool {
        self.from <= from && to <= self.to
    }

    /// Instances overlapping `[from, to)`.
    pub fn between(&self, from: DateTime<Utc>, to: DateTime<Utc>, tz: chrono_tz::Tz) -> Vec<Instance> {
        self.instances
            .iter()
            .filter(|i| {
                let (s, e) = i.span.bounds(tz);
                if s == e {
                    s >= from && s < to
                } else {
                    s < to && e > from
                }
            })
            .cloned()
            .collect()
    }
}

pub fn path(cfg: &Config) -> PathBuf {
    cfg.cache_dir().join("index.json")
}

/// Default window: local midnight `BACK_DAYS` ago to `AHEAD_DAYS` ahead.
pub fn default_window(cfg: &Config, now: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
    let tz = cfg.tz();
    let today = now.with_timezone(&tz).date_naive();
    let from = resolve_local((today - Duration::days(BACK_DAYS)).and_time(NaiveTime::MIN), tz);
    let to = resolve_local((today + Duration::days(AHEAD_DAYS)).and_time(NaiveTime::MIN), tz);
    (from, to)
}

/// Build an index for `[from, to)` from disk.
pub fn build(
    cfg: &Config,
    config_path: Option<&Path>,
    sidecar: &Sidecar,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<Index> {
    let fp = fingerprint(cfg, config_path);
    let loaded = load::load_all(cfg, sidecar)?;
    let mut warnings = loaded.warnings;
    let mut instances = view::instances(&loaded.sources, from, to, cfg.tz(), &mut warnings);
    let tz = cfg.tz();
    instances.sort_by(|a, b| a.start(tz).cmp(&b.start(tz)).then_with(|| a.title.cmp(&b.title)));
    Ok(Index {
        version: VERSION,
        fingerprint: fp,
        from,
        to,
        built_at: Utc::now(),
        sources: loaded
            .sources
            .iter()
            .map(|s| SourceSummary {
                id: s.id.clone(),
                kind: s.kind,
                overlay: s.overlay,
                key: s.key.clone(),
                events: s.series.len(),
            })
            .collect(),
        missing: loaded.missing,
        warnings: warnings
            .into_iter()
            .map(|w| {
                format!(
                    "{}{}: {}",
                    w.source,
                    w.uid.map(|u| format!(" {u}")).unwrap_or_default(),
                    w.message
                )
            })
            .collect(),
        instances,
    })
}

pub fn write(cfg: &Config, idx: &Index) -> Result<()> {
    let dir = cfg.cache_dir();
    fs::create_dir_all(&dir).map_err(io_err(&dir))?;
    let text = serde_json::to_string(idx)?;
    vdir::write_item(&dir, "index.json", &text)
}

pub fn read(cfg: &Config) -> Option<Index> {
    let text = fs::read_to_string(path(cfg)).ok()?;
    let idx: Index = serde_json::from_str(&text).ok()?;
    (idx.version == VERSION).then_some(idx)
}

/// A fresh index covering `[from, to)`: the cached one if the fingerprint
/// still matches, otherwise rebuilt (and saved when it's the default window).
pub fn get(
    cfg: &Config,
    config_path: Option<&Path>,
    now: DateTime<Utc>,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<(Index, bool)> {
    if let Some(idx) = read(cfg)
        && idx.covers(from, to)
        && idx.fingerprint == fingerprint(cfg, config_path)
    {
        return Ok((idx, false));
    }
    let sidecar = load::read_sidecar_json(cfg);
    let (dfrom, dto) = default_window(cfg, now);
    if dfrom <= from && to <= dto {
        let idx = build(cfg, config_path, &sidecar, dfrom, dto)?;
        // A failed cache write shouldn't fail the read.
        if let Err(e) = write(cfg, &idx) {
            tracing::warn!("cache write failed: {e}");
        }
        Ok((idx, true))
    } else {
        Ok((build(cfg, config_path, &sidecar, from, to)?, true))
    }
}
