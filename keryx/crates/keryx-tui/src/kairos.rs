//! Phase 7 keys: `t` task, `i` invite → event, `w` reply with free times,
//! `y` / `n` on Claude's suggestions. Only saving a form writes to kairos
//! (as Alec, through keryxd); `y` just opens the form, prefilled.

use crate::app::{App, Key, Mode};
use crate::form::{Built, Form, FormAction, Purpose};
use crate::write::Job;
use keryx_core::invite::Method;
use keryx_core::model::parse_thread_ref;
use keryx_core::reply::bare_subject;
use keryx_store::state::Suggestion;
use serde_json::Value;

impl App {
    /// The open suggestion on a thread, if any (newest first).
    pub fn suggestion_for(&self, account: &str, thread_id: &str) -> Option<Suggestion> {
        self.db
            .suggestions(Some("open"), Some((account, thread_id)))
            .ok()?
            .into_iter()
            .next()
    }

    fn after_form(&mut self) {
        self.mode = if self.open.is_some() { Mode::Thread } else { Mode::List };
    }

    /// `t` / `i` / `w` / `y` / `n`. Returns whether `k` was one of them.
    pub fn kairos_key(&mut self, k: Key) -> bool {
        if !matches!(k, Key::Char('t' | 'i' | 'w' | 'y' | 'n')) {
            return false;
        }
        let Some((r, msgs)) = self.compose_thread() else {
            return true;
        };
        let Some((account, thread_id)) = parse_thread_ref(&r).map(|(a, t)| (a.to_string(), t.to_string())) else {
            return true;
        };
        let calendar = keryx_store::view::kairos_key(&self.db, &self.cfg, &account, &msgs);
        let subject = msgs
            .first()
            .map(|m| bare_subject(&m.subject).to_string())
            .unwrap_or_default();
        match k {
            Key::Char('t') => {
                self.mode = Mode::KairosForm(Box::new(Form::task(&self.cfg, &r, &subject, &calendar, None)));
            }
            Key::Char('i') => {
                let invites: Vec<_> = msgs.iter().filter_map(|m| self.invite_of(m)).collect();
                // A later CANCEL for the same event wins.
                let cancelled: Vec<String> = invites
                    .iter()
                    .filter(|i| i.method == Method::Cancel || i.cancelled)
                    .map(|i| i.uid.clone())
                    .collect();
                let found = !invites.is_empty();
                let inv = invites
                    .into_iter()
                    .rev()
                    .find(|i| i.method == Method::Request && !cancelled.contains(&i.uid));
                let Some(inv) = inv else {
                    let why = if found {
                        "this invite was cancelled"
                    } else {
                        "no invite in this thread"
                    };
                    self.message = Some((why.into(), false));
                    return true;
                };
                let tz = self.cfg.tz();
                let start = crate::form::invite_time(&inv.start, tz);
                let end = inv
                    .end
                    .as_ref()
                    .map(|e| crate::form::invite_time(e, tz))
                    .unwrap_or_default();
                self.mode = Mode::KairosForm(Box::new(Form::event(
                    &self.cfg,
                    &r,
                    &inv.summary,
                    &calendar,
                    &start,
                    &end,
                    inv.location.as_deref().unwrap_or(""),
                    None,
                )));
            }
            Key::Char('w') => {
                self.mode = Mode::KairosForm(Box::new(Form::free(&r, self.now)));
            }
            Key::Char('y') | Key::Char('n') => {
                let Some(s) = self.suggestion_for(&account, &thread_id) else {
                    return false;
                };
                if k == Key::Char('n') {
                    self.deferred.push(Job::Dismiss(s.id));
                    self.message = Some(("suggestion dismissed".into(), false));
                    return true;
                }
                self.mode = Mode::KairosForm(Box::new(self.suggestion_form(&r, &calendar, &s)));
            }
            _ => return false,
        }
        true
    }

    /// Claude's proposal as a prefilled form (its reason shown above).
    fn suggestion_form(&self, r: &str, calendar: &str, s: &Suggestion) -> Form {
        let p = &s.proposal;
        let get = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let sid = Some((s.id.clone(), s.reason.clone()));
        if s.kind == "event" {
            return Form::event(
                &self.cfg,
                r,
                &get("title"),
                calendar,
                &get("start"),
                &get("end"),
                &get("location"),
                sid,
            );
        }
        let mut f = Form::task(&self.cfg, r, &get("title"), calendar, sid);
        let kind = get("kind");
        if crate::form::TASK_TYPES.contains(&kind.as_str()) {
            f.fields[2].value = kind;
        }
        if let Some(m) = p.get("estimate_min").and_then(Value::as_u64) {
            f.fields[3].value = format!("{m}m");
        }
        f.fields[4].value = get("due");
        f
    }

    pub fn form_key(&mut self, k: Key, mut form: Form) {
        match form.key(k, self.now) {
            FormAction::Continue => self.mode = Mode::KairosForm(Box::new(form)),
            FormAction::Cancel => self.after_form(),
            FormAction::Submit => match form.build(&self.cfg, self.now) {
                Err(e) => {
                    self.message = Some((e, true));
                    self.mode = Mode::KairosForm(Box::new(form));
                }
                Ok((built, _)) => {
                    self.deferred.push(match built {
                        Built::Task(t) => Job::KairosTask(t),
                        Built::Event(e) => Job::KairosEvent(e),
                        Built::Free(q) => match &form.purpose {
                            Purpose::Free { r#ref } => Job::Free(r#ref.clone(), q),
                            _ => return,
                        },
                    });
                    self.message = Some(("asking kairos…".into(), false));
                    self.after_form();
                }
            },
        }
    }

    /// Free slots came back: reply in `$EDITOR` with them in the body.
    pub fn free_reply(&mut self, r: &str, text: &str) {
        let Some((a, t)) = parse_thread_ref(r) else { return };
        let Ok(msgs) = self.db.thread(a, t) else { return };
        match keryx_core::reply::reply(&self.cfg, &msgs, false) {
            Ok(d) => {
                let sig = self.cfg.account(&d.account).and_then(|x| x.signature.clone());
                self.editor_request = Some(crate::send::EditorRequest {
                    text: keryx_core::compose::file_with_body(&d.to, &d.cc, &d.subject, text, sig.as_deref(), &d.quote),
                    purpose: crate::send::Purpose::Reply {
                        r#ref: r.to_string(),
                        account: d.account,
                    },
                });
            }
            Err(e) => self.message = Some((e.to_string(), true)),
        }
    }

    /// Ask keryxd for the live state of the open thread's linked tasks.
    pub fn queue_task_status(&mut self) {
        let Some(o) = &self.open else { return };
        let ids: Vec<String> = self
            .db
            .links(&o.account, &o.thread_id)
            .unwrap_or_default()
            .into_iter()
            .filter(|l| l.kind != "event" && !self.task_status.contains_key(&l.target))
            .map(|l| l.target)
            .collect();
        if !ids.is_empty() {
            self.deferred.push(Job::TaskStatus(ids));
        }
    }
}
