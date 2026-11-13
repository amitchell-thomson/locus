//! What the TUI's tests share: a fake keryxd that logs every request (nothing is ever sent;
//! the engine's semantics are tested in keryxd) and an app over the fixture mirror.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, dead_code)]

use chrono::TimeZone;
use keryx_core::changeset::{Author, Changeset, Risk, Status};
use keryx_store::api::*;
use keryx_store::client::{Daemon, Result};
use keryx_tui::app::{App, Key};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct Fake {
    pub calls: Arc<Mutex<Vec<String>>>,
    pub hold: Arc<Mutex<Option<Changeset>>>,
    pub unsub: Arc<Mutex<Option<UnsubscribeOutcome>>>,
    /// Refuse sends with this error (keryxd's live guard, say).
    pub refuse_send: Arc<Mutex<Option<String>>>,
}

impl Fake {
    pub fn log(&self, s: String) {
        self.calls.lock().unwrap().push(s);
    }
    pub fn take(&self) -> Vec<String> {
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
            "send {} to={:?} cc={:?} thread={:?} body={:?} irt={:?}",
            r.account,
            r.to.iter().map(|a| a.email.clone()).collect::<Vec<_>>(),
            r.cc.iter().map(|a| a.email.clone()).collect::<Vec<_>>(),
            r.thread,
            r.body,
            r.in_reply_to
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

pub fn cs(status: Status) -> Changeset {
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

pub fn app() -> (App, Fake) {
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

pub fn keys(a: &mut App, ks: &[Key]) {
    for k in ks {
        a.key(*k);
    }
    a.run_deferred();
}

pub fn select(a: &mut App, subject: &str) -> String {
    a.sel = a
        .rows
        .iter()
        .position(|r| r.subject == subject)
        .unwrap_or_else(|| panic!("{subject}"));
    a.rows[a.sel].r#ref.clone()
}

pub fn ts(y: i32, m: u32, d: u32, h: u32) -> i64 {
    chrono_tz::Europe::London
        .with_ymd_and_hms(y, m, d, h, 0, 0)
        .unwrap()
        .timestamp()
}
