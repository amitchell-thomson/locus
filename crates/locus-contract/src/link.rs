//! Opening one app at an item of the other (ADR-008, spec K8): kairos and the locus window ask
//! each other through a small file in locus's state directory, the way the Project window learns
//! what to show (`follow.rs`'s peek file), and bring the other's tmux session forward. Each app
//! polls its own file; a request written before the app started is ignored as stale.
//!
//! `open-locus` holds a registry id (`millennium`): the locus window opens it, an application on
//! its own page, a repo in the Project window. `open-kairos` holds a day and optionally an event
//! uid (`2026-10-16 abc@kairos`): kairos opens its day view there, the event selected.

use chrono::NaiveDate;
use std::path::PathBuf;
use std::time::SystemTime;

/// locus's state directory: `$LOCUS_STATE_DIR`, else `$XDG_STATE_HOME/locus`, else
/// `~/.local/state/locus` (the same rule as the locus window's focus and peek files).
pub fn state_dir() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("LOCUS_STATE_DIR") {
        return Some(PathBuf::from(p));
    }
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(state.join("locus"))
}

/// One app's inbox of open requests.
#[derive(Debug)]
pub struct OpenFile {
    path: Option<PathBuf>,
    seen: Option<SystemTime>,
}

impl OpenFile {
    /// `name` is `open-locus` or `open-kairos`. What is already there counts as seen.
    pub fn new(dir: Option<PathBuf>, name: &str) -> OpenFile {
        let path = dir.map(|d| d.join(name));
        let seen = path
            .as_ref()
            .and_then(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
        OpenFile { path, seen }
    }

    pub fn locus() -> OpenFile {
        OpenFile::new(state_dir(), "open-locus")
    }

    pub fn kairos() -> OpenFile {
        OpenFile::new(state_dir(), "open-kairos")
    }

    /// The request, if one arrived since the last call.
    pub fn poll(&mut self) -> Option<String> {
        let path = self.path.as_ref()?;
        let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
        if self.seen == Some(modified) {
            return None;
        }
        self.seen = Some(modified);
        let text = std::fs::read_to_string(path).ok()?;
        let s = text.trim();
        (!s.is_empty()).then(|| s.to_string())
    }

    /// Ask, written whole so the reader never sees half a request.
    pub fn write(&self, request: &str) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Err(std::io::Error::other("no state directory"));
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, request)?;
        std::fs::rename(&tmp, path)
    }
}

/// A day (and maybe an event) for kairos to open: `2026-10-16` or `2026-10-16 uid`.
pub fn kairos_request(day: NaiveDate, uid: Option<&str>) -> String {
    match uid {
        Some(u) => format!("{day} {u}"),
        None => day.to_string(),
    }
}

pub fn parse_kairos_request(s: &str) -> Option<(NaiveDate, Option<String>)> {
    let mut it = s.split_whitespace();
    let day = it.next()?.parse().ok()?;
    Some((day, it.next().map(String::from)))
}

/// Bring a tmux session forward on this client (`locus`, `kairos`). False outside tmux or when
/// there is no such session: the request still waits in the file for when it is opened.
pub fn switch_to(session: &str) -> bool {
    std::env::var_os("TMUX").is_some()
        && std::process::Command::new("tmux")
            .args(["switch-client", "-t", &format!("={session}")])
            .status()
            .is_ok_and(|s| s.success())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn a_request_arrives_once_and_a_stale_one_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("open-locus"), "jump").unwrap();
        let mut inbox = OpenFile::new(Some(dir.path().to_path_buf()), "open-locus");
        assert_eq!(inbox.poll(), None, "left from before the app started");
        std::thread::sleep(std::time::Duration::from_millis(20));
        OpenFile::new(Some(dir.path().to_path_buf()), "open-locus")
            .write("millennium")
            .unwrap();
        assert_eq!(inbox.poll().as_deref(), Some("millennium"));
        assert_eq!(inbox.poll(), None);
    }

    #[test]
    fn kairos_requests_round_trip() {
        let d = NaiveDate::from_ymd_opt(2026, 10, 16).unwrap();
        assert_eq!(
            parse_kairos_request(&kairos_request(d, Some("u@kairos"))),
            Some((d, Some("u@kairos".into())))
        );
        assert_eq!(parse_kairos_request("2026-10-16"), Some((d, None)));
        assert_eq!(parse_kairos_request("nonsense"), None);
    }
}
