//! Risk policy (SPEC §6.2). Pure: keryxd gathers [`ThreadFacts`] from the
//! mirror and calls [`assess`]; the caller never supplies the risk.
//! Order: **forbidden** → **high** → **low**.
//!
//! Interactive Alec (TUI, CLI on a TTY) gets the forbidden rules only.
//! `system` (snooze wake-ups) is low in any number. Claude and scripts get
//! the full policy, where triage applies at once only to automated mail.

use crate::changeset::{Author, Risk};
use serde::Serialize;

/// What the changeset does (send/unsubscribe arrive in Phase 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Archive, un-archive, file, flag, read, snooze, unsnooze.
    Triage,
    Send,
    Unsubscribe,
}

/// Facts about one thread an op touches, gathered by keryxd.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadFacts {
    pub account: String,
    pub account_writable: bool,
    /// Used by other people too (OQTS shared).
    pub account_shared: bool,
    /// Every message is automated ([`crate::automated`]), computed by keryxd.
    pub automated: bool,
    /// Any message flagged.
    pub flagged: bool,
    /// Every touched message's subject starts with `[keryx-test]`.
    pub test_message: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    pub author: Author,
    pub interactive: bool,
    pub kind: Kind,
    pub live: bool,
    pub batch_threshold: u32,
    pub threads: Vec<ThreadFacts>,
    /// Send: every recipient is one of Alec's own addresses.
    pub recipients_own: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Assessment {
    Forbidden(String),
    Low,
    High(Vec<String>),
}

impl Assessment {
    pub fn risk(&self) -> Option<Risk> {
        match self {
            Assessment::Forbidden(_) => None,
            Assessment::Low => Some(Risk::Low),
            Assessment::High(_) => Some(Risk::High),
        }
    }
}

pub const TEST_PREFIX: &str = "[keryx-test]";

/// Alec typing in the TUI / on a terminal.
pub fn is_direct(author: Author, interactive: bool) -> bool {
    author == Author::Alec && interactive
}

pub fn assess(f: &Facts) -> Assessment {
    // ---- forbidden: for everyone ----
    if f.kind == Kind::Send && !f.live && !f.recipients_own {
        return Assessment::Forbidden("live = false: keryx only sends to your own addresses".into());
    }
    for t in &f.threads {
        if !t.account_writable {
            return Assessment::Forbidden(format!("{} is read-only in keryx (writable = false)", t.account));
        }
        if !f.live && !t.test_message && f.kind != Kind::Send {
            return Assessment::Forbidden(format!(
                "live = false: keryx only changes messages whose subject starts with {TEST_PREFIX}"
            ));
        }
    }
    if f.kind == Kind::Unsubscribe && !is_direct(f.author, f.interactive) {
        return Assessment::Forbidden(
            "unsubscribing contacts the sender: only Alec can, from the TUI or a terminal".into(),
        );
    }

    if is_direct(f.author, f.interactive) || f.author == Author::System {
        return Assessment::Low;
    }

    // ---- high ----
    let mut reasons = Vec::new();
    if f.kind == Kind::Send {
        reasons.push("sending always needs Alec's approval".to_string());
    }
    if f.kind == Kind::Triage {
        let people = f.threads.iter().filter(|t| !t.automated).count();
        if people > 0 {
            reasons.push(format!(
                "{people} thread{} from people (only automated mail changes without approval)",
                plural(people)
            ));
        }
        if f.threads.len() > f.batch_threshold as usize {
            reasons.push(format!(
                "{} threads at once (more than {})",
                f.threads.len(),
                f.batch_threshold
            ));
        }
        let flagged = f.threads.iter().filter(|t| t.flagged).count();
        if flagged > 0 {
            reasons.push(format!("{flagged} flagged thread{}", plural(flagged)));
        }
        let shared: Vec<&str> = {
            let mut v: Vec<&str> = f
                .threads
                .iter()
                .filter(|t| t.account_shared)
                .map(|t| t.account.as_str())
                .collect();
            v.dedup();
            v
        };
        if !shared.is_empty() {
            reasons.push(format!("{} is shared with other people", shared.join(", ")));
        }
    }
    if reasons.is_empty() {
        Assessment::Low
    } else {
        Assessment::High(reasons)
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread() -> ThreadFacts {
        ThreadFacts {
            account: "personal".into(),
            account_writable: true,
            account_shared: false,
            automated: true,
            flagged: false,
            test_message: false,
        }
    }

    fn facts(author: Author, interactive: bool, kind: Kind, threads: Vec<ThreadFacts>) -> Facts {
        Facts {
            author,
            interactive,
            kind,
            live: true,
            batch_threshold: 100,
            threads,
            recipients_own: false,
        }
    }

    const AUTHORS: [(Author, bool); 6] = [
        (Author::Alec, true),
        (Author::Alec, false),
        (Author::Claude, false),
        (Author::Claude, true),
        (Author::Script, false),
        (Author::System, false),
    ];

    /// The §6.2 table, row by row, for every author × interactive pair.
    #[test]
    fn table() {
        for (author, interactive) in AUTHORS {
            let direct = is_direct(author, interactive);
            let sys = author == Author::System;
            let a = |kind, threads| assess(&facts(author, interactive, kind, threads));
            let ctx = format!("{author:?} interactive={interactive}");

            // Read-only accounts: forbidden for everyone.
            let ro = ThreadFacts {
                account_writable: false,
                ..thread()
            };
            assert!(matches!(a(Kind::Triage, vec![ro]), Assessment::Forbidden(_)), "{ctx}");

            // Automated, unflagged, non-shared, ≤ 100: low for everyone.
            assert_eq!(a(Kind::Triage, vec![thread(); 100]), Assessment::Low, "{ctx}");

            // People mail.
            let people = ThreadFacts {
                automated: false,
                ..thread()
            };
            let r = a(Kind::Triage, vec![thread(), people.clone()]);
            if direct || sys {
                assert_eq!(r, Assessment::Low, "{ctx}");
            } else {
                assert!(
                    matches!(&r, Assessment::High(v) if v[0].contains("1 thread from people")),
                    "{ctx}: {r:?}"
                );
            }

            // > batch threshold.
            let r = a(Kind::Triage, vec![thread(); 101]);
            assert_eq!(r == Assessment::Low, direct || sys, "{ctx}: {r:?}");

            // Flagged.
            let flagged = ThreadFacts {
                flagged: true,
                ..thread()
            };
            let r = a(Kind::Triage, vec![flagged]);
            assert_eq!(r == Assessment::Low, direct || sys, "{ctx}: {r:?}");

            // Shared account.
            let shared = ThreadFacts {
                account: "oqts-shared".into(),
                account_shared: true,
                ..thread()
            };
            let r = a(Kind::Triage, vec![shared]);
            assert_eq!(r == Assessment::Low, direct || sys, "{ctx}: {r:?}");
            if !(direct || sys) {
                assert!(matches!(&r, Assessment::High(v) if v.iter().any(|x| x.contains("oqts-shared is shared"))));
            }

            // Send: always high unless Alec typed it.
            let r = a(Kind::Send, vec![thread()]);
            assert_eq!(r == Assessment::Low, direct || sys, "{ctx}: {r:?}");

            // Unsubscribe: Alec only.
            let r = a(Kind::Unsubscribe, vec![thread()]);
            if direct {
                assert_eq!(r, Assessment::Low, "{ctx}");
            } else {
                assert!(matches!(r, Assessment::Forbidden(_)), "{ctx}: {r:?}");
            }
        }
    }

    #[test]
    fn live_guard_is_forbidden_for_everyone() {
        for (author, interactive) in AUTHORS {
            let mut f = facts(author, interactive, Kind::Triage, vec![thread()]);
            f.live = false;
            assert!(matches!(assess(&f), Assessment::Forbidden(m) if m.contains(TEST_PREFIX)));
            f.threads[0].test_message = true;
            assert_ne!(assess(&f), Assessment::Forbidden(String::new()));
            assert!(!matches!(assess(&f), Assessment::Forbidden(_)), "{author:?}");
        }
    }

    #[test]
    fn live_false_sends_only_to_alec() {
        for (author, interactive) in AUTHORS {
            let mut f = facts(author, interactive, Kind::Send, vec![]);
            f.live = false;
            assert!(matches!(assess(&f), Assessment::Forbidden(m) if m.contains("own addresses")));
            f.recipients_own = true;
            assert!(!matches!(assess(&f), Assessment::Forbidden(_)), "{author:?}");
        }
    }

    #[test]
    fn reasons_accumulate() {
        let t = ThreadFacts {
            automated: false,
            flagged: true,
            account: "oqts-shared".into(),
            account_shared: true,
            ..thread()
        };
        let r = assess(&facts(Author::Claude, false, Kind::Triage, vec![t; 101]));
        let Assessment::High(v) = r else { panic!() };
        assert_eq!(v.len(), 4, "{v:?}");
    }

    #[test]
    fn interactive_claude_isnt_direct() {
        assert!(!is_direct(Author::Claude, true));
        assert!(!is_direct(Author::Alec, false));
        assert!(is_direct(Author::Alec, true));
    }
}
