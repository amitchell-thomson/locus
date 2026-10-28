#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use chrono::Duration;
use common::{Env, at};
use kairos_core::api::{CheckinReq, CreateEventReq, CreateTaskReq, UpdateEventReq};
use kairos_core::changeset::{Risk, Status};
use kairos_core::model::{Author, EventKind};
use kairos_core::task::TaskStatus;
use kairosd::engine::EngineError;
use std::fs;

fn ev(title: &str, start: &str, key: &str) -> CreateEventReq {
    CreateEventReq {
        title: title.into(),
        start: start.into(),
        duration_min: Some(60),
        key: Some(key.into()),
        ..Default::default()
    }
}

// ------------------------------------------------------------ live guard

#[test]
fn not_live_redirects_new_events_to_test_calendar() {
    let env = Env::new(false);
    let mut e = env.engine();
    let cs = e
        .create_event(Author::Claude, &ev("Squash", "2026-10-13 19:00", "Sport"))
        .unwrap();
    assert_eq!(cs.status, Status::Applied);
    assert!(
        env.items("Sport").is_empty(),
        "nothing may touch real calendars while live = false"
    );
    let text = env.read_all("ZZ-kairos-test");
    assert!(text.contains("SUMMARY:Squash"));
    assert!(
        text.contains("X-KAIROS-ORIGIN-KEY:Sport"),
        "intended key kept for colouring"
    );
}

#[test]
fn not_live_refuses_to_touch_real_calendars() {
    let env = Env::new(false);
    env.phone_event("Personal", "phone-1", "Dentist", "2026-10-14 10:00", "2026-10-14 11:00");
    let before = env.read_all("Personal");
    let mut e = env.engine();
    let r = e.delete_event(Author::Claude, None, "phone-1");
    assert!(matches!(r, Err(EngineError::LiveDisabled(_))), "{r:?}");
    assert_eq!(env.read_all("Personal"), before);
}

// ------------------------------------------------------------ low risk

#[test]
fn alecs_own_interactive_edits_skip_approval_but_not_the_hard_rules() {
    let env = Env::new(true);
    let mut e = env.engine();
    let mut evs = Vec::new();
    for k in 0..10 {
        let cs = e
            .create_event(
                Author::Claude,
                &ev("Squash", &format!("2026-10-{:02} 19:00", 13 + k), "Sport"),
            )
            .unwrap();
        match &cs.ops[0] {
            kairos_core::changeset::Op::CreateEvent { uid, .. } => evs.push((None, uid.clone())),
            _ => unreachable!(),
        }
    }
    // Same 10-op move: pending for a script / Claude, applied when Alec typed it.
    let cs = e.move_events(Author::Alec, &evs, "Engineering").unwrap();
    assert_eq!(cs.status, Status::Pending);
    e.reject(&cs.id).unwrap();
    e.direct = true;
    let cs = e.move_events(Author::Alec, &evs, "Engineering").unwrap();
    assert_eq!(cs.status, Status::Applied);
    assert!(cs.reasons.is_empty());
    // Direct never applies to Claude.
    let cs = e.move_events(Author::Claude, &evs, "Sport").unwrap();
    assert_eq!(cs.status, Status::Pending);
    e.direct = false;
}

#[test]
fn stale_source_from_a_lagging_client_still_finds_the_event() {
    let env = Env::new(true);
    let mut e = env.engine();
    let cs = e
        .create_event(Author::Claude, &ev("Squash", "2026-10-13 19:00", "Sport"))
        .unwrap();
    let uid = match &cs.ops[0] {
        kairos_core::changeset::Op::CreateEvent { uid, .. } => uid.clone(),
        _ => unreachable!(),
    };
    e.direct = true;
    e.move_event(Author::Alec, Some("sport"), &uid, "Engineering").unwrap();
    // The TUI still thinks it's in Sport.
    let cs = e.move_event(Author::Alec, Some("sport"), &uid, "Sport").unwrap();
    assert_eq!(cs.status, Status::Applied);
    assert_eq!(env.items("Sport").len(), 1);
}

#[test]
fn claude_batch_edit_and_delete_are_single_changesets() {
    let env = Env::new(true);
    let mut e = env.engine();
    let mut uids = Vec::new();
    for k in 0..10 {
        let cs = e
            .create_event(
                Author::Claude,
                &ev(
                    &format!("B{k} Lecture LR2"),
                    &format!("2026-10-{:02} 10:00", 13 + k),
                    "Engineering",
                ),
            )
            .unwrap();
        match &cs.ops[0] {
            kairos_core::changeset::Op::CreateEvent { uid, .. } => uids.push(uid.clone()),
            _ => unreachable!(),
        }
    }
    let updates: Vec<kairos_core::api::EventUpdate> = uids
        .iter()
        .enumerate()
        .map(|(k, u)| kairos_core::api::EventUpdate {
            uid: u.clone(),
            source: None,
            change: kairos_core::api::UpdateEventReq {
                title: Some(format!("B{k} Lecture")),
                location: Some("LR2".into()),
                ..Default::default()
            },
        })
        .collect();
    let cs = e
        .update_events(Author::Claude, &updates, Some("Move room codes into location"))
        .unwrap();
    assert_eq!(cs.ops.len(), 10);
    assert_eq!(cs.status, Status::Pending, "Claude, > 8 ops");
    assert_eq!(cs.summary, "Move room codes into location (10 events)");
    e.approve(&cs.id).unwrap();
    let all = env.read_all("Engineering");
    assert!(all.contains("SUMMARY:B3 Lecture\r\n") || all.contains("SUMMARY:B3 Lecture\n"));
    assert!(all.contains("LOCATION:LR2"));
    let refs: Vec<(Option<String>, String)> = uids.iter().map(|u| (None, u.clone())).collect();
    let del = e.delete_events(Author::Claude, &refs[..3], Some("Drop B0–B2")).unwrap();
    assert_eq!(del.ops.len(), 3);
    if del.status == Status::Pending {
        e.approve(&del.id).unwrap();
    }
    assert_eq!(env.items("Engineering").len(), 7);
    e.undo(&del.id).unwrap();
    assert_eq!(env.items("Engineering").len(), 10);
}

#[test]
fn rekeying_many_events_is_one_changeset_with_one_undo() {
    let env = Env::new(true);
    let mut e = env.engine();
    let mut evs = Vec::new();
    for (t, w) in [("Squash", "2026-10-13 19:00"), ("Gym", "2026-10-14 19:00")] {
        let cs = e.create_event(Author::Claude, &ev(t, w, "Sport")).unwrap();
        match &cs.ops[0] {
            kairos_core::changeset::Op::CreateEvent { uid, .. } => evs.push((None, uid.clone())),
            _ => unreachable!(),
        }
    }
    let cs = e.move_events(Author::Alec, &evs, "Engineering").unwrap();
    assert_eq!(cs.ops.len(), 2);
    if cs.status == Status::Pending {
        e.approve(&cs.id).unwrap();
    }
    assert!(env.items("Sport").is_empty());
    assert_eq!(env.items("Engineering").len(), 2);
    e.undo(&cs.id).unwrap();
    assert_eq!(env.items("Sport").len(), 2);
    assert!(env.items("Engineering").is_empty());
}

#[test]
fn deleting_the_last_item_force_syncs_only_that_collection() {
    // vdirsyncer refuses to sync a "completely emptied" collection; when kairos
    // emptied it on purpose the push must say so, or the delete never reaches iCloud.
    let env = Env::new(true);
    let mut e = env.engine();
    let a = e
        .create_event(Author::Claude, &ev("Squash", "2026-10-13 19:00", "Sport"))
        .unwrap();
    let b = e
        .create_event(Author::Claude, &ev("Gym", "2026-10-14 19:00", "Sport"))
        .unwrap();
    let uid = |cs: &kairos_core::changeset::Changeset| match &cs.ops[0] {
        kairos_core::changeset::Op::CreateEvent { uid, .. } => uid.clone(),
        _ => unreachable!(),
    };
    env.rec.synced.lock().unwrap().clear();
    e.delete_event(Author::Alec, None, &uid(&a)).unwrap();
    e.delete_event(Author::Alec, None, &uid(&b)).unwrap();
    assert!(env.items("Sport").is_empty());
    assert_eq!(
        *env.rec.synced.lock().unwrap(),
        vec!["icloud/s-2222".to_string(), "--force-delete icloud/s-2222".to_string()]
    );
}

#[test]
fn live_low_risk_create_is_applied_and_synced() {
    let env = Env::new(true);
    let mut e = env.engine();
    let cs = e
        .create_event(Author::Claude, &ev("Squash", "2026-10-13 19:00", "Sport"))
        .unwrap();
    assert_eq!((cs.risk, cs.status), (Risk::Low, Status::Applied));
    assert_eq!(env.items("Sport").len(), 1);
    let t = env.read_all("Sport");
    assert!(t.contains("DTSTART;TZID=Europe/London:20261013T190000"));
    assert!(t.contains("X-KAIROS-CREATED-BY:claude"));
    assert_eq!(*env.rec.synced.lock().unwrap(), vec!["icloud/s-2222".to_string()]);
    // Sidecar has the metadata too (authoritative if the phone strips X- props).
    let uid = match &cs.ops[0] {
        kairos_core::changeset::Op::CreateEvent { uid, .. } => uid.clone(),
        _ => unreachable!(),
    };
    assert_eq!(
        e.db.meta_get("sport", &uid).unwrap().unwrap().created_by,
        Some(Author::Claude)
    );
}

#[test]
fn clash_with_fixed_event_needs_approval() {
    let env = Env::new(true);
    env.phone_event(
        "Engineering",
        "lecture",
        "B4 Lecture",
        "2026-10-13 10:00",
        "2026-10-13 11:00",
    );
    let mut e = env.engine();
    let cs = e
        .create_event(Author::Claude, &ev("Coffee", "2026-10-13 10:30", "Personal"))
        .unwrap();
    assert_eq!(cs.status, Status::Pending);
    assert!(cs.reasons.iter().any(|r| r.contains("overlaps")));
    assert!(env.items("Personal").is_empty());
    assert_eq!(
        e.db.cs_list(Some(Status::Pending), 10).unwrap().len(),
        1,
        "waits in Approve"
    );
}

// ------------------------------------------------------------ high risk: Alec's own events

#[test]
fn moving_alecs_event_is_pending_then_approved_then_undone() {
    let env = Env::new(true);
    env.phone_event("Sport", "squash", "Squash", "2026-10-15 19:00", "2026-10-15 20:00");
    let original = env.read_all("Sport");
    let mut e = env.engine();
    let req = UpdateEventReq {
        start: Some("2026-10-16 19:00".into()),
        ..Default::default()
    };
    let cs = e.update_event(Author::Claude, None, "squash", &req).unwrap().unwrap();
    assert_eq!(cs.status, Status::Pending);
    assert!(cs.reasons.iter().any(|r| r.contains("not created by kairos")));
    assert_eq!(env.read_all("Sport"), original, "pending changes write nothing");

    let cs = e.approve(&cs.id).unwrap();
    assert_eq!(cs.status, Status::Applied);
    let moved = env.read_all("Sport");
    assert!(moved.contains("20261016T190000"), "{moved}");
    assert!(
        moved.contains("X-APPLE-TRAVEL-ADVISORY-BEHAVIOR:AUTOMATIC"),
        "unknown props survive edits"
    );
    assert!(moved.contains("SEQUENCE:1"));

    let cs = e.undo(&cs.id).unwrap();
    assert_eq!(cs.status, Status::Undone);
    assert_eq!(env.read_all("Sport"), original);
}

#[test]
fn reject_writes_nothing() {
    let env = Env::new(true);
    env.phone_event("Sport", "squash", "Squash", "2026-10-15 19:00", "2026-10-15 20:00");
    let original = env.read_all("Sport");
    let mut e = env.engine();
    let cs = e.delete_event(Author::Claude, None, "squash").unwrap();
    assert_eq!(cs.status, Status::Pending);
    let cs = e.reject(&cs.id).unwrap();
    assert_eq!(cs.status, Status::Rejected);
    assert_eq!(env.read_all("Sport"), original);
    assert!(e.approve(&cs.id).is_err(), "can't approve a rejected change");
}

#[test]
fn approve_fails_cleanly_if_event_changed_meanwhile() {
    let env = Env::new(true);
    env.phone_event("Sport", "squash", "Squash", "2026-10-15 19:00", "2026-10-15 20:00");
    let mut e = env.engine();
    let cs = e.delete_event(Author::Claude, None, "squash").unwrap();
    // Alec edits it on his phone before approving.
    env.phone_event(
        "Sport",
        "squash",
        "Squash (court 3)",
        "2026-10-15 19:00",
        "2026-10-15 20:00",
    );
    let r = e.approve(&cs.id);
    assert!(matches!(r, Err(EngineError::Conflict { .. })), "{r:?}");
    assert_eq!(e.db.cs_get(&cs.id).unwrap().unwrap().status, Status::Failed);
    assert!(env.read_all("Sport").contains("court 3"));
}

// ------------------------------------------------------------ undo

#[test]
fn undo_refuses_after_external_edit_with_diff() {
    let env = Env::new(true);
    let mut e = env.engine();
    let cs = e
        .create_event(Author::Claude, &ev("Coffee", "2026-10-14 15:00", "Personal"))
        .unwrap();
    let name = &env.items("Personal")[0];
    let p = env.coll("Personal").join(name);
    let t = fs::read_to_string(&p)
        .unwrap()
        .replace("SUMMARY:Coffee", "SUMMARY:Coffee with Sam");
    fs::write(&p, t).unwrap();
    match e.undo(&cs.id) {
        Err(EngineError::Conflict { message, diff }) => {
            assert!(message.contains("edited elsewhere"));
            assert!(diff.contains("+   SUMMARY:Coffee with Sam"), "{diff}");
        }
        other => panic!("{other:?}"),
    }
    assert!(p.exists());
}

#[test]
fn undo_tolerates_cosmetic_icloud_rewrites() {
    let env = Env::new(true);
    let mut e = env.engine();
    let cs = e
        .create_event(Author::Claude, &ev("Coffee", "2026-10-14 15:00", "Personal"))
        .unwrap();
    let p = env.coll("Personal").join(&env.items("Personal")[0]);
    let t = fs::read_to_string(&p).unwrap().replace(
        "SUMMARY:Coffee",
        "SUMMARY:Coffee\r\nX-APPLE-CREATOR-IDENTITY:com.apple.mobilecal\r\nLAST-MODIFIED:20261013T000000Z",
    );
    fs::write(&p, t).unwrap();
    assert_eq!(e.undo(&cs.id).unwrap().status, Status::Undone);
    assert!(env.items("Personal").is_empty());
}

// ------------------------------------------------------------ forbidden

#[test]
fn family_and_imports_are_protected() {
    let env = Env::new(true);
    env.phone_event(
        "Family",
        "fam-1",
        "Mum's birthday dinner",
        "2026-10-17 19:00",
        "2026-10-17 21:00",
    );
    env.feed("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:g-1\r\nDTSTART:20261020T090000Z\r\nDTEND:20261020T100000Z\r\nSUMMARY:Train to London\r\nATTENDEE:mailto:x@y.z\r\nORGANIZER:mailto:x@y.z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n");
    let mut e = env.engine();
    assert!(matches!(
        e.delete_event(Author::Claude, None, "fam-1"),
        Err(EngineError::Forbidden(_))
    ));
    assert!(matches!(
        e.update_event(
            Author::Claude,
            None,
            "fam-1",
            &UpdateEventReq {
                title: Some("x".into()),
                ..Default::default()
            }
        ),
        Err(EngineError::Forbidden(_))
    ));
    assert!(matches!(
        e.delete_event(Author::Claude, None, "g-1"),
        Err(EngineError::Forbidden(_))
    ));
    assert!(matches!(
        e.move_event(Author::Claude, None, "fam-1", "Sport"),
        Err(EngineError::Forbidden(_))
    ));
}

// ------------------------------------------------------------ share / re-key / adopt

#[test]
fn share_to_family_moves_and_keeps_origin_key() {
    let env = Env::new(true);
    env.phone_event(
        "Sport",
        "match",
        "Varsity match",
        "2026-10-24 14:00",
        "2026-10-24 17:00",
    );
    let mut e = env.engine();
    let cs = e.share_to_family(Author::Claude, None, "match").unwrap();
    assert_eq!(cs.status, Status::Pending);
    e.approve(&cs.id).unwrap();
    assert!(env.items("Sport").is_empty());
    let fam = env.read_all("Family");
    assert!(fam.contains("UID:match") && fam.contains("X-KAIROS-ORIGIN-KEY:Sport"));
    assert_eq!(
        e.db.meta_get("family", "match").unwrap().unwrap().origin_key.as_deref(),
        Some("Sport")
    );
    let synced = env.rec.synced.lock().unwrap().clone();
    assert_eq!(
        synced,
        vec!["--force-delete icloud/s-2222".to_string(), "icloud/f-4444".to_string()],
        "delete side syncs first; Sport is now empty"
    );
    e.undo(&cs.id).unwrap();
    assert!(env.items("Family").is_empty());
    assert_eq!(env.items("Sport").len(), 1);
}

#[test]
fn rekey_kairos_event_is_low_risk() {
    let env = Env::new(true);
    let mut e = env.engine();
    let cs = e
        .create_event(Author::Claude, &ev("Gym", "2026-10-14 07:00", "Personal"))
        .unwrap();
    let uid = match &cs.ops[0] {
        kairos_core::changeset::Op::CreateEvent { uid, .. } => uid.clone(),
        _ => unreachable!(),
    };
    let mv = e.move_event(Author::Claude, None, &uid, "Sport").unwrap();
    assert_eq!(mv.status, Status::Applied);
    assert_eq!(env.items("Sport").len(), 1);
    assert!(env.items("Personal").is_empty());
}

#[test]
fn adopt_strips_invitations() {
    let env = Env::new(true);
    env.feed("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:g-1\r\nDTSTART:20261020T090000Z\r\nDTEND:20261020T100000Z\r\nSUMMARY:Train to London\r\nATTENDEE:mailto:x@y.z\r\nORGANIZER:mailto:x@y.z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n");
    let mut e = env.engine();
    let cs = e.adopt(Author::Alec, "g-1", "Personal").unwrap();
    assert_eq!(cs.status, Status::Applied);
    let t = env.read_all("Personal");
    assert!(t.contains("SUMMARY:Train to London"));
    assert!(!t.contains("ATTENDEE") && !t.contains("ORGANIZER"));
    assert!(t.contains("X-KAIROS-ADOPTED-FROM:gmail"));
}

// ------------------------------------------------------------ tasks & scheduler (definition of done)

fn b4(estimate: u32, due: &str) -> CreateTaskReq {
    CreateTaskReq {
        title: "B4 problem sheet".into(),
        key: "Engineering".into(),
        estimate_min: estimate,
        due: Some(due.into()),
        ..Default::default()
    }
}

#[test]
fn b4_sheet_due_friday_gets_a_due_date_and_blocks() {
    let env = Env::new(true);
    env.phone_event(
        "Engineering",
        "lec-mon",
        "Lecture",
        "2026-10-12 09:00",
        "2026-10-12 13:00",
    );
    let mut e = env.engine();
    let r = e.create_task(Author::Claude, &b4(240, "fri 17:00")).unwrap();
    assert_eq!(r.changesets[0].status, Status::Applied);
    let plan = r.plan.unwrap();
    assert!(plan.shortfalls.is_empty(), "{:?}", plan.shortfalls);
    assert_eq!(plan.changeset.as_ref().unwrap().status, Status::Applied);
    let eng = env.read_all("Engineering");
    // The due date is the task's (K5): no ⚑ event on the calendar, a ⚑ mark drawn from the task.
    assert!(!eng.contains("SUMMARY:⚑"), "{eng}");
    assert_eq!(r.task.deadline_uid, None);
    let marks = kairos_core::task::due_marks(
        &e.db.tasks(None).unwrap(),
        at("2026-10-12 00:00"),
        at("2026-10-19 00:00"),
        e.tz(),
    );
    assert_eq!(marks.len(), 1);
    assert_eq!(marks[0].title, "⚑ B4 problem sheet");
    assert_eq!(marks[0].meta.task_id.as_deref(), Some(r.task.id.as_str()));
    assert!(matches!(marks[0].span, kairos_core::model::Span::Timed { start, .. } if start == at("2026-10-16 17:00")));
    // Blocks: 240 min total, all before Fri 15:00 (2 h lead), none during the lecture.
    let world = e.world(14).unwrap();
    let blocks: Vec<_> = world
        .instances
        .iter()
        .filter(|i| i.meta.kind == EventKind::TaskBlock)
        .collect();
    let total: i64 = blocks
        .iter()
        .map(|b| (b.end(chrono_tz::Europe::London) - b.start(chrono_tz::Europe::London)).num_minutes())
        .sum();
    assert_eq!(total, 240);
    for b in &blocks {
        assert!(b.end(chrono_tz::Europe::London) <= at("2026-10-16 15:00"));
        assert!(
            b.start(chrono_tz::Europe::London) >= at("2026-10-12 13:10")
                || b.end(chrono_tz::Europe::London) <= at("2026-10-12 08:50")
        );
        assert!(b.meta.flex && b.meta.created_by == Some(Author::Scheduler));
    }
}

#[test]
fn b4_sheet_that_does_not_fit_reports_shortfall() {
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e.create_task(Author::Claude, &b4(60 * 40, "tue 17:00")).unwrap();
    let plan = r.plan.unwrap();
    assert_eq!(plan.shortfalls.len(), 1);
    assert!(
        plan.shortfalls[0].message.contains("short before Tue 13 Oct 17:00"),
        "{}",
        plan.shortfalls[0].message
    );
    assert!(plan.summary.contains("! B4 problem sheet"));
}

#[test]
fn task_squeezed_into_the_last_two_hours_is_planned_with_a_warning() {
    // Busy 08:00–17:30; due 20:00. Before the 2 h lead (18:00) there's no
    // room for the hour (and a break is due after 9.5 h of labs), so it runs
    // into the lead: planned, but warned about.
    let env = Env::new(true);
    env.phone_event("Engineering", "busy", "Labs", "2026-10-12 08:00", "2026-10-12 17:30");
    let mut e = env.engine();
    let r = e.create_task(Author::Claude, &b4(60, "today 20:00")).unwrap();
    let plan = r.plan.unwrap();
    assert!(plan.shortfalls.is_empty(), "{:?}", plan.shortfalls);
    assert_eq!(
        plan.created
            .iter()
            .map(|b| (b.end - b.start).num_minutes())
            .sum::<i64>(),
        60
    );
    assert_eq!(plan.squeezed.len(), 1, "{plan:?}");
    assert!(
        plan.summary
            .contains("⚠ B4 problem sheet: 1 h squeezed into the last 2 h before Mon 12 Oct 20:00"),
        "{}",
        plan.summary
    );
}

#[test]
fn unrelated_task_does_not_move_blocks() {
    let env = Env::new(true);
    let mut e = env.engine();
    e.create_task(Author::Claude, &b4(240, "fri 17:00")).unwrap();
    let r = e
        .create_task(
            Author::Claude,
            &CreateTaskReq {
                title: "Careers essay".into(),
                key: "Personal".into(),
                estimate_min: 90,
                due: Some("2026-10-23".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let plan = r.plan.unwrap();
    assert_eq!(plan.moved, 0);
    assert!(plan.deleted.is_empty());
    assert!(!plan.created.is_empty());
}

#[test]
fn new_fixed_event_on_a_block_moves_only_that_block() {
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e.create_task(Author::Claude, &b4(240, "fri 17:00")).unwrap();
    let first = r.plan.unwrap().created[0].clone();
    // Alec books something on his phone over the first block.
    let s = first
        .start
        .with_timezone(&chrono_tz::Europe::London)
        .format("%Y-%m-%d %H:%M")
        .to_string();
    let en = first
        .end
        .with_timezone(&chrono_tz::Europe::London)
        .format("%Y-%m-%d %H:%M")
        .to_string();
    env.phone_event("Personal", "clash", "Haircut", &s, &en);
    let p = e.plan(false, Author::Scheduler).unwrap();
    assert_eq!(p.moved, 1, "{}", p.summary);
    assert_eq!(p.deleted.len(), 1);
    assert_eq!(p.created.len(), 1);
}

#[test]
fn complete_task_removes_future_blocks() {
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e.create_task(Author::Claude, &b4(240, "fri 17:00")).unwrap();
    assert!(blocks_of(&mut e, &r.task.id) > 0);
    let done = e.complete_task(Author::Alec, &r.task.id, Some(200)).unwrap();
    assert_eq!(done.task.status, TaskStatus::Done);
    assert_eq!(done.changesets[0].status, Status::Applied);
    // The blocks go with the completion itself (one undo restores them).
    assert_eq!(blocks_of(&mut e, &r.task.id), 0);
    assert_eq!(e.db.estimates().unwrap()[0].actual_min, 200);
}

#[test]
fn dropping_a_task_needs_approval() {
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e.create_task(Author::Claude, &b4(60, "fri 17:00")).unwrap();
    let d = e.drop_task(Author::Claude, &r.task.id).unwrap();
    assert_eq!(d.changesets[0].status, Status::Pending);
    assert_eq!(e.db.task_get(&r.task.id).unwrap().unwrap().status, TaskStatus::Open);
    e.approve(&d.changesets[0].changeset_id).unwrap();
    assert_eq!(e.db.task_get(&r.task.id).unwrap().unwrap().status, TaskStatus::Dropped);
    // Approving removes every one of its blocks.
    assert_eq!(blocks_of(&mut e, &r.task.id), 0);
}

fn blocks_of(e: &mut kairosd::engine::Engine, task_id: &str) -> usize {
    e.world(30)
        .unwrap()
        .instances
        .iter()
        .filter(|i| i.meta.kind == EventKind::TaskBlock && i.meta.task_id.as_deref() == Some(task_id))
        .count()
}

#[test]
fn dropping_a_task_removes_all_its_blocks_at_once() {
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e.create_task(Author::Claude, &b4(240, "fri 17:00")).unwrap();
    let id = r.task.id.clone();
    // A pinned block Alec placed himself goes too.
    e.create_event(
        Author::Alec,
        &CreateEventReq {
            title: "B4 problem sheet".into(),
            start: "2026-10-15T19:00".into(),
            duration_min: Some(60),
            key: Some("Engineering".into()),
            task_id: Some(id.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    let n = blocks_of(&mut e, &id);
    assert!(n >= 2, "{n}");
    // Alec drops it in the TUI: applies directly, one changeset.
    e.direct = true;
    let d = e.drop_task(Author::Alec, &id).unwrap();
    e.direct = false;
    assert_eq!(d.changesets.len(), 1);
    assert_eq!(d.changesets[0].status, Status::Applied);
    assert!(
        d.changesets[0].summary.contains(&format!("{n} scheduled blocks")),
        "{}",
        d.changesets[0].summary
    );
    assert_eq!(blocks_of(&mut e, &id), 0);
    // One undo brings the task (with its due date) and every block back.
    e.undo(&d.changesets[0].changeset_id).unwrap();
    let t = e.db.task_get(&id).unwrap().unwrap();
    assert_eq!(t.status, TaskStatus::Open);
    assert!(t.due.is_some());
    assert_eq!(blocks_of(&mut e, &id), n);
}

#[test]
fn deleting_a_deadline_removes_its_task_and_planned_time() {
    // Alec deleted a test Tute Sheet ⚑ and its blocks stayed (2026-10-13).
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e.create_task(Author::Claude, &b4(240, "fri 17:00")).unwrap();
    let id = r.task.id.clone();
    let n = blocks_of(&mut e, &id);
    assert!(n >= 2, "{n}");
    // A ⚑ event from before K5, still linked to its task.
    let deadline = "old-deadline".to_string();
    env.legacy_deadline("Engineering", &deadline, "B4 problem sheet", "2026-10-16 17:00", &id);
    let mut t = e.db.task_get(&id).unwrap().unwrap();
    t.deadline_uid = Some(deadline.clone());
    e.db.task_put(&t).unwrap();
    e.direct = true;
    let cs = e.delete_event(Author::Alec, None, &deadline).unwrap();
    e.direct = false;
    assert_eq!(cs.status, Status::Applied);
    assert!(
        cs.summary.contains("with the task and its planned time"),
        "{}",
        cs.summary
    );
    assert_eq!(e.db.task_get(&id).unwrap().unwrap().status, TaskStatus::Dropped);
    assert_eq!(blocks_of(&mut e, &id), 0);
    // Nothing is planned for it again.
    let p = e.plan(false, Author::Scheduler).unwrap();
    assert!(p.created.iter().all(|b| b.task_id != id));
    // One undo brings the deadline, the task and its blocks back.
    e.undo(&cs.id).unwrap();
    assert_eq!(e.db.task_get(&id).unwrap().unwrap().status, TaskStatus::Open);
    assert_eq!(blocks_of(&mut e, &id), n);
}

#[test]
fn big_replans_of_work_blocks_apply_without_approval() {
    // Scheduler-owned blocks are exempt from the 8-op limit (2026-10-12).
    let env = Env::new(true);
    let mut e = env.engine();
    // 12 h in 1 h chunks → ≥ 9 blocks → more than 8 ops.
    let r = e
        .create_task(
            Author::Claude,
            &CreateTaskReq {
                title: "Revision".into(),
                key: "Engineering".into(),
                estimate_min: 12 * 60,
                due: Some("2026-10-23".into()),
                max_chunk_min: Some(60),
                ..Default::default()
            },
        )
        .unwrap();
    let plan = r.plan.unwrap();
    assert!(plan.created.len() > 8, "{}", plan.created.len());
    let cs = plan.changeset.unwrap();
    assert_eq!(cs.status, Status::Applied, "{:?}", cs.reasons);
}

#[test]
fn checkin_missed_returns_time_and_done_consumes_it() {
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e.create_task(Author::Claude, &b4(180, "fri 17:00")).unwrap();
    let blocks = r.plan.unwrap().created;
    // Jump to after the first block ends.
    let after_first = blocks[0].end + Duration::minutes(5);
    *env.now.lock().unwrap() = after_first;
    let items = e.checkin_items().unwrap();
    assert_eq!(items.len(), 1);
    let uid = items[0].block_uid.clone();
    let p = e
        .checkin(
            Author::Alec,
            &CheckinReq {
                block_uid: uid,
                outcome: "missed".into(),
                actual_min: None,
                remaining_min: None,
                not_before: None,
            },
        )
        .unwrap();
    let t = e.db.task_get(&r.task.id).unwrap().unwrap();
    assert_eq!(t.remaining_min, 180, "missed time returns to remaining");
    let created: i64 = p.created.iter().map(|b| (b.end - b.start).num_minutes()).sum();
    assert_eq!(
        created,
        (blocks[0].end - blocks[0].start).num_minutes(),
        "replan re-places the missed time"
    );

    // Next block: done.
    let world = e.world(14).unwrap();
    let next = world
        .instances
        .iter()
        .filter(|i| i.meta.kind == EventKind::TaskBlock && i.start(chrono_tz::Europe::London) > after_first)
        .min_by_key(|i| i.start(chrono_tz::Europe::London))
        .unwrap()
        .clone();
    *env.now.lock().unwrap() = next.end(chrono_tz::Europe::London) + Duration::minutes(1);
    e.checkin(
        Author::Alec,
        &CheckinReq {
            block_uid: next.uid.clone(),
            outcome: "done".into(),
            actual_min: None,
            remaining_min: None,
            not_before: None,
        },
    )
    .unwrap();
    let t = e.db.task_get(&r.task.id).unwrap().unwrap();
    let mins = (next.end(chrono_tz::Europe::London) - next.start(chrono_tz::Europe::London)).num_minutes() as u32;
    assert_eq!(t.remaining_min, 180 - mins);
}

#[test]
fn unanswered_blocks_wait_for_a_check_in_then_count_after_a_week() {
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e.create_task(Author::Claude, &b4(120, "fri 17:00")).unwrap();
    let first = r.plan.unwrap().created[0].clone();
    let day_after = first.start.with_timezone(&chrono_tz::Europe::London).date_naive() + Duration::days(1);
    // 10:00 the next morning: nothing was decided for Alec — it's still waiting.
    env.set_now(&format!("{day_after} 10:00"));
    assert_eq!(e.settle().unwrap(), 0);
    assert!(e.checkin_items().unwrap().iter().any(|i| i.outcome.is_none()));
    // A week on, it counts as done.
    env.set_now(&format!("{} 10:00", day_after + Duration::days(7)));
    assert!(e.settle().unwrap() >= 1);
    let t = e.db.task_get(&r.task.id).unwrap().unwrap();
    assert!(t.remaining_min < 120);
}

#[test]
fn deadline_deletion_needs_approval() {
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e.create_task(Author::Claude, &b4(60, "fri 17:00")).unwrap();
    // A ⚑ event from before K5, still linked to its task.
    let uid = "old-deadline".to_string();
    env.legacy_deadline("Engineering", &uid, "B4 problem sheet", "2026-10-16 17:00", &r.task.id);
    let mut t = e.db.task_get(&r.task.id).unwrap().unwrap();
    t.deadline_uid = Some(uid.clone());
    e.db.task_put(&t).unwrap();
    let cs = e.delete_event(Author::Claude, None, &uid).unwrap();
    assert_eq!(cs.status, Status::Pending);
    // A task's ⚑ takes the task with it, so it's a task drop (still Alec's call).
    assert!(
        cs.reasons.iter().any(|x| x.contains("dropping a task")),
        "{:?}",
        cs.reasons
    );
    assert_eq!(e.db.task_get(&r.task.id).unwrap().unwrap().status, TaskStatus::Open);
}

#[test]
fn changing_due_moves_the_mark_and_retires_an_old_deadline_event() {
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e.create_task(Author::Claude, &b4(60, "fri 17:00")).unwrap();
    env.legacy_deadline(
        "Engineering",
        "old-deadline",
        "B4 problem sheet",
        "2026-10-16 17:00",
        &r.task.id,
    );
    let mut t = e.db.task_get(&r.task.id).unwrap().unwrap();
    t.deadline_uid = Some("old-deadline".into());
    e.db.task_put(&t).unwrap();
    // You change the date (removing an old ⚑ event is a deadline delete: yours to make).
    e.direct = true;
    let u = e
        .update_task(
            Author::Alec,
            &r.task.id,
            &kairos_core::api::UpdateTaskReq {
                due: Some("2026-10-20 12:00".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(u.changesets[0].status, Status::Applied);
    // The old ⚑ event goes; the date lives on the task, where the mark is drawn from.
    assert!(!env.read_all("Engineering").contains("⚑"));
    let t = e.db.task_get(&r.task.id).unwrap().unwrap();
    assert_eq!(t.deadline_uid, None);
    assert_eq!(t.due.map(|d| d.instant(e.tz())), Some(at("2026-10-20 12:00")));
}

#[test]
fn old_deadline_events_migrate_to_due_dates_in_one_changeset() {
    let env = Env::new(true);
    let mut e = env.engine();
    assert!(
        e.migrate_deadlines().unwrap().is_none(),
        "nothing to migrate proposes nothing"
    );
    let r = e.create_task(Author::Claude, &b4(60, "fri 17:00")).unwrap();
    env.legacy_deadline(
        "Engineering",
        "linked",
        "B4 problem sheet",
        "2026-10-16 17:00",
        &r.task.id,
    );
    env.legacy_deadline("Personal", "loose", "G-Research closes", "2026-10-30 23:00", "gone");
    let cs = e.migrate_deadlines().unwrap().unwrap();
    // Claude proposes, you approve: it deletes events.
    assert_eq!(cs.status, Status::Pending);
    assert!(cs.summary.contains("2 ⚑ event(s)"), "{}", cs.summary);
    assert!(e.migrate_deadlines().unwrap().is_none(), "proposed once");
    e.direct = true;
    e.approve(&cs.id).unwrap();
    assert!(!env.read_all("Engineering").contains("⚑") && !env.read_all("Personal").contains("⚑"));
    let loose =
        e.db.tasks(None)
            .unwrap()
            .into_iter()
            .find(|t| t.title == "G-Research closes")
            .unwrap();
    assert_eq!(loose.key, "Personal");
    assert_eq!(loose.estimate_min, 0);
    assert_eq!(loose.due.map(|d| d.instant(e.tz())), Some(at("2026-10-30 23:00")));
}

#[test]
fn find_free_respects_events_and_day_window() {
    let env = Env::new(true);
    env.phone_event("Engineering", "lec", "Lecture", "2026-10-13 09:00", "2026-10-13 11:00");
    let e = env.engine();
    let slots = e.find_free(at("2026-10-13 00:00"), at("2026-10-14 00:00"), 30).unwrap();
    assert_eq!(slots[0].start, at("2026-10-13 08:30"));
    assert_eq!(slots[0].end, at("2026-10-13 09:00"));
    assert_eq!(slots[1].start, at("2026-10-13 11:00"));
    assert_eq!(slots.last().unwrap().end, at("2026-10-13 22:00"));
}

#[test]
fn empty_due_means_no_deadline() {
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e
        .create_task(
            Author::Alec,
            &CreateTaskReq {
                title: "Read paper".into(),
                key: "Personal".into(),
                estimate_min: 60,
                due: Some(String::new()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(r.task.due.is_none() && r.task.deadline_uid.is_none());
}

#[test]
fn import_timetable_is_one_undoable_changeset() {
    let env = Env::new(true);
    let mut e = env.engine();
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/edge/recur_count_until_exdate.ics"
    ))
    .unwrap();
    let cs = e
        .import_ics(Author::Alec, &text, "Engineering", "timetable.ics")
        .unwrap();
    // Recurring events → needs approval.
    assert_eq!(cs.status, Status::Pending);
    assert!(cs.reasons.iter().any(|r| r.contains("recurring")));
    e.approve(&cs.id).unwrap();
    assert_eq!(env.items("Engineering").len(), 2);
    let t = env.read_all("Engineering");
    assert!(t.contains("X-KAIROS-ADOPTED-FROM:import:timetable.ics") && t.contains("BEGIN:VTIMEZONE"));
    // Re-importing skips what's there.
    assert!(matches!(
        e.import_ics(Author::Alec, &text, "Engineering", "timetable.ics"),
        Err(EngineError::BadRequest(_))
    ));
    e.undo(&cs.id).unwrap();
    assert!(env.items("Engineering").is_empty());
}

#[test]
fn family_events_do_not_block_time_unless_marked() {
    let env = Env::new(true);
    env.phone_event(
        "Family",
        "fam-day",
        "Family lunch",
        "2026-10-12 09:00",
        "2026-10-12 18:00",
    );
    let mut e = env.engine();
    let (from, to) = (at("2026-10-12 09:00"), at("2026-10-12 18:00"));
    let free = e.find_free(from, to, 60).unwrap();
    assert!(!free.is_empty(), "Family time is free for planning by default");
    // Opting this one event in (TUI `B`) makes it block.
    let req = kairos_core::api::UpdateEventReq {
        blocking: Some(true),
        ..Default::default()
    };
    assert!(e.update_event(Author::Alec, None, "fam-day", &req).unwrap().is_none());
    let free = e.find_free(from, to, 60).unwrap();
    assert!(free.is_empty(), "{free:?}");
}

// ------------------------------------------------------------ 2026-10-12 scheme

fn london(t: chrono::DateTime<chrono::Utc>) -> String {
    t.with_timezone(&chrono_tz::Europe::London)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

#[test]
fn task_types_set_block_sizes_and_default_estimates() {
    let env = Env::new(true);
    let mut e = env.engine();
    let req = |kind: &str, est: u32| CreateTaskReq {
        title: format!("a {kind}"),
        key: "Engineering".into(),
        estimate_min: est,
        kind: Some(kind.into()),
        due: Some("fri 17:00".into()),
        ..Default::default()
    };
    // A sheet with no estimate: 5 h, in 2–3 h blocks.
    let r = e.create_task(Author::Claude, &req("sheet", 0)).unwrap();
    assert_eq!(r.task.kind.as_deref(), Some("sheet"));
    assert_eq!(r.task.estimate_min, 300);
    for b in &r.plan.unwrap().created {
        assert!((120..=180).contains(&(b.end - b.start).num_minutes()), "{b:?}");
    }
    // Admin: one go.
    let r = e.create_task(Author::Claude, &req("admin", 0)).unwrap();
    assert_eq!((r.task.estimate_min, r.task.splittable), (45, false));
    // Coursework needs an estimate; unknown types are refused with the list.
    assert!(matches!(
        e.create_task(Author::Claude, &req("coursework", 0)),
        Err(EngineError::BadRequest(m)) if m.contains("estimate required for coursework")
    ));
    assert!(matches!(
        e.create_task(Author::Claude, &req("essay", 60)),
        Err(EngineError::BadRequest(m)) if m.contains("sheet, coursework, admin")
    ));
    // No type: up to an hour is admin, longer is a sheet.
    let mut r0 = req("x", 30);
    r0.kind = None;
    assert_eq!(
        e.create_task(Author::Claude, &r0).unwrap().task.kind.as_deref(),
        Some("admin")
    );
}

#[test]
fn estimates_are_learned_per_type() {
    let env = Env::new(true);
    let mut e = env.engine();
    // Three sheets that each took 1.5× the estimate.
    for i in 0..3 {
        let r = e
            .create_task(
                Author::Claude,
                &CreateTaskReq {
                    title: format!("old sheet {i}"),
                    key: "Engineering".into(),
                    estimate_min: 240,
                    kind: Some("sheet".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        e.complete_task(Author::Claude, &r.task.id, Some(360)).unwrap();
    }
    let r = e
        .create_task(
            Author::Claude,
            &CreateTaskReq {
                title: "B4 sheet".into(),
                key: "Engineering".into(),
                estimate_min: 240,
                kind: Some("sheet".into()),
                due: Some("2026-10-23".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let plan = r.plan.unwrap();
    let a = plan.adjusted.iter().find(|a| a.title == "B4 sheet").expect("adjusted");
    assert_eq!((a.remaining_min, a.planned_min, a.samples), (240, 360, 3));
    assert!(
        plan.summary
            .contains("≈ B4 sheet: planning 6 h for 4 h left (sheet tasks usually take ×1.50)"),
        "{}",
        plan.summary
    );
    let planned: i64 = plan
        .created
        .iter()
        .filter(|b| b.title == "B4 sheet")
        .map(|b| (b.end - b.start).num_minutes())
        .sum();
    assert_eq!(planned, 360);
}

#[test]
fn coursework_checkin_asks_and_takes_what_is_left() {
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e
        .create_task(
            Author::Claude,
            &CreateTaskReq {
                title: "3YP report".into(),
                key: "Engineering".into(),
                estimate_min: 360,
                kind: Some("coursework".into()),
                due: Some("2026-10-23".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let first = r.plan.unwrap().created[0].clone();
    *env.now.lock().unwrap() = first.end + Duration::minutes(5);
    let items = e.checkin_items().unwrap();
    assert!(items[0].ask_remaining);
    // Done, and it turns out 8 h are left (it grew).
    e.checkin(
        Author::Alec,
        &CheckinReq {
            block_uid: items[0].block_uid.clone(),
            outcome: "done".into(),
            actual_min: None,
            remaining_min: Some(480),
            not_before: None,
        },
    )
    .unwrap();
    let t = e.db.task_get(&r.task.id).unwrap().unwrap();
    assert_eq!(t.remaining_min, 480);
    let world = e.world(30).unwrap();
    let ahead: i64 = world
        .instances
        .iter()
        .filter(|i| i.meta.task_id.as_deref() == Some(r.task.id.as_str()) && i.meta.kind == EventKind::TaskBlock)
        .filter(|i| i.start(chrono_tz::Europe::London) > first.end)
        .map(|i| (i.end(chrono_tz::Europe::London) - i.start(chrono_tz::Europe::London)).num_minutes())
        .sum();
    assert_eq!(ahead, 480, "the replan places what's left");
}

#[test]
fn a_new_event_moves_work_out_of_the_way_without_approval() {
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e.create_task(Author::Claude, &b4(240, "fri 17:00")).unwrap();
    let first = r.plan.unwrap().created[0].clone();
    // Alec adds a social on top of it (through kairos), starting in 30 min:
    // replanning is flagged and the work moves right away, no approval.
    *env.now.lock().unwrap() = first.start - Duration::minutes(30);
    e.direct = true;
    let w = e
        .create_event(
            Author::Alec,
            &CreateEventReq {
                title: "Drinks".into(),
                start: first.start.to_rfc3339(),
                end: Some(first.end.to_rfc3339()),
                key: Some("Personal".into()),
                ..Default::default()
            },
        )
        .unwrap();
    e.direct = false;
    assert_eq!(w.status, Status::Applied);
    assert!(e.replan_soon);
    let p = e.replan_if_needed().unwrap().unwrap();
    assert_eq!(p.moved, 1, "{}", p.summary);
    assert_eq!(p.changeset.unwrap().status, Status::Applied);
    assert!(!e.replan_soon);
    assert!(e.replan_if_needed().unwrap().is_none());
}

#[test]
fn moving_a_work_block_by_hand_pins_it() {
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e.create_task(Author::Claude, &b4(120, "fri 17:00")).unwrap();
    let b = r.plan.unwrap().created[0].clone();
    let world = e.world(14).unwrap();
    let inst = world
        .instances
        .iter()
        .find(|i| i.meta.kind == EventKind::TaskBlock && i.start(chrono_tz::Europe::London) == b.start)
        .unwrap()
        .clone();
    e.direct = true;
    let to = at("2026-10-15 20:00");
    e.update_event(
        Author::Alec,
        Some(&inst.source),
        &inst.uid,
        &UpdateEventReq {
            start: Some(to.to_rfc3339()),
            ..Default::default()
        },
    )
    .unwrap();
    e.direct = false;
    let m = e.db.meta_get(&inst.source, &inst.uid).unwrap().unwrap();
    assert!(m.pinned && !m.flex, "{m:?}");
    // A replan leaves it where Alec put it.
    let p = e.plan(false, Author::Scheduler).unwrap();
    assert_eq!(p.moved, 0, "{}", p.summary);
    let world = e.world(14).unwrap();
    let now_at = world.instances.iter().find(|i| i.uid == inst.uid).unwrap();
    assert_eq!(london(now_at.start(chrono_tz::Europe::London)), "2026-10-15 20:00");
}

#[test]
fn weekly_term_time_event_is_one_series_per_term() {
    let env = Env::new(true);
    let mut e = env.engine();
    let req = CreateEventReq {
        title: "Futsal training".into(),
        start: "2026-10-15T07:00".into(),
        end: Some("2026-10-15T09:00".into()),
        key: Some("Sport".into()),
        repeat: Some(kairos_core::api::Repeat {
            every: Some("week".into()),
            term_only: true,
            until: None,
        }),
        ..Default::default()
    };
    // From Claude: recurring → Alec approves.
    let cs = e.create_event(Author::Claude, &req).unwrap();
    assert_eq!(cs.status, Status::Pending);
    assert!(cs.reasons.iter().any(|r| r.contains("recurring")), "{:?}", cs.reasons);
    e.reject(&cs.id).unwrap();
    // Alec at the keyboard: applied, three series (Michaelmas, Hilary, Trinity).
    e.direct = true;
    let cs = e.create_event(Author::Alec, &req).unwrap();
    e.direct = false;
    assert_eq!(cs.status, Status::Applied);
    assert_eq!(cs.ops.len(), 3);
    assert!(
        cs.summary.contains("weekly \"Futsal training\"") && cs.summary.contains("term time (3 terms)"),
        "{}",
        cs.summary
    );
    let all = env.read_all("Sport");
    assert!(all.contains("RRULE:FREQ=WEEKLY;UNTIL=20261205T235959Z"), "{all}");
    assert!(
        all.contains("DTSTART;TZID=Europe/London:20270114T070000"),
        "first Thursday of Hilary"
    );
    assert!(
        all.contains("DTSTART;TZID=Europe/London:20270422T070000"),
        "first Thursday of Trinity"
    );
    // Expanded: Thursdays in term at 07:00 local (DST-safe), none in the vacation.
    let world = e.world(400).unwrap();
    let starts: Vec<String> = world
        .instances
        .iter()
        .filter(|i| i.title == "Futsal training")
        .map(|i| london(i.start(chrono_tz::Europe::London)))
        .collect();
    assert!(starts.contains(&"2026-10-15 07:00".to_string()));
    assert!(
        starts.contains(&"2026-10-29 07:00".to_string()),
        "after the clocks change"
    );
    assert!(starts.contains(&"2026-12-03 07:00".to_string()));
    assert!(
        !starts
            .iter()
            .any(|s| s.starts_with("2026-12-10") || s.starts_with("2026-12-31"))
    );
    assert!(starts.contains(&"2027-01-14 07:00".to_string()));
}

#[test]
fn start_now_puts_a_block_from_now_and_reflows_the_rest() {
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e.create_task(Author::Claude, &b4(240, "fri 17:00")).unwrap();
    assert!(r.plan.unwrap().created.len() >= 2);
    // A lecture at 11:00; it's 10:02 and Alec wants to start now.
    env.phone_event("Engineering", "lec", "Lecture", "2026-10-12 11:00", "2026-10-12 12:00");
    env.set_now("2026-10-12 10:02");
    e.direct = true;
    let s = e.start_task(Author::Alec, &r.task.id, None).unwrap();
    e.direct = false;
    assert_eq!(s.changesets[0].status, Status::Applied);
    let world = e.world(14).unwrap();
    let blocks: Vec<_> = world
        .instances
        .iter()
        .filter(|i| i.meta.task_id.as_deref() == Some(r.task.id.as_str()) && i.meta.kind == EventKind::TaskBlock)
        .collect();
    let now_block = blocks
        .iter()
        .find(|i| london(i.start(chrono_tz::Europe::London)) == "2026-10-12 10:00")
        .expect("a block from now (rounded down to the 10-min slot)");
    // Stops when the lecture starts; pinned so the scheduler leaves it.
    assert_eq!(london(now_block.end(chrono_tz::Europe::London)), "2026-10-12 11:00");
    assert!(now_block.meta.pinned);
    // The rest still adds up to the estimate (no double counting).
    let total: i64 = blocks
        .iter()
        .map(|i| (i.end(chrono_tz::Europe::London) - i.start(chrono_tz::Europe::London)).num_minutes())
        .sum();
    assert_eq!(
        total,
        240,
        "{:?}",
        blocks
            .iter()
            .map(|i| london(i.start(chrono_tz::Europe::London)))
            .collect::<Vec<_>>()
    );
    // No room at all: a clear refusal.
    env.set_now("2026-10-12 10:55");
    assert!(matches!(
        e.start_task(Author::Alec, &r.task.id, None),
        Err(EngineError::BadRequest(m)) if m.contains("no time now")
    ));
}

#[test]
fn tasks_never_finish_by_themselves() {
    // All of a 2 h task's blocks pass unanswered: they count as done, but the
    // task stays open (0 left) and asks — it's not silently dropped.
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e.create_task(Author::Claude, &b4(120, "fri 17:00")).unwrap();
    let last = r.plan.unwrap().created.iter().map(|b| b.end).max().unwrap();
    let day_after = last.with_timezone(&chrono_tz::Europe::London).date_naive() + Duration::days(8);
    env.set_now(&format!("{day_after} 09:30"));
    e.settle().unwrap();
    let t = e.db.task_get(&r.task.id).unwrap().unwrap();
    assert_eq!((t.status, t.remaining_min), (TaskStatus::Open, 0));
    assert!(e.db.estimates().unwrap().is_empty(), "nothing learned from a guess");
    // Alec says it needs 2 h more (deadline moved on, the old one is past): planned again.
    let u = e
        .update_task(
            Author::Alec,
            &r.task.id,
            &kairos_core::api::UpdateTaskReq {
                remaining_min: Some(120),
                due: Some(format!("{} 17:00", day_after + Duration::days(7))),
                ..Default::default()
            },
        )
        .unwrap();
    let more: i64 = u
        .plan
        .unwrap()
        .created
        .iter()
        .map(|b| (b.end - b.start).num_minutes())
        .sum();
    assert_eq!(more, 120);
}

#[test]
fn last_block_checkin_asks_finished_and_learned_time_isnt_dropped() {
    let env = Env::new(true);
    let mut e = env.engine();
    // Sheets take 1.5× the estimate.
    for i in 0..3 {
        let r = e
            .create_task(
                Author::Claude,
                &CreateTaskReq {
                    title: format!("old {i}"),
                    key: "Engineering".into(),
                    estimate_min: 240,
                    kind: Some("sheet".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        e.complete_task(Author::Claude, &r.task.id, Some(360)).unwrap();
    }
    let r = e
        .create_task(
            Author::Claude,
            &CreateTaskReq {
                title: "B4 sheet".into(),
                key: "Engineering".into(),
                estimate_min: 120,
                kind: Some("sheet".into()),
                due: Some("2026-10-23".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let blocks = r.plan.unwrap().created;
    let planned: i64 = blocks.iter().map(|b| (b.end - b.start).num_minutes()).sum();
    assert_eq!(planned, 180, "2 h × 1.5");
    let first = blocks.iter().min_by_key(|b| b.start).unwrap().clone();
    // One 3 h block: doing it uses up the 2 h estimate → the check-in asks.
    *env.now.lock().unwrap() = first.end + Duration::minutes(1);
    let items = e.checkin_items().unwrap();
    let it = items.iter().find(|i| i.title == "B4 sheet").unwrap();
    assert!(it.ask_remaining);
    // "Done, and that's it": finished, and learned from.
    e.checkin(
        Author::Alec,
        &CheckinReq {
            block_uid: it.block_uid.clone(),
            outcome: "done".into(),
            actual_min: None,
            remaining_min: Some(0),
            not_before: None,
        },
    )
    .unwrap();
    let t = e.db.task_get(&r.task.id).unwrap().unwrap();
    assert_eq!(t.status, TaskStatus::Done);
    assert!(e.db.estimates().unwrap().iter().any(|x| x.title == "B4 sheet"));
}

#[test]
fn done_blocks_use_up_the_estimate_at_the_learned_rate() {
    let env = Env::new(true);
    let mut e = env.engine();
    for i in 0..3 {
        let r = e
            .create_task(
                Author::Claude,
                &CreateTaskReq {
                    title: format!("old {i}"),
                    key: "Engineering".into(),
                    estimate_min: 240,
                    kind: Some("sheet".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        e.complete_task(Author::Claude, &r.task.id, Some(360)).unwrap();
    }
    let r = e
        .create_task(
            Author::Claude,
            &CreateTaskReq {
                title: "B4 sheet".into(),
                key: "Engineering".into(),
                estimate_min: 240,
                kind: Some("sheet".into()),
                due: Some("2026-10-23".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let first = r.plan.unwrap().created.iter().min_by_key(|b| b.start).unwrap().clone();
    let len = (first.end - first.start).num_minutes();
    *env.now.lock().unwrap() = first.end + Duration::minutes(1);
    let uid = e.checkin_items().unwrap()[0].block_uid.clone();
    e.checkin(
        Author::Alec,
        &CheckinReq {
            block_uid: uid,
            outcome: "done".into(),
            actual_min: None,
            remaining_min: None,
            not_before: None,
        },
    )
    .unwrap();
    let t = e.db.task_get(&r.task.id).unwrap().unwrap();
    assert_eq!(i64::from(t.remaining_min), 240 - (len as f64 / 1.5).round() as i64);
    // The rest of the 6 h is still planned (not dropped early).
    let world = e.world(30).unwrap();
    let ahead: i64 = world
        .instances
        .iter()
        .filter(|i| i.meta.task_id.as_deref() == Some(r.task.id.as_str()) && i.meta.kind == EventKind::TaskBlock)
        .filter(|i| i.start(chrono_tz::Europe::London) > first.end)
        .map(|i| (i.end(chrono_tz::Europe::London) - i.start(chrono_tz::Europe::London)).num_minutes())
        .sum();
    assert_eq!(ahead + len, 360);
}

// ------------------------------------------------------------ 2026-10-13 gap fixes

fn block_starts(e: &mut kairosd::engine::Engine, task: &str) -> Vec<String> {
    let world = e.world(30).unwrap();
    let mut v: Vec<String> = world
        .instances
        .iter()
        .filter(|i| i.meta.task_id.as_deref() == Some(task) && i.meta.kind == EventKind::TaskBlock)
        .map(|i| london(i.start(chrono_tz::Europe::London)))
        .collect();
    v.sort();
    v
}

#[test]
fn undo_puts_back_the_change_and_the_replan_it_caused() {
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e.create_task(Author::Claude, &b4(240, "fri 17:00")).unwrap();
    let before = block_starts(&mut e, &r.task.id);
    let first = r.plan.unwrap().created.iter().min_by_key(|b| b.start).unwrap().clone();
    e.direct = true;
    let cs = e
        .create_event(
            Author::Alec,
            &CreateEventReq {
                title: "Drinks".into(),
                start: first.start.to_rfc3339(),
                end: Some(first.end.to_rfc3339()),
                key: Some("Personal".into()),
                ..Default::default()
            },
        )
        .unwrap();
    e.direct = false;
    e.replan_if_needed().unwrap();
    assert_ne!(
        block_starts(&mut e, &r.task.id),
        before,
        "the block moved out of the way"
    );
    e.undo_with_replans(&cs.id).unwrap();
    assert!(!env.read_all("Personal").contains("SUMMARY:Drinks"));
    assert_eq!(block_starts(&mut e, &r.task.id), before, "and came back");
}

#[test]
fn renaming_or_rekeying_a_task_carries_to_its_blocks_and_deadline() {
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e.create_task(Author::Claude, &b4(240, "fri 17:00")).unwrap();
    let u = e
        .update_task(
            Author::Alec,
            &r.task.id,
            &kairos_core::api::UpdateTaskReq {
                title: Some("B4 sheet 3".into()),
                key: Some("Personal".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(u.changesets[0].status, Status::Applied);
    let eng = env.read_all("Engineering");
    assert!(
        !eng.contains("B4 problem sheet"),
        "nothing left behind in Engineering:\n{eng}"
    );
    let per = env.read_all("Personal");
    assert!(!per.contains("SUMMARY:⚑"), "{per}");
    assert!(per.matches("SUMMARY:☐ B4 sheet 3").count() >= 2, "{per}");
}

#[test]
fn skipping_a_work_block_keeps_work_out_of_that_slot() {
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e.create_task(Author::Claude, &b4(180, "fri 17:00")).unwrap();
    let world = e.world(14).unwrap();
    let b = world
        .instances
        .iter()
        .filter(|i| i.meta.task_id.as_deref() == Some(r.task.id.as_str()) && i.meta.kind == EventKind::TaskBlock)
        .min_by_key(|i| i.start(chrono_tz::Europe::London))
        .unwrap()
        .clone();
    let (s, en) = b.span.bounds(chrono_tz::Europe::London);
    e.direct = true;
    let cs = e.skip_block(Author::Alec, Some(&b.source), &b.uid).unwrap();
    e.direct = false;
    assert_eq!(cs.status, Status::Applied);
    e.replan_if_needed().unwrap();
    let world = e.world(14).unwrap();
    let blocks: Vec<_> = world
        .instances
        .iter()
        .filter(|i| i.meta.task_id.as_deref() == Some(r.task.id.as_str()) && i.meta.kind == EventKind::TaskBlock)
        .collect();
    let total: i64 = blocks
        .iter()
        .map(|i| (i.end(chrono_tz::Europe::London) - i.start(chrono_tz::Europe::London)).num_minutes())
        .sum();
    assert_eq!(total, 180, "the work is planned elsewhere");
    assert!(
        blocks
            .iter()
            .all(|i| i.end(chrono_tz::Europe::London) <= s || i.start(chrono_tz::Europe::London) >= en),
        "not back in the skipped slot"
    );
    // Undo: the block is back (and the slot no longer skipped).
    e.undo_with_replans(&cs.id).unwrap();
    assert!(e.db.skips().unwrap().is_empty());
    assert!(block_starts(&mut e, &r.task.id).contains(&london(s)));
}

#[test]
fn finishing_a_task_clears_its_deadline_even_from_the_phone() {
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e.create_task(Author::Claude, &b4(60, "fri 17:00")).unwrap();
    let first = r.plan.unwrap().created[0].clone();
    *env.now.lock().unwrap() = first.end + Duration::minutes(5);
    let uid = e.checkin_items().unwrap()[0].block_uid.clone();
    // Not direct (a phone link): "0 left" finishes it, ⚑ and all, no approval.
    e.checkin(
        Author::Alec,
        &CheckinReq {
            block_uid: uid,
            outcome: "done".into(),
            actual_min: None,
            remaining_min: Some(0),
            not_before: None,
        },
    )
    .unwrap();
    let t = e.db.task_get(&r.task.id).unwrap().unwrap();
    assert_eq!(t.status, TaskStatus::Done);
    assert!(!env.read_all("Engineering").contains("SUMMARY:⚑ B4 problem sheet"));
}

#[test]
fn a_finished_task_can_be_reopened() {
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e.create_task(Author::Claude, &b4(60, "")).unwrap();
    e.complete_task(Author::Alec, &r.task.id, Some(60)).unwrap();
    assert_eq!(e.db.estimates().unwrap().len(), 1);
    let req = |reopen: Option<bool>, left: Option<u32>| kairos_core::api::UpdateTaskReq {
        reopen,
        remaining_min: left,
        ..Default::default()
    };
    // Editing a finished task without reopening is refused; reopening needs "how much".
    assert!(e.update_task(Author::Alec, &r.task.id, &req(None, Some(30))).is_err());
    assert!(e.update_task(Author::Alec, &r.task.id, &req(Some(true), None)).is_err());
    let u = e
        .update_task(Author::Alec, &r.task.id, &req(Some(true), Some(90)))
        .unwrap();
    assert_eq!((u.task.status, u.task.remaining_min), (TaskStatus::Open, 90));
    assert!(
        e.db.estimates().unwrap().is_empty(),
        "the finished run isn't learned from"
    );
    let planned: i64 = u
        .plan
        .unwrap()
        .created
        .iter()
        .map(|b| (b.end - b.start).num_minutes())
        .sum();
    assert_eq!(planned, 90);
}

#[test]
fn partly_done_block_gets_the_rest_planned_again() {
    // Alec's example: a 1 h block, only 20 min managed — 40 min go back in the plan.
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e
        .create_task(
            Author::Alec,
            &CreateTaskReq {
                title: "Purchase Furniture".into(),
                key: "Personal".into(),
                estimate_min: 60,
                due: Some("fri 17:00".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let created = r.plan.unwrap().created;
    assert_eq!(created.len(), 1);
    *env.now.lock().unwrap() = created[0].end + Duration::minutes(5);
    let uid = e.checkin_items().unwrap()[0].block_uid.clone();
    let p = e
        .checkin(
            Author::Alec,
            &CheckinReq {
                block_uid: uid,
                outcome: "partial".into(),
                actual_min: Some(20),
                remaining_min: None,
                not_before: None,
            },
        )
        .unwrap();
    let again: i64 = p.created.iter().map(|b| (b.end - b.start).num_minutes()).sum();
    assert_eq!(again, 40, "{p:?}");
    assert!(p.created.iter().all(|b| b.start > created[0].end));
    assert_eq!(e.db.task_get(&r.task.id).unwrap().unwrap().remaining_min, 40);
}

#[test]
fn took_longer_records_the_time_spent_and_plans_what_alec_says_is_left() {
    // A 2 h block took 3 h, and Alec thinks it needs 1h30 more — not before Thursday.
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e.create_task(Author::Alec, &b4(120, "fri 17:00")).unwrap();
    let created = r.plan.unwrap().created;
    assert_eq!(created.len(), 1);
    let b = created[0].clone();
    *env.now.lock().unwrap() = b.end + Duration::hours(2);
    let uid = e.checkin_items().unwrap()[0].block_uid.clone();
    let p = e
        .checkin(
            Author::Alec,
            &CheckinReq {
                block_uid: uid.clone(),
                outcome: "longer".into(),
                actual_min: Some(180),
                remaining_min: Some(90),
                not_before: Some("2026-10-15".into()),
            },
        )
        .unwrap();
    // The ✓ block shows the 3 h spent.
    let world = e.world(14).unwrap();
    let i = world.instances.iter().find(|i| i.uid == uid).unwrap();
    assert_eq!(i.title, "✓ B4 problem sheet");
    let tz = chrono_tz::Europe::London;
    assert_eq!((i.end(tz) - i.start(tz)).num_minutes(), 180);
    // What's left is what Alec said, planned from Thursday.
    let t = e.db.task_get(&r.task.id).unwrap().unwrap();
    assert_eq!((t.status, t.remaining_min), (TaskStatus::Open, 90));
    let again: i64 = p.created.iter().map(|b| (b.end - b.start).num_minutes()).sum();
    assert_eq!(again, 90, "{p:?}");
    assert!(p.created.iter().all(|b| b.start >= at("2026-10-15 00:00")), "{p:?}");
}

#[test]
fn checking_in_a_block_moved_back_over_a_lecture_needs_no_approval() {
    // Alec moved a 10 min leftover block back to when he did it (during a
    // lecture), then pressed y: his own check-in was queued for approval
    // (2026-10-14). History may overlap other events.
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e
        .create_task(
            Author::Alec,
            &CreateTaskReq {
                title: "Purchase Furniture".into(),
                key: "Personal".into(),
                estimate_min: 30,
                due: Some("fri 17:00".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let b = r.plan.unwrap().created[0].clone();
    let day = b.start.with_timezone(&chrono_tz::Europe::London).date_naive();
    env.phone_event(
        "Engineering",
        "lecture",
        "Quantum Research",
        &format!("{day} 06:00"),
        &format!("{day} 07:00"),
    );
    *env.now.lock().unwrap() = at(&format!("{day} 08:00"));
    let uid = e
        .world(7)
        .unwrap()
        .instances
        .iter()
        .find(|i| i.meta.task_id.as_deref() == Some(r.task.id.as_str()) && i.meta.kind == EventKind::TaskBlock)
        .unwrap()
        .uid
        .clone();
    e.direct = true;
    let mv = e
        .update_event(
            Author::Alec,
            None,
            &uid,
            &UpdateEventReq {
                start: Some(at(&format!("{day} 06:00")).to_rfc3339()),
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap();
    e.direct = false;
    assert_eq!(mv.status, Status::Applied);
    e.checkin(
        Author::Alec,
        &CheckinReq {
            block_uid: uid.clone(),
            outcome: "done".into(),
            actual_min: None,
            remaining_min: None,
            not_before: None,
        },
    )
    .unwrap();
    assert!(
        e.db.cs_list(Some(Status::Pending), 10).unwrap().is_empty(),
        "nothing waits for approval"
    );
    let i = e
        .world(7)
        .unwrap()
        .instances
        .into_iter()
        .find(|i| i.uid == uid)
        .unwrap();
    assert!(i.title.starts_with('✓'), "{}", i.title);
}

#[test]
fn deleting_an_event_reflows_work_into_the_freed_time() {
    // Tue is taken all day; Alec deletes it (here or on the phone): the next
    // replan moves work into Tuesday, and later replans leave it be.
    let env = Env::new(true);
    env.phone_event("Personal", "away", "Away day", "2026-10-13 08:00", "2026-10-13 22:00");
    let mut e = env.engine();
    let r = e.create_task(Author::Claude, &b4(360, "fri 17:00")).unwrap();
    let on_tue = |e: &mut kairosd::engine::Engine| {
        e.world(14)
            .unwrap()
            .instances
            .iter()
            .filter(|i| i.meta.kind == EventKind::TaskBlock && i.meta.task_id.as_deref() == Some(r.task.id.as_str()))
            .filter(|i| london(i.start(chrono_tz::Europe::London)).starts_with("2026-10-13"))
            .count()
    };
    assert_eq!(on_tue(&mut e), 0);
    e.direct = true;
    e.delete_event(Author::Alec, None, "away").unwrap();
    e.direct = false;
    e.replan_if_needed().unwrap();
    assert!(on_tue(&mut e) >= 1, "work moved into the freed day");
    let again = e.plan(false, Author::Scheduler).unwrap();
    assert!(again.created.is_empty() && again.moved == 0, "{}", again.summary);
}

#[test]
fn checked_in_blocks_show_what_happened_on_the_calendar() {
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e.create_task(Author::Claude, &b4(240, "fri 17:00")).unwrap();
    let mut blocks = r.plan.unwrap().created;
    blocks.sort_by_key(|b| b.start);
    let (b1, b2) = (blocks[0].clone(), blocks[1].clone());
    // Block 1: partly done (30 min) — trimmed to the 30 min and ticked.
    *env.now.lock().unwrap() = b1.end + Duration::minutes(5);
    let uid1 = e.checkin_items().unwrap()[0].block_uid.clone();
    e.checkin(
        Author::Alec,
        &CheckinReq {
            block_uid: uid1.clone(),
            outcome: "partial".into(),
            actual_min: Some(30),
            remaining_min: None,
            not_before: None,
        },
    )
    .unwrap();
    let world = e.world(14).unwrap();
    let i1 = world.instances.iter().find(|i| i.uid == uid1).unwrap();
    assert_eq!(i1.title, "✓ B4 problem sheet");
    assert_eq!(
        (i1.end(chrono_tz::Europe::London) - i1.start(chrono_tz::Europe::London)).num_minutes(),
        30
    );
    // It still moves (2026-10-27: it says when you did the work), and the record keeps its
    // 30 min.
    e.direct = true;
    e.update_event(
        Author::Alec,
        Some(&i1.source),
        &uid1,
        &UpdateEventReq {
            start: Some((b1.start - Duration::minutes(60)).to_rfc3339()),
            ..Default::default()
        },
    )
    .unwrap();
    e.direct = false;
    let rec = e.db.checkin_get(&uid1).unwrap().unwrap();
    assert_eq!(rec.actual_min, Some(30));
    // Block 2: missed — removed (its time is planned again).
    *env.now.lock().unwrap() = b2.end + Duration::minutes(5);
    let uid2 = e
        .checkin_items()
        .unwrap()
        .into_iter()
        .find(|c| c.outcome.is_none())
        .map(|c| c.block_uid)
        .unwrap_or_else(|| {
            // b2 may be on another day than b1.
            String::new()
        });
    if !uid2.is_empty() {
        e.checkin(
            Author::Alec,
            &CheckinReq {
                block_uid: uid2.clone(),
                outcome: "missed".into(),
                actual_min: None,
                remaining_min: None,
                not_before: None,
            },
        )
        .unwrap();
        let world = e.world(14).unwrap();
        assert!(world.instances.iter().all(|i| i.uid != uid2), "missed block removed");
    }
    // Unanswered blocks auto-done the next morning get their ✓ too.
    let day_after = b2.end.with_timezone(&chrono_tz::Europe::London).date_naive() + Duration::days(3);
    env.set_now(&format!("{day_after} 09:30"));
    e.settle().unwrap();
    assert!(env.read_all("Engineering").contains("SUMMARY:✓ B4 problem sheet"));
}

// ------------------------------------------------------------ check-ins (2026-10-13)

#[test]
fn logging_work_outside_a_block_shows_on_the_calendar_and_counts() {
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e.create_task(Author::Claude, &b4(240, "fri 17:00")).unwrap();
    env.set_now("2026-10-12 16:40");
    // Not interactive (e.g. Claude via MCP / the phone): still low-risk — it's history.
    let l = e.log_work(Author::Claude, &r.task.id, 40).unwrap();
    assert_eq!(l.changesets[0].status, Status::Applied, "{:?}", l.changesets[0]);
    assert!(env.read_all("Engineering").contains("SUMMARY:✓ B4 problem sheet"));
    let t = e.db.task_get(&r.task.id).unwrap().unwrap();
    assert_eq!(t.remaining_min, 200);
    let planned: i64 = l
        .plan
        .unwrap()
        .created
        .iter()
        .map(|b| (b.end - b.start).num_minutes())
        .sum();
    assert!(planned <= 200, "the rest replans around the logged work");
}

#[test]
fn finished_at_checkin_completes_the_task_and_earlier_blocks_wait_to_be_checked_in() {
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e.create_task(Author::Claude, &b4(240, "fri 17:00")).unwrap();
    let mut blocks = r.plan.unwrap().created;
    blocks.sort_by_key(|b| b.start);
    // A day later (before 09:00, so not auto-done yet), the first block still waits.
    let next_day = blocks[0].start.with_timezone(&chrono_tz::Europe::London).date_naive() + Duration::days(1);
    env.set_now(&format!("{next_day} 08:00"));
    let items = e.checkin_items().unwrap();
    let it = items
        .iter()
        .find(|i| i.outcome.is_none())
        .expect("yesterday's block is still listed");
    // "Finished" = done, and the whole task is.
    e.checkin(
        Author::Alec,
        &CheckinReq {
            block_uid: it.block_uid.clone(),
            outcome: "finished".into(),
            actual_min: None,
            remaining_min: None,
            not_before: None,
        },
    )
    .unwrap();
    assert_eq!(e.db.task_get(&r.task.id).unwrap().unwrap().status, TaskStatus::Done);
}

#[test]
fn project_time_is_measured_and_checked_in_and_an_hour_counts_once() {
    // A block of a task for the project, worked through Claude: the check-in and the measured
    // span describe the same hour, and it used to be counted twice.
    let env = Env::new(true);
    let mut e = env.engine();
    e.registry = Some(
        locus_contract::taxonomy::load(&locus_contract::taxonomy::root())
            .unwrap()
            .registry,
    );
    e.sync_registry().unwrap();
    env.set_now("2026-10-12 07:00");
    let r = e
        .create_task(
            Author::Alec,
            &CreateTaskReq {
                title: "locus Approve page".into(),
                key: "Engineering".into(),
                estimate_min: 300,
                due: Some("2026-10-16 17:00".into()),
                project: Some("locus".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let mut blocks = r.plan.unwrap().created;
    blocks.sort_by_key(|b| b.start);
    let (b, next) = (blocks[0].clone(), blocks[1].clone());
    let planned = (b.end - b.start).num_minutes() as u32;
    let locus = |e: &kairosd::engine::Engine| {
        e.projects()
            .unwrap()
            .into_iter()
            .find(|p| p.project.registry.as_deref() == Some("locus"))
            .unwrap()
    };
    *env.now.lock().unwrap() = b.end + Duration::minutes(5);
    let p = locus(&e);
    assert_eq!((p.week_min, p.unconfirmed_min), (0, planned), "waiting for a check-in");
    assert_eq!(p.next_block, Some(next.start));
    let uid = e
        .checkin_items()
        .unwrap()
        .into_iter()
        .find(|c| c.task_id == r.task.id)
        .unwrap()
        .block_uid;
    e.checkin(
        Author::Alec,
        &CheckinReq {
            block_uid: uid,
            outcome: "done".into(),
            actual_min: None,
            remaining_min: None,
            not_before: None,
        },
    )
    .unwrap();
    // Claude was open from half an hour before the block to an hour into it.
    e.set_worked(&kairos_core::api::WorkedReq {
        machine: "mac".into(),
        from: at("2026-10-01 00:00"),
        to: b.end + Duration::minutes(5),
        spans: vec![kairos_core::api::WorkedSpan {
            project: "locus".into(),
            start: b.start - Duration::minutes(30),
            end: b.start + Duration::minutes(60),
            activity: Vec::new(),
        }],
    })
    .unwrap();
    let p = locus(&e);
    assert_eq!(p.unconfirmed_min, 0);
    assert_eq!(
        (p.week_min, p.month_min),
        (planned + 30, planned + 30),
        "once, not {}",
        planned * 2 + 30
    );
    assert_eq!(p.last_worked, Some(b.end));
}

#[test]
fn old_project_sessions_keep_their_project_as_what_they_are_for() {
    let env = Env::new(true);
    env.phone_event("Engineering", "s1", "◆ quast", "2026-10-05 10:00", "2026-10-05 12:00");
    env.phone_event(
        "Engineering",
        "s2",
        "Quantum research",
        "2026-10-06 10:00",
        "2026-10-06 12:00",
    );
    let mut e = env.engine();
    e.registry = Some(
        locus_contract::taxonomy::load(&locus_contract::taxonomy::root())
            .unwrap()
            .registry,
    );
    e.sync_registry().unwrap();
    let quast = e
        .projects()
        .unwrap()
        .into_iter()
        .find(|p| p.project.registry.as_deref() == Some("quast"))
        .unwrap();
    let source = e
        .events(at("2026-10-05 00:00"), at("2026-10-07 00:00"))
        .unwrap()
        .into_iter()
        .find(|i| i.uid == "s1")
        .unwrap()
        .source;
    // As kairosd left them: s1 a plain session, s2 a session already linked to something else.
    e.db.session_link_put(&source, "s1", &kairos_core::model::Meta::default(), &quast.project.id)
        .unwrap();
    let m = kairos_core::model::Meta {
        locus: Some("nabla".into()),
        ..Default::default()
    };
    e.db.session_link_put(&source, "s2", &m, &quast.project.id).unwrap();
    assert_eq!(e.retire_sessions().unwrap(), 1);
    let locus_of = |uid: &str| e.db.meta_get(&source, uid).unwrap().and_then(|m| m.locus);
    assert_eq!(locus_of("s1").as_deref(), Some("quast"));
    assert_eq!(
        locus_of("s2").as_deref(),
        Some("nabla"),
        "a link it already had is kept"
    );
    assert!(e.db.retire_sessions().unwrap().is_empty(), "the old field is gone");
}

#[test]
fn not_going_frees_your_time_for_the_scheduler() {
    let env = Env::new(true);
    env.phone_event("Personal", "drinks", "Drinks", "2026-10-12 09:00", "2026-10-12 17:00");
    let mut e = env.engine();
    let busy = |e: &mut kairosd::engine::Engine| {
        e.events(at("2026-10-12 00:00"), at("2026-10-13 00:00"))
            .unwrap()
            .iter()
            .any(|i| i.uid == "drinks" && kairosd::engine::is_busy(i))
    };
    assert!(busy(&mut e));
    // You aren't going: kairos's own record, no calendar write and no approval.
    let r = e
        .update_event(
            Author::Claude,
            None,
            "drinks",
            &kairos_core::api::UpdateEventReq {
                not_going: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(r.is_none(), "no changeset: nothing on iCloud changes");
    assert!(e.replan_soon);
    assert!(!busy(&mut e));
    assert!(env.read_all("Personal").contains("SUMMARY:Drinks"), "the event stays");
    e.update_event(
        Author::Claude,
        None,
        "drinks",
        &kairos_core::api::UpdateEventReq {
            not_going: Some(false),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(busy(&mut e));
}

#[test]
fn projects_are_the_registry_repos() {
    let env = Env::new(true);
    let mut e = env.engine();
    // kairos's own list from before: one matches by name, one by a component's name, one by
    // nothing at all.
    for n in ["quast", "oxdaq", "aulos"] {
        e.create_project(
            Author::Alec,
            &kairos_core::api::CreateProjectReq {
                name: n.into(),
                ..Default::default()
            },
        )
        .unwrap();
    }
    e.registry = Some(
        locus_contract::taxonomy::load(&locus_contract::taxonomy::root())
            .unwrap()
            .registry,
    );
    let cs = e.sync_registry().unwrap().unwrap();
    assert_eq!(cs.status, Status::Applied);
    assert!(
        cs.summary.contains("quast = quast") && cs.summary.contains("oxdaq = oqts"),
        "{}",
        cs.summary
    );
    assert!(e.sync_registry().unwrap().is_none(), "nothing more to do");
    let ps = e.projects().unwrap();
    let by_reg = |id: &str| ps.iter().find(|p| p.project.registry.as_deref() == Some(id)).unwrap();
    // Linked projects take the registry's name; every registry repo is a project.
    assert_eq!(by_reg("oqts").project.name, "OQTS");
    for id in ["quast", "nabla", "locus", "oqts", "cv", "3yp", "tanker-flow"] {
        by_reg(id);
    }
    assert_eq!(
        by_reg("tanker-flow").project.status,
        kairos_core::project::ProjectStatus::Paused,
        "inactive in the registry"
    );
    assert_eq!(e.unregistered().unwrap(), ["aulos"]);
    // New projects come from the registry.
    let err = e
        .create_project(
            Author::Alec,
            &kairos_core::api::CreateProjectReq {
                name: "a brand new thing".into(),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(err.to_string().contains("taxonomy/registry.toml"), "{err}");
    // Measured time counts for its registry project, no check-in needed.
    e.set_worked(&kairos_core::api::WorkedReq {
        machine: "mac".into(),
        from: at("2026-10-01 00:00"),
        to: at("2026-10-13 00:00"),
        spans: vec![kairos_core::api::WorkedSpan {
            project: "locus".into(),
            start: at("2026-10-12 08:00"),
            end: at("2026-10-12 09:30"),
            activity: Vec::new(),
        }],
    })
    .unwrap();
    env.set_now("2026-10-12 12:00");
    let ps = e.projects().unwrap();
    let locus = ps
        .iter()
        .find(|p| p.project.registry.as_deref() == Some("locus"))
        .unwrap();
    assert_eq!((locus.week_min, locus.month_min), (90, 90));
}

#[test]
fn an_event_names_its_locus_item_without_touching_icloud() {
    let env = Env::new(true);
    env.phone_event(
        "Personal",
        "intv",
        "Millennium final round",
        "2026-10-16 10:00",
        "2026-10-16 11:00",
    );
    let mut e = env.engine();
    e.registry = Some(
        locus_contract::taxonomy::load(&locus_contract::taxonomy::root())
            .unwrap()
            .registry,
    );
    let before = env.read_all("Personal");
    let set = |e: &mut kairosd::engine::Engine, l: &str| {
        e.update_event(
            Author::Claude,
            None,
            "intv",
            &kairos_core::api::UpdateEventReq {
                locus: Some(l.into()),
                ..Default::default()
            },
        )
    };
    assert!(
        set(&mut e, "millennium").unwrap().is_none(),
        "kairos's own record: no changeset"
    );
    assert_eq!(env.read_all("Personal"), before, "nothing written to the calendar");
    let linked = |e: &kairosd::engine::Engine| {
        e.events(at("2026-10-16 00:00"), at("2026-10-17 00:00"))
            .unwrap()
            .into_iter()
            .find(|i| i.uid == "intv")
            .and_then(|i| i.meta.locus)
    };
    assert_eq!(linked(&e).as_deref(), Some("millennium"));
    // Only registry items: a typo is refused, not stored.
    assert!(set(&mut e, "milennium").unwrap_err().to_string().contains("registry"));
    // An event made from locus carries its link from the start.
    e.create_event(
        Author::Claude,
        &CreateEventReq {
            title: "Jump call".into(),
            start: "2026-10-20T15:00".into(),
            duration_min: Some(30),
            key: Some("Personal".into()),
            locus: Some("jump".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let made = e
        .events(at("2026-10-20 00:00"), at("2026-10-21 00:00"))
        .unwrap()
        .into_iter()
        .find(|i| i.title == "Jump call")
        .unwrap();
    assert_eq!(made.meta.locus.as_deref(), Some("jump"));
    assert!(set(&mut e, "").unwrap().is_none());
    assert_eq!(linked(&e), None, "cleared");
}

#[test]
fn how_a_task_is_planned_follows_from_its_estimate_and_what_it_is_for() {
    // ADR-009: you say what it is for and how long; kairos works out how to plan it.
    let env = Env::new(true);
    let mut e = env.engine();
    e.registry = Some(
        locus_contract::taxonomy::load(&locus_contract::taxonomy::root())
            .unwrap()
            .registry,
    );
    e.sync_registry().unwrap();
    let add = |e: &mut kairosd::engine::Engine, title: &str, key: &str, est: u32, locus: Option<&str>| {
        e.create_task(
            Author::Alec,
            &CreateTaskReq {
                title: title.into(),
                key: key.into(),
                estimate_min: est,
                locus: locus.map(String::from),
                plan: Some(false),
                ..Default::default()
            },
        )
    };
    // A course's sheet with no estimate: five hours in blocks.
    let t = add(&mut e, "B15 sheet 4", "Engineering", 0, Some("b15")).unwrap().task;
    assert_eq!((t.kind.as_deref(), t.estimate_min), (Some("sheet"), 300));
    assert_eq!(t.locus.as_deref(), Some("b15"));
    // An errand with no estimate: a short chore for a gap.
    let t = add(&mut e, "laundry", "Personal", 0, None).unwrap().task;
    assert_eq!((t.kind.as_deref(), t.estimate_min), (Some("chore"), 20));
    // Interview prep: three hours, in two to three hour blocks.
    let t = add(&mut e, "Millennium prep", "Engineering", 180, Some("millennium"))
        .unwrap()
        .task;
    assert_eq!(t.kind.as_deref(), Some("coursework"));
    // An hour or less: one sitting.
    let t = add(&mut e, "Email the 3YP group", "Engineering", 40, Some("3yp"))
        .unwrap()
        .task;
    assert_eq!(t.kind.as_deref(), Some("admin"));
    // A task for a repo belongs to its project.
    let p = e
        .projects()
        .unwrap()
        .into_iter()
        .find(|p| p.project.registry.as_deref() == Some("3yp"))
        .unwrap();
    assert_eq!(t.project_id.as_deref(), Some(p.project.id.as_str()));
    // Anything longer with no estimate asks for one.
    let err = add(&mut e, "3YP report", "Engineering", 0, Some("3yp")).unwrap_err();
    assert!(err.to_string().contains("how long"), "{err}");
}

#[test]
fn prep_is_planned_before_its_interview_and_moves_with_it() {
    // Spec E4: "move interview should move its prep".
    let env = Env::new(true);
    env.phone_event(
        "Personal",
        "intv",
        "Millennium final round",
        "2026-10-16 10:00",
        "2026-10-16 11:00",
    );
    let mut e = env.engine();
    e.registry = Some(
        locus_contract::taxonomy::load(&locus_contract::taxonomy::root())
            .unwrap()
            .registry,
    );
    e.update_event(
        Author::Alec,
        None,
        "intv",
        &kairos_core::api::UpdateEventReq {
            locus: Some("millennium".into()),
            ..Default::default()
        },
    )
    .unwrap();
    // Prep for the application, no due date: planned before its next event, the interview.
    let r = e
        .create_task(
            Author::Alec,
            &CreateTaskReq {
                title: "Millennium prep".into(),
                key: "Engineering".into(),
                estimate_min: 180,
                locus: Some("millennium".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(r.task.before.as_deref(), Some("intv"));
    assert_eq!(r.task.due.map(|d| d.instant(e.tz())), Some(at("2026-10-16 10:00")));
    // The interview moves to the Monday after: the prep's due date follows, and its work.
    e.direct = true;
    e.update_event(
        Author::Alec,
        None,
        "intv",
        &kairos_core::api::UpdateEventReq {
            start: Some("2026-10-19T14:00".into()),
            ..Default::default()
        },
    )
    .unwrap();
    e.direct = false;
    e.plan(false, Author::Scheduler).unwrap();
    let t = e.db.task_get(&r.task.id).unwrap().unwrap();
    assert_eq!(t.due.map(|d| d.instant(e.tz())), Some(at("2026-10-19 14:00")));
}

#[test]
fn a_work_block_stays_timed_and_moves_until_checked_in() {
    // Round 7b: blocks put on a whole day or in the past by accident were hard to move.
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e.create_task(Author::Claude, &b4(120, "fri 17:00")).unwrap();
    let id = r.task.id.clone();
    let blocks = |e: &mut kairosd::engine::Engine| {
        e.events(at("2026-10-12 00:00"), at("2026-10-20 00:00"))
            .unwrap()
            .into_iter()
            .filter(|i| i.meta.task_id.as_deref() == Some(id.as_str()) && i.meta.kind == EventKind::TaskBlock)
            .collect::<Vec<_>>()
    };
    let b = blocks(&mut e).into_iter().next().unwrap();
    let before = b.span.bounds(e.tz());
    e.direct = true;
    // A date alone: the same time of day on that day, still timed.
    e.update_event(
        Author::Alec,
        None,
        &b.uid,
        &kairos_core::api::UpdateEventReq {
            start: Some("2026-10-15".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let moved = blocks(&mut e).into_iter().find(|x| x.uid == b.uid).unwrap();
    assert!(!moved.span.is_all_day());
    let (s, en) = moved.span.bounds(e.tz());
    assert_eq!(s.with_timezone(&e.tz()).time(), before.0.with_timezone(&e.tz()).time());
    assert_eq!(en - s, before.1 - before.0);
    // Placing one by hand needs a time.
    let err = e
        .create_event(
            Author::Alec,
            &CreateEventReq {
                title: "B4 problem sheet".into(),
                start: "2026-10-16".into(),
                task_id: Some(id.clone()),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(err.to_string().contains("needs a time"), "{err}");
}

#[test]
fn tasks_and_chores_go_to_locus_as_its_own_types() {
    // L6: kairosd serves the Tasks page's and Home's types, labels worked out here from the
    // registry, so locus converts nothing.
    let env = Env::new(true);
    let mut e = env.engine();
    e.registry = Some(
        locus_contract::taxonomy::load(&locus_contract::taxonomy::root())
            .unwrap()
            .registry,
    );
    e.sync_registry().unwrap();
    for (title, key, locus, due) in [
        ("B15 sheet 4", "Engineering", Some("b15"), Some("2026-10-30")),
        ("laundry", "Personal", None, None),
    ] {
        e.create_task(
            Author::Alec,
            &CreateTaskReq {
                title: title.into(),
                key: key.into(),
                locus: locus.map(String::from),
                due: due.map(String::from),
                plan: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
    }
    let items = e.tasks_overview().unwrap();
    let b15 = items.iter().find(|t| t.title == "B15 sheet 4").unwrap();
    assert_eq!(
        (b15.for_id.as_deref(), b15.for_label.as_str(), b15.area.as_str()),
        (Some("b15"), "B15 Control Systems", "Studies")
    );
    assert!(b15.due_is_date, "a date alone");
    assert_eq!(
        b15.due.unwrap().format("%Y-%m-%d %H:%M").to_string(),
        "2026-10-30 23:59"
    );
    let chores = e.chores().unwrap();
    assert!(
        chores.iter().any(|c| c.title == "laundry" && c.id.starts_with("t-")),
        "{chores:?}"
    );
}

#[test]
fn lectures_link_to_their_course_by_its_code() {
    let env = Env::new(true);
    env.phone_event(
        "Engineering",
        "l1",
        "B14 Image Processing #3",
        "2026-10-22 09:00",
        "2026-10-22 10:00",
    );
    env.phone_event(
        "Engineering",
        "l2",
        "B1: Engineering Computation lab",
        "2026-10-22 11:00",
        "2026-10-22 13:00",
    );
    env.phone_event(
        "Engineering",
        "l3",
        "B14 lecture, linked by hand",
        "2026-10-23 09:00",
        "2026-10-23 10:00",
    );
    env.phone_event("Engineering", "l4", "Library", "2026-10-23 14:00", "2026-10-23 15:00");
    env.phone_event(
        "Personal",
        "l5",
        "B14 revision with friends",
        "2026-10-24 14:00",
        "2026-10-24 15:00",
    );
    let mut e = env.engine();
    e.registry = Some(
        locus_contract::taxonomy::load(&locus_contract::taxonomy::root())
            .unwrap()
            .registry,
    );
    let src = |e: &kairosd::engine::Engine, uid: &str| {
        e.events(at("2026-10-22 00:00"), at("2026-10-25 00:00"))
            .unwrap()
            .into_iter()
            .find(|i| i.uid == uid)
            .unwrap()
    };
    let l3 = src(&e, "l3");
    e.db.meta_set(
        &l3.source,
        "l3",
        &kairos_core::model::Meta {
            locus: Some("b15".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(e.link_courses().unwrap(), 2);
    assert_eq!(src(&e, "l1").meta.locus.as_deref(), Some("b14"));
    assert_eq!(
        src(&e, "l2").meta.locus.as_deref(),
        Some("b1"),
        "B1: matches B1, not B14"
    );
    assert_eq!(src(&e, "l3").meta.locus.as_deref(), Some("b15"), "your own link stays");
    assert_eq!(src(&e, "l4").meta.locus, None);
    assert_eq!(src(&e, "l5").meta.locus, None, "only Studies events");
    assert_eq!(e.link_courses().unwrap(), 0, "nothing more to do");
}

// ---------------------------------------------------------------- start and length (2026-10-27)

/// A task's work blocks, earliest first.
fn work_blocks(e: &kairosd::engine::Engine, task_id: &str) -> Vec<kairos_core::model::Instance> {
    let mut v: Vec<_> = e
        .world(14)
        .unwrap()
        .instances
        .into_iter()
        .filter(|i| i.meta.kind == EventKind::TaskBlock && i.meta.task_id.as_deref() == Some(task_id))
        .collect();
    v.sort_by_key(|i| i.start(chrono_tz::Europe::London));
    v
}

fn span_of(i: &kairos_core::model::Instance) -> (chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>) {
    let tz = chrono_tz::Europe::London;
    (i.start(tz), i.end(tz))
}

/// Alec's own edit from the TUI (direct, as `x-kairos-interactive` makes it).
fn retime(
    e: &mut kairosd::engine::Engine,
    i: &kairos_core::model::Instance,
    start: Option<chrono::DateTime<chrono::Utc>>,
    end: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<Option<kairos_core::changeset::Changeset>, EngineError> {
    e.direct = true;
    let r = e.update_event(
        Author::Alec,
        Some(&i.source),
        &i.uid,
        &UpdateEventReq {
            start: start.map(|s| s.to_rfc3339()),
            end: end.map(|s| s.to_rfc3339()),
            ..Default::default()
        },
    );
    e.direct = false;
    r
}

fn last_checkin(e: &kairosd::engine::Engine) -> kairos_core::changeset::Changeset {
    e.db.cs_list(Some(Status::Applied), 50)
        .unwrap()
        .into_iter()
        .find(|c| c.summary.starts_with("Check-in"))
        .unwrap()
}

#[test]
fn a_block_cannot_be_piled_onto_a_checked_in_one() {
    // 2026-10-27: a pinned block nudged back a day landed on a morning block already ticked,
    // and the time counted twice (and the view hid it). Now the edit is refused, naming it.
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e.create_task(Author::Alec, &b4(240, "fri 17:00")).unwrap();
    let bs = work_blocks(&e, &r.task.id);
    assert!(bs.len() >= 2);
    let (s0, e0) = span_of(&bs[0]);
    *env.now.lock().unwrap() = e0 + Duration::minutes(5);
    e.checkin(
        Author::Alec,
        &CheckinReq {
            block_uid: bs[0].uid.clone(),
            outcome: "done".into(),
            actual_min: None,
            remaining_min: None,
            not_before: None,
        },
    )
    .unwrap();
    let later = work_blocks(&e, &r.task.id)
        .into_iter()
        .find(|i| span_of(i).0 > e0)
        .unwrap();
    let err = retime(&mut e, &later, Some(s0 + Duration::minutes(10)), None).unwrap_err();
    assert!(
        matches!(&err, EngineError::BadRequest(m) if m.contains("overlap") && m.contains("B4")),
        "{err:?}"
    );
}

#[test]
fn a_flexible_block_in_the_way_moves_out_of_it() {
    let env = Env::new(true);
    let mut e = env.engine();
    let a = e.create_task(Author::Alec, &b4(120, "fri 17:00")).unwrap();
    let mut other = b4(120, "fri 17:00");
    other.title = "B5 sheet".into();
    let b = e.create_task(Author::Alec, &other).unwrap();
    let ba = work_blocks(&e, &a.task.id)[0].clone();
    let bb = work_blocks(&e, &b.task.id)[0].clone();
    retime(&mut e, &ba, Some(span_of(&bb).0), None).unwrap();
    e.replan_if_needed().unwrap();
    let (sa, ea) = span_of(&work_blocks(&e, &a.task.id)[0]);
    for i in work_blocks(&e, &b.task.id) {
        let (s, en) = span_of(&i);
        assert!(en <= sa || s >= ea, "B5 moved out of the way");
    }
}

#[test]
fn lengthening_a_block_ahead_gives_the_task_that_time_and_leaves_the_rest() {
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e.create_task(Author::Alec, &b4(240, "fri 17:00")).unwrap();
    let before = work_blocks(&e, &r.task.id);
    let (s, en) = span_of(&before[0]);
    let cs = retime(&mut e, &before[0], None, Some(en + Duration::minutes(30)))
        .unwrap()
        .unwrap();
    assert!(cs.summary.contains("+30 m for the task"), "{}", cs.summary);
    assert_eq!(e.db.task_get(&r.task.id).unwrap().unwrap().remaining_min, 270);
    e.replan_if_needed().unwrap();
    let after = work_blocks(&e, &r.task.id);
    assert_eq!(span_of(&after[0]), (s, en + Duration::minutes(30)));
    assert_eq!(
        after[1..].iter().map(span_of).collect::<Vec<_>>(),
        before[1..].iter().map(span_of).collect::<Vec<_>>(),
        "the other blocks stay"
    );
    // Shorter gives it back.
    retime(&mut e, &after[0], None, Some(en - Duration::minutes(30))).unwrap();
    assert_eq!(e.db.task_get(&r.task.id).unwrap().unwrap().remaining_min, 210);
}

#[test]
fn halfway_through_a_block_start_later_and_keep_going() {
    // 14:00–16:00, started at 14:30 and working on to 17:00: ⇧↓ then ⌥↓↓.
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e.create_task(Author::Alec, &b4(240, "fri 17:00")).unwrap();
    let b = work_blocks(&e, &r.task.id)[0].clone();
    let (s, en) = span_of(&b);
    *env.now.lock().unwrap() = s + Duration::minutes(45);
    retime(&mut e, &b, Some(s + Duration::minutes(30)), None).unwrap();
    let b = work_blocks(&e, &r.task.id)[0].clone();
    retime(&mut e, &b, None, Some(en + Duration::minutes(60))).unwrap();
    let b = work_blocks(&e, &r.task.id)[0].clone();
    assert_eq!(span_of(&b), (s + Duration::minutes(30), en + Duration::minutes(60)));
    assert_eq!(e.db.task_get(&r.task.id).unwrap().unwrap().remaining_min, 270);
}

#[test]
fn fixing_a_past_block_then_checking_in_records_what_happened() {
    // Planned 2 h, actually 1 h 30: shorten it, then y.
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e.create_task(Author::Alec, &b4(240, "fri 17:00")).unwrap();
    let b = work_blocks(&e, &r.task.id)[0].clone();
    let (s, en) = span_of(&b);
    *env.now.lock().unwrap() = en + Duration::minutes(10);
    let planned = (en - s).num_minutes() as u32;
    retime(&mut e, &b, None, Some(en - Duration::minutes(30))).unwrap();
    e.checkin(
        Author::Alec,
        &CheckinReq {
            block_uid: b.uid.clone(),
            outcome: "done".into(),
            actual_min: None,
            remaining_min: None,
            not_before: None,
        },
    )
    .unwrap();
    let rec = e.db.checkin_get(&b.uid).unwrap().unwrap();
    assert_eq!(rec.actual_min, Some(planned - 30));
    // 240 − 30 (given back) − (planned − 30) done.
    assert_eq!(e.db.task_get(&r.task.id).unwrap().unwrap().remaining_min, 240 - planned);
}

#[test]
fn a_checked_in_block_stays_editable() {
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e.create_task(Author::Alec, &b4(240, "fri 17:00")).unwrap();
    let b = work_blocks(&e, &r.task.id)[0].clone();
    let (s, en) = span_of(&b);
    let planned = (en - s).num_minutes() as u32;
    *env.now.lock().unwrap() = en + Duration::hours(3);
    e.checkin(
        Author::Alec,
        &CheckinReq {
            block_uid: b.uid.clone(),
            outcome: "done".into(),
            actual_min: None,
            remaining_min: None,
            not_before: None,
        },
    )
    .unwrap();
    let left = e.db.task_get(&r.task.id).unwrap().unwrap().remaining_min;
    // Worked 30 min longer than recorded: the record grows; what's left doesn't change.
    let b = work_blocks(&e, &r.task.id)[0].clone();
    retime(&mut e, &b, None, Some(en + Duration::minutes(30))).unwrap();
    assert_eq!(
        e.db.checkin_get(&b.uid).unwrap().unwrap().actual_min,
        Some(planned + 30)
    );
    assert_eq!(e.db.task_get(&r.task.id).unwrap().unwrap().remaining_min, left);
    // It didn't happen after all: x answers it again — gone, and the time back to the task.
    e.checkin(
        Author::Alec,
        &CheckinReq {
            block_uid: b.uid.clone(),
            outcome: "missed".into(),
            actual_min: None,
            remaining_min: None,
            not_before: None,
        },
    )
    .unwrap();
    assert!(work_blocks(&e, &r.task.id).iter().all(|i| i.uid != b.uid));
    assert_eq!(
        e.db.task_get(&r.task.id).unwrap().unwrap().remaining_min,
        left + planned + 30
    );
}

#[test]
fn undo_takes_back_an_accidental_check_in_whole() {
    // 2026-10-27: "done" pressed by accident, and u went past it (it was the scheduler's).
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e.create_task(Author::Alec, &b4(240, "fri 17:00")).unwrap();
    let b = work_blocks(&e, &r.task.id)[0].clone();
    *env.now.lock().unwrap() = span_of(&b).1 + Duration::minutes(5);
    let task_before = e.db.task_get(&r.task.id).unwrap().unwrap();
    // Even "finished".
    e.checkin(
        Author::Alec,
        &CheckinReq {
            block_uid: b.uid.clone(),
            outcome: "finished".into(),
            actual_min: None,
            remaining_min: None,
            not_before: None,
        },
    )
    .unwrap();
    assert_eq!(e.db.task_get(&r.task.id).unwrap().unwrap().status, TaskStatus::Done);
    let cs = last_checkin(&e);
    assert_eq!(cs.author, Author::Alec, "u finds it");
    e.undo_with_replans(&cs.id).unwrap();
    let t = e.db.task_get(&r.task.id).unwrap().unwrap();
    assert_eq!(
        (t.status, t.remaining_min),
        (TaskStatus::Open, task_before.remaining_min)
    );
    assert!(e.db.checkin_get(&b.uid).unwrap().is_none_or(|c| c.outcome.is_none()));
    let again = work_blocks(&e, &r.task.id);
    let i = again.iter().find(|i| i.uid == b.uid).unwrap();
    assert!(i.title.starts_with('☐'), "{}", i.title);
    assert!(again.len() >= 2, "its blocks ahead are back");
    assert!(
        e.checkin_items()
            .unwrap()
            .iter()
            .any(|c| c.block_uid == b.uid && c.outcome.is_none())
    );
}

#[test]
fn a_check_in_claude_records_for_alec_applies_at_once() {
    // "Only managed 20 min" said to Claude: still no approval needed, as when the scheduler
    // authored the ✓ (before 2026-10-27).
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e.create_task(Author::Alec, &b4(240, "fri 17:00")).unwrap();
    let b = work_blocks(&e, &r.task.id)[0].clone();
    *env.now.lock().unwrap() = span_of(&b).1 + Duration::minutes(5);
    e.checkin(
        Author::Claude,
        &CheckinReq {
            block_uid: b.uid.clone(),
            outcome: "partial".into(),
            actual_min: Some(20),
            remaining_min: None,
            not_before: None,
        },
    )
    .unwrap();
    assert_eq!(
        last_checkin(&e).status,
        Status::Applied,
        "{:?}",
        last_checkin(&e).reasons
    );
    // And a pinned (hand-moved) one.
    let b2 = work_blocks(&e, &r.task.id)
        .into_iter()
        .find(|i| i.uid != b.uid)
        .unwrap();
    let (s2, _) = span_of(&b2);
    retime(&mut e, &b2, Some(s2 + Duration::minutes(30)), None).unwrap();
    let b2 = work_blocks(&e, &r.task.id)
        .into_iter()
        .find(|i| i.uid == b2.uid)
        .unwrap();
    *env.now.lock().unwrap() = span_of(&b2).1 + Duration::minutes(5);
    e.checkin(
        Author::Claude,
        &CheckinReq {
            block_uid: b2.uid.clone(),
            outcome: "done".into(),
            actual_min: None,
            remaining_min: None,
            not_before: None,
        },
    )
    .unwrap();
    assert_eq!(
        last_checkin(&e).status,
        Status::Applied,
        "{:?}",
        last_checkin(&e).reasons
    );
}

#[test]
fn the_replan_waits_while_alec_moves_things_by_hand() {
    // 27 Oct: each of five quick nudges set off a replan that resized another task's block.
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e.create_task(Author::Alec, &b4(240, "fri 17:00")).unwrap();
    let b = work_blocks(&e, &r.task.id)[0].clone();
    let (s, _) = span_of(&b);
    retime(&mut e, &b, Some(s + Duration::minutes(30)), None).unwrap();
    assert!(e.replan_soon);
    assert!(e.replan_wait().is_some_and(|w| w > std::time::Duration::from_secs(3)));
    assert!(e.replan_due().unwrap().is_none(), "held while nudging");
    assert!(e.replan_soon, "still owed");
    // Once he has paused it runs (the hold elapsed).
    e.replan_hold = Some(std::time::Instant::now());
    assert!(e.replan_due().unwrap().is_some());
    assert!(!e.replan_soon && e.replan_hold.is_none());
    // A change that isn't his at a keyboard (Claude, a sync) is not held.
    let b = work_blocks(&e, &r.task.id).into_iter().last().unwrap();
    e.update_event(
        Author::Claude,
        Some(&b.source),
        &b.uid,
        &UpdateEventReq {
            title: Some("☐ B4 problem sheet (Q3)".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(e.replan_wait().is_none());
}
