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
    Send(keryx_store::api::SendReq),
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

    fn triage(&mut self, intent: Intent, doing: &str) {
        let refs = self.targets();
        if refs.is_empty() {
            return;
        }
        let n = refs.len();
        let leaves = matches!(intent, Intent::Archive | Intent::File { .. } | Intent::Snooze { .. });
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
            Key::Char('d') => self.triage(Intent::Archive, "archiving"),
            Key::Char('s') if self.cfg.snooze.enabled => self.mode = Mode::Snooze(None),
            Key::Char('m') => {
                if self.file_keys().is_empty() {
                    self.message = Some(("no key is mapped for this account (config [[keys]])".into(), true));
                } else {
                    self.mode = Mode::File;
                }
            }
            Key::Char('f') => {
                let on = !self
                    .rows
                    .iter()
                    .filter(|r| self.targets().contains(&r.r#ref))
                    .all(|r| r.flagged);
                self.triage(Intent::Flag { on }, if on { "flagging" } else { "unflagging" });
            }
            Key::Char('U') => self.triage(Intent::Read { on: false }, "marking unread"),
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
            Key::Char('N') => {
                for r in self.targets() {
                    let tagged = self.rows.iter().any(|x| x.r#ref == r && x.needs_reply.is_some());
                    self.deferred.push(if tagged { Job::Untag(r) } else { Job::Tag(r) });
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

    /// Keys usable for `m` on the targets: mapped on every target's account.
    pub fn file_keys(&self) -> Vec<(usize, String)> {
        let accounts: Vec<String> = self
            .targets()
            .iter()
            .filter_map(|r| keryx_core::model::parse_thread_ref(r).map(|(a, _)| a.to_string()))
            .collect();
        self.cfg
            .keys
            .iter()
            .enumerate()
            .filter(|(_, k)| !accounts.is_empty() && accounts.iter().all(|a| k.folders.contains_key(a)))
            .map(|(i, k)| (i + 1, k.name.clone()))
            .collect()
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

    pub fn file_key(&mut self, k: Key) {
        let back = if self.open.is_some() { Mode::Thread } else { Mode::List };
        match k {
            Key::Char(c) if c.is_ascii_digit() && c != '0' => {
                let n = c as usize - '0' as usize;
                if let Some((_, name)) = self.file_keys().into_iter().find(|(i, _)| *i == n) {
                    self.mode = back;
                    self.triage(Intent::File { key: name.clone() }, &format!("filing into {name}:"));
                }
            }
            _ => self.mode = back,
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
        let jobs = std::mem::take(&mut self.deferred);
        if jobs.is_empty() {
            return;
        }
        let Some(d) = self.daemon.as_ref() else {
            if jobs
                .iter()
                .any(|j| !matches!(j, Job::Triage { quiet: true, .. } | Job::TaskStatus(_)))
            {
                self.message = Some(("can't change mail: keryxd isn't running (keryx doctor)".into(), true));
            }
            return;
        };
        let mut last: Option<(String, bool)> = None;
        let mut mailto: Option<(String, String, String, String)> = None;
        let mut free: Option<(String, String)> = None;
        let mut statuses = Vec::new();
        for job in jobs {
            let res: Result<Option<String>, String> = match &job {
                Job::Triage { req, quiet } => d
                    .triage(req)
                    .map(|o| (!quiet).then_some(o.summary))
                    .map_err(|e| e.to_string()),
                Job::UndoLast => d.undo_last().map(|c| Some(c.summary)).map_err(|e| e.to_string()),
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
                Job::Tag(r) => d
                    .tag(r, "tagged by Alec")
                    .map(|_| Some("tagged: needs reply".into()))
                    .map_err(|e| e.to_string()),
                Job::Untag(r) => d
                    .untag(r)
                    .map(|_| Some("needs-reply tag cleared".into()))
                    .map_err(|e| e.to_string()),
                Job::Sync => d.sync().map(|_| Some("syncing".into())).map_err(|e| e.to_string()),
                Job::Send(r) => d.send(r).map(|c| Some(sent_message(&c))).map_err(|e| e.to_string()),
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
                Job::Undo(id) => d.undo(id).map(|c| Some(c.summary)).map_err(|e| e.to_string()),
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
        self.refresh_hold();
        self.reload();
    }
}
