//! The Project view follows the project window (plan B.4): a tmux hook writes the session that
//! window switched to into `<state>/focus`, and the view changes project to match.
//!
//! "It never moves you between views, never switches while you are scrolling or typing in it,
//! and `m` pins it." Typing is read as a key pressed in the last `QUIET`, or a detail screen
//! open on top of the view: switching project under an open Decisions list would change what
//! the list is about mid-read. A switch that arrives then is kept and applied once it is quiet,
//! so the view never ends up on a project the project window has already left.

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

/// How long after a key the view holds still.
pub const QUIET: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Follow {
    /// The session the view is showing.
    pub session: Option<String>,
    /// A session the project window moved to while the view was busy.
    pub waiting: Option<String>,
    pub pinned: bool,
}

impl Follow {
    pub fn new(session: Option<String>) -> Follow {
        Follow {
            session,
            waiting: None,
            pinned: false,
        }
    }

    /// The project window is now on `session`. Switch, or keep it for later.
    pub fn offer(&mut self, session: &str, busy: bool) {
        if self.session.as_deref() == Some(session) {
            self.waiting = None;
        } else if self.pinned || busy {
            self.waiting = Some(session.to_string());
        } else {
            self.session = Some(session.to_string());
            self.waiting = None;
        }
    }

    /// Called each tick: apply a waiting switch once nothing holds it back.
    pub fn settle(&mut self, busy: bool) {
        if !self.pinned
            && !busy
            && let Some(s) = self.waiting.take()
        {
            self.session = Some(s);
        }
    }

    /// `m`: pin to the current project, or let go and catch up.
    pub fn toggle_pin(&mut self) {
        self.pinned = !self.pinned;
    }
}

/// Busy = a key in the last `QUIET`, or a detail screen open.
pub fn busy(last_key: Option<Instant>, now: Instant, detail_open: bool) -> bool {
    detail_open || last_key.is_some_and(|k| now.duration_since(k) < QUIET)
}

/// `$LOCUS_STATE_DIR`, else `$XDG_STATE_HOME/locus`, else `~/.local/state/locus`. The tmux
/// snippet's `@locus-state-dir` must name the same place.
pub fn state_dir() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("LOCUS_STATE_DIR") {
        return Some(PathBuf::from(p));
    }
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(state.join("locus"))
}

/// Reads `<state>/focus` when it changes. A missing or unreadable file means "no news", never
/// an error: the view keeps what it shows.
#[derive(Debug)]
pub struct FocusFile {
    path: Option<PathBuf>,
    seen: Option<SystemTime>,
}

impl FocusFile {
    pub fn new(dir: Option<PathBuf>) -> FocusFile {
        FocusFile {
            path: dir.map(|d| d.join("focus")),
            seen: None,
        }
    }

    /// The session named in the file, if it changed since the last call.
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_when_idle() {
        let mut f = Follow::new(Some("quast".into()));
        f.offer("nabla", false);
        assert_eq!(f.session.as_deref(), Some("nabla"));
    }

    #[test]
    fn busy_defers_then_catches_up() {
        let mut f = Follow::new(Some("quast".into()));
        f.offer("nabla", true);
        assert_eq!(f.session.as_deref(), Some("quast"));
        f.settle(true);
        assert_eq!(f.session.as_deref(), Some("quast"));
        f.settle(false);
        assert_eq!(f.session.as_deref(), Some("nabla"));
    }

    #[test]
    fn pinned_holds_until_released() {
        let mut f = Follow::new(Some("quast".into()));
        f.toggle_pin();
        f.offer("nabla", false);
        f.settle(false);
        assert_eq!(f.session.as_deref(), Some("quast"));
        f.toggle_pin();
        f.settle(false);
        assert_eq!(f.session.as_deref(), Some("nabla"));
    }

    #[test]
    fn going_back_cancels_a_waiting_switch() {
        let mut f = Follow::new(Some("quast".into()));
        f.offer("nabla", true);
        f.offer("quast", true);
        f.settle(false);
        assert_eq!(f.session.as_deref(), Some("quast"));
    }

    #[test]
    fn busy_window() {
        let t0 = Instant::now();
        assert!(busy(Some(t0), t0 + Duration::from_secs(1), false));
        assert!(!busy(Some(t0), t0 + QUIET, false));
        assert!(busy(None, t0, true));
    }

    #[test]
    fn focus_file_reports_changes_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut ff = FocusFile::new(Some(dir.path().to_path_buf()));
        assert_eq!(ff.poll(), None, "no file yet");
        std::fs::write(dir.path().join("focus"), "nabla\n").unwrap();
        assert_eq!(ff.poll().as_deref(), Some("nabla"));
        assert_eq!(ff.poll(), None, "unchanged");
    }
}
