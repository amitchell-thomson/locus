//! vdirsyncer `filesystem` storage: `<root>/<collection>/<item>.ics`, with
//! optional `displayname` / `color` metadata files (vdirsyncer `metasync`).

use crate::{Result, StoreError, io_err};
use kairos_core::config::{Config, Source, SourceKind};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collection {
    pub dir: PathBuf,
    /// Directory name (vdirsyncer collection id; iCloud uses opaque ids).
    pub name: String,
    pub displayname: Option<String>,
    pub colour: Option<String>,
}

fn read_trim(p: &Path) -> Option<String> {
    fs::read_to_string(p)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// All collections under `root` (directories, one level deep, recursing one
/// more level for `<pair>/<collection>` layouts).
pub fn collections(root: &Path) -> Result<Vec<Collection>> {
    let mut out = Vec::new();
    let rd = match fs::read_dir(root) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(io_err(root)(e)),
    };
    let mut dirs: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    for d in dirs {
        let name = d
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.starts_with('.') {
            continue;
        }
        let has_items = fs::read_dir(&d)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .any(|e| e.path().extension().is_some_and(|x| x == "ics"))
            })
            .unwrap_or(false);
        let has_meta = d.join("displayname").exists();
        if has_items || has_meta {
            out.push(Collection {
                displayname: read_trim(&d.join("displayname")),
                colour: read_trim(&d.join("color")),
                name,
                dir: d,
            });
        } else {
            // Possibly a pair directory: look one level down.
            out.extend(
                collections(&d)?
                    .into_iter()
                    .filter(|c| c.dir.parent() == Some(d.as_path())),
            );
        }
    }
    Ok(out)
}

/// Directory holding an iCloud source's items.
pub fn resolve(cfg: &Config, src: &Source, cols: &[Collection]) -> Result<PathBuf> {
    if let Some(p) = &src.path {
        return Ok(cfg.vdir().join(p));
    }
    let want = src.calendar.as_deref().unwrap_or(&src.id);
    cols.iter()
        .find(|c| c.displayname.as_deref().is_some_and(|d| d.eq_ignore_ascii_case(want)))
        .or_else(|| cols.iter().find(|c| c.name.eq_ignore_ascii_case(want)))
        .map(|c| c.dir.clone())
        .ok_or_else(|| {
            StoreError::Missing(format!(
                "source {:?}: no vdir collection named {want:?} under {} (has `vdirsyncer sync` run?)",
                src.id,
                cfg.vdir().display()
            ))
        })
}

/// Where a read-only feed is cached.
pub fn feed_path(cfg: &Config, source_id: &str) -> PathBuf {
    cfg.data_dir().join("feeds").join(format!("{source_id}.ics"))
}

/// Path of a source's data (dir for iCloud, file for feeds).
pub fn source_path(cfg: &Config, src: &Source, cols: &[Collection]) -> Result<PathBuf> {
    match src.kind {
        SourceKind::Icloud => resolve(cfg, src, cols),
        SourceKind::IcsReadonly => Ok(feed_path(cfg, &src.id)),
    }
}

/// A safe file name for a UID.
pub fn href_for_uid(uid: &str) -> String {
    let safe: String = uid
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.@".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{safe}.ics")
}

pub fn hash(content: &str) -> String {
    hex::encode(Sha256::digest(content.as_bytes()))
}

pub fn read_item(dir: &Path, href: &str) -> Result<String> {
    let p = item_path(dir, href)?;
    fs::read_to_string(&p).map_err(io_err(&p))
}

fn item_path(dir: &Path, href: &str) -> Result<PathBuf> {
    if href.contains('/') || href.contains("..") || href.is_empty() {
        return Err(StoreError::Missing(format!("refusing suspicious href {href:?}")));
    }
    Ok(dir.join(href))
}

/// Atomic write (temp file + rename), so vdirsyncer never sees a torn file.
pub fn write_item(dir: &Path, href: &str, content: &str) -> Result<()> {
    let p = item_path(dir, href)?;
    fs::create_dir_all(dir).map_err(io_err(dir))?;
    let tmp = dir.join(format!(".{href}.kairos-tmp"));
    {
        let mut f = fs::File::create(&tmp).map_err(io_err(&tmp))?;
        f.write_all(content.as_bytes()).map_err(io_err(&tmp))?;
        f.sync_all().map_err(io_err(&tmp))?;
    }
    fs::rename(&tmp, &p).map_err(io_err(&p))
}

pub fn delete_item(dir: &Path, href: &str) -> Result<()> {
    let p = item_path(dir, href)?;
    match fs::remove_file(&p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_err(&p)(e)),
    }
}

/// Modification time in nanoseconds since the epoch (0 if missing).
pub fn mtime_ns(p: &Path) -> i64 {
    fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as i64)
}

/// Newest mtime of any item in `dir` (for sync-freshness checks).
pub fn newest_mtime(dir: &Path) -> Option<SystemTime> {
    fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter_map(|e| e.metadata().ok()?.modified().ok())
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_collections_with_displaynames() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("A1B2-opaque");
        fs::create_dir_all(&a).unwrap();
        fs::write(a.join("displayname"), "Sport\n").unwrap();
        fs::write(a.join("color"), "#a6e3a1").unwrap();
        let b = t.path().join("icloud").join("home");
        fs::create_dir_all(&b).unwrap();
        fs::write(b.join("x.ics"), "BEGIN:VCALENDAR\nEND:VCALENDAR\n").unwrap();
        let cols = collections(t.path()).unwrap();
        assert_eq!(cols.len(), 2, "{cols:?}");
        assert_eq!(cols[0].displayname.as_deref(), Some("Sport"));
        assert_eq!(cols[0].colour.as_deref(), Some("#a6e3a1"));
        assert_eq!(cols[1].name, "home");
    }

    #[test]
    fn atomic_write_read_delete() {
        let t = tempfile::tempdir().unwrap();
        write_item(t.path(), "a@b.ics", "hello").unwrap();
        assert_eq!(read_item(t.path(), "a@b.ics").unwrap(), "hello");
        write_item(t.path(), "a@b.ics", "bye").unwrap();
        assert_eq!(read_item(t.path(), "a@b.ics").unwrap(), "bye");
        delete_item(t.path(), "a@b.ics").unwrap();
        delete_item(t.path(), "a@b.ics").unwrap(); // idempotent
        assert!(read_item(t.path(), "a@b.ics").is_err());
        assert!(write_item(t.path(), "../escape.ics", "x").is_err());
    }

    #[test]
    fn hrefs_are_safe() {
        assert_eq!(href_for_uid("abc/def:1@x"), "abc_def_1@x.ics");
    }
}
