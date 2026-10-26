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
    fn sync(&self) -> Result<()> {
        self.log("sync".into());
        Ok(())
    }
    fn send(&self, r: &SendReq) -> Result<Changeset> {
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
        text.starts_with("To: Jane Doe <jane.doe@example.com>\nCc: \nBcc: \nSubject: Re: Lab partner for Thursday?\n"),
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
        text.starts_with("To: Jane Doe <jane.doe@example.com>, Sam <sam@example.com>\nCc: Team <team@example.com>\n"),
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
    assert_eq!(a.message.as_ref().unwrap().0, "Nothing sent");
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
    assert!(text.starts_with("To: \nCc: \nBcc: \nSubject: Fwd: Grüße\n"), "{text}");
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
        s.contains("→ Lab partner for Thursday? · Personal · admin · 45 min · due Fri 16") && s.contains("Oct 17:00"),
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
    assert_eq!(body["key"], "Personal");
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
        s.contains("→ Reply to Jane · Personal · admin · 15 min · due Thu 15 Oct 09:00"),
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
        s.contains("→ Project meeting · Personal · Thu 22 Oct 14:00–15:00 · Thom Building,") && s.contains("Room 2"),
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
    assert!(req.text.starts_with("To: "), "{}", req.text);
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
    assert!(render_to_string(&mut a, 80, 24).contains("c  Claude runs"));
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
