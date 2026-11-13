//! A rebuilt keryx takes over its window in place (cleanup C7, 13 Nov 2026).
//!
//! Ported from locus-tui's `Rebuilt` (`crates/locus-tui/src/main.rs`, 68e3846): every window ran
//! the build it started with until restarted by hand, so a fix was "deployed" and the screen still
//! ran the old code. The binary must have changed and then stayed unchanged for two seconds, so a
//! build still being written is never exec'd half-done; and the app must be idle ([`idle`]), so
//! nothing being typed or sent is lost. Kept small and local on purpose: locus-theme may grow a
//! shared one, and the two can be unified then.

use crate::app::{App, Mode};
use std::time::{Duration, Instant, SystemTime};

/// The thread to reopen after the restart (`account:thread`), handed from one build to the next.
pub const ENV_THREAD: &str = "KERYX_REEXEC_THREAD";

/// Watches this binary's file for a rebuild.
pub struct Rebuilt {
    path: Option<std::path::PathBuf>,
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

    /// The file changed since start and has been still for two seconds (checked once a second).
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
        stable(start, m, &mut self.seen, Instant::now())
    }
}

/// The rule, apart from the clock and the file: `m` differs from `start` and was the same at
/// the last look two seconds or more ago.
pub fn stable(start: SystemTime, m: SystemTime, seen: &mut Option<(SystemTime, Instant)>, now: Instant) -> bool {
    if m == start {
        return false;
    }
    match *seen {
        Some((prev, since)) if prev == m => now.duration_since(since) >= Duration::from_secs(2),
        _ => {
            *seen = Some((m, now));
            false
        }
    }
}

/// Nothing is being typed or sent: the list or a thread, no editor, form or confirm screen open,
/// no send in its hold or waiting for keryxd, no change waiting to go, and no key for 10 s.
pub fn idle(app: &App, writing: bool, quiet_for: Duration) -> bool {
    matches!(app.mode, Mode::List | Mode::Thread)
        && !writing
        && app.writing.is_none()
        && app.editor_request.is_none()
        && app.hold.is_none()
        && app.sending.is_none()
        && app.deferred.is_empty()
        && !app.outbox.iter().any(|o| {
            matches!(
                o.state,
                crate::send::SendState::Asking | crate::send::SendState::Holding | crate::send::SendState::Cancelling
            )
        })
        && quiet_for >= Duration::from_secs(10)
}

/// The thread on screen, to reopen after the restart.
pub fn view_of(app: &App) -> Option<String> {
    let o = app.open.as_ref().filter(|_| app.mode == Mode::Thread)?;
    Some(keryx_core::model::thread_ref(&o.account, &o.thread_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rebuild_counts_once_the_file_has_been_still_for_two_seconds() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
        let t1 = t0 + Duration::from_secs(5);
        let now = Instant::now();
        let mut seen = None;
        assert!(!stable(t0, t0, &mut seen, now), "unchanged");
        assert!(!stable(t0, t1, &mut seen, now), "just changed");
        assert!(
            !stable(t0, t1, &mut seen, now + Duration::from_secs(1)),
            "still being written?"
        );
        let t2 = t1 + Duration::from_secs(1);
        assert!(
            !stable(t0, t2, &mut seen, now + Duration::from_secs(2)),
            "changed again: wait again"
        );
        assert!(stable(t0, t2, &mut seen, now + Duration::from_secs(4)));
    }
}
