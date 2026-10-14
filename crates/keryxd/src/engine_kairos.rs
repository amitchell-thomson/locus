//! The kairos link (SPEC §9): tasks from mail, events from invites, free
//! time for replies, and Claude's suggestions. **Only Alec writes to kairos
//! through keryx**: Claude is refused here (it suggests; Alec saves the
//! prefilled form), and keryx always tells kairos who is really asking.

use crate::engine::{Engine, EngineError, Result, Who};
use keryx_core::changeset::Author;
use keryx_core::freetime;
use keryx_core::model::{Message, keryx_url, thread_ref};
use keryx_core::reply::bare_subject;
use keryx_core::risk::TEST_PREFIX;
use keryx_core::text::sanitize;
use keryx_store::api::{
    FreeOut, FreeReq, KairosEventReq, KairosOutcome, KairosTaskReq, SuggestEventReq, SuggestTaskReq, TaskStatus,
};
use keryx_store::kairos::{As, Kairos, KairosError};
use keryx_store::state::{Link, Suggestion};
use keryx_store::view;
use serde_json::{Value, json};

/// One line of email-derived text, safe to store and show.
fn one_line(s: &str, max: usize) -> String {
    let s = sanitize(s).split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() > max {
        format!("{}…", s.chars().take(max - 1).collect::<String>())
    } else {
        s
    }
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string)
}

impl Engine {
    fn kairos_as(&self, who: Who) -> Result<As> {
        if who.author == Author::Claude {
            return Err(EngineError::Forbidden(
                "Claude doesn't write to kairos through keryx: use suggest_task / suggest_event, and Alec saves them"
                    .into(),
            ));
        }
        Ok(if who.direct() { As::Alec } else { As::Script })
    }

    fn kairos_client(&self) -> Result<&dyn Kairos> {
        self.kairos
            .as_deref()
            .ok_or_else(|| EngineError::Source(format!("kairos isn't set up ({})", self.kairos_note)))
    }

    fn kairos_err(&self, e: KairosError, what: &str) -> EngineError {
        match e {
            KairosError::Unreachable { .. } => EngineError::Source(format!(
                "kairos isn't reachable at {}: {what} not created",
                self.cfg
                    .kairos
                    .url
                    .trim_start_matches("http://")
                    .trim_start_matches("https://")
            )),
            e => EngineError::Source(format!("{e}: {what} not created")),
        }
    }

    /// `live = false`: kairos writes only from `[keryx-test]` threads, as mail writes.
    fn kairos_live_gate(&self, msgs: &[Message]) -> Result<()> {
        if self.cfg.live || msgs.iter().any(|m| bare_subject(&m.subject).starts_with(TEST_PREFIX)) {
            Ok(())
        } else {
            Err(EngineError::Forbidden(format!(
                "live = false: keryx only links {TEST_PREFIX} threads to kairos"
            )))
        }
    }

    /// The sender + subject + link that go in kairos's notes.
    fn kairos_notes(&self, account: &str, thread_id: &str, msgs: &[Message]) -> String {
        let own = self.cfg.own_addresses();
        let latest = msgs
            .iter()
            .rev()
            .find(|m| m.from.as_ref().is_some_and(|f| !own.contains(&f.key())))
            .or(msgs.last());
        let from = latest
            .and_then(|m| m.from.as_ref())
            .map(|a| one_line(&a.display(), 120))
            .unwrap_or_default();
        let subject = msgs.first().map(|m| one_line(&m.subject, 160)).unwrap_or_default();
        format!("From {from} · {subject}\n{}", keryx_url(account, thread_id))
    }

    /// An open suggestion being accepted: it must be this thread's.
    fn check_suggestion(&self, id: Option<&str>, account: &str, thread_id: &str) -> Result<()> {
        let Some(id) = id else { return Ok(()) };
        let found = self
            .db
            .suggestions(Some("open"), Some((account, thread_id)))?
            .into_iter()
            .any(|s| s.id == id);
        if found {
            Ok(())
        } else {
            Err(EngineError::NotFound(format!("open suggestion {id} on this thread")))
        }
    }

    /// `t`: create a kairos task for a thread, then store the link.
    pub fn kairos_task(&mut self, req: &KairosTaskReq, who: Who) -> Result<KairosOutcome> {
        let who_as = self.kairos_as(who)?;
        let (account, thread_id, msgs) = self.thread(&req.r#ref)?;
        let title = one_line(&req.title, 200);
        if title.is_empty() {
            return Err(EngineError::BadRequest("the task needs a title".into()));
        }
        self.kairos_live_gate(&msgs)?;
        self.check_suggestion(req.suggestion.as_deref(), &account, &thread_id)?;
        let key = req
            .key
            .clone()
            .filter(|k| !k.trim().is_empty())
            .unwrap_or_else(|| view::kairos_key(&self.db, &self.cfg, &account, &msgs));
        let mut body = json!({
            "title": title,
            "key": key,
            "estimate_min": req.estimate_min.unwrap_or(0),
            "notes": self.kairos_notes(&account, &thread_id, &msgs),
            "plan": true,
        });
        if let Some(k) = req.kind.as_deref().filter(|k| !k.trim().is_empty()) {
            body["kind"] = json!(k);
        }
        if let Some(d) = req.due.as_deref().filter(|d| !d.trim().is_empty()) {
            body["due"] = json!(d);
        }
        let resp = self
            .kairos_client()?
            .create_task(&body, who_as)
            .map_err(|e| self.kairos_err(e, "task"))?;
        let task = resp.get("task").cloned().unwrap_or(Value::Null);
        let id = s(&task, "id").ok_or_else(|| EngineError::Source("kairos didn't return a task id".into()))?;
        let link = Link {
            account: account.clone(),
            thread_id: thread_id.clone(),
            app: "kairos".into(),
            kind: if body.get("due").is_some() { "deadline" } else { "task" }.into(),
            target: id,
            title: s(&task, "title").unwrap_or(title),
            created_at: self.now(),
        };
        self.db.add_link(&link)?;
        if let Some(sid) = &req.suggestion {
            self.db.set_suggestion_status(sid, "accepted")?;
        }
        let plan = resp.get("plan").cloned().unwrap_or(Value::Null);
        let blocks = plan.get("created").and_then(Value::as_array).map_or(0, Vec::len);
        let mut notes: Vec<String> = ["shortfalls", "squeezed"]
            .iter()
            .flat_map(|k| plan.get(*k).and_then(Value::as_array).cloned().unwrap_or_default())
            .filter_map(|x| s(&x, "message"))
            .collect();
        if resp
            .get("changesets")
            .and_then(Value::as_array)
            .is_some_and(|v| v.iter().any(|c| s(c, "status").as_deref() == Some("pending")))
        {
            notes.push("part of this waits for your approval in kairos".into());
        }
        let summary = match blocks {
            0 => format!("kairos task: {}", link.title),
            1 => format!("kairos task: {} · 1 block planned", link.title),
            n => format!("kairos task: {} · {n} blocks planned", link.title),
        };
        Ok(KairosOutcome { link, summary, notes })
    }

    /// `i`: an invite (or Claude's suggested event) into kairos.
    pub fn kairos_event(&mut self, req: &KairosEventReq, who: Who) -> Result<KairosOutcome> {
        let who_as = self.kairos_as(who)?;
        let (account, thread_id, msgs) = self.thread(&req.r#ref)?;
        let title = one_line(&req.title, 200);
        if title.is_empty() || req.start.trim().is_empty() {
            return Err(EngineError::BadRequest("the event needs a title and a start".into()));
        }
        self.kairos_live_gate(&msgs)?;
        self.check_suggestion(req.suggestion.as_deref(), &account, &thread_id)?;
        let key = req
            .key
            .clone()
            .filter(|k| !k.trim().is_empty())
            .unwrap_or_else(|| view::kairos_key(&self.db, &self.cfg, &account, &msgs));
        let mut body = json!({
            "title": title,
            "start": req.start.trim(),
            "key": key,
            "description": self.kairos_notes(&account, &thread_id, &msgs),
        });
        if let Some(e) = req.end.as_deref().filter(|e| !e.trim().is_empty()) {
            body["end"] = json!(e.trim());
        }
        if let Some(l) = req.location.as_deref().filter(|l| !l.trim().is_empty()) {
            body["location"] = json!(one_line(l, 200));
        }
        let resp = self
            .kairos_client()?
            .create_event(&body, who_as)
            .map_err(|e| self.kairos_err(e, "event"))?;
        let id = s(&resp, "changeset_id")
            .ok_or_else(|| EngineError::Source("kairos didn't return a changeset id".into()))?;
        let link = Link {
            account,
            thread_id,
            app: "kairos".into(),
            kind: "event".into(),
            target: id,
            title: title.clone(),
            created_at: self.now(),
        };
        self.db.add_link(&link)?;
        if let Some(sid) = &req.suggestion {
            self.db.set_suggestion_status(sid, "accepted")?;
        }
        let mut notes = Vec::new();
        if s(&resp, "status").as_deref() == Some("pending") {
            notes.push("kairos is holding this event for your approval".into());
        }
        Ok(KairosOutcome {
            link,
            summary: format!("kairos event: {title}"),
            notes,
        })
    }

    /// `w`: free slots, spread across days, as reply text.
    pub fn kairos_free(&self, req: &FreeReq) -> Result<FreeOut> {
        if req.duration_min == 0 || req.count == 0 || req.count > 20 {
            return Err(EngineError::BadRequest(
                "duration must be over 0 min and count 1–20".into(),
            ));
        }
        let slots = self
            .kairos_client()?
            .free(&req.from, &req.to, req.duration_min)
            .map_err(|e| match e {
                KairosError::Unreachable { .. } => EngineError::Source(format!(
                    "kairos isn't reachable at {}: no free times",
                    self.cfg.kairos.url.trim_start_matches("http://")
                )),
                e => EngineError::Source(e.to_string()),
            })?;
        let tz = self.cfg.tz();
        let picked = freetime::pick(&slots, req.duration_min, req.count, tz);
        Ok(FreeOut {
            slots: picked.iter().map(|s| (s.start, s.end)).collect(),
            text: freetime::text(&picked, tz),
        })
    }

    /// A linked task's live state (`None`: kairos no longer has it).
    pub fn kairos_task_status(&self, id: &str) -> Result<Option<TaskStatus>> {
        let v = self
            .kairos_client()?
            .task(id)
            .map_err(|e| EngineError::Source(e.to_string()))?;
        Ok(v.map(|v| {
            let t = v.get("task").cloned().unwrap_or(v);
            TaskStatus {
                id: s(&t, "id").unwrap_or_else(|| id.to_string()),
                title: s(&t, "title").unwrap_or_default(),
                status: s(&t, "status").unwrap_or_else(|| "open".into()),
                due: t.get("due").and_then(|d| {
                    d.as_str()
                        .map(str::to_string)
                        .or_else(|| s(d, "at"))
                        .or_else(|| s(d, "date"))
                }),
                remaining_min: t.get("remaining_min").and_then(Value::as_u64).map(|m| m as u32),
            }
        }))
    }

    fn suggest(&mut self, r: &str, kind: &str, proposal: Value, reason: &str, who: Who) -> Result<Suggestion> {
        let (account, thread_id, _) = self.thread(r)?;
        let reason = one_line(reason, 200);
        if reason.is_empty() {
            return Err(EngineError::BadRequest(
                "say why in `reason` (one line Alec reads)".into(),
            ));
        }
        let past = self.db.suggestions(None, Some((&account, &thread_id)))?;
        if past.iter().any(|s| s.kind == kind && s.status == "dismissed") {
            return Err(EngineError::Forbidden(format!(
                "Alec dismissed a {kind} suggestion for {}; not suggesting it again",
                thread_ref(&account, &thread_id)
            )));
        }
        if let Some(s) = past.iter().find(|s| s.kind == kind && s.status == "open") {
            return Err(EngineError::BadRequest(format!(
                "there's already an open {kind} suggestion for this thread ({})",
                s.id
            )));
        }
        let sg = Suggestion {
            id: format!("sg-{}", &uuid::Uuid::new_v4().simple().to_string()[..10]),
            account,
            thread_id,
            kind: kind.into(),
            proposal,
            reason,
            author: who.author.as_str().into(),
            run_id: None,
            status: "open".into(),
            created_at: self.now(),
        };
        self.db.add_suggestion(&sg)?;
        Ok(sg)
    }

    /// Claude's idea for a task: stored for Alec, never sent to kairos.
    pub fn suggest_task(&mut self, req: &SuggestTaskReq, who: Who) -> Result<Suggestion> {
        let title = one_line(&req.title, 200);
        if title.is_empty() {
            return Err(EngineError::BadRequest("the task needs a title".into()));
        }
        let mut p = json!({ "title": title });
        if let Some(k) = &req.kind {
            p["kind"] = json!(one_line(k, 40));
        }
        if let Some(e) = req.estimate_min {
            p["estimate_min"] = json!(e);
        }
        if let Some(d) = &req.due {
            p["due"] = json!(one_line(d, 60));
        }
        self.suggest(&req.r#ref, "task", p, &req.reason, who)
    }

    /// Claude's idea for an event.
    pub fn suggest_event(&mut self, req: &SuggestEventReq, who: Who) -> Result<Suggestion> {
        let title = one_line(&req.title, 200);
        if title.is_empty() || req.start.trim().is_empty() {
            return Err(EngineError::BadRequest("the event needs a title and a start".into()));
        }
        let mut p = json!({ "title": title, "start": one_line(&req.start, 60) });
        if let Some(e) = &req.end {
            p["end"] = json!(one_line(e, 60));
        }
        if let Some(l) = &req.location {
            p["location"] = json!(one_line(l, 200));
        }
        self.suggest(&req.r#ref, "event", p, &req.reason, who)
    }

    /// `n` on a suggestion: Alec's call only.
    pub fn dismiss_suggestion(&mut self, id: &str, who: Who) -> Result<()> {
        if who.author == Author::Claude {
            return Err(EngineError::Forbidden("only Alec dismisses suggestions".into()));
        }
        let open = self.db.suggestions(Some("open"), None)?.into_iter().any(|s| s.id == id);
        if !open {
            return Err(EngineError::NotFound(format!("open suggestion {id}")));
        }
        self.db.set_suggestion_status(id, "dismissed")?;
        Ok(())
    }
}
