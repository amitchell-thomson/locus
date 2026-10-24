//! Load configured sources from disk.

use crate::vdir::{self, Collection};
use crate::{Result, io_err};
use kairos_core::config::{Config, Source, SourceKind};
use kairos_core::ics;
use kairos_core::model::{EventSeries, Meta};
use kairos_core::normalize::{Warning, series_from_calendars};
use kairos_core::view::LoadedSource;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Sidecar metadata keyed by `(source id, uid)`.
pub type Sidecar = HashMap<(String, String), Meta>;

#[derive(Debug, Default)]
pub struct Loaded {
    pub sources: Vec<LoadedSource>,
    pub warnings: Vec<Warning>,
    /// Sources that couldn't be found (e.g. not synced yet), with reasons.
    pub missing: Vec<(String, String)>,
}

fn parse_file(path: &Path, source: &str, href: Option<&str>, warnings: &mut Vec<Warning>) -> Vec<EventSeries> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            warnings.push(Warning {
                source: source.into(),
                uid: None,
                message: format!("{}: {e}", path.display()),
            });
            return Vec::new();
        }
    };
    match ics::parse(&text) {
        Ok(cals) => series_from_calendars(&cals, source, href, warnings),
        Err(e) => {
            warnings.push(Warning {
                source: source.into(),
                uid: None,
                message: format!("{}: {e}", path.display()),
            });
            Vec::new()
        }
    }
}

/// Parse every `.ics` in a collection directory, in parallel.
pub fn load_dir(dir: &Path, source: &str) -> Result<(Vec<EventSeries>, Vec<Warning>)> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(io_err(dir))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "ics"))
        .collect();
    files.sort();
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    let chunk = files.len().div_ceil(threads).max(64);
    let results: Vec<(Vec<EventSeries>, Vec<Warning>)> = std::thread::scope(|s| {
        let handles: Vec<_> = files
            .chunks(chunk)
            .map(|part| {
                s.spawn(move || {
                    let mut w = Vec::new();
                    let mut v = Vec::new();
                    for p in part {
                        let href = p.file_name().map(|n| n.to_string_lossy().into_owned());
                        v.extend(parse_file(p, source, href.as_deref(), &mut w));
                    }
                    (v, w)
                })
            })
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    });
    let mut series = Vec::new();
    let mut warnings = Vec::new();
    for (s, w) in results {
        series.extend(s);
        warnings.extend(w);
    }
    Ok((series, warnings))
}

fn apply_sidecar(series: &mut [EventSeries], source: &str, sidecar: &Sidecar) {
    if sidecar.is_empty() {
        return;
    }
    for s in series {
        if let Some(m) = sidecar.get(&(source.to_string(), s.uid.clone())) {
            if let Some(master) = &mut s.master {
                master.meta = master.meta.merged_with(m);
            }
            for o in &mut s.overrides {
                o.meta = o.meta.merged_with(m);
            }
        }
    }
}

pub fn load_source(
    cfg: &Config,
    src: &Source,
    cols: &[Collection],
    sidecar: &Sidecar,
) -> Result<(LoadedSource, Vec<Warning>)> {
    let path = vdir::source_path(cfg, src, cols)?;
    let (mut series, warnings) = match src.kind {
        SourceKind::Icloud => load_dir(&path, &src.id)?,
        SourceKind::IcsReadonly => {
            if !path.exists() {
                return Err(crate::StoreError::Missing(format!(
                    "feed {:?} not fetched yet (run `kairos sync`)",
                    src.id
                )));
            }
            let mut w = Vec::new();
            let s = parse_file(&path, &src.id, None, &mut w);
            (s, w)
        }
    };
    apply_sidecar(&mut series, &src.id, sidecar);
    Ok((
        LoadedSource {
            id: src.id.clone(),
            kind: src.kind,
            overlay: src.overlay,
            key: src.key.clone(),
            series,
        },
        warnings,
    ))
}

/// Load every configured source; missing ones are reported, not fatal.
pub fn load_all(cfg: &Config, sidecar: &Sidecar) -> Result<Loaded> {
    let cols = vdir::collections(&cfg.vdir())?;
    let mut out = Loaded::default();
    for src in &cfg.sources {
        match load_source(cfg, src, &cols, sidecar) {
            Ok((ls, w)) => {
                out.sources.push(ls);
                out.warnings.extend(w);
            }
            Err(e) => out.missing.push((src.id.clone(), e.to_string())),
        }
    }
    Ok(out)
}

/// Fingerprint of everything a view depends on: source dirs/files and the
/// sidecar cache. vdirsyncer writes atomically, so directory mtimes change on
/// every add/update/delete.
pub fn fingerprint(cfg: &Config, config_path: Option<&Path>) -> Vec<(String, i64)> {
    let cols = vdir::collections(&cfg.vdir()).unwrap_or_default();
    let mut v = Vec::new();
    for src in &cfg.sources {
        let p = vdir::source_path(cfg, src, &cols).ok();
        v.push((src.id.clone(), p.map_or(0, |p| vdir::mtime_ns(&p))));
    }
    v.push(("~sidecar".into(), vdir::mtime_ns(&cfg.data_dir().join("sidecar.json"))));
    if let Some(c) = config_path {
        v.push(("~config".into(), vdir::mtime_ns(c)));
    }
    v
}

/// Client-side sidecar copy (fetched from kairosd by `kairos sync`).
pub fn read_sidecar_json(cfg: &Config) -> Sidecar {
    let p = cfg.data_dir().join("sidecar.json");
    let Ok(text) = fs::read_to_string(&p) else {
        return Sidecar::new();
    };
    let rows: Vec<(String, String, Meta)> = serde_json::from_str(&text).unwrap_or_default();
    rows.into_iter().map(|(s, u, m)| ((s, u), m)).collect()
}

pub fn write_sidecar_json(cfg: &Config, sidecar: &Sidecar) -> Result<()> {
    let mut rows: Vec<(&String, &String, &Meta)> = sidecar.iter().map(|((s, u), m)| (s, u, m)).collect();
    rows.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
    let dir = cfg.data_dir();
    fs::create_dir_all(&dir).map_err(io_err(&dir))?;
    let text = serde_json::to_string(&rows)?;
    vdir::write_item(&dir, "sidecar.json", &text)
}
