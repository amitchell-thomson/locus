#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Phase 4 keys: each sends exactly the right request to keryxd (a fake
//! here; the engine's semantics are tested in keryxd).

use chrono::TimeZone;
use keryx_core::changeset::{Author, Changeset, Risk, Status};
use keryx_store::api::*;
use keryx_store::client::{Daemon, Result};
use keryx_tui::app::{App, Key, Mode};
use keryx_tui::render_to_string;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Fake {
    calls: Arc<Mutex<Vec<String>>>,
    hold: Arc<Mutex<Option<Changeset>>>,
    unsub: Arc<Mutex<Option<UnsubscribeOutcome>>>,
    /// Refuse sends with this error (keryxd's live guard, say).
    refuse_send: Arc<Mutex<Option<String>>>,
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
    fn kairos_task(&self, r: &KairosTaskReq) -> Result<KairosOutcome> {
        self.log(format!("kairos-task {}", serde_json::to_string(r).unwrap()));
        Ok(KairosOutcome {
            link: keryx_store::state::Link {
                account: "oxford".into(),
                thread_id: "1".into(),
                app: "kairos".into(),
                kind: "task".into(),
                target: "t-1".into(),
                title: r.title.clone(),
                created_at: 0,
            },
            summary: format!("kairos task: {}", r.title),
            notes: vec![],
        })
    }
    fn kairos_event(&self, r: &KairosEventReq) -> Result<KairosOutcome> {
        self.log(format!("kairos-event {}", serde_json::to_string(r).unwrap()));
        Ok(KairosOutcome {
            link: keryx_store::state::Link {
                account: "oxford".into(),
                thread_id: "1".into(),
                app: "kairos".into(),
                kind: "event".into(),
                target: "cs-1".into(),
                title: r.title.clone(),
                created_at: 0,
            },
            summary: format!("kairos event: {}", r.title),
            notes: vec![],
        })
    }
    fn kairos_free(&self, r: &FreeReq) -> Result<FreeOut> {
        self.log(format!("kairos-free {}", serde_json::to_string(r).unwrap()));
        Ok(FreeOut {
            slots: vec![(0, 3600), (86_400, 90_000)],
            text: "Tue 20 Oct 14:00–15:00\nWed 21 Oct 10:00–11:00".into(),
        })
    }
    fn task_status(&self, _: &str) -> Result<Option<TaskStatus>> {
        Ok(None)
    }
    fn undo(&self, id: &str) -> Result<Changeset> {
        let c = self.undo_last();
        // undo_last logged "undo": say which one instead.
        self.calls.lock().unwrap().pop();
        self.log(format!("undo {id}"));
        c
    }
    fn dismiss_suggestion(&self, id: &str) -> Result<()> {
        self.log(format!("dismiss {id}"));
        Ok(())
    }
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
            hold_until: None,
            warnings: vec![],
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
    fn mark_for(&self, r: &str, id: Option<&str>) -> Result<()> {
        self.log(format!("for {} {r}", id.unwrap_or("-")));
        Ok(())
    }
    fn sync(&self) -> Result<()> {
        self.log("sync".into());
        Ok(())
    }
    fn send(&self, r: &SendReq) -> Result<Changeset> {
        if let Some(e) = self.refuse_send.lock().unwrap().clone() {
            return Err(keryx_store::client::ClientError::Daemon(e));
        }
        self.log(format!(
            "send {} to={:?} cc={:?} thread={:?} body={:?}",
            r.account,
            r.to.iter().map(|a| a.email.clone()).collect::<Vec<_>>(),
            r.cc.iter().map(|a| a.email.clone()).collect::<Vec<_>>(),
            r.thread,
            r.body
        ));
        Ok(cs(Status::Holding))
    }
    fn approve(&self, id: &str) -> Result<Changeset> {
        self.log(format!("approve {id}"));
        Ok(cs(Status::Holding))
    }
    fn reject(&self, id: &str) -> Result<Changeset> {
        self.log(format!("reject {id}"));
        Ok(cs(Status::Rejected))
    }
    fn cancel(&self, id: &str) -> Result<Changeset> {
        self.log(format!("cancel {id}"));
        Ok(cs(Status::Cancelled))
    }
    fn replace(&self, id: &str, r: &SendReq) -> Result<Changeset> {
        self.log(format!("replace {id} body={:?}", r.body));
        Ok(cs(Status::Holding))
    }
    fn holding(&self) -> Result<Option<Changeset>> {
        Ok(self.hold.lock().unwrap().clone())
    }
    fn unsubscribe(&self, r: &str) -> Result<UnsubscribeOutcome> {
        self.log(format!("unsubscribe {r}"));
        Ok(self.unsub.lock().unwrap().clone().unwrap_or(UnsubscribeOutcome::None))
    }
}

fn cs(status: Status) -> Changeset {
    Changeset {
        id: "cs-1".into(),
        author: Author::Alec,
        interactive: true,
        created_at: 0,
        ops: vec![],
        risk: Risk::Low,
        reasons: vec![],
        status,
        summary: "x".into(),
        review_id: None,
        undoes: None,
        error: None,
        hold_until: None,
        warnings: vec![],
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
    a.key(Key::Char('v'));
    let r2 = select(&mut a, "Hello");
    a.key(Key::Char('v'));
    assert_eq!(a.marked.len(), 2);
    let s = render_to_string(&mut a, 80, 24);
    assert!(s.contains("2 marked") && s.contains("d done"), "{s}");
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
    a.cfg.snooze.enabled = true;
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

fn with_taxonomy(a: &mut App) {
    a.shared = Some(locus_contract::taxonomy::load(&locus_contract::taxonomy::root()).unwrap());
}

#[test]
fn done_files_for_what_it_is_for_with_the_guess_chosen() {
    let (mut a, f) = app();
    with_taxonomy(&mut a);
    // A receipt: guessed Errands, Enter takes it.
    let ticket = select(&mut a, "Your e-ticket: Oxford to London, Fri 23 Oct");
    keys(&mut a, &[Key::Char('d')]);
    insta::assert_snapshot!("file_picker", render_to_string(&mut a, 80, 16));
    keys(&mut a, &[Key::Enter]);
    assert_eq!(
        f.take(),
        vec![format!(r#"triage {{"action":"file","key":"Errands"}} ["{ticket}"]"#)]
    );

    // No guess: Enter chooses nothing until you type.
    let lab = select(&mut a, "Lab partner for Thursday?");
    keys(&mut a, &[Key::Char('d'), Key::Enter]);
    assert!(f.take().is_empty() && matches!(a.mode, Mode::File(_)));
    for c in "b14".chars() {
        a.key(Key::Char(c));
    }
    keys(&mut a, &[Key::Enter]);
    assert_eq!(
        f.take(),
        vec![format!(
            r#"triage {{"action":"file","key":"Studies","item":"b14"}} ["{lab}"]"#
        )]
    );

    // a archives straight away, no question; Esc leaves the picker without a change.
    select(&mut a, "Lab partner for Thursday?");
    keys(&mut a, &[Key::Char('m'), Key::Esc]);
    assert!(f.take().is_empty() && a.mode == Mode::List);
    keys(&mut a, &[Key::Char('a')]);
    assert_eq!(f.take(), vec![format!(r#"triage {{"action":"archive"}} ["{lab}"]"#)]);
    // d's picker no longer offers "for nothing": that was ambiguous beside e.
    keys(&mut a, &[Key::Char('d')]);
    for c in "nothing".chars() {
        a.key(Key::Char(c));
    }
    keys(&mut a, &[Key::Enter]);
    assert!(f.take().is_empty(), "nothing to choose");
}

#[test]
fn needs_reply_unread_undo() {
    let (mut a, f) = app();
    let r = select(&mut a, "Lab partner for Thursday?");
    keys(&mut a, &[Key::Char('f'), Key::Char('U'), Key::Char('u')]);
    let calls = f.take();
    // f is needs reply now (round 4); keryx no longer flags.
    assert_eq!(calls[0], format!("tag {r}"));
    assert_eq!(calls[1], format!(r#"triage {{"action":"read","on":false}} ["{r}"]"#));
    assert_eq!(calls[2], "undo");
}

#[test]
fn m_marks_what_it_is_for_and_it_stays_then_d_files_it_there() {
    let (mut a, f) = app();
    with_taxonomy(&mut a);
    let lab = select(&mut a, "Lab partner for Thursday?");
    keys(&mut a, &[Key::Char('m')]);
    let s = render_to_string(&mut a, 90, 16);
    assert!(s.contains("stays in your inbox"), "{s}");
    for c in "b14".chars() {
        a.key(Key::Char(c));
    }
    keys(&mut a, &[Key::Enter]);
    assert_eq!(f.take(), vec![format!("for b14 {lab}")], "only marked: nothing moves");
    assert!(a.rows.iter().any(|x| x.r#ref == lab), "still in the inbox");
    // What keryxd records, read back as the list reloads.
    let (acc, tid) = keryx_core::model::parse_thread_ref(&lab).unwrap();
    a.db.set_for(acc, tid, Some(("item", "b14", "B14")), 0).unwrap();
    a.reload();
    assert!(
        render_to_string(&mut a, 90, 30).contains("b14"),
        "its tag shows on the row"
    );
    // d now files it for its mark, with no question.
    keys(&mut a, &[Key::Char('d')]);
    assert_eq!(
        f.take(),
        vec![format!(
            r#"triage {{"action":"file","key":"Studies","item":"b14"}} ["{lab}"]"#
        )]
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
    assert!(a.message.as_ref().unwrap().0.contains("mail service isn't running"));
}

// ---------------- Phase 6: compose and send (approving drafts is locus's Approve) ----------------

use keryx_tui::send::Purpose;

/// Type `body` where the cursor would be (after the headers' blank line).
fn type_body(text: &str, body: &str) -> String {
    text.replacen("\n\n\n", &format!("\n\n{body}\n"), 1)
}

fn editor(a: &mut App) -> (String, Purpose) {
    let r = a.editor_request.take().expect("editor requested");
    (r.text, r.purpose)
}

#[test]
fn reply_flow_confirm_and_send() {
    let (mut a, f) = app();
    let r = select(&mut a, "Lab partner for Thursday?");
    a.key(Key::Char('r'));
    let (text, purpose) = editor(&mut a);
    assert!(
        text.starts_with(
            "From: Oxford <newc7370@ox.ac.uk>\nTo: Jane Doe <jane.doe@example.com>\nCc: \nBcc: \nSubject: Re: Lab partner for Thursday?\n"
        ),
        "{text}"
    );
    assert!(text.contains(keryx_core::compose::MARKER) && text.contains("> Are you free"));
    a.editor_done(purpose, Ok(type_body(&text, "Yes — 2pm works.")));
    assert!(matches!(a.mode, Mode::ConfirmSend(_)));
    insta::assert_snapshot!("confirm_send", render_to_string(&mut a, 80, 20));
    keys(&mut a, &[Key::Char('y')]);
    let call = f.take().remove(0);
    assert!(
        call.contains("to=[\"jane.doe@example.com\"]") && call.contains(&format!("thread=Some(\"{r}\")")),
        "{call}"
    );
    assert!(
        call.contains("Yes — 2pm works.") && call.contains("> Are you free"),
        "quote kept: {call}"
    );
    assert!(a.message.as_ref().unwrap().0.contains("u cancels"));
}

#[test]
fn reply_all_when_alec_wrote_last() {
    let (mut a, _) = app();
    select(&mut a, "Re: Group project plan");
    a.key(Key::Char('R'));
    let (text, _) = editor(&mut a);
    assert!(
        text.starts_with(
            "From: Oxford <newc7370@ox.ac.uk>\nTo: Jane Doe <jane.doe@example.com>, Sam <sam@example.com>\nCc: Team <team@example.com>\n"
        ),
        "{text}"
    );
}

#[test]
fn empty_and_bad_input() {
    let (mut a, f) = app();
    select(&mut a, "Lab partner for Thursday?");
    a.key(Key::Char('r'));
    let (text, purpose) = editor(&mut a);
    a.editor_done(purpose.clone(), Ok(text.clone()));
    assert_eq!(a.message.as_ref().unwrap().0, "nothing sent: the message is empty");
    assert_eq!(a.mode, Mode::List);
    let bad = type_body(&text, "hi").replacen("To: Jane Doe <jane.doe@example.com>", "To: jane at example", 1);
    a.editor_done(purpose, Ok(bad));
    let (again, _) = editor(&mut a);
    assert!(
        again.starts_with("# keryx: \"jane at example\" isn't an email address"),
        "{again}"
    );
    assert!(f.take().is_empty());
}

#[test]
fn forward_and_compose() {
    let (mut a, _) = app();
    select(&mut a, "Grüße");
    a.key(Key::Char('F'));
    let (text, purpose) = editor(&mut a);
    assert!(
        text.starts_with("From: Oxford <newc7370@ox.ac.uk>\nTo: \nCc: \nBcc: \nSubject: Fwd: Grüße\n"),
        "{text}"
    );
    assert!(text.contains("---------- Forwarded message ----------"));
    assert_eq!(
        purpose,
        Purpose::Forward {
            account: "oxford".into()
        }
    );
    a.key(Key::Tab); // Personal
    a.key(Key::Char('+'));
    let (_, purpose) = editor(&mut a);
    assert_eq!(
        purpose,
        Purpose::Compose {
            account: "personal".into()
        }
    );
}

#[test]
fn hold_shows_and_u_cancels() {
    let (mut a, f) = app();
    let mut h = cs(Status::Holding);
    h.id = "cs-hold".into();
    h.hold_until = Some(a.now.timestamp() + 7);
    *f.hold.lock().unwrap() = Some(h);
    a.refresh_hold();
    let s = render_to_string(&mut a, 80, 6);
    assert!(s.contains("sending in 7 s · u cancel"), "{s}");
    keys(&mut a, &[Key::Char('u')]);
    assert_eq!(f.take(), vec!["cancel cs-hold"]);
}

#[test]
fn unsubscribe_asks_first_and_mailto_goes_through_confirm() {
    let (mut a, f) = app();
    a.sel = a
        .rows
        .iter()
        .position(|r| r.subject == "This week in engineering" && r.account == "personal")
        .unwrap();
    keys(&mut a, &[Key::Char('L')]);
    assert!(matches!(&a.mode, Mode::ConfirmUnsubscribe { how, .. } if how.contains("weekly.example.com")));
    keys(&mut a, &[Key::Char('y')]);
    assert!(f.take()[0].starts_with("unsubscribe personal:"));
    *f.unsub.lock().unwrap() = Some(UnsubscribeOutcome::Mailto {
        account: "oxford".into(),
        to: "leave-club@sports.example.org".into(),
        subject: "unsubscribe".into(),
        body: String::new(),
    });
    select(&mut a, "Training schedule");
    keys(&mut a, &[Key::Char('L'), Key::Char('y')]);
    assert!(matches!(&a.mode, Mode::ConfirmSend(d) if d.req.to[0].email == "leave-club@sports.example.org"));
    // A plain message has nothing to unsubscribe from.
    a.mode = Mode::List;
    select(&mut a, "Lab partner for Thursday?");
    keys(&mut a, &[Key::Char('L')]);
    assert!(a.message.as_ref().unwrap().0.contains("no unsubscribe"));
}

// ---- Phase 7: kairos ----

fn suggest(a: &App, r: &str, kind: &str, proposal: serde_json::Value) -> String {
    let (acc, t) = keryx_core::model::parse_thread_ref(r).unwrap();
    let id = format!("sg-{kind}");
    a.db.add_suggestion(&keryx_store::state::Suggestion {
        id: id.clone(),
        account: acc.into(),
        thread_id: t.into(),
        kind: kind.into(),
        proposal,
        reason: "Jane needs to know before Thursday".into(),
        author: "claude".into(),
        run_id: None,
        status: "open".into(),
        created_at: 0,
    })
    .unwrap();
    id
}

fn type_str(a: &mut App, s: &str) {
    for ch in s.chars() {
        a.key(Key::Char(ch));
    }
}

#[test]
fn t_makes_a_kairos_task_through_a_form() {
    let (mut a, f) = app();
    let r = select(&mut a, "Lab partner for Thursday?");
    keys(&mut a, &[Key::Char('t')]);
    assert!(matches!(a.mode, Mode::KairosForm(_)));
    // Due: typed, previewed exactly as it will be sent.
    keys(
        &mut a,
        &[Key::Tab, Key::Tab, Key::Right, Key::Right, Key::Right, Key::Tab],
    );
    type_str(&mut a, "45m");
    a.key(Key::Tab);
    type_str(&mut a, "fri 17:00");
    let s = render_to_string(&mut a, 80, 30);
    assert!(
        s.contains("→ Lab partner for Thursday? · Errands · admin · 45 min · due Fri 16") && s.contains("Oct 17:00"),
        "{s}"
    );
    insta::assert_snapshot!("kairos_task_form", s);
    keys(&mut a, &[Key::Enter]);
    let calls = f.take();
    assert_eq!(calls.len(), 1, "{calls:?}");
    let body: serde_json::Value = serde_json::from_str(calls[0].strip_prefix("kairos-task ").unwrap()).unwrap();
    assert_eq!(body["ref"], r.as_str());
    assert_eq!(body["due"], "2026-10-16T17:00");
    assert_eq!(body["kind"], "admin");
    assert_eq!(body["estimate_min"], 45);
    assert_eq!(body["key"], "Errands");
    assert_eq!(a.mode, Mode::List);
    assert_eq!(a.message.as_ref().unwrap().0, "kairos task: Lab partner for Thursday?");
    // Esc leaves without asking kairos anything.
    keys(&mut a, &[Key::Char('t'), Key::Esc]);
    assert!(f.take().is_empty());
}

#[test]
fn a_bad_form_stays_open_and_says_why() {
    let (mut a, f) = app();
    select(&mut a, "Lab partner for Thursday?");
    keys(&mut a, &[Key::Char('t'), Key::BackTab]);
    type_str(&mut a, "wed");
    keys(&mut a, &[Key::Enter]);
    assert!(matches!(a.mode, Mode::KairosForm(_)));
    assert!(a.message.as_ref().unwrap().0.contains("ambiguous"));
    assert!(f.take().is_empty());
}

#[test]
fn suggestions_y_opens_the_prefilled_form_n_dismisses() {
    let (mut a, f) = app();
    let r = select(&mut a, "Lab partner for Thursday?");
    let id = suggest(
        &a,
        &r,
        "task",
        serde_json::json!({"title": "Reply to Jane", "kind": "admin", "estimate_min": 15, "due": "thu 9:00"}),
    );
    a.reload();
    let s = render_to_string(&mut a, 80, 6);
    assert!(s.contains("󰄱?"), "{s}");
    keys(&mut a, &[Key::Enter]);
    let s = render_to_string(&mut a, 110, 20);
    assert!(
        s.contains("󰄱? kairos task: Reply to Jane · due Thu 15 Oct 09:00 · ~15 min · y create · n dismiss"),
        "{s}"
    );
    assert!(s.contains("Claude: Jane needs to know before Thursday"), "{s}");
    assert!(s.contains("y create · n dismiss · r reply"), "hints: {s}");
    // y opens the form only: nothing reaches kairos until Enter.
    f.take();
    keys(&mut a, &[Key::Char('y')]);
    assert!(f.take().is_empty());
    let s = render_to_string(&mut a, 80, 30);
    assert!(s.contains("Claude  Jane needs to know before Thursday"), "{s}");
    assert!(
        s.contains("→ Reply to Jane · Errands · admin · 15 min · due Thu 15 Oct 09:00"),
        "{s}"
    );
    keys(&mut a, &[Key::Esc]);
    assert_eq!(a.mode, Mode::Thread);
    assert!(f.take().is_empty(), "Esc leaves the suggestion open");
    keys(&mut a, &[Key::Char('y'), Key::Enter]);
    let calls = f.take();
    assert!(calls[0].contains(&format!(r#""suggestion":"{id}""#)), "{calls:?}");
    // n dismisses (keryxd records it; the fake just logs).
    keys(&mut a, &[Key::Char('n')]);
    assert_eq!(f.take(), vec![format!("dismiss {id}")]);
}

#[test]
fn i_turns_an_invite_into_an_event() {
    let (mut a, f) = app();
    select(&mut a, "Cancelled: Project meeting");
    keys(&mut a, &[Key::Char('i')]);
    assert_eq!(a.message.as_ref().unwrap().0, "this invite was cancelled");
    assert!(f.take().is_empty());
    // Without the cancellation, the request is offered at London time.
    a.db.conn()
        .execute("DELETE FROM messages WHERE subject LIKE 'Cancelled:%'", [])
        .unwrap();
    a.reload();
    select(&mut a, "Invitation: Project meeting");
    keys(&mut a, &[Key::Char('i')]);
    let s = render_to_string(&mut a, 80, 30);
    assert!(
        s.contains("→ Project meeting · Errands · Thu 22 Oct 14:00–15:00 · Thom Building,") && s.contains("Room 2"),
        "{s}"
    );
    keys(&mut a, &[Key::Enter]);
    let calls = f.take();
    let body: serde_json::Value = serde_json::from_str(calls[0].strip_prefix("kairos-event ").unwrap()).unwrap();
    assert_eq!(body["start"], "2026-10-22T14:00");
    assert_eq!(body["end"], "2026-10-22T15:00");
}

#[test]
fn w_puts_free_times_in_a_reply() {
    let (mut a, f) = app();
    select(&mut a, "Lab partner for Thursday?");
    keys(&mut a, &[Key::Char('w')]);
    let s = render_to_string(&mut a, 80, 30);
    assert!(
        s.contains("→ up to 4 slots of 1 h between Wed 14 Oct 16:00 and Mon 19 Oct"),
        "{s}"
    );
    keys(&mut a, &[Key::Enter]);
    let calls = f.take();
    assert!(calls[0].starts_with("kairos-free "), "{calls:?}");
    let req = a.editor_request.take().expect("editor");
    assert!(
        req.text
            .contains("\n\nTue 20 Oct 14:00–15:00\nWed 21 Oct 10:00–11:00\n"),
        "{}",
        req.text
    );
    assert!(
        req.text.starts_with("From: Oxford <newc7370@ox.ac.uk>\nTo: "),
        "{}",
        req.text
    );
    assert!(matches!(req.purpose, keryx_tui::send::Purpose::Reply { .. }));
}

#[test]
fn linked_task_line_and_row_icon() {
    let (mut a, _) = app();
    let r = select(&mut a, "Lab partner for Thursday?");
    let (acc, t) = keryx_core::model::parse_thread_ref(&r).unwrap();
    a.db.add_link(&keryx_store::state::Link {
        account: acc.into(),
        thread_id: t.into(),
        app: "kairos".into(),
        kind: "deadline".into(),
        target: "t-9".into(),
        title: "Find a lab partner".into(),
        created_at: 0,
    })
    .unwrap();
    a.task_status.insert(
        "t-9".into(),
        TaskStatus {
            id: "t-9".into(),
            title: "Find a lab partner".into(),
            status: "open".into(),
            due: Some("2026-10-16T17:00".into()),
            remaining_min: Some(120),
        },
    );
    a.reload();
    assert!(a.rows[a.sel].linked);
    keys(&mut a, &[Key::Enter]);
    let s = render_to_string(&mut a, 80, 12);
    assert!(
        s.contains("󰄱 kairos: Find a lab partner · due Fri 16 Oct 17:00 · 2 h left"),
        "{s}"
    );
    // has:task finds it.
    a.search = Some("has:task".into());
    a.reload();
    assert_eq!(a.rows.len(), 1);
}

// ---- Phase 9: the Claude runs view ----

#[test]
fn runs_view_lists_runs_and_what_they_made() {
    use keryx_store::runs::{Run, RunItem, RunKind};
    let (mut a, f) = app();
    let lab = select(&mut a, "Lab partner for Thursday?");
    let (acc, t) = keryx_core::model::parse_thread_ref(&lab).unwrap();
    let start = a.now.timestamp() - 7 * 3600;
    a.db.create_run(
        &Run {
            id: "run-1".into(),
            kind: RunKind::Morning,
            started_at: start,
            finished_at: Some(start + 120),
            status: "running".into(),
            summary: String::new(),
            error: None,
            expires_at: start + 1200,
            scope: serde_json::json!({}),
        },
        "h",
    )
    .unwrap();
    let mut applied = cs(Status::Applied);
    applied.id = "cs-run".into();
    a.db.save_changeset(&applied).unwrap();
    for (kind, id) in [("changeset", format!("cs-run:{acc}:{t}")), ("tag", lab.clone())] {
        a.db.add_run_item(&RunItem {
            run_id: "run-1".into(),
            kind: kind.into(),
            item_id: id,
            account: Some(acc.into()),
            thread_id: Some(t.into()),
            created_at: start + 10,
        })
        .unwrap();
    }
    a.db.finish_run("run-1", "done", "Jane asked about the lab; tagged.", None, start + 120)
        .unwrap();
    keys(&mut a, &[Key::Char('g')]);
    // Unlisted in go-to until the locus job replaces it (X8), still reachable with g c.
    keys(&mut a, &[Key::Char('c')]);
    let s = render_to_string(&mut a, 90, 8);
    assert!(
        s.contains("morning") && s.contains("done") && s.contains("1 cleared · 1 need reply"),
        "{s}"
    );
    keys(&mut a, &[Key::Enter]);
    let s = render_to_string(&mut a, 90, 12);
    insta::assert_snapshot!("run_detail", s);
    assert!(s.contains("Jane asked about the lab; tagged."), "{s}");
    // u undoes a cleared thread.
    keys(&mut a, &[Key::Char('u')]);
    assert_eq!(f.take(), vec!["undo cs-run".to_string()]);
    // Enter opens the item's thread; Esc comes back to the run.
    keys(&mut a, &[Key::Down, Key::Enter]);
    assert_eq!(a.mode, Mode::Thread);
    keys(&mut a, &[Key::Esc]);
    assert!(matches!(a.mode, Mode::RunDetail { .. }));
    keys(&mut a, &[Key::Esc, Key::Esc]);
    assert_eq!(a.mode, Mode::List);
}

#[test]
fn snoozing_off_hides_the_key_and_view() {
    let (mut a, f) = app();
    assert!(!a.cfg.snooze.enabled, "off by default");
    select(&mut a, "Lab partner for Thursday?");
    keys(&mut a, &[Key::Char('s')]);
    assert_eq!(a.mode, Mode::List);
    assert!(f.take().is_empty());
    keys(&mut a, &[Key::Char('g')]);
    assert!(!render_to_string(&mut a, 80, 24).contains("Snoozed"));
    keys(&mut a, &[Key::Char('s')]);
    assert_ne!(a.view, keryx_store::view::View::Snoozed);
}

#[test]
fn a_opens_locus_approve_and_keryx_has_no_approve_screen_of_its_own() {
    // One approve place (keryx-joins-locus X2): drafts and cleanups wait in locus's Approve.
    let (mut a, _f) = app();
    keys(&mut a, &[Key::Char('A')]);
    assert_eq!(a.mode, Mode::List);
    assert!(
        a.message
            .as_ref()
            .is_some_and(|(m, _)| m.to_lowercase().contains("approve")),
        "{:?}",
        a.message
    );
}

// ---------------- X6: writing in your Neovim, in the right pane ----------------

#[test]
fn the_from_line_shows_the_address_you_chose_and_sends_as_before() {
    let (mut a, _f) = app();
    let ox = a.cfg.accounts.iter_mut().find(|x| x.id == "oxford").unwrap();
    ox.shown_address = Some("you@college.ox.ac.uk".into());
    a.key(Key::Char('+'));
    let (text, purpose) = editor(&mut a);
    assert!(text.starts_with("From: Oxford <you@college.ox.ac.uk>\n"), "{text}");
    let typed = text.replacen("To: ", "To: jane.doe@example.com", 1);
    a.editor_done(purpose, Ok(type_body(&typed, "Hello")));
    match &a.mode {
        Mode::ConfirmSend(d) => assert_eq!(d.req.account, "oxford", "Mail still sends from the account"),
        m => panic!("{m:?}"),
    }
}

#[test]
fn from_switches_the_account_for_new_mail_but_never_for_a_reply() {
    let (mut a, _f) = app();
    a.key(Key::Char('+'));
    let (text, purpose) = editor(&mut a);
    let first = text.lines().next().unwrap().to_string();
    let typed = text
        .replacen(&first, "From: Personal <alec.mitchellthomson@gmail.com>", 1)
        .replacen("To: ", "To: jane.doe@example.com", 1);
    a.editor_done(purpose, Ok(type_body(&typed, "Hello")));
    match &a.mode {
        Mode::ConfirmSend(d) => {
            assert_eq!(d.req.account, "personal");
            assert_eq!(
                d.purpose,
                Purpose::Compose {
                    account: "personal".into()
                }
            );
        }
        m => panic!("{m:?}"),
    }

    let (mut a, _f) = app();
    select(&mut a, "Lab partner for Thursday?");
    a.key(Key::Char('r'));
    let (text, purpose) = editor(&mut a);
    a.editor_done(
        purpose.clone(),
        Ok(type_body(
            &text.replacen("From: Oxford <newc7370@ox.ac.uk>", "From: nowhere", 1),
            "hi",
        )),
    );
    let (again, _) = editor(&mut a);
    assert!(
        again.starts_with("# keryx: no account \"nowhere\" to send from"),
        "{again}"
    );
    a.editor_done(
        purpose,
        Ok(type_body(
            &text.replacen("From: Oxford <newc7370@ox.ac.uk>", "From: personal", 1),
            "hi",
        )),
    );
    let (again, _) = editor(&mut a);
    assert!(
        again.contains("a reply goes from the account it came to (oxford)"),
        "{again}"
    );
}

#[test]
fn quitting_keeps_a_draft_that_r_reopens_and_sending_drops() {
    use keryx_tui::writing::Finished;
    let (mut a, f) = app();
    let dir = tempfile::tempdir().unwrap();
    a.drafts_dir = Some(dir.path().to_path_buf());
    select(&mut a, "Lab partner for Thursday?");
    a.key(Key::Char('r'));
    let (text, purpose) = editor(&mut a);
    a.writing_started(purpose.clone());
    assert!(render_to_string(&mut a, 120, 10).contains("writing a reply"));
    // What :q leaves behind (keryx.lua writes it), then the session's verdict.
    let half = type_body(&text, "Half a thought");
    std::fs::write(a.draft_path(&purpose).unwrap(), &half).unwrap();
    a.writing_finished(Finished::Kept);
    assert_eq!(a.message.as_ref().unwrap().0, "kept as a draft · r opens it again");
    assert!(a.writing.is_none());

    a.key(Key::Char('r'));
    let (text, purpose) = editor(&mut a);
    assert_eq!(text, half, "r opens the kept draft");
    assert!(a.message.as_ref().unwrap().0.contains(":cq throws it away"));
    a.writing_started(purpose.clone());
    a.writing_finished(Finished::Send(type_body(&text, "and the rest")));
    assert!(matches!(a.mode, Mode::ConfirmSend(_)));
    assert!(a.draft_path(&purpose).unwrap().exists(), "kept until it is sent");
    keys(&mut a, &[Key::Char('y')]);
    assert!(a.draft_path(&purpose).unwrap().exists(), "in its hold: the draft stays");
    // keryxd records the send as done; the next look settles it.
    let (id, _) = a.sending.clone().expect("a send in its hold");
    let mut done = cs(Status::Applied);
    done.id = id;
    a.db.save_changeset(&done).unwrap();
    a.hold = None;
    a.settle_send();
    assert!(!a.draft_path(&purpose).unwrap().exists(), "sent: the draft goes");
    assert!(f.take()[0].contains("Half a thought"));
}

#[test]
fn the_attach_picker_chooses_a_file() {
    use keryx_tui::pick::Picker;
    let (mut a, _f) = app();
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join("Documents")).unwrap();
    std::fs::write(home.path().join("Documents/form.pdf"), b"%PDF").unwrap();
    std::fs::write(home.path().join("Documents/notes.txt"), b"x").unwrap();
    a.key(Key::Char('+'));
    let (_, purpose) = editor(&mut a);
    a.writing_started(purpose);
    a.open_picker(Picker::scan(home.path()));
    let s = render_to_string(&mut a, 100, 24);
    assert!(s.contains("attach a file") && s.contains("⏎ attach"), "{s}");
    for c in "form".chars() {
        assert_eq!(a.picker_key(Key::Char(c)), None);
    }
    a.dry_open = true;
    assert_eq!(a.picker_key(Key::Ctrl('o')), None, "^o opens to check");
    assert_eq!(a.opened.last(), Some(&home.path().join("Documents/form.pdf")));
    assert!(a.writing.as_ref().unwrap().picker.is_some(), "and the picker stays");
    assert_eq!(a.picker_key(Key::Enter), Some(home.path().join("Documents/form.pdf")));
    assert!(a.writing.as_ref().unwrap().picker.is_none());
}

#[test]
fn the_editor_draws_in_the_right_pane_with_the_list_beside_it() {
    use keryx_tui::pane::Pane;
    use portable_pty::CommandBuilder;
    let (mut a, _f) = app();
    a.width = 120;
    select(&mut a, "Lab partner for Thursday?");
    a.key(Key::Char('r'));
    let (_, purpose) = editor(&mut a);
    a.writing_started(purpose);
    let size = ratatui::layout::Rect::new(0, 0, 120, 14);
    let r = keryx_tui::ui::writing_rect(&a, size);
    assert!(r.x > 30 && r.width < 90, "{r:?}");
    let mut cmd = CommandBuilder::new("/bin/sh");
    cmd.args(["-c", "printf 'To: jane\\r\\nSubject: Re: Lab'; sleep 5"]);
    let pane = Pane::spawn(cmd, r.height, r.width, "#1e1e2e").unwrap();
    let start = std::time::Instant::now();
    while !pane.with_screen(|s| s.contents()).unwrap().contains("Re: Lab") {
        assert!(start.elapsed().as_secs() < 5);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 14)).unwrap();
    term.draw(|f| keryx_tui::ui::draw_writing(f, &a, Some(&pane))).unwrap();
    let buf = term.backend().buffer();
    let rows: Vec<String> = (0..14)
        .map(|y| (0..120).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
        .collect();
    let s = rows.join("\n");
    assert!(s.contains("writing a reply"), "{s}");
    assert!(
        rows[1].contains("To: jane") && rows[2].contains("Subject: Re: Lab"),
        "{s}"
    );
    assert!(
        rows[1].find("To: jane").unwrap() >= r.x as usize,
        "in the right pane: {s}"
    );
    assert!(s.contains(":w send") && s.contains(":q keep as a draft"), "{s}");
}

#[test]
fn what_was_filed_by_itself_is_listed_and_u_puts_it_back() {
    let (mut a, f) = app();
    keys(&mut a, &[Key::Char('g'), Key::Char('l')]);
    assert!(render_to_string(&mut a, 90, 6).contains("nothing yet"));
    keys(&mut a, &[Key::Esc]);
    let ticket = a
        .rows
        .iter()
        .find(|r| r.subject.starts_with("Your e-ticket"))
        .unwrap()
        .clone();
    let m = a.db.thread(&ticket.account, &ticket.thread_id).unwrap().remove(0);
    let state = keryx_core::changeset::MsgState {
        location: m.location.clone(),
        read: m.read,
        flagged: m.flagged,
    };
    let mut c = cs(Status::Applied);
    c.id = "cs-auto".into();
    c.author = Author::System;
    c.interactive = false;
    c.summary = "filed as it arrived, for Errands (1 thread)".into();
    c.ops = vec![keryx_core::changeset::Op::State {
        msg: keryx_core::changeset::MsgRef {
            account: m.account.clone(),
            thread_id: m.thread_id.clone(),
            source_id: m.source_id.clone(),
            message_id: m.message_id.clone(),
        },
        before: state.clone(),
        after: state,
    }];
    a.db.save_changeset(&c).unwrap();
    keys(&mut a, &[Key::Char('g')]);
    assert!(render_to_string(&mut a, 80, 24).contains("Filed by itself"));
    keys(&mut a, &[Key::Char('l')]);
    let s = render_to_string(&mut a, 90, 6);
    assert!(
        s.contains("filed by itself") && s.contains("Errands") && s.contains("Your e-ticket"),
        "{s}"
    );
    keys(&mut a, &[Key::Char('u')]);
    a.run_deferred();
    assert_eq!(f.take(), vec!["undo cs-auto"]);
}

#[test]
fn kept_drafts_are_listed_under_g_d_and_enter_reopens_one() {
    let (mut a, _f) = app();
    let dir = tempfile::tempdir().unwrap();
    a.drafts_dir = Some(dir.path().to_path_buf());
    keys(&mut a, &[Key::Char('g'), Key::Char('d')]);
    assert!(render_to_string(&mut a, 90, 6).contains("no drafts"));
    keys(&mut a, &[Key::Esc]);
    let lab = select(&mut a, "Lab partner for Thursday?");
    a.key(Key::Char('r'));
    let (text, purpose) = editor(&mut a);
    let half = type_body(&text, "Half a thought");
    std::fs::write(a.draft_path(&purpose).unwrap(), &half).unwrap();
    keys(&mut a, &[Key::Char('g'), Key::Char('d')]);
    let s = render_to_string(&mut a, 100, 8);
    assert!(
        s.contains("drafts") && s.contains("reply") && s.contains("Re: Lab partner for Thursday?"),
        "{s}"
    );
    keys(&mut a, &[Key::Enter]);
    let (again, p) = editor(&mut a);
    assert_eq!(again, half, "where you left it");
    assert_eq!(
        p,
        Purpose::Reply {
            r#ref: lab,
            account: "oxford".into()
        }
    );
}

#[test]
fn go_to_has_your_lists_on_the_left_and_the_areas_on_the_right() {
    let (mut a, _f) = app();
    with_taxonomy(&mut a);
    keys(&mut a, &[Key::Char('g')]);
    insta::assert_snapshot!("goto_areas", render_to_string(&mut a, 80, 22));
    keys(&mut a, &[Key::Char('3')]);
    assert_eq!(a.view, keryx_store::view::View::Key("Careers".into()));
}

#[test]
fn a_refused_send_keeps_your_mail_and_the_confirm_screen_warns_first() {
    use keryx_tui::writing::Finished;
    let (mut a, f) = app();
    a.cfg.live = false;
    let dir = tempfile::tempdir().unwrap();
    a.drafts_dir = Some(dir.path().to_path_buf());
    a.key(Key::Char('+'));
    let (text, purpose) = editor(&mut a);
    a.writing_started(purpose.clone());
    let typed = type_body(
        &text.replacen("To: ", "To: someone@example.com", 1),
        "Words worth keeping",
    );
    a.writing_finished(Finished::Send(typed.clone()));
    let s = render_to_string(&mut a, 100, 24);
    assert!(s.contains("isn't one of your addresses"), "warned before y: {s}");
    *f.refuse_send.lock().unwrap() = Some("live = false: keryx only sends to your own addresses".into());
    keys(&mut a, &[Key::Char('y')]);
    a.run_deferred();
    let msg = a.message.clone().unwrap().0;
    assert!(msg.contains("not sent") && msg.contains("drafts (g d)"), "{msg}");
    assert_eq!(
        std::fs::read_to_string(a.draft_path(&purpose).unwrap()).unwrap(),
        typed,
        "nothing lost"
    );
}

#[test]
fn after_the_hold_the_line_says_sent_or_why_not_and_the_draft_follows() {
    let (mut a, _f) = app();
    let dir = tempfile::tempdir().unwrap();
    let draft = dir.path().join("new-oxford.eml");
    for (status, error, said, kept) in [
        (
            Status::Failed,
            Some("Mail: can't attach the file"),
            "not sent: Mail: can't attach the file",
            true,
        ),
        (Status::Applied, None, "sent", false),
    ] {
        std::fs::write(&draft, "To: me\n\nhi\n").unwrap();
        let mut c = cs(status);
        c.id = "cs-send".into();
        c.error = error.map(str::to_string);
        a.db.save_changeset(&c).unwrap();
        a.sending = Some(("cs-send".into(), Some(draft.clone())));
        a.hold = None;
        a.settle_send();
        assert!(a.message.as_ref().unwrap().0.starts_with(said), "{:?}", a.message);
        assert_eq!(draft.exists(), kept);
        assert!(a.sending.is_none());
    }
}

#[test]
fn capital_i_puts_a_thread_back_in_the_inbox() {
    let (mut a, f) = app();
    let r = select(&mut a, "Lab partner for Thursday?");
    keys(&mut a, &[Key::Char('I')]);
    assert_eq!(f.take(), vec![format!(r#"triage {{"action":"unarchive"}} ["{r}"]"#)]);
}

#[test]
fn filing_is_gone_from_the_list_at_once_and_comes_back_if_it_fails() {
    let (mut a, _f) = app();
    let (tx, rx) = std::sync::mpsc::channel();
    a.background = Some(tx);
    let lab = select(&mut a, "Lab partner for Thursday?");
    keys(&mut a, &[Key::Char('d')]); // no taxonomy here: d archives
    a.run_deferred();
    assert!(!a.rows.iter().any(|r| r.r#ref == lab), "gone at once");
    let job = rx.try_recv().expect("handed to the worker");
    assert_eq!(job.hides(), vec![lab.clone()]);
    // Another reload while the worker is busy does not bring it back.
    a.reload();
    assert!(!a.rows.iter().any(|r| r.r#ref == lab));
    a.background_done(job.hides(), Err("archive: not done · Mail refused".into()));
    assert!(a.rows.iter().any(|r| r.r#ref == lab), "failed: it is back");
    assert!(a.message.as_ref().unwrap().0.contains("Mail refused"));
}

#[test]
fn back_to_the_inbox_shows_in_the_inbox_at_once() {
    let (mut a, _f) = app();
    let (tx, rx) = std::sync::mpsc::channel();
    a.background = Some(tx);
    keys(&mut a, &[Key::Char('g'), Key::Char('v')]); // the archive
    let row = a.rows.first().expect("an archived thread").clone();
    keys(&mut a, &[Key::Char('I')]);
    a.run_deferred();
    let job = rx.try_recv().expect("handed to the worker");
    keys(&mut a, &[Key::Char('g'), Key::Char('i')]);
    assert!(a.rows.iter().any(|r| r.r#ref == row.r#ref), "in the inbox at once");
    // Mail refused: it goes again.
    a.background_done([job.hides(), job.shows()].concat(), Err("not done".into()));
    assert!(!a.rows.iter().any(|r| r.r#ref == row.r#ref));
}

#[test]
fn u_brings_an_archived_thread_back_at_once_and_undoes_that_exact_change() {
    let (mut a, _f) = app();
    let (tx, rx) = std::sync::mpsc::channel();
    a.background = Some(tx);
    let lab = select(&mut a, "Lab partner for Thursday?");
    keys(&mut a, &[Key::Char('a')]);
    a.run_deferred();
    let archive = rx.try_recv().unwrap();
    assert!(!a.rows.iter().any(|r| r.r#ref == lab), "gone");
    // keryxd answers with the change it applied.
    a.move_applied(&archive.hides(), Some("cs-archive".into()));
    a.background_done(archive.hides(), Ok(None));
    keys(&mut a, &[Key::Char('u')]);
    a.run_deferred();
    assert!(a.rows.iter().any(|r| r.r#ref == lab), "back at once");
    match rx.try_recv().unwrap() {
        keryx_tui::write::Job::UndoMove { cs, refs, back_in } => {
            assert_eq!((cs.as_deref(), refs, back_in), (Some("cs-archive"), vec![lab], true));
        }
        j => panic!("{j:?}"),
    }
}

#[test]
fn a_thread_you_answered_last_is_marked_replied() {
    let (a, _f) = app();
    let row = a.rows.iter().find(|r| r.subject == "Re: Group project plan").unwrap();
    assert!(row.replied);
    let lab = a
        .rows
        .iter()
        .find(|r| r.subject == "Lab partner for Thursday?")
        .unwrap();
    assert!(!lab.replied);
}

#[test]
fn archiving_leaves_the_cursor_where_it_was_not_at_the_top() {
    let (mut a, _f) = app();
    let (tx, _rx) = std::sync::mpsc::channel();
    a.background = Some(tx);
    assert!(a.rows.len() > 3);
    a.sel = 2;
    let below = a.rows[3].r#ref.clone();
    keys(&mut a, &[Key::Char('a')]);
    assert_eq!(a.sel, 2, "the cursor stays put");
    assert_eq!(a.rows[a.sel].r#ref, below, "on the thread that moved up into its place");
    // The last row: the cursor stays at the end, on the one above.
    a.sel = a.rows.len() - 1;
    keys(&mut a, &[Key::Char('a')]);
    assert_eq!(a.sel, a.rows.len() - 1);
}

#[test]
fn shift_up_and_down_go_to_the_top_and_the_bottom() {
    let (mut a, _f) = app();
    keys(&mut a, &[Key::Bottom]);
    assert_eq!(a.sel, a.rows.len() - 1);
    keys(&mut a, &[Key::Top]);
    assert_eq!(a.sel, 0);
    // In a thread: to its end, and no further, so ↑ moves at once.
    keys(&mut a, &[Key::Enter]);
    let _ = render_to_string(&mut a, 40, 6);
    let open = a.open.as_ref().unwrap();
    let end = open.max_scroll.get();
    assert!(end > 0, "a thread longer than the screen");
    keys(&mut a, &[Key::Bottom]);
    assert_eq!(a.open.as_ref().unwrap().scroll, end);
    keys(&mut a, &[Key::Down, Key::Down]);
    let _ = render_to_string(&mut a, 40, 6);
    assert_eq!(a.open.as_ref().unwrap().scroll, end);
    keys(&mut a, &[Key::Up]);
    assert_eq!(a.open.as_ref().unwrap().scroll, end - 1);
    keys(&mut a, &[Key::Top]);
    assert_eq!(a.open.as_ref().unwrap().scroll, 0);
}

#[test]
fn x_twice_throws_a_kept_draft_away() {
    let (mut a, _f) = app();
    let dir = tempfile::tempdir().unwrap();
    a.drafts_dir = Some(dir.path().to_path_buf());
    let kept = dir.path().join("new-oxford.eml");
    std::fs::write(&kept, "To: someone@example.com\nSubject: Half a thought\n\nbody\n").unwrap();
    keys(&mut a, &[Key::Char('g'), Key::Char('d'), Key::Char('x')]);
    assert!(kept.exists(), "the first x only asks");
    assert!(a.message.as_ref().unwrap().0.contains("x again"));
    // Anything else in between, and x asks again.
    keys(&mut a, &[Key::Down, Key::Char('x')]);
    assert!(kept.exists());
    keys(&mut a, &[Key::Char('x')]);
    assert!(!kept.exists());
    assert!(a.message.as_ref().unwrap().0.contains("threw away"));
    assert!(render_to_string(&mut a, 90, 6).contains("no drafts"));
}

#[test]
fn a_thread_mail_puts_back_in_the_inbox_is_said_not_silent() {
    let (mut a, _f) = app();
    let (tx, rx) = std::sync::mpsc::channel();
    a.background = Some(tx);
    let lab = select(&mut a, "Lab partner for Thursday?");
    keys(&mut a, &[Key::Char('a')]);
    let job = rx.try_recv().unwrap();
    assert!(!a.rows.iter().any(|r| r.r#ref == lab));
    // keryxd said done, and the fake mirror still has it in the inbox: Mail put it back.
    a.background_done(job.hides(), Ok(None));
    assert!(a.rows.iter().any(|r| r.r#ref == lab));
    let (said, err) = a.message.clone().unwrap();
    assert!(
        err && said.contains("back in the inbox") && said.contains("Lab partner"),
        "{said}"
    );
    // Said once.
    a.message = None;
    a.reload();
    assert!(a.message.is_none());
    // Back because something newer came is not Mail's doing.
    let row_date = a.rows.iter().find(|r| r.r#ref == lab).unwrap().date;
    a.gone.insert(lab, row_date - 1);
    a.reload();
    assert!(a.message.is_none());
}
