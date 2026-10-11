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
        vec!["icloud/s-2222".to_string(), "icloud/f-4444".to_string()],
        "delete side syncs first"
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
    // Blocks: 240 min total, all before Fri 05:00 (12 h lead), none during the lecture.
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
        assert!(b.end(chrono_tz::Europe::London) <= at("2026-10-16 05:00"));
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
    let n = r.plan.unwrap().created.len();
    let done = e.complete_task(Author::Alec, &r.task.id, Some(200)).unwrap();
    assert_eq!(done.task.status, TaskStatus::Done);
    let p = done.plan.unwrap();
    assert_eq!(p.deleted.len(), n);
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
    // Approval replans: the dropped task's blocks are gone.
    let world = e.world(14).unwrap();
    assert!(!world.instances.iter().any(|i| i.meta.kind == EventKind::TaskBlock && i.start(chrono_tz::Europe::London) > at("2026-10-12 09:00")));
}

#[test]
fn big_replans_are_pending() {
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
    let cs = r.plan.unwrap().changeset.unwrap();
    assert_eq!(cs.status, Status::Pending);
    assert!(cs.reasons.iter().any(|x| x.contains("more than 8")));
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
    })
    .unwrap();
    let t = e.db.task_get(&r.task.id).unwrap().unwrap();
    let mins = (next.end(chrono_tz::Europe::London) - next.start(chrono_tz::Europe::London)).num_minutes() as u32;
    assert_eq!(t.remaining_min, 180 - mins);
}

#[test]
fn unanswered_blocks_auto_done_next_morning() {
    let env = Env::new(true);
    let mut e = env.engine();
    let r = e.create_task(Author::Claude, &b4(120, "fri 17:00")).unwrap();
    let first = r.plan.unwrap().created[0].clone();
    let day_after = first.start.with_timezone(&chrono_tz::Europe::London).date_naive() + Duration::days(1);
    env.set_now(&format!("{day_after} 09:30"));
    let n = e.settle().unwrap();
    assert!(n >= 1);
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
