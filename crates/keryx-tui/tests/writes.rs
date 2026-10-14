#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Phase 4 keys: each sends exactly the right request to keryxd (a fake
//! here; the engine's semantics are tested in keryxd).

use chrono::TimeZone;
use keryx_core::changeset::{Author, Changeset, Risk, Status};
use keryx_store::api::*;
use keryx_store::client::{Daemon, Result};
use keryx_store::state::{Action, Proposal, ProposalStatus, Review};
use keryx_tui::app::{App, Key, Mode};
use keryx_tui::render_to_string;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Fake {
    calls: Arc<Mutex<Vec<String>>>,
}

impl Fake {
    fn log(&self, s: String) {
        self.calls.lock().unwrap().push(s);
    }
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut self.calls.lock().unwrap())
    }
}

impl Daemon for Fake {
    fn triage(&self, req: &TriageReq) -> Result<TriageOutcome> {
        self.log(format!(
            "triage {} {:?}",
            serde_json::to_string(&req.intent).unwrap(),
            req.refs
        ));
        Ok(TriageOutcome {
            summary: "done".into(),
            ..Default::default()
        })
    }
    fn undo_last(&self) -> Result<Changeset> {
        self.log("undo".into());
        Ok(Changeset {
            id: "cs".into(),
            author: Author::Alec,
            interactive: true,
            created_at: 0,
            ops: vec![],
            risk: Risk::Low,
            reasons: vec![],
            status: Status::Applied,
            summary: "undo: archive".into(),
            review_id: None,
            undoes: None,
            error: None,
        })
    }
    fn reviews(&self, _: Option<&str>) -> Result<Vec<ReviewView>> {
        Ok(vec![])
    }
    fn review(&self, _: &str) -> Result<ReviewView> {
        unimplemented!()
    }
    fn decide(&self, p: &str, d: &Decision) -> Result<Option<Changeset>> {
        self.log(format!("decide {p} {}", serde_json::to_string(d).unwrap()));
        Ok(None)
    }
    fn accept_group(&self, r: &str, g: &str) -> Result<AcceptGroup> {
        self.log(format!("accept-group {r} {g}"));
        Ok(AcceptGroup::default())
    }
    fn undo_review(&self, r: &str) -> Result<UndoReview> {
        self.log(format!("undo-review {r}"));
        Ok(UndoReview::default())
    }
    fn tag(&self, r: &str, _: &str) -> Result<()> {
        self.log(format!("tag {r}"));
        Ok(())
    }
    fn untag(&self, r: &str) -> Result<()> {
        self.log(format!("untag {r}"));
        Ok(())
    }
    fn sync(&self) -> Result<()> {
        self.log("sync".into());
        Ok(())
    }
}

fn app() -> (App, Fake) {
    let (cfg, db) = keryx_store::testkit::fixture_db();
    let now = chrono_tz::Europe::London
        .with_ymd_and_hms(2026, 10, 14, 15, 0, 0)
        .unwrap();
    for a in &cfg.accounts {
        let state = db
            .sync_state(&a.id)
            .unwrap()
            .map(|(s, _)| s)
            .unwrap_or_else(|| "{}".into());
        db.set_sync_state(&a.id, &state, now.timestamp()).unwrap();
    }
    let mut a = App::new(cfg, db, now);
    a.dry_open = true;
    let fake = Fake::default();
    a.daemon = Some(Box::new(fake.clone()));
    (a, fake)
}

fn keys(a: &mut App, ks: &[Key]) {
    for k in ks {
        a.key(*k);
    }
    a.run_deferred();
}

fn select(a: &mut App, subject: &str) -> String {
    a.sel = a
        .rows
        .iter()
        .position(|r| r.subject == subject)
        .unwrap_or_else(|| panic!("{subject}"));
    a.rows[a.sel].r#ref.clone()
}

fn ts(y: i32, m: u32, d: u32, h: u32) -> i64 {
    chrono_tz::Europe::London
        .with_ymd_and_hms(y, m, d, h, 0, 0)
        .unwrap()
        .timestamp()
}

#[test]
fn archive_selected_or_marked() {
    let (mut a, f) = app();
    let r = select(&mut a, "Lab partner for Thursday?");
    keys(&mut a, &[Key::Char('d')]);
    assert_eq!(f.take(), vec![format!(r#"triage {{"action":"archive"}} ["{r}"]"#)]);
    assert_eq!(a.message.as_ref().unwrap().0, "done");

    let r1 = select(&mut a, "Grüße");
    a.key(Key::Char('x'));
    let r2 = select(&mut a, "Hello");
    a.key(Key::Char('x'));
    assert_eq!(a.marked.len(), 2);
    let s = render_to_string(&mut a, 80, 24);
    assert!(s.contains("2 marked · d archive"), "{s}");
    keys(&mut a, &[Key::Char('d')]);
    let call = f.take().remove(0);
    assert!(call.contains(&r1) && call.contains(&r2), "{call}");
    assert!(a.marked.is_empty());
}

#[test]
fn opening_marks_read_quietly_and_only_when_unread() {
    let (mut a, f) = app();
    let r = select(&mut a, "Lab partner for Thursday?");
    assert!(a.rows[a.sel].unread);
    keys(&mut a, &[Key::Enter]);
    assert_eq!(
        f.take(),
        vec![format!(r#"triage {{"action":"read","on":true}} ["{r}"]"#)]
    );
    assert_eq!(a.message, None, "quiet");
    keys(&mut a, &[Key::Esc]);
    select(&mut a, "Grüße");
    keys(&mut a, &[Key::Enter]);
    assert!(f.take().is_empty(), "already read");
}

#[test]
fn snooze_presets_and_pick() {
    let (mut a, f) = app();
    let r = select(&mut a, "Lab partner for Thursday?");
    keys(&mut a, &[Key::Char('s')]);
    insta::assert_snapshot!("snooze_picker", render_to_string(&mut a, 80, 14));
    keys(&mut a, &[Key::Char('2')]);
    assert_eq!(
        f.take(),
        vec![format!(
            r#"triage {{"action":"snooze","until":{}}} ["{r}"]"#,
            ts(2026, 10, 15, 8)
        )]
    );
    // Monday 08:00 from Wednesday 14 Oct.
    keys(&mut a, &[Key::Char('s'), Key::Char('3')]);
    assert!(f.take()[0].contains(&ts(2026, 10, 19, 8).to_string()));
    // Pick… with a live preview.
    a.key(Key::Char('s'));
    a.key(Key::Char('4'));
    for c in "fri 17:00".chars() {
        a.key(Key::Char(c));
    }
    let s = render_to_string(&mut a, 80, 14);
    assert!(s.contains("→ Fri 16 Oct, 17:00"), "{s}");
    keys(&mut a, &[Key::Enter]);
    assert!(f.take()[0].contains(&ts(2026, 10, 16, 17).to_string()));
    // Ambiguous input is explained, not guessed.
    a.key(Key::Char('s'));
    a.key(Key::Char('4'));
    for c in "wed".chars() {
        a.key(Key::Char(c));
    }
    let s = render_to_string(&mut a, 80, 14);
    assert!(s.contains("ambiguous"), "{s}");
    keys(&mut a, &[Key::Enter]);
    assert!(f.take().is_empty());
    assert!(matches!(a.mode, Mode::Snooze(Some(_))));
}

#[test]
fn file_picker_only_offers_mapped_keys() {
    let (mut a, f) = app();
    let r = select(&mut a, "Lab partner for Thursday?");
    keys(&mut a, &[Key::Char('m')]);
    let names: Vec<String> = a.file_keys().into_iter().map(|(_, n)| n).collect();
    assert_eq!(names, vec!["Admin", "Engineering", "Careers", "OQTS", "JCR Treasurer"]);
    insta::assert_snapshot!("file_picker", render_to_string(&mut a, 80, 12));
    keys(&mut a, &[Key::Char('4')]);
    assert!(f.take().is_empty(), "Travel isn't mapped for Oxford");
    assert_eq!(a.mode, Mode::File, "picker stays open");
    keys(&mut a, &[Key::Char('1')]);
    assert_eq!(
        f.take(),
        vec![format!(r#"triage {{"action":"file","key":"Admin"}} ["{r}"]"#)]
    );
    // An account with no keys mapped says so.
    select(&mut a, "Poster for the talk");
    keys(&mut a, &[Key::Char('m')]);
    assert!(a.message.as_ref().unwrap().0.contains("no key is mapped"));
}

#[test]
fn flag_unread_undo_tag() {
    let (mut a, f) = app();
    let r = select(&mut a, "Lab partner for Thursday?");
    keys(
        &mut a,
        &[Key::Char('f'), Key::Char('U'), Key::Char('u'), Key::Char('N')],
    );
    let calls = f.take();
    assert_eq!(calls[0], format!(r#"triage {{"action":"flag","on":true}} ["{r}"]"#));
    assert_eq!(calls[1], format!(r#"triage {{"action":"read","on":false}} ["{r}"]"#));
    assert_eq!(calls[2], "undo");
    assert_eq!(calls[3], format!("tag {r}"));
    // Flagged thread: f unflags.
    let fr = select(&mut a, "RE: Reimbursement");
    keys(&mut a, &[Key::Char('f')]);
    assert_eq!(
        f.take(),
        vec![format!(r#"triage {{"action":"flag","on":false}} ["{fr}"]"#)]
    );
}

#[test]
fn archiving_from_the_thread_goes_back_to_the_list() {
    let (mut a, f) = app();
    let r = select(&mut a, "Grüße");
    keys(&mut a, &[Key::Enter, Key::Char('d')]);
    assert_eq!(a.mode, Mode::List);
    assert_eq!(f.take(), vec![format!(r#"triage {{"action":"archive"}} ["{r}"]"#)]);
}

#[test]
fn without_keryxd_changes_are_explained() {
    let (mut a, _) = app();
    a.daemon = None;
    select(&mut a, "Lab partner for Thursday?");
    keys(&mut a, &[Key::Char('d')]);
    assert!(a.message.as_ref().unwrap().0.contains("keryxd isn't running"));
}

fn seed_review(a: &App) -> (String, Vec<String>) {
    let subjects = [
        "Lab partner for Thursday?",
        "Grüße",
        "Hello",
        "Coursework submission portal",
    ];
    let actions = [
        Action::Archive,
        Action::Archive,
        Action::File { key: "Admin".into() },
        Action::Keep {
            why: keryx_store::state::Keep::Action,
            needs_reply: true,
        },
    ];
    a.db.create_review(&Review {
        id: "rv-1".into(),
        title: "Oxford inbox".into(),
        author: "claude".into(),
        run_id: None,
        created_at: 0,
        status: "open".into(),
    })
    .unwrap();
    let mut ids = Vec::new();
    for (i, (s, act)) in subjects.iter().zip(actions).enumerate() {
        let row = a.rows.iter().find(|r| r.subject == *s).unwrap();
        let msgs = a.db.thread(&row.account, &row.thread_id).unwrap();
        let id = format!("p-{i}");
        a.db.add_proposal(&Proposal {
            id: id.clone(),
            review_id: "rv-1".into(),
            position: i as i64,
            account: row.account.clone(),
            thread_id: row.thread_id.clone(),
            action: act,
            reason: format!("reason {i}"),
            fingerprint: keryx_store::state::fingerprint(&msgs),
            status: ProposalStatus::Open,
            decided_at: None,
            changeset_id: None,
        })
        .unwrap();
        ids.push(id);
    }
    ("rv-1".into(), ids)
}

#[test]
fn cleanup_review_screen() {
    let (mut a, f) = app();
    let (rid, ids) = seed_review(&a);
    a.reload();
    let s = render_to_string(&mut a, 80, 6);
    assert!(s.contains("Cleanup: 4 to review · A"), "{s}");
    keys(&mut a, &[Key::Char('A')]);
    insta::assert_snapshot!("approvals", render_to_string(&mut a, 80, 8));
    keys(&mut a, &[Key::Enter]);
    assert!(matches!(a.mode, Mode::Review { .. }));
    insta::assert_snapshot!("review", render_to_string(&mut a, 100, 16));

    // Archive group first (largest): y accepts the selection.
    let first = a.review_selected(0).unwrap();
    let first_id = a.review.as_ref().unwrap().items[first].proposal.id.clone();
    keys(&mut a, &[Key::Char('y')]);
    assert_eq!(f.take(), vec![format!(r#"decide {first_id} {{"decision":"accept"}}"#)]);
    // n declines the next; c 1 changes the one after to File → Admin.
    keys(&mut a, &[Key::Char('n')]);
    assert!(f.take()[0].ends_with(r#"{"decision":"decline"}"#));
    keys(&mut a, &[Key::Char('c'), Key::Char('1')]);
    assert!(f.take()[0].ends_with(r#"{"decision":"change","action":"file","key":"Admin"}"#));
    // Y accepts the selection's whole group, after a confirm.
    keys(
        &mut a,
        &[Key::Char('k'), Key::Char('k'), Key::Char('k'), Key::Char('Y')],
    );
    assert!(matches!(a.mode, Mode::ConfirmGroup { ref group, count: 2, .. } if group == "Archive"));
    keys(&mut a, &[Key::Char('y')]);
    assert_eq!(f.take(), vec![format!("accept-group {rid} Archive")]);
    // Reading a thread from the review doesn't mark it read; Esc comes back.
    keys(&mut a, &[Key::Enter]);
    assert_eq!(a.mode, Mode::Thread);
    assert!(f.take().is_empty());
    keys(&mut a, &[Key::Esc]);
    assert!(matches!(a.mode, Mode::Review { .. }));
    // x marks several, y decides them all.
    keys(&mut a, &[Key::Char('x'), Key::Char('x'), Key::Char('y')]);
    assert_eq!(f.take().len(), 2);
    assert!(ids.len() == 4);
}

#[test]
fn split_review_previews_without_marking_read() {
    let (mut a, f) = app();
    seed_review(&a);
    a.reload();
    keys(&mut a, &[Key::Char('A'), Key::Enter]);
    let s = render_to_string(&mut a, 160, 20);
    assert!(s.contains("Thread"), "{s}");
    assert!(f.take().is_empty());
}
