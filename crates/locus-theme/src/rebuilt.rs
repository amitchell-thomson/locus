//! A rebuilt binary takes over its window in place (cleanup C7, 13 Nov 2026).
//!
//! Every window ran the build it started with until it was restarted by hand, which is how
//! Approve once showed only mail after a release that added facts, and how a fix was "deployed"
//! while the screen still ran the old code. Each app keeps a [`Rebuilt`], asks [`Rebuilt::ready`]
//! once a loop, and when it says yes and nothing is being typed (the app decides what that
//! means: no form, no popup, no text input, no key for [`IDLE`]) restores the terminal and calls
//! [`reexec`] with whatever it needs to come back on the same screen.
//!
//! The file must have changed and then stayed unchanged for [`SETTLE`], so a build still being
//! written is never exec'd half-done. First used by Home (68e3846), then kairos; keryx can take
//! the same.

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

/// How long the new binary must be left alone before it is trusted to be whole.
pub const SETTLE: Duration = Duration::from_secs(2);

/// How long since the last key before a window counts as idle enough to restart.
pub const IDLE: Duration = Duration::from_secs(10);

/// Watches this process's own binary for a rebuild.
#[derive(Debug)]
pub struct Rebuilt {
    path: Option<PathBuf>,
    started: Option<SystemTime>,
    seen: Option<(SystemTime, Instant)>,
    checked: Instant,
}

impl Default for Rebuilt {
    fn default() -> Self {
        Self::new()
    }
}

impl Rebuilt {
    /// The running binary as it is now: a later change to it is a rebuild.
    pub fn new() -> Self {
        let path = std::env::current_exe().ok();
        let started = path.as_ref().and_then(|p| std::fs::metadata(p).ok()?.modified().ok());
        Rebuilt {
            path,
            started,
            seen: None,
            checked: Instant::now(),
        }
    }

    /// True once the binary has changed and then rested for [`SETTLE`]. Looks at the file at most
    /// once a second, so it is cheap to ask every frame.
    pub fn ready(&mut self) -> bool {
        if self.checked.elapsed() < Duration::from_secs(1) {
            return false;
        }
        self.checked = Instant::now();
        let (Some(p), Some(start)) = (&self.path, self.started) else {
            return false;
        };
        let Some(m) = std::fs::metadata(p).ok().and_then(|m| m.modified().ok()) else {
            return false;
        };
        self.settled(start, m, Instant::now())
    }

    /// The decision, apart from the clock and the file: `m` is the binary's mtime now.
    fn settled(&mut self, start: SystemTime, m: SystemTime, now: Instant) -> bool {
        if m == start {
            return false;
        }
        match self.seen {
            Some((prev, since)) if prev == m => now.duration_since(since) >= SETTLE,
            _ => {
                self.seen = Some((m, now));
                false
            }
        }
    }
}

/// Exec this binary again with the same arguments and `env` added. Restore the terminal first.
/// Returns only when the exec failed, with why.
pub fn reexec(env: &[(&str, String)]) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => return e,
    };
    let mut cmd = std::process::Command::new(exe);
    cmd.args(std::env::args_os().skip(1));
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.exec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_counts_only_once_it_has_rested() {
        let t0 = SystemTime::UNIX_EPOCH;
        let t1 = t0 + Duration::from_secs(5);
        let t2 = t0 + Duration::from_secs(9);
        let mut r = Rebuilt {
            path: None,
            started: Some(t0),
            seen: None,
            checked: Instant::now(),
        };
        let now = Instant::now();
        assert!(!r.settled(t0, t0, now), "unchanged");
        assert!(!r.settled(t0, t1, now), "just changed");
        assert!(!r.settled(t0, t1, now + Duration::from_secs(1)), "still settling");
        assert!(
            !r.settled(t0, t2, now + Duration::from_secs(3)),
            "written again: starts over"
        );
        assert!(r.settled(t0, t2, now + Duration::from_secs(6)));
    }
}
