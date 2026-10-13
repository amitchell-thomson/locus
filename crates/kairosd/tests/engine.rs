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
    assert_eq!(env.rec.pending.lock().unwrap().len(), 1, "push notification sent");
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
fn b4_sheet_due_friday_gets_deadline_and_blocks() {
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
    // Deadline: ⚑, zero-length, alarms −1 day and −2 h, linked to the task.
    assert!(eng.contains("SUMMARY:⚑ B4 problem sheet"));
    assert!(eng.contains("DTSTART;TZID=Europe/London:20261016T170000\r\nDTEND;TZID=Europe/London:20261016T170000"));
    assert!(eng.contains("TRIGGER:-P1D") && eng.contains("TRIGGER:-PT2H"));
    assert!(eng.contains(&format!("X-KAIROS-TASK-ID:{}", r.task.id)));
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
        d.changesets[0]
            .summary
            .contains(&format!("{n} scheduled blocks and its deadline")),
        "{}",
        d.changesets[0].summary
    );
    assert_eq!(blocks_of(&mut e, &id), 0);
    assert!(
        !env.read_all("Engineering").contains("SUMMARY:⚑ B4 problem sheet"),
        "deadline dropped too"
    );
    // One undo brings the task, every block and the deadline back.
    e.undo(&d.changesets[0].changeset_id).unwrap();
    assert_eq!(e.db.task_get(&id).unwrap().unwrap().status, TaskStatus::Open);
    assert_eq!(blocks_of(&mut e, &id), n);
    assert!(env.read_all("Engineering").contains("SUMMARY:⚑ B4 problem sheet"));
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
        .checkin(&CheckinReq {
            block_uid: uid,
            outcome: "missed".into(),
            actual_min: None,
            remaining_min: None,
        })
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
    e.checkin(&CheckinReq {
        block_uid: next.uid.clone(),
        outcome: "done".into(),
        actual_min: None,
        remaining_min: None,
    })
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
    let uid = r.task.deadline_uid.clone().unwrap();
    let cs = e.delete_event(Author::Claude, None, &uid).unwrap();
    assert_eq!(cs.status, Status::Pending);
    assert!(cs.reasons.iter().any(|x| x.contains("deadline")));
}

#[test]
fn changing_due_moves_the_deadline_event() {
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e.create_task(Author::Claude, &b4(60, "fri 17:00")).unwrap();
    let u = e
        .update_task(
            Author::Claude,
            &r.task.id,
            &kairos_core::api::UpdateTaskReq {
                due: Some("2026-10-20 12:00".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(u.changesets[0].status, Status::Applied);
    assert!(
        env.read_all("Engineering")
            .contains("DTSTART;TZID=Europe/London:20261020T120000")
    );
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
    e.checkin(&CheckinReq {
        block_uid: items[0].block_uid.clone(),
        outcome: "done".into(),
        actual_min: None,
        remaining_min: Some(480),
    })
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
fn project_time_is_proposed_accepted_and_dropped() {
    let env = Env::new(true);
    let mut e = env.engine();
    env.phone_event("Sport", "rowing", "Rowing", "2026-10-13 17:00", "2026-10-13 18:00");
    e.plan(false, Author::Scheduler).unwrap();
    let props = e.proposals().unwrap();
    assert!(!props.is_empty());
    let day = |t: chrono::DateTime<chrono::Utc>| t.with_timezone(&chrono_tz::Europe::London).date_naive();
    let mut days: Vec<_> = props.iter().map(|p| day(p.start)).collect();
    days.dedup();
    assert_eq!(days.len(), props.len(), "at most one a day");
    for p in &props {
        let m = (p.end - p.start).num_minutes();
        assert!((60..=180).contains(&m), "{m}");
        assert_eq!(p.title, "Project time");
        // Never in the hour after sport.
        assert!(
            p.end <= at("2026-10-13 17:00") || p.start >= at("2026-10-13 19:00"),
            "{} – {}",
            london(p.start),
            london(p.end)
        );
    }
    // Dry runs don't touch proposals; a replan with nothing new keeps them.
    e.plan(true, Author::Scheduler).unwrap();
    e.plan(false, Author::Scheduler).unwrap();
    assert_eq!(e.proposals().unwrap(), props);
    // Accept the first as "kairos TUI work": an event in the default key.
    e.direct = true;
    let w = e
        .accept_proposal(
            Author::Alec,
            &props[0].id,
            &kairos_core::api::AcceptProposalReq {
                title: Some("kairos TUI work".into()),
            },
        )
        .unwrap();
    e.direct = false;
    assert_eq!(w.status, Status::Applied);
    assert!(env.read_all("Personal").contains("SUMMARY:kairos TUI work"));
    // Drop the second: it isn't proposed again.
    e.reject_proposal(&props[1].id).unwrap();
    e.replan_if_needed().unwrap();
    e.plan(false, Author::Scheduler).unwrap();
    let after = e.proposals().unwrap();
    assert!(after.iter().all(|p| p.id != props[0].id && p.id != props[1].id));
    // The accepted day gets no more; the dropped day may get another slot.
    assert!(after.iter().all(|p| day(p.start) != day(props[0].start)));
    // Also when the proposals table has forgotten the acceptance (sessions
    // accepted before it was recorded): the event itself marks the day.
    e.db.proposals_replace(&[], at("2026-10-12 08:00")).unwrap();
    e.db.proposal_set_status(&props[0].id, "gone").unwrap();
    e.plan(false, Author::Scheduler).unwrap();
    assert!(
        e.proposals()
            .unwrap()
            .iter()
            .all(|p| day(p.start) != day(props[0].start))
    );
    assert!(
        after.iter().all(|p| p.end <= props[1].start || p.start >= props[1].end),
        "re-proposed over a dropped span"
    );
    assert!(matches!(e.reject_proposal("nope"), Err(EngineError::NotFound(_))));
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
    assert!(
        kairosd::jobs::morning_text(&e)
            .contains("B4 problem sheet: planned time used up — finished, or how much more?")
    );
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
    e.checkin(&CheckinReq {
        block_uid: it.block_uid.clone(),
        outcome: "done".into(),
        actual_min: None,
        remaining_min: Some(0),
    })
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
    e.checkin(&CheckinReq {
        block_uid: uid,
        outcome: "done".into(),
        actual_min: None,
        remaining_min: None,
    })
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
    assert!(per.contains("SUMMARY:⚑ B4 sheet 3"), "{per}");
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
    e.checkin(&CheckinReq {
        block_uid: uid,
        outcome: "done".into(),
        actual_min: None,
        remaining_min: Some(0),
    })
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
        .checkin(&CheckinReq {
            block_uid: uid,
            outcome: "partial".into(),
            actual_min: Some(20),
            remaining_min: None,
        })
        .unwrap();
    let again: i64 = p.created.iter().map(|b| (b.end - b.start).num_minutes()).sum();
    assert_eq!(again, 40, "{p:?}");
    assert!(p.created.iter().all(|b| b.start > created[0].end));
    assert_eq!(e.db.task_get(&r.task.id).unwrap().unwrap().remaining_min, 40);
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
    e.checkin(&CheckinReq {
        block_uid: uid1.clone(),
        outcome: "partial".into(),
        actual_min: Some(30),
        remaining_min: None,
    })
    .unwrap();
    let world = e.world(14).unwrap();
    let i1 = world.instances.iter().find(|i| i.uid == uid1).unwrap();
    assert_eq!(i1.title, "✓ B4 problem sheet");
    assert_eq!(
        (i1.end(chrono_tz::Europe::London) - i1.start(chrono_tz::Europe::London)).num_minutes(),
        30
    );
    // It can't be moved any more.
    assert!(matches!(
        e.update_event(
            Author::Alec,
            Some(&i1.source),
            &uid1,
            &UpdateEventReq {
                start: Some(at("2026-10-15 20:00").to_rfc3339()),
                ..Default::default()
            }
        ),
        Err(EngineError::BadRequest(m)) if m.contains("check in")
    ));
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
        e.checkin(&CheckinReq {
            block_uid: uid2.clone(),
            outcome: "missed".into(),
            actual_min: None,
            remaining_min: None,
        })
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
    e.checkin(&CheckinReq {
        block_uid: it.block_uid.clone(),
        outcome: "finished".into(),
        actual_min: None,
        remaining_min: None,
    })
    .unwrap();
    assert_eq!(e.db.task_get(&r.task.id).unwrap().unwrap().status, TaskStatus::Done);
}
