//! Phase 4 in the TUI: triage keys, snooze and file pickers, the `A` pane
//! and the cleanup review screen. Changes go to keryxd as Alec (interactive);
//! the list re-reads the mirror afterwards. Reviews are read from the mirror
//! directly; only decisions go to keryxd.

use crate::app::{App, Key, Mode};
use chrono::{Datelike, Duration, NaiveTime, TimeZone, Weekday};
use keryx_store::api::{Decision, Intent, TriageReq, UnsubscribeOutcome};

/// Work for keryxd, run after the key is drawn (so "archiving…" shows).
#[derive(Debug, Clone, PartialEq)]
pub enum Job {
    Triage {
        req: TriageReq,
        quiet: bool,
    },
    UndoLast,
    /// `u` right after a move: undo it, showing the threads back where they were at once.
    UndoMove {
        refs: Vec<String>,
        /// The move took them out of the list (file, archive): undoing brings them back.
        back_in: bool,
        /// The move's change, once keryxd has answered: undone exactly, not just "the last".
        cs: Option<String>,
    },
    /// `m`: what the threads are for, kept in keryx; nothing moves.
    MarkFor(Vec<String>, Option<String>),
    Decide {
        proposal: String,
        decision: Decision,
    },
    AcceptGroup {
        review: String,
        group: String,
    },
    UndoReview(String),
    Tag(String),
    Untag(String),
    Sync,
    /// A send, and the draft that holds its text until keryxd takes it (kept when refused).
    Send(keryx_store::api::SendReq, Option<std::path::PathBuf>),
    Replace(String, keryx_store::api::SendReq),
    Approve(String),
    Reject(String),
    Cancel(String),
    Unsubscribe(String),
    KairosTask(keryx_store::api::KairosTaskReq),
    KairosEvent(keryx_store::api::KairosEventReq),
    /// Free slots for a reply to this thread.
    Free(String, keryx_store::api::FreeReq),
    Dismiss(String),
    TaskStatus(Vec<String>),
    Undo(String),
}

fn kairos_message(o: &keryx_store::api::KairosOutcome) -> String {
    std::iter::once(o.summary.clone())
        .chain(o.notes.iter().cloned())
        .collect::<Vec<_>>()
        .join(" · ")
}

impl Job {
    /// Changes to mail that need no answer before the screen moves on.
    pub fn is_background(&self) -> bool {
        matches!(
            self,
            Job::Triage { .. }
                | Job::MarkFor(..)
                | Job::Tag(_)
                | Job::Untag(_)
                | Job::UndoLast
                | Job::UndoMove { .. }
                | Job::Undo(_)
        )
    }

    /// The threads a change puts back in the inbox, shown there until keryxd answers.
    pub fn shows(&self) -> Vec<String> {
        match self {
            Job::Triage { req, .. } if req.intent == Intent::Unarchive => req.refs.clone(),
            Job::UndoMove {
                refs, back_in: true, ..
            } => refs.clone(),
            _ => vec![],
        }
    }

    /// The threads a change takes out of the list, shown gone until keryxd answers.
    pub fn hides(&self) -> Vec<String> {
        match self {
            Job::Triage { req, .. } if req.intent.leaves_inbox() => req.refs.clone(),
            Job::UndoMove {
                refs, back_in: false, ..
            } => refs.clone(),
            _ => vec![],
        }
    }
}

/// Run one background job: its message, or why it failed. A change keryxd recorded as failed
/// (Mail refused it) is a failure here too, with Mail's reason.
pub fn run_background(d: &dyn keryx_store::client::Daemon, job: &Job) -> Result<Option<String>, String> {
    run_job(d, job).0
}

/// As [`run_background`], with the id of the change it applied (for an exact undo).
pub fn run_job(d: &dyn keryx_store::client::Daemon, job: &Job) -> (Result<Option<String>, String>, Option<String>) {
    if let Job::Triage { req, .. } = job {
        return match d.triage(req) {
            Ok(o) => {
                let id = o.applied.as_ref().map(|c| c.id.clone());
                (triage_message(req, job, o), id)
            }
            Err(e) => (Err(e.to_string()), None),
        };
    }
    (run_one(d, job), None)
}

fn triage_message(req: &TriageReq, job: &Job, o: keryx_store::api::TriageOutcome) -> Result<Option<String>, String> {
    let quiet = matches!(job, Job::Triage { quiet: true, .. });
    match &o.applied {
        Some(cs) if cs.status == keryx_core::changeset::Status::Failed => Err(format!(
            "{}: not done · {}",
            req.intent.verb(),
            cs.error.clone().unwrap_or_default()
        )),
        None if !o.skipped.is_empty() => {
            let (_, why) = &o.skipped[0];
            let more = if o.skipped.len() > 1 {
                format!(" (and {} more)", o.skipped.len() - 1)
            } else {
                String::new()
            };
            Ok(Some(format!("{}: {why}{more}", req.intent.verb())))
        }
        _ => Ok((!quiet).then_some(o.summary)),
    }
}

fn run_one(d: &dyn keryx_store::client::Daemon, job: &Job) -> Result<Option<String>, String> {
    match job {
        Job::Triage { req, .. } => {
            let o = d.triage(req).map_err(|e| e.to_string())?;
            triage_message(req, job, o)
        }
        Job::UndoMove { cs: Some(id), .. } => d
            .undo(id)
            .map(|c| Some(format!("undone: {}", c.summary)))
            .map_err(|e| e.to_string()),
        Job::UndoLast | Job::UndoMove { cs: None, .. } => d
            .undo_last()
            .map(|c| Some(format!("undone: {}", c.summary)))
            .map_err(|e| e.to_string()),
        Job::MarkFor(refs, id) => refs
            .iter()
            .try_for_each(|r| d.mark_for(r, id.as_deref()))
            .map(|_| None)
            .map_err(|e| e.to_string()),
        Job::Undo(id) => d.undo(id).map(|c| Some(c.summary)).map_err(|e| e.to_string()),
        Job::Tag(r) => d
            .tag(r, "")
            .map(|_| Some("needs reply on".into()))
            .map_err(|e| e.to_string()),
        Job::Untag(r) => d
            .untag(r)
            .map(|_| Some("needs reply off".into()))
            .map_err(|e| e.to_string()),
        _ => Ok(None),
    }
}

fn sent_message(c: &keryx_core::changeset::Changeset) -> String {
    match c.status {
        keryx_core::changeset::Status::Holding => "sending… (u cancels during the hold)".into(),
        keryx_core::changeset::Status::Pending => "draft waiting for approval".into(),
        s => s.as_str().into(),
    }
}

/// Snooze presets: (label, unix time).
pub fn snooze_presets(app: &App) -> Vec<(String, i64)> {
    let tz = app.cfg.tz();
    let now = app.now.with_timezone(&tz);
    let morning = keryx_core::config::parse_hhmm(&app.cfg.snooze.morning)
        .unwrap_or_else(|| NaiveTime::from_hms_opt(8, 0, 0).unwrap_or_default());
    let at = |d: chrono::NaiveDate| {
        tz.from_local_datetime(&d.and_time(morning))
            .earliest()
            .map(|t| t.timestamp())
            .unwrap_or(0)
    };
    let today = now.date_naive();
    let tomorrow = today + Duration::days(1);
    let mut monday = tomorrow;
    while monday.weekday() != Weekday::Mon {
        monday += Duration::days(1);
    }
    let later = now + Duration::hours(i64::from(app.cfg.snooze.later_today_hours));
    vec![
        ("Later today".into(), later.timestamp()),
        ("Tomorrow morning".into(), at(tomorrow)),
        ("Monday morning".into(), at(monday)),
    ]
}

/// `Pick…`: natural input ("fri 17:00", "23 oct", "tomorrow 9") → time.
/// A bare date means the morning time. Must be in the future.
pub fn parse_pick(app: &App, input: &str) -> Result<i64, String> {
    parse_when(&app.cfg, app.now, input)
}

/// Natural time input → Unix seconds in the future (`keryx_core::when`).
pub fn parse_when(
    cfg: &keryx_core::config::Config,
    now: chrono::DateTime<chrono_tz::Tz>,
    input: &str,
) -> Result<i64, String> {
    keryx_core::when::parse_future(cfg, now, input)
}

pub fn when_label(app: &App, t: i64) -> String {
    app.cfg
        .tz()
        .timestamp_opt(t, 0)
        .single()
        .map(|d| d.format("%a %-d %b, %H:%M").to_string())
        .unwrap_or_default()
}

impl App {
    /// The threads a triage key acts on: marked rows, else the open thread,
    /// else the selected row.
    pub fn targets(&self) -> Vec<String> {
        if !self.marked.is_empty() {
            return self.marked.iter().cloned().collect();
        }
        if self.mode == Mode::Thread
            && let Some(o) = &self.open
        {
            return vec![keryx_core::model::thread_ref(&o.account, &o.thread_id)];
        }
        self.selected().map(|r| vec![r.r#ref.clone()]).unwrap_or_default()
    }

    /// The account's name when every thread in `refs` is on an account keryx may not archive on.
    pub(crate) fn archives_off(&self, refs: &[String]) -> Option<String> {
        let mut name = None;
        for r in refs {
            let (a, _) = keryx_core::model::parse_thread_ref(r)?;
            let acc = self.cfg.account(a)?;
            if acc.archives {
                return None;
            }
            name = Some(acc.name.clone());
        }
        name
    }

    pub(crate) fn triage_intent(&mut self, intent: Intent, doing: &str) {
        self.triage(intent, doing);
    }

    fn triage(&mut self, intent: Intent, doing: &str) {
        let refs = self.targets();
        if refs.is_empty() {
            return;
        }
        // An account keryx may not archive on (round 11: Personal): a says so at once; d still
        // goes, so keryxd records what it is for, and the thread stays where it is.
        if matches!(intent, Intent::Archive)
            && let Some(acc) = self.archives_off(&refs)
        {
            self.message = Some((format!("{}: archive it in Mail for now", acc), true));
            return;
        }
        let n = refs.len();
        let leaves = matches!(intent, Intent::Archive | Intent::File { .. } | Intent::Snooze { .. });
        // What u undoes at once: the threads this moved, and which way.
        self.last_move = match intent {
            Intent::Archive | Intent::File { .. } => Some((refs.clone(), true, None)),
            Intent::Unarchive => Some((refs.clone(), false, None)),
            _ => None,
        };
        self.deferred.push(Job::Triage {
            req: TriageReq {
                intent,
                refs,
                summary: None,
                review_id: None,
            },
            quiet: false,
        });
        self.message = Some((format!("{doing} {n} thread{}…", if n == 1 { "" } else { "s" }), false));
        self.marked.clear();
        if leaves && self.mode == Mode::Thread {
            self.open = None;
            self.mode = Mode::List;
        }
    }

    /// Triage keys in the list and thread view. Returns whether `k` was one.
    pub fn write_key(&mut self, k: Key) -> bool {
        if self.compose_key(k) || self.kairos_key(k) {
            return true;
        }
        match k {
            Key::Char('L') => self.unsubscribe_key(),
            Key::Char('u') if self.hold.as_ref().is_some_and(|(_, t)| *t > self.now.timestamp()) => {
                if let Some((id, _)) = self.hold.take() {
                    self.deferred.push(Job::Cancel(id));
                    self.message = Some(("cancelling the send…".into(), false));
                }
            }
            // Done: filed for what it is for: the mark when there is one, else the picker (X7).
            Key::Char('d') => self.done(),
            // Archive straight away, no question (rounds 10, 11: a). Any mark stays.
            Key::Char('a') => self.triage(Intent::Archive, "archiving"),
            // What it is for, staying in the inbox (round 4).
            Key::Char('m') => self.open_for_picker(true),
            Key::Char('s') if self.cfg.snooze.enabled => self.mode = Mode::Snooze(None),
            // Needs reply is the one attention marker (round 4); keryx no longer flags.
            Key::Char('f') => {
                for r in self.targets() {
                    let tagged = self.rows.iter().any(|x| x.r#ref == r && x.needs_reply.is_some());
                    self.deferred.push(if tagged { Job::Untag(r) } else { Job::Tag(r) });
                }
            }
            Key::Char('U') => self.triage(Intent::Read { on: false }, "marking unread"),
            // Back to the inbox, from the archive, a folder or anywhere (round 8: nothing could).
            Key::Char('I') => self.triage(Intent::Unarchive, "moving back to the inbox"),
            // Undo: a move you just made is put back on screen at once (round 11).
            Key::Char('u') if self.last_move.is_some() => {
                if let Some((refs, left, cs)) = self.last_move.take() {
                    self.deferred.push(Job::UndoMove {
                        refs,
                        back_in: left,
                        cs,
                    });
                    self.message = Some(("undoing…".into(), false));
                }
            }
            Key::Char('u') => {
                self.deferred.push(Job::UndoLast);
                self.message = Some(("undoing…".into(), false));
            }
            // v marks (as in visual mode): x removes elsewhere in locus (keryx-joins-locus X-e).
            Key::Char('v') if self.mode == Mode::List => {
                if let Some(r) = self.selected().map(|r| r.r#ref.clone()) {
                    if !self.marked.remove(&r) {
                        self.marked.insert(r);
                    }
                    self.move_sel_by(1);
                }
            }
            // One approve place (keryx-joins-locus X2): drafts and cleanups wait in locus's Approve.
            Key::Char('A') => self.open_locus_approve(),
            Key::Ctrl('r') => {
                self.deferred.push(Job::Sync);
                self.message = Some(("syncing…".into(), false));
            }
            _ => return false,
        }
        true
    }

    pub fn snooze_key(&mut self, k: Key, input: Option<String>) {
        let back = if self.open.is_some() { Mode::Thread } else { Mode::List };
        match (input, k) {
            (None, Key::Esc) => self.mode = back,
            (None, Key::Char(c)) if ('1'..='3').contains(&c) => {
                let p = snooze_presets(self);
                let (label, t) = p[c as usize - '1' as usize].clone();
                self.mode = back;
                self.triage(Intent::Snooze { until: t }, &format!("snoozing to {label}:"));
            }
            (None, Key::Char('4' | 'p')) => self.mode = Mode::Snooze(Some(String::new())),
            (None, _) => {}
            (Some(_), Key::Esc) => self.mode = Mode::Snooze(None),
            (Some(mut s), Key::Backspace) => {
                s.pop();
                self.mode = Mode::Snooze(Some(s));
            }
            (Some(mut s), Key::Char(c)) => {
                s.push(c);
                self.mode = Mode::Snooze(Some(s));
            }
            (Some(s), Key::Enter) => match parse_pick(self, &s) {
                Ok(t) => {
                    let label = when_label(self, t);
                    self.mode = back;
                    self.triage(Intent::Snooze { until: t }, &format!("snoozing to {label}:"));
                }
                Err(e) => self.message = Some((e, true)),
            },
            (Some(s), _) => self.mode = Mode::Snooze(Some(s)),
        }
    }

    /// `A`: locus's Approve, in the locus window.
    pub fn open_locus_approve(&mut self) {
        let shown = locus_contract::link::OpenFile::locus().write("approve").is_ok()
            && locus_contract::link::switch_to("locus");
        self.message = Some(if shown {
            ("opened approve in locus".into(), false)
        } else {
            (
                "drafts and cleanups wait in locus's Approve (Alt-1, then a)".into(),
                false,
            )
        });
    }

    pub fn run_deferred(&mut self) {
        let mut jobs = std::mem::take(&mut self.deferred);
        if jobs.is_empty() {
            return;
        }
        // Changes to mail go to the worker and the screen moves on (round 8: filing one mail
        // took ten seconds and more, Mail's own pace). The list already shows them done.
        // Any other change ends what u can put back at once: u then means "undo the last change".
        if jobs.iter().any(|j| {
            matches!(j, Job::MarkFor(..) | Job::Tag(_) | Job::Untag(_))
                || matches!(j, Job::Triage { req, quiet: false } if !matches!(req.intent, Intent::Archive | Intent::File { .. } | Intent::Unarchive))
        }) {
            self.last_move = None;
        }
        if let Some(tx) = self.background.clone() {
            let (bg, rest): (Vec<Job>, Vec<Job>) = jobs.into_iter().partition(Job::is_background);
            let mut gone = false;
            for j in bg {
                for r in j.hides() {
                    if self.archives_off(std::slice::from_ref(&r)).is_some() {
                        continue;
                    }
                    self.shown.remove(&r);
                    self.gone.insert(r.clone(), self.now.timestamp());
                    gone |= self.hidden.insert(r);
                }
                for r in j.shows() {
                    self.gone.remove(&r);
                    self.hidden.remove(&r);
                    gone |= self.shown.insert(r);
                }
                let _ = tx.send(j);
            }
            if gone {
                self.reload();
            }
            jobs = rest;
            if jobs.is_empty() {
                return;
            }
        }
        let Some(d) = self.daemon.as_ref() else {
            if jobs
                .iter()
                .any(|j| !matches!(j, Job::Triage { quiet: true, .. } | Job::TaskStatus(_)))
            {
                self.message = Some((
                    "changes are off: keryx's mail service isn't running (keryx doctor says why)".into(),
                    true,
                ));
            }
            return;
        };
        let mut last: Option<(String, bool)> = None;
        let mut mailto: Option<(String, String, String, String)> = None;
        let mut free: Option<(String, String)> = None;
        let mut statuses = Vec::new();
        let mut sending: Option<(String, Option<std::path::PathBuf>)> = None;
        for job in jobs {
            let res: Result<Option<String>, String> = match &job {
                j @ (Job::Triage { .. }
                | Job::UndoLast
                | Job::UndoMove { .. }
                | Job::MarkFor(..)
                | Job::Tag(_)
                | Job::Untag(_)
                | Job::Undo(_)) => run_background(d.as_ref(), j),
                Job::Decide { proposal, decision } => {
                    d.decide(proposal, decision).map(|_| None).map_err(|e| e.to_string())
                }
                Job::AcceptGroup { review, group } => d
                    .accept_group(review, group)
                    .map(|g| {
                        Some(format!(
                            "accepted {}{}",
                            g.accepted,
                            if g.skipped_stale > 0 {
                                format!(" · {} changed since proposed, left for you", g.skipped_stale)
                            } else {
                                String::new()
                            }
                        ))
                    })
                    .map_err(|e| e.to_string()),
                Job::UndoReview(r) => d
                    .undo_review(r)
                    .map(|u| Some(format!("undid {} change(s)", u.undone)))
                    .map_err(|e| e.to_string()),
                Job::Sync => d.sync().map(|_| Some("syncing".into())).map_err(|e| e.to_string()),
                // The draft goes only once keryxd has the send; refused, your text stays (round 8:
                // a refused send once took the only copy with it).
                Job::Send(r, draft) => d
                    .send(r)
                    .map(|c| {
                        // In its hold: the draft stays until the mail has gone (round 8, the
                        // first attachment failed at send time and the line said "sending" on).
                        if c.status == keryx_core::changeset::Status::Holding {
                            sending = Some((c.id.clone(), draft.clone()));
                        } else if let Some(p) = draft {
                            let _ = std::fs::remove_file(p);
                        }
                        Some(sent_message(&c))
                    })
                    .map_err(|e| match draft {
                        Some(p) if p.exists() => format!("not sent: {e} · your mail is kept in drafts (g d)"),
                        _ => format!("not sent: {e}"),
                    }),
                Job::Replace(id, r) => d
                    .replace(id, r)
                    .map(|c| Some(sent_message(&c)))
                    .map_err(|e| e.to_string()),
                Job::Approve(id) => d.approve(id).map(|c| Some(sent_message(&c))).map_err(|e| e.to_string()),
                Job::Reject(id) => d
                    .reject(id)
                    .map(|_| Some("draft rejected".into()))
                    .map_err(|e| e.to_string()),
                Job::Cancel(id) => d
                    .cancel(id)
                    .map(|_| Some("send cancelled: nothing sent".into()))
                    .map_err(|e| e.to_string()),
                Job::Unsubscribe(r) => match d.unsubscribe(r) {
                    Ok(UnsubscribeOutcome::Done { .. }) => Ok(Some("unsubscribed and archived".into())),
                    Ok(UnsubscribeOutcome::Mailto {
                        account,
                        to,
                        subject,
                        body,
                    }) => {
                        mailto = Some((account, to, subject, body));
                        Ok(None)
                    }
                    Ok(UnsubscribeOutcome::Link { url }) => Ok(Some(format!("open this page to unsubscribe: {url}"))),
                    Ok(UnsubscribeOutcome::None) => Ok(Some("no unsubscribe link".into())),
                    Err(e) => Err(e.to_string()),
                },
                Job::KairosTask(r) => d
                    .kairos_task(r)
                    .map(|o| Some(kairos_message(&o)))
                    .map_err(|e| e.to_string()),
                Job::KairosEvent(r) => d
                    .kairos_event(r)
                    .map(|o| Some(kairos_message(&o)))
                    .map_err(|e| e.to_string()),
                Job::Free(r, q) => match d.kairos_free(q) {
                    Ok(out) if out.slots.is_empty() => Err(format!(
                        "kairos has no free {} slots in that window",
                        crate::form::minutes_label(q.duration_min)
                    )),
                    Ok(out) => {
                        free = Some((r.clone(), out.text));
                        Ok(None)
                    }
                    Err(e) => Err(e.to_string()),
                },
                Job::Dismiss(id) => d.dismiss_suggestion(id).map(|_| None).map_err(|e| e.to_string()),
                Job::TaskStatus(ids) => {
                    for id in ids {
                        // kairos down: the line just shows without live state.
                        if let Ok(Some(s)) = d.task_status(id) {
                            statuses.push(s);
                        }
                    }
                    Ok(None)
                }
            };
            match res {
                Ok(Some(m)) => last = Some((m, false)),
                Ok(None) => {}
                Err(e) => last = Some((e, true)),
            }
        }
        self.message = last;
        for s in statuses {
            self.task_status.insert(s.id.clone(), s);
        }
        if let Some((r, text)) = free {
            self.free_reply(&r, &text);
        }
        if let Some((account, to, subject, body)) = mailto
            && let Ok(addr) = keryx_core::compose::parse_addresses(&to)
        {
            // Unsubscribing by email: Alec confirms it like any send.
            let req = keryx_store::api::SendReq {
                account: account.clone(),
                thread: None,
                to: addr,
                cc: vec![],
                bcc: vec![],
                subject,
                body: if body.is_empty() { "unsubscribe".into() } else { body },
                attachments: vec![],
            };
            self.mode = Mode::ConfirmSend(Box::new(crate::send::SendDraft {
                warnings: vec![],
                text: String::new(),
                purpose: crate::send::Purpose::Compose { account },
                req,
            }));
        }
        if sending.is_some() {
            self.sending = sending;
        }
        self.refresh_hold();
        self.reload();
    }
}
