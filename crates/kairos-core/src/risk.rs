//! Risk policy (SPEC §6). Pure: kairosd gathers [`OpFacts`] from the store
//! and calls [`assess`] for every changeset. The caller never supplies the
//! risk — Claude cannot label its own change low-risk.
//!
//! Order of evaluation: **forbidden** checks, then **high** triggers; if
//! nothing triggers, the change is **low** (auto-applied, logged, undoable).

use crate::changeset::Risk;
use crate::config::Overlay;
use crate::model::{Author, EventKind};
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;

/// Facts about a calendar touched by an op.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarFacts {
    pub id: String,
    pub overlay: Overlay,
    pub writable: bool,
    /// An `ics_readonly` feed (Google imports, holidays).
    pub readonly_feed: bool,
}

/// Facts about an event image (before or after the op).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventFacts {
    pub kind: EventKind,
    /// `None` = not created through kairos (phone, Calendar.app, pre-existing).
    pub created_by: Option<Author>,
    pub flex: bool,
    pub recurring: bool,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Create,
    Update,
    Delete,
    /// Re-key: move between personal calendars.
    Move,
    ShareToFamily,
    TaskCreate,
    TaskUpdate,
    TaskComplete,
    TaskDrop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpFacts {
    pub action: Action,
    /// Calendar written to (create/update/delete: the event's calendar; move: destination).
    pub target: Option<CalendarFacts>,
    /// Move/share: the calendar the event leaves.
    pub origin: Option<CalendarFacts>,
    pub before: Option<EventFacts>,
    pub after: Option<EventFacts>,
    /// The post-image overlaps a non-flex, opaque event.
    pub overlaps_fixed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum Assessment {
    Forbidden { reasons: Vec<String> },
    Allowed { risk: Risk, reasons: Vec<String> },
}

impl Assessment {
    pub fn risk(&self) -> Option<Risk> {
        match self {
            Assessment::Allowed { risk, .. } => Some(*risk),
            Assessment::Forbidden { .. } => None,
        }
    }
}

pub const MAX_LOW_RISK_OPS: usize = 8;
pub const IMMINENT: Duration = Duration::hours(2);
pub const BLOCK_LEAD: Duration = Duration::minutes(60);

fn forbidden(op: &OpFacts) -> Option<String> {
    for cal in [&op.target, &op.origin].into_iter().flatten() {
        if cal.readonly_feed || !cal.writable {
            return Some(format!("{} is read-only", cal.id));
        }
        if matches!(cal.overlay, Overlay::Background | Overlay::Imported) {
            return Some(format!("{} is a read-only layer", cal.id));
        }
    }
    let touches_family = [&op.target, &op.origin]
        .into_iter()
        .flatten()
        .any(|c| c.overlay == Overlay::Family);
    match op.action {
        Action::ShareToFamily => {
            let ok_target = op.target.as_ref().is_some_and(|t| t.overlay == Overlay::Family);
            let ok_origin = op.origin.as_ref().is_some_and(|o| o.overlay == Overlay::Personal);
            if !(ok_target && ok_origin) {
                return Some("share_to_family must move a personal event into Family".into());
            }
        }
        _ if touches_family => {
            return Some("Family is read-only except share_to_family".into());
        }
        _ => {}
    }
    let is_event = matches!(
        op.action,
        Action::Create | Action::Update | Action::Delete | Action::Move | Action::ShareToFamily
    );
    if is_event && op.target.is_none() {
        return Some("event op without a target calendar".into());
    }
    None
}

/// The scheduler rearranging its own work: creating, moving or deleting
/// flex task blocks it made (not pinned ones, not anyone's events). Low-risk
/// at any time and exempt from the op-count limit (Alec, 2026-10-12), so
/// work can move out of the way of new events and start 15 min from now.
pub fn is_scheduler_block_op(op: &OpFacts) -> bool {
    let imgs: Vec<&EventFacts> = [&op.before, &op.after].into_iter().flatten().collect();
    matches!(op.action, Action::Create | Action::Update | Action::Delete)
        && !imgs.is_empty()
        && imgs.iter().all(|e| {
            e.kind == EventKind::TaskBlock && e.created_by == Some(Author::Scheduler) && e.flex && !e.recurring
        })
        && !op.overlaps_fixed
}

fn high_reasons(op: &OpFacts, now: DateTime<Utc>) -> Vec<String> {
    let mut r = Vec::new();
    if is_scheduler_block_op(op) {
        return r;
    }
    let imgs: Vec<&EventFacts> = [&op.before, &op.after].into_iter().flatten().collect();
    match op.action {
        Action::TaskDrop => r.push("dropping a task".into()),
        Action::TaskCreate | Action::TaskUpdate | Action::TaskComplete => {}
        Action::ShareToFamily => r.push("share to Family always needs approval".into()),
        Action::Update | Action::Delete | Action::Move => {
            if op.before.as_ref().is_some_and(|b| b.created_by.is_none()) {
                r.push("changes an event not created by kairos".into());
            }
            if op.action == Action::Delete && op.before.as_ref().is_some_and(|b| b.kind == EventKind::Deadline) {
                r.push("deletes a deadline".into());
            }
        }
        Action::Create => {}
    }
    if imgs.iter().any(|e| e.recurring) {
        r.push("recurring-series change".into());
    }
    if imgs.iter().any(|e| e.start < now + IMMINENT && e.end >= now) {
        r.push("touches an event starting within 2 h".into());
    }
    let creates_or_moves_time = matches!(op.action, Action::Create | Action::Update | Action::Move);
    if creates_or_moves_time && op.overlaps_fixed {
        r.push("overlaps a fixed (non-flex) event".into());
    }
    // Task blocks are only low-risk if flex, kairos-made, and > 60 min away.
    for e in &imgs {
        if e.kind == EventKind::TaskBlock && matches!(op.action, Action::Create | Action::Update | Action::Delete) {
            let kairos_made = matches!(e.created_by, Some(Author::Scheduler | Author::Claude));
            if !kairos_made || !e.flex {
                if e.created_by.is_some() {
                    r.push("task block that isn't a flex scheduler/Claude block".into());
                }
            } else if e.start <= now + BLOCK_LEAD {
                r.push("task block within the next 60 min".into());
            }
        }
    }
    r.sort();
    r.dedup();
    r
}

/// Alec's own edits, typed by him (TUI or CLI on a terminal): only the
/// hard rules apply. Approval exists to check *other* actors' changes;
/// asking him to approve what he just typed would be noise.
pub fn assess_direct(ops: &[OpFacts]) -> Assessment {
    let forb: Vec<String> = ops.iter().filter_map(forbidden).collect();
    if !forb.is_empty() {
        return Assessment::Forbidden { reasons: forb };
    }
    Assessment::Allowed {
        risk: Risk::Low,
        reasons: Vec::new(),
    }
}

/// Assess a whole changeset.
pub fn assess(ops: &[OpFacts], now: DateTime<Utc>) -> Assessment {
    let forb: Vec<String> = ops.iter().filter_map(forbidden).collect();
    if !forb.is_empty() {
        return Assessment::Forbidden { reasons: forb };
    }
    let mut reasons: Vec<String> = ops.iter().flat_map(|o| high_reasons(o, now)).collect();
    let counted = ops.iter().filter(|o| !is_scheduler_block_op(o)).count();
    if counted > MAX_LOW_RISK_OPS {
        reasons.push(format!("more than {MAX_LOW_RISK_OPS} operations ({counted})"));
    }
    reasons.sort();
    reasons.dedup();
    if reasons.is_empty() {
        Assessment::Allowed {
            risk: Risk::Low,
            reasons,
        }
    } else {
        Assessment::Allowed {
            risk: Risk::High,
            reasons,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 12, 9, 0, 0).unwrap()
    }
    fn cal(id: &str, overlay: Overlay, writable: bool, feed: bool) -> Option<CalendarFacts> {
        Some(CalendarFacts {
            id: id.into(),
            overlay,
            writable,
            readonly_feed: feed,
        })
    }
    fn personal() -> Option<CalendarFacts> {
        cal("sport", Overlay::Personal, true, false)
    }
    fn other_personal() -> Option<CalendarFacts> {
        cal("social", Overlay::Personal, true, false)
    }
    fn family() -> Option<CalendarFacts> {
        cal("family", Overlay::Family, true, false)
    }
    fn feed() -> Option<CalendarFacts> {
        cal("oqts-shared", Overlay::Personal, false, true)
    }
    fn holidays() -> Option<CalendarFacts> {
        cal("uk-holidays", Overlay::Background, false, true)
    }
    fn ev(kind: EventKind, by: Option<Author>, flex: bool, recurring: bool, in_hours: i64) -> Option<EventFacts> {
        let start = now() + Duration::hours(in_hours);
        Some(EventFacts {
            kind,
            created_by: by,
            flex,
            recurring,
            start,
            end: start + Duration::hours(1),
        })
    }
    fn op(
        action: Action,
        target: Option<CalendarFacts>,
        origin: Option<CalendarFacts>,
        before: Option<EventFacts>,
        after: Option<EventFacts>,
        overlaps: bool,
    ) -> OpFacts {
        OpFacts {
            action,
            target,
            origin,
            before,
            after,
            overlaps_fixed: overlaps,
        }
    }

    use Action::*;
    use EventKind::{Deadline, Event, TaskBlock};
    const SCHED: Option<Author> = Some(Author::Scheduler);
    const CLAUDE: Option<Author> = Some(Author::Claude);
    const ALEC_K: Option<Author> = Some(Author::Alec);
    const EXTERNAL: Option<Author> = None;

    enum Want {
        Low,
        High(&'static str),
        Forbidden(&'static str),
    }

    #[test]
    fn policy_table() {
        let cases: Vec<(&str, OpFacts, Want)> = vec![
            // ---- forbidden
            (
                "create in read-only feed",
                op(Create, feed(), None, None, ev(Event, CLAUDE, false, false, 24), false),
                Want::Forbidden("read-only"),
            ),
            (
                "update in holidays layer",
                op(
                    Update,
                    holidays(),
                    None,
                    ev(Event, None, false, false, 24),
                    ev(Event, None, false, false, 25),
                    false,
                ),
                Want::Forbidden("read-only"),
            ),
            (
                "create in Family",
                op(Create, family(), None, None, ev(Event, CLAUDE, false, false, 24), false),
                Want::Forbidden("Family"),
            ),
            (
                "update Family event",
                op(
                    Update,
                    family(),
                    None,
                    ev(Event, None, false, false, 24),
                    ev(Event, None, false, false, 25),
                    false,
                ),
                Want::Forbidden("Family"),
            ),
            (
                "delete Family event",
                op(Delete, family(), None, ev(Event, None, false, false, 24), None, false),
                Want::Forbidden("Family"),
            ),
            (
                "move Family → personal",
                op(
                    Move,
                    personal(),
                    family(),
                    ev(Event, None, false, false, 24),
                    ev(Event, None, false, false, 24),
                    false,
                ),
                Want::Forbidden("Family"),
            ),
            (
                "share from a feed",
                op(
                    ShareToFamily,
                    family(),
                    feed(),
                    ev(Event, None, false, false, 24),
                    ev(Event, None, false, false, 24),
                    false,
                ),
                Want::Forbidden("read-only"),
            ),
            (
                "share into personal (wrong way)",
                op(
                    ShareToFamily,
                    other_personal(),
                    personal(),
                    ev(Event, None, false, false, 24),
                    ev(Event, None, false, false, 24),
                    false,
                ),
                Want::Forbidden("share_to_family"),
            ),
            (
                "move into feed",
                op(
                    Move,
                    feed(),
                    personal(),
                    ev(Event, CLAUDE, false, false, 24),
                    ev(Event, CLAUDE, false, false, 24),
                    false,
                ),
                Want::Forbidden("read-only"),
            ),
            (
                "event op without target",
                op(Create, None, None, None, ev(Event, CLAUDE, false, false, 24), false),
                Want::Forbidden("target"),
            ),
            // ---- low
            (
                "scheduler creates flex block tomorrow",
                op(
                    Create,
                    personal(),
                    None,
                    None,
                    ev(TaskBlock, SCHED, true, false, 24),
                    false,
                ),
                Want::Low,
            ),
            (
                "claude moves flex block",
                op(
                    Update,
                    personal(),
                    None,
                    ev(TaskBlock, CLAUDE, true, false, 24),
                    ev(TaskBlock, CLAUDE, true, false, 26),
                    false,
                ),
                Want::Low,
            ),
            (
                "scheduler deletes flex block",
                op(
                    Delete,
                    personal(),
                    None,
                    ev(TaskBlock, SCHED, true, false, 24),
                    None,
                    false,
                ),
                Want::Low,
            ),
            (
                "create single event, no overlap",
                op(
                    Create,
                    personal(),
                    None,
                    None,
                    ev(Event, CLAUDE, false, false, 24),
                    false,
                ),
                Want::Low,
            ),
            (
                "create deadline",
                op(
                    Create,
                    personal(),
                    None,
                    None,
                    ev(Deadline, CLAUDE, false, false, 72),
                    false,
                ),
                Want::Low,
            ),
            (
                "update deadline",
                op(
                    Update,
                    personal(),
                    None,
                    ev(Deadline, CLAUDE, false, false, 72),
                    ev(Deadline, CLAUDE, false, false, 96),
                    false,
                ),
                Want::Low,
            ),
            ("create task", op(TaskCreate, None, None, None, None, false), Want::Low),
            ("update task", op(TaskUpdate, None, None, None, None, false), Want::Low),
            (
                "complete task",
                op(TaskComplete, None, None, None, None, false),
                Want::Low,
            ),
            (
                "re-key claude event",
                op(
                    Move,
                    other_personal(),
                    personal(),
                    ev(Event, CLAUDE, false, false, 24),
                    ev(Event, CLAUDE, false, false, 24),
                    false,
                ),
                Want::Low,
            ),
            (
                "re-key scheduler block",
                op(
                    Move,
                    other_personal(),
                    personal(),
                    ev(TaskBlock, SCHED, true, false, 24),
                    ev(TaskBlock, SCHED, true, false, 24),
                    false,
                ),
                Want::Low,
            ),
            (
                "update kairos-made event",
                op(
                    Update,
                    personal(),
                    None,
                    ev(Event, ALEC_K, false, false, 24),
                    ev(Event, ALEC_K, false, false, 25),
                    false,
                ),
                Want::Low,
            ),
            // ---- high
            (
                "update Alec's phone event",
                op(
                    Update,
                    personal(),
                    None,
                    ev(Event, EXTERNAL, false, false, 24),
                    ev(Event, EXTERNAL, false, false, 25),
                    false,
                ),
                Want::High("not created by kairos"),
            ),
            (
                "delete Alec's phone event",
                op(
                    Delete,
                    personal(),
                    None,
                    ev(Event, EXTERNAL, false, false, 24),
                    None,
                    false,
                ),
                Want::High("not created by kairos"),
            ),
            (
                "re-key Alec's phone event",
                op(
                    Move,
                    other_personal(),
                    personal(),
                    ev(Event, EXTERNAL, false, false, 24),
                    ev(Event, EXTERNAL, false, false, 24),
                    false,
                ),
                Want::High("not created by kairos"),
            ),
            (
                "share to family",
                op(
                    ShareToFamily,
                    family(),
                    personal(),
                    ev(Event, CLAUDE, false, false, 24),
                    ev(Event, CLAUDE, false, false, 24),
                    false,
                ),
                Want::High("Family"),
            ),
            (
                "create recurring event",
                op(
                    Create,
                    personal(),
                    None,
                    None,
                    ev(Event, CLAUDE, false, true, 24),
                    false,
                ),
                Want::High("recurring"),
            ),
            (
                "update recurring series",
                op(
                    Update,
                    personal(),
                    None,
                    ev(Event, CLAUDE, false, true, 24),
                    ev(Event, CLAUDE, false, true, 24),
                    false,
                ),
                Want::High("recurring"),
            ),
            (
                "create event in 1h",
                op(
                    Create,
                    personal(),
                    None,
                    None,
                    ev(Event, CLAUDE, false, false, 1),
                    false,
                ),
                Want::High("2 h"),
            ),
            (
                "move event that is ongoing",
                op(
                    Update,
                    personal(),
                    None,
                    ev(Event, CLAUDE, false, false, 0),
                    ev(Event, CLAUDE, false, false, 5),
                    false,
                ),
                Want::High("2 h"),
            ),
            (
                "move event INTO next 2h",
                op(
                    Update,
                    personal(),
                    None,
                    ev(Event, CLAUDE, false, false, 5),
                    ev(Event, CLAUDE, false, false, 1),
                    false,
                ),
                Want::High("2 h"),
            ),
            (
                "create overlapping fixed",
                op(
                    Create,
                    personal(),
                    None,
                    None,
                    ev(Event, CLAUDE, false, false, 24),
                    true,
                ),
                Want::High("overlaps"),
            ),
            (
                "pinned block over a lecture",
                op(
                    Create,
                    personal(),
                    None,
                    None,
                    ev(TaskBlock, CLAUDE, true, false, 24),
                    true,
                ),
                Want::High("overlaps"),
            ),
            (
                "delete deadline",
                op(
                    Delete,
                    personal(),
                    None,
                    ev(Deadline, CLAUDE, false, false, 72),
                    None,
                    false,
                ),
                Want::High("deadline"),
            ),
            (
                "drop task",
                op(TaskDrop, None, None, None, None, false),
                Want::High("dropping"),
            ),
            (
                // Scheduler-owned flex blocks are low-risk at any time (2026-10-12).
                "scheduler block starting now",
                op(
                    Create,
                    personal(),
                    None,
                    None,
                    ev(TaskBlock, SCHED, true, false, 0),
                    false,
                ),
                Want::Low,
            ),
            (
                "claude flex block within 60 min",
                op(
                    Create,
                    personal(),
                    None,
                    None,
                    ev(TaskBlock, CLAUDE, true, false, 0),
                    false,
                ),
                Want::High("2 h"),
            ),
            (
                "non-flex claude block",
                op(
                    Create,
                    personal(),
                    None,
                    None,
                    ev(TaskBlock, CLAUDE, false, false, 24),
                    false,
                ),
                Want::High("flex"),
            ),
            (
                "past events ok, but delete external still high",
                op(
                    Delete,
                    personal(),
                    None,
                    ev(Event, EXTERNAL, false, false, -48),
                    None,
                    false,
                ),
                Want::High("not created by kairos"),
            ),
        ];
        for (name, o, want) in cases {
            let got = assess(std::slice::from_ref(&o), now());
            match (want, &got) {
                (Want::Low, Assessment::Allowed { risk: Risk::Low, .. }) => {}
                (
                    Want::High(s),
                    Assessment::Allowed {
                        risk: Risk::High,
                        reasons,
                    },
                ) => {
                    assert!(
                        reasons.iter().any(|r| r.contains(s)),
                        "{name}: reasons {reasons:?} lack {s:?}"
                    );
                }
                (Want::Forbidden(s), Assessment::Forbidden { reasons }) => {
                    assert!(
                        reasons.iter().any(|r| r.contains(s)),
                        "{name}: reasons {reasons:?} lack {s:?}"
                    );
                }
                (_, got) => panic!("{name}: got {got:?}"),
            }
        }
    }

    #[test]
    fn scheduler_moving_its_own_blocks_is_low_at_any_time() {
        // Create in 15 min, move one that starts in 30 min, delete one in 90 min.
        let soon = |mins: i64| {
            let mut e = ev(TaskBlock, SCHED, true, false, 0).unwrap();
            e.start = now() + Duration::minutes(mins);
            e.end = e.start + Duration::minutes(60);
            Some(e)
        };
        let ops = [
            op(Create, personal(), None, None, soon(15), false),
            op(Update, personal(), None, soon(30), soon(200), false),
            op(Delete, personal(), None, soon(90), None, false),
        ];
        assert_eq!(assess(&ops, now()).risk(), Some(Risk::Low));
        // …and any number of them (a big replan after a new event).
        let many = vec![op(Create, personal(), None, None, soon(600), false); 30];
        assert_eq!(assess(&many, now()).risk(), Some(Risk::Low));
        // A pinned (non-flex) block is Alec's: the normal rules apply.
        let mut pinned = soon(30);
        if let Some(p) = pinned.as_mut() {
            p.flex = false;
        }
        let a = assess(&[op(Update, personal(), None, pinned, soon(300), false)], now());
        assert_eq!(a.risk(), Some(Risk::High));
        // Overlapping a fixed event is never the scheduler's own business.
        let a = assess(&[op(Create, personal(), None, None, soon(300), true)], now());
        assert_eq!(a.risk(), Some(Risk::High));
    }

    #[test]
    fn claude_block_between_60_and_120_min_is_high_because_imminent() {
        // start > now+60 satisfies the block rule, but "starts within 2 h" still applies.
        let mut e = ev(TaskBlock, CLAUDE, true, false, 0).unwrap();
        e.start = now() + Duration::minutes(90);
        e.end = e.start + Duration::minutes(30);
        let a = assess(&[op(Create, personal(), None, None, Some(e), false)], now());
        assert_eq!(a.risk(), Some(Risk::High));
    }

    #[test]
    fn more_than_eight_ops_is_high() {
        // Scheduler block ops don't count (they're exempt); Claude's events do.
        let one = op(
            Create,
            personal(),
            None,
            None,
            ev(Event, CLAUDE, false, false, 24),
            false,
        );
        let eight = vec![one.clone(); 8];
        assert_eq!(assess(&eight, now()).risk(), Some(Risk::Low));
        let nine = vec![one; 9];
        match assess(&nine, now()) {
            Assessment::Allowed {
                risk: Risk::High,
                reasons,
            } => assert!(reasons[0].contains("more than 8")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn one_forbidden_op_poisons_changeset() {
        let ok = op(
            Create,
            personal(),
            None,
            None,
            ev(TaskBlock, SCHED, true, false, 24),
            false,
        );
        let bad = op(Create, family(), None, None, ev(Event, CLAUDE, false, false, 24), false);
        assert!(matches!(assess(&[ok, bad], now()), Assessment::Forbidden { .. }));
    }

    #[test]
    fn one_high_op_makes_changeset_high() {
        let ok = op(
            Create,
            personal(),
            None,
            None,
            ev(TaskBlock, SCHED, true, false, 24),
            false,
        );
        let high = op(
            Delete,
            personal(),
            None,
            ev(Event, EXTERNAL, false, false, 24),
            None,
            false,
        );
        assert_eq!(assess(&[ok, high], now()).risk(), Some(Risk::High));
    }

    #[test]
    fn empty_changeset_is_low() {
        assert_eq!(assess(&[], now()).risk(), Some(Risk::Low));
    }
}
