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

/// Home asks the Project window to show a project for a moment (round 1, 21 Oct: "clicking on
/// projects should take you to the projects tab ... escape on the project tab should take you
/// back to home and set the project tab back to whatever it was before"). Home writes the id
/// to `<state>/peek` and switches to the Project window; the Project view shows it until Esc,
/// which empties the file and switches back. A file left from an earlier run is ignored.
pub struct PeekFile {
    path: Option<PathBuf>,
    seen: Option<SystemTime>,
}

impl PeekFile {
    pub fn new(dir: Option<PathBuf>) -> PeekFile {
        let path = dir.map(|d| d.join("peek"));
        let seen = path
            .as_ref()
            .and_then(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
        PeekFile { path, seen }
    }

    /// `Some(change)` when the file changed: the project to show, or `None` to stop showing it.
    pub fn poll(&mut self) -> Option<Option<String>> {
        let path = self.path.as_ref()?;
        let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
        if self.seen == Some(modified) {
            return None;
        }
        self.seen = Some(modified);
        let text = std::fs::read_to_string(path).ok()?;
        let s = text.trim();
        Some((!s.is_empty()).then(|| s.to_string()))
    }

    /// Ask for `id` (or stop asking), written whole so a reader never sees half a name.
    pub fn write(&self, id: Option<&str>) -> std::io::Result<()> {
        let Some(path) = &self.path else { return Ok(()) };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, id.unwrap_or(""))?;
        std::fs::rename(&tmp, path)
    }
}

/// Show a window of the locus session (`home`, `project`): the one real effect the prototype
/// has besides the focus file. False when not inside the locus session or tmux refused.
pub fn select_window(view: &str) -> bool {
    std::process::Command::new("tmux")
        .args(["select-window", "-t", &format!("=locus:={view}")])
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(test)]
mod peek_tests {
    use super::*;

    #[test]
    fn peek_round_trips_and_ignores_a_stale_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("peek"), "quast").unwrap();
        let mut reader = PeekFile::new(Some(dir.path().to_path_buf()));
        assert_eq!(reader.poll(), None, "left from an earlier run");
        std::thread::sleep(std::time::Duration::from_millis(20));
        let writer = PeekFile::new(Some(dir.path().to_path_buf()));
        writer.write(Some("nabla")).unwrap();
        assert_eq!(reader.poll(), Some(Some("nabla".into())));
        std::thread::sleep(std::time::Duration::from_millis(20));
        writer.write(None).unwrap();
        assert_eq!(reader.poll(), Some(None));
    }
}
