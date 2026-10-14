//! Engine semantics (SPEC §6, §12 Phase 4 AC) against the fixture mirror and
//! a fake source.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use keryx_core::model::{Address, Message};
use keryx_store::db::Raw;
use std::sync::{Arc, Mutex};

/// Applies whatever it's asked; can be told to fail on the n-th call.
#[derive(Clone, Default)]
struct Fake {
    calls: Arc<Mutex<Vec<(String, MsgState)>>>,
    fail_on: Arc<Mutex<Option<usize>>>,
}

impl Actuator for Fake {
    fn apply(
        &mut self,
        _a: &Account,
        msg: &MsgRef,
        _b: &MsgState,
        after: &MsgState,
    ) -> std::result::Result<Option<MsgState>, String> {
        let mut calls = self.calls.lock().unwrap();
        let mut fail = self.fail_on.lock().unwrap();
        if *fail == Some(calls.len()) {
            *fail = None;
            return Err("source said no".into());
        }
        calls.push((msg.source_id.clone(), after.clone()));
        Ok(Some(after.clone()))
    }
    fn resync(&mut self, _db: &mut Db, _a: &Account) -> std::result::Result<(), String> {
        Ok(())
    }
}

const NOW: i64 = 1_791_986_400; // Wed 14 Oct 2026 15:00 BST

fn engine() -> (Engine, Fake) {
    let (mut cfg, db) = keryx_store::testkit::fixture_db();
    cfg.live = true;
    let fake = Fake::default();
    let mut e = Engine::new(cfg, db, Box::new(fake.clone())).unwrap();
    let clock = Arc::new(Mutex::new(NOW));
    let c = clock.clone();
    e.clock = Box::new(move || *c.lock().unwrap());
    (e, fake)
}

fn set_clock(e: &mut Engine, t: i64) {
    e.clock = Box::new(move || t);
}

/// The ref of the thread whose latest subject is `subject`.
fn r(e: &Engine, subject: &str) -> String {
    let rows = view::list(&e.db, &e.cfg, &view::View::All, &view::Filter::default()).unwrap();
    rows.into_iter()
        .find(|x| x.subject == subject)
        .unwrap_or_else(|| panic!("no thread {subject}"))
        .r#ref
}

fn thread(e: &Engine, rf: &str) -> Vec<Message> {
    let (a, t) = parse_thread_ref(rf).unwrap();
    e.db.thread(a, t).unwrap()
}

fn in_inbox(e: &Engine, rf: &str) -> bool {
    thread(e, rf).iter().any(|m| m.in_inbox)
}

fn req(intent: Intent, refs: Vec<String>) -> TriageReq {
    TriageReq {
        intent,
        refs,
        summary: None,
        review_id: None,
    }
}

/// Add a synthetic message (thread `t<n>`) to an account's inbox.
fn add(e: &Engine, account: &str, n: usize, from: &str, list: bool) -> String {
    let m = Message {
        account: account.into(),
        source_id: format!("syn{n}"),
        message_id: format!("syn{n}@x"),
        thread_id: format!("tsyn{n}"),
        from: Some(Address::new(None, from)),
        to: vec![],
        cc: vec![],
        bcc: vec![],
        reply_to: vec![],
        subject: format!("Synthetic {n}"),
        date: NOW - 3600,
        location: match account {
            "personal" => Location::Labels {
                labels: vec!["INBOX".into()],
            },
            "oxford" => Location::Mailboxes {
                physical: "Inbox".into(),
                labels: vec![],
            },
            _ => Location::Mailboxes {
                physical: "[Gmail]/All Mail".into(),
                labels: vec!["INBOX".into()],
            },
        },
        in_inbox: true,
        read: false,
        flagged: false,
        size: 1,
        attachments: vec![],
        body_text: "x".into(),
        hidden_text_removed: false,
        snippet: "x".into(),
        in_reply_to: vec![],
        references: vec![],
        list_headers: if list {
            vec![("list-id".into(), "<news.example.com>".into())]
        } else {
            vec![]
        },
    };
    e.db.upsert(&m, Raw::None, keryx_core::text::TEXT_VERSION).unwrap();
    keryx_core::model::thread_ref(account, &format!("tsyn{n}"))
}

#[test]
fn alec_archives_and_undoes_people_mail() {
    let (mut e, fake) = engine();
    let t = r(&e, "Lab partner for Thursday?");
    let out = e.triage(req(Intent::Archive, vec![t.clone()]), Who::ALEC).unwrap();
    let cs = out.applied.unwrap();
    assert_eq!(cs.status, Status::Applied);
    assert!(!in_inbox(&e, &t));
    // Oxford is Exchange: the message moved to Archive.
    assert_eq!(
        fake.calls.lock().unwrap()[0].1.location,
        Location::Mailboxes {
            physical: "Archive".into(),
            labels: vec![]
        }
    );
    let u = e.undo_last(Who::ALEC).unwrap();
    assert_eq!(u.status, Status::Applied);
    assert!(in_inbox(&e, &t));
    assert_eq!(e.db.changeset(&cs.id).unwrap().unwrap().status, Status::Undone);
}

#[test]
fn claude_mixed_batch_splits_into_applied_and_proposals() {
    let (mut e, _) = engine();
    let mut refs: Vec<String> = (0..10)
        .map(|i| add(&e, "personal", i, &format!("news{i}@lists.example.com"), true))
        .collect();
    refs.push(add(&e, "personal", 100, "jane@example.com", false));
    refs.push(r(&e, "Lab partner for Thursday?"));
    let out = e.triage(req(Intent::Archive, refs.clone()), Who::CLAUDE).unwrap();
    let cs = out.applied.expect("automated part applied");
    assert_eq!(cs.status, Status::Applied);
    assert_eq!(cs.ops.len(), 10);
    assert_eq!(out.proposed, 2);
    let review = e.review(out.review_id.as_deref().unwrap()).unwrap();
    assert_eq!(review.items.len(), 2);
    assert!(review.items.iter().all(|i| i.proposal.reason.contains("from people")));
    assert!(
        out.summary.contains("10 threads") && out.summary.contains("2 threads proposed"),
        "{}",
        out.summary
    );
    // Nothing from people was touched.
    assert!(in_inbox(&e, &refs[10]) && in_inbox(&e, &refs[11]));
    assert!(!in_inbox(&e, &refs[0]));
}

#[test]
fn claude_needs_alec_for_flagged_shared_and_big_batches() {
    let (mut e, _) = engine();
    // Flagged newsletter (Personal m05 is starred and a promotion).
    let flagged = r(&e, "Your account needs attention");
    let out = e
        .triage(req(Intent::Archive, vec![flagged.clone()]), Who::CLAUDE)
        .unwrap();
    assert!(out.applied.is_none());
    assert_eq!(out.proposed, 1);
    assert!(in_inbox(&e, &flagged));
    // OQTS shared: always Alec, even for a newsletter.
    let shared = add(&e, "oqts-shared", 200, "news@lists.example.com", true);
    let out = e
        .triage(req(Intent::Archive, vec![shared.clone()]), Who::CLAUDE)
        .unwrap();
    assert!(out.applied.is_none() && out.proposed == 1);
    // > 100 automated threads at once.
    let many: Vec<String> = (300..401)
        .map(|i| add(&e, "personal", i, "news@lists.example.com", true))
        .collect();
    let out = e.triage(req(Intent::Archive, many.clone()), Who::CLAUDE).unwrap();
    assert!(out.applied.is_none());
    assert_eq!(out.proposed, 101);
    // 100 is fine.
    let out = e
        .triage(req(Intent::Archive, many[..100].to_vec()), Who::CLAUDE)
        .unwrap();
    assert_eq!(out.applied.unwrap().ops.len(), 100);
}

#[test]
fn flag_and_read_changes_that_need_alec_are_refused_not_proposed() {
    let (mut e, _) = engine();
    let t = r(&e, "Lab partner for Thursday?");
    let out = e.triage(req(Intent::Flag { on: true }, vec![t]), Who::CLAUDE).unwrap();
    assert!(out.applied.is_none() && out.review_id.is_none());
    assert!(out.skipped[0].1.contains("needs Alec"), "{:?}", out.skipped);
}

#[test]
fn live_guard_at_propose_and_apply() {
    let (mut e, _) = engine();
    e.cfg.live = false;
    let t = r(&e, "Lab partner for Thursday?");
    let out = e.triage(req(Intent::Archive, vec![t.clone()]), Who::ALEC).unwrap();
    assert!(out.applied.is_none());
    assert!(out.skipped[0].1.contains("[keryx-test]"));
    // A changeset planned while live, applied after live was turned off.
    e.cfg.live = true;
    let (a, _, msgs) = e.thread(&t).unwrap();
    let acc = e.account(&a).unwrap().clone();
    let ops = e.plan(&Intent::Archive, &acc, &msgs).unwrap();
    e.cfg.live = false;
    let mut cs = Changeset {
        id: "cs-x".into(),
        author: Author::Alec,
        interactive: true,
        created_at: NOW,
        ops,
        risk: Risk::Low,
        reasons: vec![],
        status: Status::Pending,
        summary: "x".into(),
        review_id: None,
        undoes: None,
        error: None,
    };
    e.apply(&mut cs).unwrap();
    assert_eq!(cs.status, Status::Failed);
    assert!(in_inbox(&e, &t));
    // A [keryx-test] thread is allowed with live = false.
    let test = add(&e, "personal", 900, "jane@example.com", false);
    e.db.conn()
        .execute(
            "UPDATE messages SET subject = '[keryx-test] hello' WHERE source_id = 'syn900'",
            [],
        )
        .unwrap();
    let out = e.triage(req(Intent::Archive, vec![test.clone()]), Who::ALEC).unwrap();
    assert_eq!(out.applied.unwrap().status, Status::Applied);
}

#[test]
fn changed_target_fails_and_undo_refuses_with_diff() {
    let (mut e, fake) = engine();
    let t = r(&e, "Lab partner for Thursday?");
    let (a, _, msgs) = e.thread(&t).unwrap();
    let acc = e.account(&a).unwrap().clone();
    let ops = e.plan(&Intent::Archive, &acc, &msgs).unwrap();
    // Someone flags it on the phone before the changeset runs.
    e.db.conn()
        .execute(
            "UPDATE messages SET flagged = 1 WHERE source_id = '1' AND account = 'oxford'",
            [],
        )
        .unwrap();
    let mut cs = Changeset {
        id: "cs-y".into(),
        author: Author::Alec,
        interactive: true,
        created_at: NOW,
        ops,
        risk: Risk::Low,
        reasons: vec![],
        status: Status::Pending,
        summary: "x".into(),
        review_id: None,
        undoes: None,
        error: None,
    };
    e.apply(&mut cs).unwrap();
    assert_eq!(cs.status, Status::Failed);
    assert!(cs.error.unwrap().contains("changed since"));
    assert!(fake.calls.lock().unwrap().is_empty(), "nothing applied");

    // Undo after the thread was moved elsewhere → refused with a diff.
    let t2 = r(&e, "Coursework submission portal");
    let out = e.triage(req(Intent::Archive, vec![t2.clone()]), Who::ALEC).unwrap();
    let id = out.applied.unwrap().id;
    e.db.conn()
        .execute(
            "UPDATE messages SET location_json = '{\"kind\":\"mailboxes\",\"physical\":\"Admin\",\"labels\":[]}' WHERE account = 'oxford' AND source_id = '2'",
            [],
        )
        .unwrap();
    let err = e.undo(&id, Who::ALEC).unwrap_err();
    let EngineError::Conflict(diffs) = err else {
        panic!("{err}")
    };
    assert!(diffs[0].contains("expected Archive, now Admin"), "{diffs:?}");
}

#[test]
fn source_failure_reverts_what_was_done() {
    let (mut e, fake) = engine();
    let a = r(&e, "Lab partner for Thursday?");
    let b = r(&e, "Coursework submission portal");
    *fake.fail_on.lock().unwrap() = Some(1);
    let out = e
        .triage(req(Intent::Archive, vec![a.clone(), b.clone()]), Who::ALEC)
        .unwrap();
    let cs = out.applied.unwrap();
    assert_eq!(cs.status, Status::Failed);
    assert!(cs.error.unwrap().contains("source said no"));
    assert!(in_inbox(&e, &a) && in_inbox(&e, &b), "first archive was reverted");
}

#[test]
fn filing_by_key_per_backend() {
    let (mut e, fake) = engine();
    let ox = r(&e, "Lab partner for Thursday?");
    let gm = r(&e, "Café on Sunday");
    let out = e
        .triage(
            req(Intent::File { key: "Admin".into() }, vec![ox.clone(), gm.clone()]),
            Who::ALEC,
        )
        .unwrap();
    assert_eq!(out.applied.unwrap().status, Status::Applied);
    let calls = fake.calls.lock().unwrap().clone();
    assert!(calls.iter().any(|(_, s)| s.location
        == Location::Mailboxes {
            physical: "Admin".into(),
            labels: vec![]
        }));
    // Gmail: the label *id* for "Admin & Receipts".
    assert!(calls.iter().any(|(_, s)| matches!(&s.location, Location::Labels { labels } if labels.contains(&"Label_1".to_string()) && !labels.contains(&"INBOX".to_string()))));
    let rows = view::list(
        &e.db,
        &e.cfg,
        &view::View::Key("Admin".into()),
        &view::Filter::default(),
    )
    .unwrap();
    assert!(rows.iter().any(|x| x.r#ref == ox) && rows.iter().any(|x| x.r#ref == gm));
    // A key not mapped on the account is skipped with the reason.
    let oq = r(&e, "Poster for the talk");
    let out = e
        .triage(req(Intent::File { key: "Admin".into() }, vec![oq]), Who::ALEC)
        .unwrap();
    assert!(
        out.skipped[0].1.contains("isn't available on OQTS"),
        "{:?}",
        out.skipped
    );
}

#[test]
fn snooze_wakes_at_time_and_early_on_reply() {
    let (mut e, _) = engine();
    let t = r(&e, "Lab partner for Thursday?");
    let until = NOW + 3 * 3600;
    let out = e
        .triage(req(Intent::Snooze { until }, vec![t.clone()]), Who::ALEC)
        .unwrap();
    assert_eq!(out.applied.unwrap().status, Status::Applied);
    assert!(!in_inbox(&e, &t));
    let (a, tid) = parse_thread_ref(&t).unwrap();
    assert_eq!(e.db.snooze(a, tid).unwrap().unwrap().until, until);
    assert_eq!(e.wake_snoozes().unwrap(), 0, "not yet");
    set_clock(&mut e, until + 1);
    assert_eq!(e.wake_snoozes().unwrap(), 1);
    assert!(in_inbox(&e, &t));
    assert!(e.db.snooze(a, tid).unwrap().is_none());
    assert!(thread(&e, &t).iter().any(|m| !m.read), "woken thread is unread");

    // Early wake: a reply from someone else arrives while snoozed.
    set_clock(&mut e, NOW);
    let t2 = r(&e, "Coursework submission portal");
    e.triage(req(Intent::Snooze { until: NOW + 86_400 }, vec![t2.clone()]), Who::ALEC)
        .unwrap();
    let mut reply = thread(&e, &t2)[0].clone();
    reply.source_id = "reply1".into();
    reply.message_id = "reply1@x".into();
    reply.date = NOW + 60;
    e.db.upsert(&reply, Raw::None, keryx_core::text::TEXT_VERSION).unwrap();
    set_clock(&mut e, NOW + 120);
    assert_eq!(e.wake_snoozes().unwrap(), 1);
    assert!(in_inbox(&e, &t2));
}

#[test]
fn snooze_in_the_past_is_refused_and_undo_restores() {
    let (mut e, _) = engine();
    let t = r(&e, "Lab partner for Thursday?");
    assert!(matches!(
        e.triage(req(Intent::Snooze { until: NOW - 1 }, vec![t.clone()]), Who::ALEC),
        Err(EngineError::BadRequest(_))
    ));
    e.triage(req(Intent::Snooze { until: NOW + 100 }, vec![t.clone()]), Who::ALEC)
        .unwrap();
    e.undo_last(Who::ALEC).unwrap();
    let (a, tid) = parse_thread_ref(&t).unwrap();
    assert!(e.db.snooze(a, tid).unwrap().is_none());
    assert!(in_inbox(&e, &t));
}

#[test]
fn needs_reply_tags_clear_on_archive_and_on_reply() {
    let (mut e, _) = engine();
    let t = r(&e, "Lab partner for Thursday?");
    e.tag(&t, "asks about Thursday", Who::CLAUDE).unwrap();
    let (a, tid) = parse_thread_ref(&t).unwrap();
    assert!(e.db.tag(a, tid, NEEDS_REPLY).unwrap().is_some());
    e.triage(req(Intent::Archive, vec![t.clone()]), Who::ALEC).unwrap();
    assert!(e.db.tag(a, tid, NEEDS_REPLY).unwrap().is_none());

    let t2 = r(&e, "Coursework submission portal");
    e.tag(&t2, "submit by Friday", Who::CLAUDE).unwrap();
    assert_eq!(e.housekeeping().unwrap(), 0);
    let mut mine = thread(&e, &t2)[0].clone();
    mine.source_id = "mine1".into();
    mine.message_id = "mine1@x".into();
    mine.from = Some(Address::new(None, "newc7370@ox.ac.uk"));
    mine.date = NOW + 60;
    e.db.upsert(&mine, Raw::None, keryx_core::text::TEXT_VERSION).unwrap();
    assert_eq!(e.housekeeping().unwrap(), 1);
}

#[test]
fn mark_read_on_open() {
    let (mut e, _) = engine();
    let t = r(&e, "Lab partner for Thursday?");
    assert!(thread(&e, &t).iter().any(|m| !m.read));
    e.triage(req(Intent::Read { on: true }, vec![t.clone()]), Who::ALEC)
        .unwrap();
    assert!(thread(&e, &t).iter().all(|m| m.read));
    let out = e.triage(req(Intent::Read { on: true }, vec![t]), Who::ALEC).unwrap();
    assert!(out.applied.is_none(), "nothing to do twice");
}

fn propose(e: &mut Engine, items: Vec<(String, Action)>) -> String {
    e.propose(
        ProposeReq {
            review_id: None,
            title: "Oxford cleanup".into(),
            items: items
                .into_iter()
                .map(|(r, action)| ProposeItem {
                    r#ref: r,
                    action,
                    reason: "test".into(),
                })
                .collect(),
        },
        Who::CLAUDE,
    )
    .unwrap()
    .review_id
}

#[test]
fn cleanup_review_decisions() {
    let (mut e, fake) = engine();
    let a = r(&e, "Lab partner for Thursday?");
    let b = r(&e, "Coursework submission portal");
    let c = r(&e, "Grüße");
    let d = r(&e, "Hello");
    let rid = propose(
        &mut e,
        vec![
            (a.clone(), Action::Archive),
            (b.clone(), Action::Archive),
            (c.clone(), Action::File { key: "Admin".into() }),
            (
                d.clone(),
                Action::Keep {
                    why: keryx_store::state::Keep::Action,
                    needs_reply: true,
                },
            ),
        ],
    );
    // Proposals never touch a mailbox.
    assert!(fake.calls.lock().unwrap().is_empty());
    assert!(in_inbox(&e, &a) && in_inbox(&e, &c));

    // Claude can't decide.
    let p = |e: &Engine, rf: &str| {
        e.review(&rid)
            .unwrap()
            .items
            .into_iter()
            .find(|i| i.r#ref == rf)
            .unwrap()
            .proposal
            .id
    };
    assert!(matches!(
        e.decide(&p(&e, &a), Decision::Accept, Who::CLAUDE),
        Err(EngineError::Forbidden(_))
    ));

    e.decide(&p(&e, &a), Decision::Accept, Who::ALEC).unwrap();
    assert!(!in_inbox(&e, &a));
    e.decide(&p(&e, &b), Decision::Decline, Who::ALEC).unwrap();
    assert!(in_inbox(&e, &b), "declined: untouched");
    e.decide(&p(&e, &c), Decision::Change(Action::Archive), Who::ALEC)
        .unwrap();
    let rv = e.review(&rid).unwrap();
    let ci = rv.items.iter().find(|i| i.r#ref == c).unwrap();
    assert_eq!(ci.proposal.status, ProposalStatus::Changed);
    assert_eq!(ci.proposal.action, Action::Archive);
    e.decide(&p(&e, &d), Decision::Accept, Who::ALEC).unwrap();
    let (da, dt) = parse_thread_ref(&d).unwrap();
    assert!(
        e.db.tag(da, dt, NEEDS_REPLY).unwrap().is_some(),
        "keep + needs reply tags it"
    );
    assert!(in_inbox(&e, &d));
    assert_eq!(e.db.review(&rid).unwrap().unwrap().status, "done");

    // Undo the whole review: accepted/changed restored, everything reopened.
    let u = e.undo_review(&rid, Who::ALEC).unwrap();
    assert_eq!(u.undone, 2, "{:?}", u.refused);
    assert!(in_inbox(&e, &a) && in_inbox(&e, &c));
    assert!(
        e.review(&rid)
            .unwrap()
            .items
            .iter()
            .all(|i| i.proposal.status == ProposalStatus::Open)
    );

    // Feedback shows Alec's overrides.
    e.decide(&p(&e, &b), Decision::Decline, Who::ALEC).unwrap();
    let fb = e.cleanup_feedback(None, 10).unwrap();
    assert_eq!(fb.len(), 1);
    assert_eq!(fb[0].decision, "declined");
}

#[test]
fn accept_group_skips_stale_proposals() {
    let (mut e, _) = engine();
    let a = r(&e, "Lab partner for Thursday?");
    let b = r(&e, "Coursework submission portal");
    let rid = propose(&mut e, vec![(a.clone(), Action::Archive), (b.clone(), Action::Archive)]);
    // A new message arrives in b after the proposal.
    let mut m = thread(&e, &b)[0].clone();
    m.source_id = "late1".into();
    m.message_id = "late1@x".into();
    m.date = NOW + 60;
    e.db.upsert(&m, Raw::None, keryx_core::text::TEXT_VERSION).unwrap();
    let rv = e.review(&rid).unwrap();
    assert!(rv.items.iter().find(|i| i.r#ref == b).unwrap().stale);
    let g = e.accept_group(&rid, "Archive", Who::ALEC).unwrap();
    assert_eq!((g.accepted, g.skipped_stale, g.changesets.len()), (1, 1, 1));
    assert!(!in_inbox(&e, &a));
    assert!(in_inbox(&e, &b), "stale proposal left for Alec");
    // One undo undoes the whole group decision.
    e.undo_last(Who::ALEC).unwrap();
    assert!(in_inbox(&e, &a));
}

#[test]
fn proposals_validate_refs_and_keys() {
    let (mut e, _) = engine();
    let a = r(&e, "Poster for the talk");
    let out = e
        .propose(
            ProposeReq {
                review_id: None,
                title: "x".into(),
                items: vec![
                    ProposeItem {
                        r#ref: "nowhere:1".into(),
                        action: Action::Archive,
                        reason: "x".into(),
                    },
                    ProposeItem {
                        r#ref: a.clone(),
                        action: Action::File { key: "Admin".into() },
                        reason: "x".into(),
                    },
                    ProposeItem {
                        r#ref: a.clone(),
                        action: Action::Snooze { until: NOW - 5 },
                        reason: "x".into(),
                    },
                    ProposeItem {
                        r#ref: a,
                        action: Action::Archive,
                        reason: "ok\u{1b}[31m".into(),
                    },
                ],
            },
            Who::CLAUDE,
        )
        .unwrap();
    assert_eq!(out.added, 1);
    assert_eq!(out.rejected.len(), 3);
    let rv = e.review(&out.review_id).unwrap();
    assert_eq!(rv.items[0].proposal.reason, "ok", "reasons are sanitised");
}
