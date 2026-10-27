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
    sent: Arc<Mutex<Vec<keryx_core::changeset::Outgoing>>>,
    unsubscribed: Arc<Mutex<Vec<String>>>,
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
    fn send(&mut self, _a: &Account, o: &keryx_core::changeset::Outgoing) -> std::result::Result<(), String> {
        self.sent.lock().unwrap().push(o.clone());
        Ok(())
    }
    fn unsubscribe(&mut self, url: &str) -> std::result::Result<(), String> {
        self.unsubscribed.lock().unwrap().push(url.into());
        Ok(())
    }
}

const NOW: i64 = 1_791_986_400; // Wed 14 Oct 2026 15:00 BST

fn engine() -> (Engine, Fake) {
    let (mut cfg, db) = keryx_store::testkit::fixture_db();
    cfg.live = true;
    // The snooze semantics stay tested while the feature is off by default.
    cfg.snooze.enabled = true;
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
        out.summary.contains("10 threads") && out.summary.contains("2 threads left for you"),
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
    assert!(out.skipped[0].1.contains("left for you"), "{:?}", out.skipped);
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
        hold_until: None,
        warnings: vec![],
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
        hold_until: None,
        warnings: vec![],
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
            req(
                Intent::File {
                    key: "Admin".into(),
                    item: None,
                },
                vec![ox.clone(), gm.clone()],
            ),
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
        .triage(
            req(
                Intent::File {
                    key: "Admin".into(),
                    item: None,
                },
                vec![oq],
            ),
            Who::ALEC,
        )
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
fn needs_reply_survives_filing_and_clears_on_reply() {
    let (mut e, _) = engine();
    let t = r(&e, "Lab partner for Thursday?");
    e.tag(&t, "asks about Thursday", Who::CLAUDE).unwrap();
    let (a, tid) = parse_thread_ref(&t).unwrap();
    assert!(e.db.tag(a, tid, NEEDS_REPLY).unwrap().is_some());
    // Round 9: filed or archived, a thread you owe a reply stays marked, and in the inbox list.
    e.triage(req(Intent::Archive, vec![t.clone()]), Who::ALEC).unwrap();
    assert!(e.db.tag(a, tid, NEEDS_REPLY).unwrap().is_some());
    let inbox = view::list(&e.db, &e.cfg, &view::View::Inbox, &view::Filter::default()).unwrap();
    assert!(inbox.iter().any(|r| r.r#ref == t), "still listed in the inbox");

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
            (
                c.clone(),
                Action::File {
                    key: "Admin".into(),
                    item: None,
                },
            ),
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
                        action: Action::File {
                            key: "Admin".into(),
                            item: None,
                        },
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

// ---------------- sending (Phase 6) ----------------

use keryx_store::api::{DraftEmailReq, DraftReplyReq, SendReq, UnsubscribeOutcome};

fn send_req(to: &str) -> SendReq {
    SendReq {
        account: "oxford".into(),
        thread: None,
        to: vec![Address::new(None, to)],
        cc: vec![],
        bcc: vec![],
        subject: "[keryx-test] hello".into(),
        body: "hi".into(),
        attachments: vec![],
    }
}

#[test]
fn alecs_send_holds_then_sends_and_can_be_cancelled() {
    let (mut e, fake) = engine();
    let cs = e.send(&send_req("jane.doe@example.com"), Who::ALEC).unwrap();
    assert_eq!(cs.status, Status::Holding);
    assert_eq!(cs.hold_until, Some(NOW + 10));
    assert_eq!(e.process_holds().unwrap(), 0, "still holding");
    set_clock(&mut e, NOW + 10);
    assert_eq!(e.process_holds().unwrap(), 1);
    assert_eq!(e.db.changeset(&cs.id).unwrap().unwrap().status, Status::Sent);
    assert_eq!(fake.sent.lock().unwrap().len(), 1);
    assert!(matches!(e.undo(&cs.id, Who::ALEC), Err(EngineError::BadRequest(m)) if m.contains("can't be unsent")));

    set_clock(&mut e, NOW);
    let cs2 = e.send(&send_req("jane.doe@example.com"), Who::ALEC).unwrap();
    assert_eq!(e.holding().unwrap().unwrap().id, cs2.id);
    e.cancel(&cs2.id, Who::ALEC).unwrap();
    set_clock(&mut e, NOW + 60);
    assert_eq!(e.process_holds().unwrap(), 0);
    assert_eq!(fake.sent.lock().unwrap().len(), 1, "cancelled send never left");
}

#[test]
fn claude_drafts_wait_for_alec_with_thread_recipients() {
    let (mut e, fake) = engine();
    let t = r(&e, "Lab partner for Thursday?");
    let cs = e
        .draft_reply(
            &DraftReplyReq {
                r#ref: t.clone(),
                body: "Thursday works.".into(),
                reply_all: false,
            },
            Who::claude(),
        )
        .unwrap();
    assert_eq!(cs.status, Status::Pending);
    assert!(cs.reasons.iter().any(|x| x.contains("always needs Alec")));
    let o = cs.outgoing().unwrap();
    assert_eq!(
        o.to.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(),
        vec!["jane.doe@example.com"]
    );
    assert_eq!(o.subject, "Re: Lab partner for Thursday?");
    assert_eq!(o.in_reply_to.as_deref(), Some("plain-01@example.com"));
    assert!(o.body.starts_with("Thursday works.") && o.body.contains("> Are you free"));
    // Only Alec approves.
    assert!(matches!(
        e.approve(&cs.id, Who::claude()),
        Err(EngineError::Forbidden(_))
    ));
    let held = e.approve(&cs.id, Who::ALEC).unwrap();
    assert_eq!(held.status, Status::Holding);
    set_clock(&mut e, NOW + 11);
    e.process_holds().unwrap();
    assert_eq!(fake.sent.lock().unwrap()[0].to[0].email, "jane.doe@example.com");
}

#[test]
fn new_recipients_are_flagged_and_claude_cant_attach() {
    let (mut e, _) = engine();
    let cs = e
        .draft_email(
            &DraftEmailReq {
                account: "oxford".into(),
                to: vec![Address::new(None, "x@evil.example")],
                cc: vec![],
                subject: "hello".into(),
                body: "see https://evil.example/?d=secret".into(),
            },
            Who::claude(),
        )
        .unwrap();
    assert_eq!(cs.status, Status::Pending);
    assert!(
        cs.warnings.iter().any(|w| w.contains("⚠ new: x@evil.example")),
        "{:?}",
        cs.warnings
    );
    let mut with_file = send_req("jane.doe@example.com");
    with_file.attachments = vec!["/etc/hosts".into()];
    assert!(matches!(
        e.send(&with_file, Who::claude()),
        Err(EngineError::Forbidden(_))
    ));
}

#[test]
fn live_false_sends_only_to_alec() {
    let (mut e, _) = engine();
    e.cfg.live = false;
    assert!(matches!(
        e.send(&send_req("jane.doe@example.com"), Who::ALEC),
        Err(EngineError::Forbidden(_))
    ));
    assert_eq!(
        e.send(&send_req("alec.mitchellthomson@gmail.com"), Who::ALEC)
            .unwrap()
            .status,
        Status::Holding
    );
}

#[test]
fn changed_attachment_fails_the_send() {
    let (mut e, fake) = engine();
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("form.pdf");
    std::fs::write(&f, b"v1").unwrap();
    let mut req = send_req("jane.doe@example.com");
    req.attachments = vec![f.to_string_lossy().into_owned()];
    let cs = e.send(&req, Who::ALEC).unwrap();
    std::fs::write(&f, b"v2 changed").unwrap();
    set_clock(&mut e, NOW + 11);
    assert_eq!(e.process_holds().unwrap(), 0);
    let after = e.db.changeset(&cs.id).unwrap().unwrap();
    assert_eq!(after.status, Status::Failed);
    assert!(after.error.unwrap().contains("changed after you confirmed"));
    assert!(fake.sent.lock().unwrap().is_empty());
}

#[test]
fn editing_a_draft_replaces_it_and_replies_clear_the_tag() {
    let (mut e, _) = engine();
    let t = r(&e, "Lab partner for Thursday?");
    e.tag(&t, "asks about Thursday", Who::claude()).unwrap();
    let draft = e
        .draft_reply(
            &DraftReplyReq {
                r#ref: t.clone(),
                body: "Yes".into(),
                reply_all: false,
            },
            Who::claude(),
        )
        .unwrap();
    let mut mine = send_req("jane.doe@example.com");
    mine.thread = Some(t.clone());
    mine.body = "Yes — see you at 2".into();
    let new = e.replace(&draft.id, &mine, Who::ALEC).unwrap();
    assert_eq!(new.status, Status::Holding);
    let old = e.db.changeset(&draft.id).unwrap().unwrap();
    assert_eq!(old.status, Status::Rejected);
    assert!(old.error.unwrap().contains("replaced"));
    set_clock(&mut e, NOW + 11);
    e.process_holds().unwrap();
    let (a, tid) = parse_thread_ref(&t).unwrap();
    assert!(e.db.tag(a, tid, NEEDS_REPLY).unwrap().is_none());
}

#[test]
fn unsubscribe_paths() {
    let (mut e, fake) = engine();
    let news = view::list(&e.db, &e.cfg, &view::View::Inbox, &view::Filter::default())
        .unwrap()
        .into_iter()
        .find(|x| x.subject == "This week in engineering" && x.account == "personal")
        .unwrap()
        .r#ref;
    assert!(matches!(
        e.unsubscribe(&news, Who::claude()),
        Err(EngineError::Forbidden(_))
    ));
    let out = e.unsubscribe(&news, Who::ALEC).unwrap();
    assert!(matches!(out, UnsubscribeOutcome::Done { .. }));
    assert_eq!(
        fake.unsubscribed.lock().unwrap()[0],
        "https://weekly.example.com/unsub?u=abc"
    );
    assert!(!in_inbox(&e, &news), "archived too");
    let club = view::list(&e.db, &e.cfg, &view::View::Inbox, &view::Filter::default())
        .unwrap()
        .into_iter()
        .find(|x| x.subject == "Training schedule")
        .unwrap()
        .r#ref;
    assert!(
        matches!(e.unsubscribe(&club, Who::ALEC).unwrap(), UnsubscribeOutcome::Mailto { to, .. } if to == "leave-club@sports.example.org")
    );
    let plain = r(&e, "Lab partner for Thursday?");
    assert_eq!(e.unsubscribe(&plain, Who::ALEC).unwrap(), UnsubscribeOutcome::None);
}

// ---- kairos link (SPEC §9, Phase 7 AC) ----

use keryx_store::api::{FreeReq, KairosEventReq, KairosTaskReq, SuggestEventReq, SuggestTaskReq};
use keryx_store::kairos::{As, Kairos, KairosError};
use serde_json::{Value, json};

#[derive(Clone, Default)]
struct MockKairos {
    calls: Arc<Mutex<Vec<(String, Value, As)>>>,
    down: Arc<Mutex<bool>>,
}

impl MockKairos {
    fn check(&self) -> keryx_store::kairos::Result<()> {
        if *self.down.lock().unwrap() {
            return Err(KairosError::Unreachable {
                url: "http://compute-node:7420".into(),
                msg: "connection refused".into(),
            });
        }
        Ok(())
    }
}

impl Kairos for MockKairos {
    fn create_task(&self, req: &Value, who: As) -> keryx_store::kairos::Result<Value> {
        self.check()?;
        self.calls.lock().unwrap().push(("task".into(), req.clone(), who));
        Ok(json!({
            "task": {"id": "t-42", "title": req["title"], "status": "open"},
            "plan": {"created": [{}, {}, {}], "shortfalls": [{"message": "1 h short before Fri 17:00"}], "squeezed": []}
        }))
    }
    fn create_event(&self, req: &Value, who: As) -> keryx_store::kairos::Result<Value> {
        self.check()?;
        self.calls.lock().unwrap().push(("event".into(), req.clone(), who));
        Ok(json!({"changeset_id": "kcs-1", "risk": "low", "status": "applied", "summary": "event"}))
    }
    fn task(&self, id: &str) -> keryx_store::kairos::Result<Option<Value>> {
        self.check()?;
        Ok((id == "t-42").then(|| json!({"id": "t-42", "title": "Lab", "status": "done", "remaining_min": 0})))
    }
    fn free(&self, _: &str, _: &str, _: u32) -> keryx_store::kairos::Result<Vec<keryx_core::freetime::Slot>> {
        self.check()?;
        let at = |d: i64, h: i64| NOW + d * 86_400 + h * 3600;
        Ok(vec![
            keryx_core::freetime::Slot {
                start: at(1, 0),
                end: at(1, 1),
            },
            keryx_core::freetime::Slot {
                start: at(1, 1),
                end: at(1, 2),
            },
            keryx_core::freetime::Slot {
                start: at(1, 2),
                end: at(1, 3),
            },
            keryx_core::freetime::Slot {
                start: at(2, -4),
                end: at(2, -3),
            },
        ])
    }
}

fn with_kairos() -> (Engine, MockKairos) {
    let (mut e, _) = engine();
    let k = MockKairos::default();
    e.kairos = Some(Box::new(k.clone()));
    (e, k)
}

fn task_req(rf: &str) -> KairosTaskReq {
    KairosTaskReq {
        r#ref: rf.into(),
        title: "Find a lab partner".into(),
        due: Some("fri 17:00".into()),
        estimate_min: Some(30),
        kind: Some("admin".into()),
        ..Default::default()
    }
}

#[test]
fn kairos_task_as_alec_with_link_in_notes() {
    let (mut e, k) = with_kairos();
    let t = r(&e, "Lab partner for Thursday?");
    let out = e.kairos_task(&task_req(&t), Who::ALEC).unwrap();
    let calls = k.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    let (_, body, who) = &calls[0];
    assert_eq!(*who, As::Alec);
    let (a, tid) = parse_thread_ref(&t).unwrap();
    let notes = body["notes"].as_str().unwrap();
    assert!(notes.contains(&format!("keryx://thread/{a}/{tid}")), "{notes}");
    assert!(notes.contains("Lab partner for Thursday?"), "{notes}");
    assert_eq!(body["due"], "fri 17:00");
    assert_eq!(body["kind"], "admin");
    assert_eq!(body["estimate_min"], 30);
    assert_eq!(body["key"], "Personal", "unfiled thread → Personal");
    assert_eq!(out.link.target, "t-42");
    assert_eq!(out.link.kind, "deadline");
    assert_eq!(out.summary, "kairos task: Find a lab partner · 3 blocks planned");
    assert_eq!(out.notes, vec!["1 h short before Fri 17:00"]);
    assert_eq!(e.db.links(a, tid).unwrap().len(), 1);
    // Live status for the thread view.
    assert_eq!(e.kairos_task_status("t-42").unwrap().unwrap().status, "done");
    assert!(e.kairos_task_status("gone").unwrap().is_none());
}

#[test]
fn kairos_key_follows_the_threads_key() {
    let (mut e, k) = with_kairos();
    let t = r(&e, "Lab partner for Thursday?");
    e.triage(
        req(
            Intent::File {
                key: "Careers".into(),
                item: None,
            },
            vec![t.clone()],
        ),
        Who::ALEC,
    )
    .unwrap();
    e.kairos_task(&task_req(&t), Who::ALEC).unwrap();
    assert_eq!(k.calls.lock().unwrap()[0].1["key"], "Careers");
    // An explicit key wins.
    let mut rq = task_req(&t);
    rq.key = Some("Sport".into());
    e.kairos_task(&rq, Who::ALEC).unwrap();
    assert_eq!(k.calls.lock().unwrap()[1].1["key"], "Sport");
}

#[test]
fn kairos_author_rules() {
    let (mut e, k) = with_kairos();
    let t = r(&e, "Lab partner for Thursday?");
    // A script (CLI without a TTY) is a script to kairos too.
    let script = Who {
        author: Author::Script,
        interactive: false,
    };
    e.kairos_task(&task_req(&t), script).unwrap();
    assert_eq!(k.calls.lock().unwrap()[0].2, As::Script);
    // Claude never writes to kairos through keryx.
    let err = e.kairos_task(&task_req(&t), Who::CLAUDE).unwrap_err();
    assert!(matches!(err, EngineError::Forbidden(m) if m.contains("suggest_task")));
    let ev = KairosEventReq {
        r#ref: t.clone(),
        title: "x".into(),
        start: "2026-10-20T10:00".into(),
        ..Default::default()
    };
    assert!(matches!(
        e.kairos_event(&ev, Who::CLAUDE),
        Err(EngineError::Forbidden(_))
    ));
    assert_eq!(k.calls.lock().unwrap().len(), 1);
}

#[test]
fn kairos_down_is_a_clear_error_and_no_link() {
    let (mut e, k) = with_kairos();
    *k.down.lock().unwrap() = true;
    let t = r(&e, "Lab partner for Thursday?");
    let err = e.kairos_task(&task_req(&t), Who::ALEC).unwrap_err();
    assert_eq!(
        err.to_string(),
        "kairos isn't reachable at compute-node:7420: task not created"
    );
    let (a, tid) = parse_thread_ref(&t).unwrap();
    assert!(e.db.links(a, tid).unwrap().is_empty());
}

#[test]
fn kairos_respects_live_false() {
    let (mut e, k) = with_kairos();
    e.cfg.live = false;
    let t = r(&e, "Lab partner for Thursday?");
    assert!(
        matches!(e.kairos_task(&task_req(&t), Who::ALEC), Err(EngineError::Forbidden(m)) if m.contains("[keryx-test]"))
    );
    assert!(k.calls.lock().unwrap().is_empty());
    let test = add(&e, "personal", 901, "jane@example.com", false);
    e.db.conn()
        .execute(
            "UPDATE messages SET subject = '[keryx-test] task me' WHERE source_id = 'syn901'",
            [],
        )
        .unwrap();
    e.kairos_task(&task_req(&test), Who::ALEC).unwrap();
}

#[test]
fn kairos_event_from_an_invite() {
    let (mut e, k) = with_kairos();
    let t = r(&e, "Lab partner for Thursday?");
    let out = e
        .kairos_event(
            &KairosEventReq {
                r#ref: t.clone(),
                title: "Design review".into(),
                start: "2026-10-20T14:00:00+01:00".into(),
                end: Some("2026-10-20T15:00:00+01:00".into()),
                location: Some("LR6".into()),
                ..Default::default()
            },
            Who::ALEC,
        )
        .unwrap();
    let (_, body, who) = k.calls.lock().unwrap()[0].clone();
    assert_eq!(who, As::Alec);
    assert_eq!(body["start"], "2026-10-20T14:00:00+01:00");
    assert_eq!(body["end"], "2026-10-20T15:00:00+01:00");
    assert_eq!(body["location"], "LR6");
    assert!(body["description"].as_str().unwrap().contains("keryx://thread/"));
    assert_eq!(out.link.kind, "event");
    assert_eq!(out.link.target, "kcs-1");
}

#[test]
fn suggestions_accept_dismiss_and_never_again() {
    let (mut e, k) = with_kairos();
    let t = r(&e, "Lab partner for Thursday?");
    let sg = e
        .suggest_task(
            &SuggestTaskReq {
                r#ref: t.clone(),
                title: "Reply about lab\u{1b}[31m partner".into(),
                due: Some("thu 9:00".into()),
                reason: "Jane needs an answer before Thursday".into(),
                ..Default::default()
            },
            Who::CLAUDE,
        )
        .unwrap();
    // Suggesting writes nothing to kairos, and terminal escapes are gone.
    assert!(k.calls.lock().unwrap().is_empty());
    assert!(!sg.proposal["title"].as_str().unwrap().contains('\u{1b}'));
    // Claude can't dismiss; Alec can. A dismissed idea isn't re-suggested.
    assert!(matches!(
        e.dismiss_suggestion(&sg.id, Who::CLAUDE),
        Err(EngineError::Forbidden(_))
    ));
    e.dismiss_suggestion(&sg.id, Who::ALEC).unwrap();
    let again = e.suggest_task(
        &SuggestTaskReq {
            r#ref: t.clone(),
            title: "Reply about lab partner".into(),
            reason: "still".into(),
            ..Default::default()
        },
        Who::CLAUDE,
    );
    assert!(matches!(again, Err(EngineError::Forbidden(m)) if m.contains("dismissed")));
    // An event suggestion is a different kind: allowed, then accepted by saving the form.
    let ev = e
        .suggest_event(
            &SuggestEventReq {
                r#ref: t.clone(),
                title: "Lab".into(),
                start: "2026-10-15T09:00".into(),
                reason: "lab session".into(),
                ..Default::default()
            },
            Who::CLAUDE,
        )
        .unwrap();
    e.kairos_event(
        &KairosEventReq {
            r#ref: t.clone(),
            title: "Lab".into(),
            start: "2026-10-15T09:00".into(),
            suggestion: Some(ev.id.clone()),
            ..Default::default()
        },
        Who::ALEC,
    )
    .unwrap();
    let all = e.db.suggestions(None, None).unwrap();
    assert_eq!(all.iter().find(|s| s.id == ev.id).unwrap().status, "accepted");
    assert_eq!(all.iter().find(|s| s.id == sg.id).unwrap().status, "dismissed");
    // A suggestion from another thread can't be accepted here.
    let other = r(&e, "Café on Sunday");
    let mut rq = task_req(&other);
    rq.suggestion = Some(ev.id);
    assert!(matches!(e.kairos_task(&rq, Who::ALEC), Err(EngineError::NotFound(_))));
}

#[test]
fn free_times_spread_across_days() {
    let (e, _) = with_kairos();
    let out = e
        .kairos_free(&FreeReq {
            from: "2026-10-15".into(),
            to: "2026-10-17".into(),
            duration_min: 60,
            count: 2,
        })
        .unwrap();
    // Three slots on Thursday afternoon and one on Friday morning: one from each day.
    assert_eq!(out.text, "Thu 15 Oct 15:00–16:00\nFri 16 Oct 11:00–12:00");
}

#[test]
fn snoozing_off_is_refused() {
    let (mut e, fake) = engine();
    e.cfg.snooze.enabled = false;
    let t = r(&e, "Lab partner for Thursday?");
    let out = e.triage(req(Intent::Snooze { until: NOW + 3600 }, vec![t.clone()]), Who::ALEC);
    assert!(matches!(out, Err(EngineError::Forbidden(m)) if m.contains("snoozing is off")));
    assert!(fake.calls.lock().unwrap().is_empty());
    assert!(in_inbox(&e, &t));
}

// ---------------- keryx-joins-locus X7: filed for what it is for ----------------

fn with_taxonomy(e: &mut Engine) {
    e.shared = Some(locus_contract::taxonomy::load(&locus_contract::taxonomy::root()).unwrap());
}

#[test]
fn filing_for_an_item_archives_on_the_server_and_remembers_the_item() {
    let (mut e, fake) = engine();
    with_taxonomy(&mut e);
    let lab = r(&e, "Lab partner for Thursday?");
    let file = |item: &str| Intent::File {
        key: "Studies".into(),
        item: Some(item.into()),
    };
    let out = e.triage(req(file("b14"), vec![lab.clone()]), Who::ALEC).unwrap();
    assert_eq!(out.applied.unwrap().status, Status::Applied);
    assert!(!in_inbox(&e, &lab), "out of the inbox");
    // Option C (27 Oct): the server only archives; no folder is made or moved into.
    assert!(fake.calls.lock().unwrap().iter().all(|(_, s)| !matches!(&s.location,
            Location::Mailboxes { physical, .. } if physical == "Studies")));
    let (a, t) = parse_thread_ref(&lab).unwrap();
    assert_eq!(e.db.item_of(a, t).unwrap().as_deref(), Some("b14"));
    assert!(e.db.links(a, t).unwrap().is_empty(), "an item is not a kairos link");

    // Already out of the inbox: filing it again is only the mark.
    let out = e.triage(req(file("b15"), vec![lab.clone()]), Who::ALEC).unwrap();
    assert!(out.skipped.is_empty() && out.applied.is_none(), "{out:?}");
    assert_eq!(e.db.item_of(a, t).unwrap().as_deref(), Some("b15"));
    assert_eq!(
        e.db.threads_for_item("b15").unwrap(),
        vec![(a.to_string(), t.to_string(), NOW)]
    );
}

#[test]
fn an_unknown_item_moves_nothing_and_a_stream_is_marked_as_such() {
    let (mut e, fake) = engine();
    with_taxonomy(&mut e);
    let lab = r(&e, "Lab partner for Thursday?");
    let err = e
        .triage(
            req(
                Intent::File {
                    key: "Studies".into(),
                    item: Some("no-such-item".into()),
                },
                vec![lab.clone()],
            ),
            Who::ALEC,
        )
        .unwrap_err();
    assert!(err.to_string().contains("no registry item"), "{err}");
    assert!(fake.calls.lock().unwrap().is_empty() && in_inbox(&e, &lab));

    let (a, t) = parse_thread_ref(&lab).unwrap();
    e.db.set_item(a, t, Some(("b14", "B14")), NOW).unwrap();
    e.triage(
        req(
            Intent::File {
                key: "errands".into(),
                item: None,
            },
            vec![lab.clone()],
        ),
        Who::ALEC,
    )
    .unwrap();
    assert_eq!(e.db.item_of(a, t).unwrap(), None);
    assert_eq!(
        e.db.fors().unwrap().get(&(a.to_string(), t.to_string())),
        Some(&("folder".to_string(), "Errands".to_string()))
    );
    assert!(!in_inbox(&e, &lab));
}

#[test]
fn automated_mail_is_filed_as_it_arrives_and_people_mail_never() {
    let (mut e, fake) = engine();
    with_taxonomy(&mut e);
    assert_eq!(e.file_arrivals().unwrap(), 0, "the first run only sets the mark");
    assert!(
        fake.calls.lock().unwrap().is_empty(),
        "the inbox of before is never swept"
    );

    // As if keryxd had started a week ago: everything since is an arrival.
    e.db.set_meta("autofile.since", &(NOW - 7 * 86_400).to_string())
        .unwrap();
    let ticket = r(&e, "Your e-ticket: Oxford to London, Fri 23 Oct");
    let lab = r(&e, "Lab partner for Thursday?");
    let n = e.file_arrivals().unwrap();
    assert!(n >= 1, "{n}");
    assert!(!in_inbox(&e, &ticket), "a receipt went to Errands");
    assert!(in_inbox(&e, &lab), "a person's mail is never moved");
    let (ta, tt) = parse_thread_ref(&ticket).unwrap();
    assert_eq!(
        e.db.fors().unwrap().get(&(ta.to_string(), tt.to_string())),
        Some(&("folder".to_string(), "Errands".to_string())),
        "marked for Errands"
    );
    let cs = e.db.changesets(None, 10).unwrap();
    assert!(cs.iter().any(|c| c.author == keryx_core::changeset::Author::System
        && c.summary.starts_with("filed as it arrived, for Errands")));
    assert_eq!(e.file_arrivals().unwrap(), 0, "nothing new since");

    e.cfg.automated.file_on_arrival = false;
    e.db.set_meta("autofile.since", "0").unwrap();
    assert_eq!(e.file_arrivals().unwrap(), 0, "off means off");
}

#[test]
fn not_live_only_test_mail_is_filed_on_arrival() {
    let (mut e, fake) = engine();
    with_taxonomy(&mut e);
    e.cfg.live = false;
    e.db.set_meta("autofile.since", &(NOW - 7 * 86_400).to_string())
        .unwrap();
    assert_eq!(e.file_arrivals().unwrap(), 0);
    assert!(fake.calls.lock().unwrap().is_empty());
    assert!(
        e.db.changesets(None, 10).unwrap().is_empty(),
        "no failed changesets left behind"
    );
}

#[test]
fn marking_what_a_thread_is_for_moves_nothing() {
    let (mut e, fake) = engine();
    with_taxonomy(&mut e);
    let lab = r(&e, "Lab partner for Thursday?");
    let (a, t) = parse_thread_ref(&lab).unwrap();
    e.mark_for(&lab, Some("b14")).unwrap();
    assert!(fake.calls.lock().unwrap().is_empty() && in_inbox(&e, &lab));
    assert_eq!(e.db.item_of(a, t).unwrap().as_deref(), Some("b14"));
    e.mark_for(&lab, Some("errands")).unwrap();
    assert_eq!(e.db.item_of(a, t).unwrap(), None, "a stream is no item");
    assert_eq!(
        e.db.fors().unwrap().get(&(a.to_string(), t.to_string())),
        Some(&("folder".to_string(), "Errands".to_string()))
    );
    assert!(e.mark_for(&lab, Some("nope")).is_err());
    e.mark_for(&lab, None).unwrap();
    assert!(e.db.fors().unwrap().is_empty());
}

#[test]
fn a_mail_you_sent_yourself_comes_back_to_the_inbox() {
    let (mut e, _fake) = engine();
    let lab = r(&e, "Lab partner for Thursday?");
    let (a, t) = parse_thread_ref(&lab).unwrap();
    // As if you had written it to yourself: from your own address, received in the inbox.
    e.db.conn()
        .execute(
            "UPDATE messages SET from_json = '{\"name\":null,\"email\":\"newc7370@ox.ac.uk\"}'
             WHERE account = ?1 AND thread_id = ?2",
            rusqlite::params![a, t],
        )
        .unwrap();
    e.triage(req(Intent::Archive, vec![lab.clone()]), Who::ALEC).unwrap();
    assert!(!in_inbox(&e, &lab));
    let out = e.triage(req(Intent::Unarchive, vec![lab.clone()]), Who::ALEC).unwrap();
    assert!(out.skipped.is_empty(), "{:?}", out.skipped);
    assert!(in_inbox(&e, &lab), "back in the inbox");
}

#[test]
fn going_live_starts_filing_on_arrival_afresh() {
    let (mut e, fake) = engine();
    with_taxonomy(&mut e);
    e.cfg.live = false;
    assert_eq!(e.file_arrivals().unwrap(), 0, "the mark is set");
    e.db.set_meta("autofile.since", &(NOW - 7 * 86_400).to_string())
        .unwrap();
    e.cfg.live = true;
    assert_eq!(
        e.file_arrivals().unwrap(),
        0,
        "live: the mark starts again, nothing filed in a batch"
    );
    assert!(fake.calls.lock().unwrap().is_empty());
    assert_eq!(
        e.db.meta("autofile.since").unwrap().as_deref(),
        Some(NOW.to_string().as_str())
    );
}
