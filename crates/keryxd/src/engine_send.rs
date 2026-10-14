//! Sending (SPEC §6.3–6.4): sends are changesets with one `Send` op. Alec's
//! own go straight to **holding** (10 s, cancellable); everyone else's are
//! **pending** until Alec approves, then hold. After the hold, preconditions
//! are checked again and the source sends. Sent mail can't be undone.

use crate::engine::{Engine, EngineError, Result, Who};
use keryx_core::changeset::{Changeset, Op, OutFile, Outgoing, Risk, Status, UnsubscribeVia};
use keryx_core::model::{Address, parse_thread_ref};
use keryx_core::outgoing;
use keryx_core::risk::{self, Assessment, Facts, Kind, ThreadFacts};
use keryx_core::unsubscribe::{self, Unsubscribe};
use keryx_store::api::{DraftEmailReq, DraftReplyReq, SendReq, UnsubscribeOutcome};
use keryx_store::view;

fn mime_for(name: &str) -> &'static str {
    match name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).as_deref() {
        Some("pdf") => "application/pdf",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("txt" | "md") => "text/plain",
        Some("csv") => "text/csv",
        Some("ics") => "text/calendar",
        Some("docx") => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        Some("xlsx") => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        Some("pptx") => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        Some("zip") => "application/zip",
        _ => "application/octet-stream",
    }
}

pub fn sha256_file(path: &str) -> std::result::Result<(u64, String), String> {
    use sha2::Digest;
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    Ok((bytes.len() as u64, hex::encode(sha2::Sha256::digest(&bytes))))
}

fn new_id() -> String {
    format!("cs-{}", &uuid::Uuid::new_v4().simple().to_string()[..10])
}

impl Engine {
    fn send_facts(&self, who: Who, o: &Outgoing) -> Result<Facts> {
        let acc = self
            .cfg
            .account(&o.account)
            .ok_or_else(|| EngineError::NotFound(format!("account {}", o.account)))?;
        let own = self.cfg.own_addresses();
        Ok(Facts {
            author: who.author,
            interactive: who.interactive,
            kind: Kind::Send,
            live: self.cfg.live,
            batch_threshold: self.cfg.risk.batch_threshold,
            threads: vec![ThreadFacts {
                account: acc.id.clone(),
                account_writable: acc.writable,
                account_shared: acc.shared,
                automated: false,
                flagged: false,
                test_message: true,
            }],
            recipients_own: o.recipients().iter().all(|r| own.contains(r)),
        })
    }

    fn warnings_for(&self, o: &Outgoing, drafted_at: i64) -> Result<Vec<String>> {
        let sent_to = view::sent_to(&self.db, &self.cfg)?;
        let thread = match &o.thread_id {
            Some(t) => self.db.thread(&o.account, t)?,
            None => vec![],
        };
        Ok(outgoing::warnings(
            o,
            &sent_to,
            &self.cfg.own_addresses(),
            &thread,
            drafted_at,
        ))
    }

    /// Queue a send: holding for Alec, pending for everyone else.
    pub fn submit(&mut self, o: Outgoing, who: Who, summary: String) -> Result<Changeset> {
        if o.recipients().is_empty() {
            return Err(EngineError::BadRequest("no recipients".into()));
        }
        let facts = self.send_facts(who, &o)?;
        let now = (self.clock)();
        let warnings = self.warnings_for(&o, now)?;
        let (status, risk, reasons, hold_until) = match risk::assess(&facts) {
            Assessment::Forbidden(why) => return Err(EngineError::Forbidden(why)),
            Assessment::Low => (
                Status::Holding,
                Risk::Low,
                vec![],
                Some(now + i64::from(self.cfg.send.hold_seconds)),
            ),
            Assessment::High(r) => (Status::Pending, Risk::High, r, None),
        };
        let cs = Changeset {
            id: new_id(),
            author: who.author,
            interactive: who.interactive,
            created_at: now,
            ops: vec![Op::Send(o)],
            risk,
            reasons,
            status,
            summary,
            review_id: None,
            undoes: None,
            error: None,
            hold_until,
            warnings,
        };
        self.db.save_changeset(&cs)?;
        if cs.status == Status::Pending {
            if self.current_run.is_some() {
                let thread = cs
                    .outgoing()
                    .and_then(|o| o.thread_id.clone().map(|t| (o.account.clone(), t)));
                self.note_run(
                    "draft",
                    &cs.id,
                    thread.as_ref().map(|(a, _)| a.as_str()),
                    thread.as_ref().map(|(_, t)| t.as_str()),
                )?;
            } else {
                self.push_draft(&cs);
            }
        }
        Ok(cs)
    }

    /// Build an `Outgoing` from a request (threading headers from the thread).
    pub fn outgoing(&self, r: &SendReq, allow_attachments: bool) -> Result<Outgoing> {
        let acc = self
            .cfg
            .account(&r.account)
            .ok_or_else(|| EngineError::NotFound(format!("account {}", r.account)))?;
        if !allow_attachments && !r.attachments.is_empty() {
            return Err(EngineError::Forbidden("only Alec can attach files".into()));
        }
        let from = Address::new(None, &acc.address);
        let (thread_id, in_reply_to, references) = match &r.thread {
            Some(t) => {
                let (a, tid) = parse_thread_ref(t).ok_or_else(|| EngineError::BadRequest(format!("{t}: bad ref")))?;
                if a != acc.id {
                    // Replying from another account: thread it anyway.
                }
                let msgs = self.db.thread(a, tid)?;
                let last = msgs.last().ok_or_else(|| EngineError::NotFound(t.clone()))?;
                let mut refs = last.references.clone();
                if !last.message_id.is_empty() && !refs.contains(&last.message_id) {
                    refs.push(last.message_id.clone());
                }
                let irt = (!last.message_id.is_empty()).then(|| last.message_id.clone());
                (if a == acc.id { Some(tid.to_string()) } else { None }, irt, refs)
            }
            None => (None, None, vec![]),
        };
        let mut attachments = Vec::new();
        for p in &r.attachments {
            let (size, sha256) = sha256_file(p).map_err(EngineError::BadRequest)?;
            let name = std::path::Path::new(p)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "attachment".into());
            attachments.push(OutFile {
                path: p.clone(),
                mime: mime_for(&name).into(),
                name,
                size,
                sha256,
            });
        }
        let total: u64 = attachments.iter().map(|a| a.size).sum();
        if total > u64::from(self.cfg.send.max_attachments_mb) * 1024 * 1024 {
            return Err(EngineError::BadRequest(format!(
                "attachments total more than {} MB",
                self.cfg.send.max_attachments_mb
            )));
        }
        Ok(Outgoing {
            account: acc.id.clone(),
            thread_id,
            message_id: outgoing::new_message_id(&from),
            from,
            to: r.to.clone(),
            cc: r.cc.clone(),
            bcc: r.bcc.clone(),
            subject: r.subject.clone(),
            body: r.body.clone(),
            in_reply_to,
            references,
            attachments,
        })
    }

    /// Alec (or a script) sends a message they wrote.
    pub fn send(&mut self, r: &SendReq, who: Who) -> Result<Changeset> {
        let o = self.outgoing(r, who.direct())?;
        let summary = format!("send “{}” to {}", o.subject, recipients_line(&o));
        self.submit(o, who, summary)
    }

    /// Claude drafts a reply: recipients come from the thread, never from
    /// Claude. The account's signature is added.
    pub fn draft_reply(&mut self, r: &DraftReplyReq, who: Who) -> Result<Changeset> {
        let (a, t) =
            parse_thread_ref(&r.r#ref).ok_or_else(|| EngineError::BadRequest(format!("{}: bad ref", r.r#ref)))?;
        let msgs = self.db.thread(a, t)?;
        if msgs.is_empty() {
            return Err(EngineError::NotFound(r.r#ref.clone()));
        }
        let d = keryx_core::reply::reply(&self.cfg, &msgs, r.reply_all)
            .map_err(|e| EngineError::BadRequest(e.to_string()))?;
        let sig = self.cfg.account(&d.account).and_then(|a| a.signature.clone());
        let mut body = r.body.trim_end().to_string();
        if let Some(s) = sig.filter(|s| !s.trim().is_empty()) {
            body.push_str(&format!("\n\n{}", s.trim_end()));
        }
        body.push_str(&format!("\n\n{}", d.quote.trim_end()));
        let req = SendReq {
            account: d.account.clone(),
            thread: Some(r.r#ref.clone()),
            to: d.to,
            cc: d.cc,
            bcc: vec![],
            subject: d.subject,
            body,
            attachments: vec![],
        };
        let o = self.outgoing(&req, false)?;
        let summary = format!("reply to {}", recipients_line(&o));
        self.submit(o, who, summary)
    }

    /// Claude drafts a new message (always pending; recipients flagged).
    pub fn draft_email(&mut self, r: &DraftEmailReq, who: Who) -> Result<Changeset> {
        let sig = self.cfg.account(&r.account).and_then(|a| a.signature.clone());
        let mut body = r.body.trim_end().to_string();
        if let Some(s) = sig.filter(|s| !s.trim().is_empty()) {
            body.push_str(&format!("\n\n{}", s.trim_end()));
        }
        let req = SendReq {
            account: r.account.clone(),
            thread: None,
            to: r.to.clone(),
            cc: r.cc.clone(),
            bcc: vec![],
            subject: r.subject.clone(),
            body,
            attachments: vec![],
        };
        let o = self.outgoing(&req, false)?;
        let summary = format!("new message “{}” to {}", o.subject, recipients_line(&o));
        self.submit(o, who, summary)
    }

    fn pending_send(&self, id: &str) -> Result<Changeset> {
        let cs = self
            .db
            .changeset(id)?
            .ok_or_else(|| EngineError::NotFound(format!("changeset {id}")))?;
        if cs.outgoing().is_none() {
            return Err(EngineError::BadRequest(format!("{id} isn't a send")));
        }
        Ok(cs)
    }

    /// Alec approves a pending send: it holds, then sends.
    pub fn approve(&mut self, id: &str, who: Who) -> Result<Changeset> {
        if !who.direct() {
            return Err(EngineError::Forbidden(
                "only Alec approves sends (TUI, a terminal, or his phone)".into(),
            ));
        }
        self.approve_unchecked(id)
    }

    /// Approval from a signed phone link (the signature is the check).
    pub fn approve_unchecked(&mut self, id: &str) -> Result<Changeset> {
        let mut cs = self.pending_send(id)?;
        if cs.status != Status::Pending {
            return Err(EngineError::BadRequest(format!(
                "{id} is {}, not pending",
                cs.status.as_str()
            )));
        }
        cs.status = Status::Holding;
        cs.hold_until = Some((self.clock)() + i64::from(self.cfg.send.hold_seconds));
        self.db.save_changeset(&cs)?;
        Ok(cs)
    }

    pub fn reject(&mut self, id: &str, _who: Who) -> Result<Changeset> {
        let mut cs = self.pending_send(id)?;
        if cs.status != Status::Pending {
            return Err(EngineError::BadRequest(format!(
                "{id} is {}, not pending",
                cs.status.as_str()
            )));
        }
        cs.status = Status::Rejected;
        self.db.save_changeset(&cs)?;
        Ok(cs)
    }

    /// Stop a send during its hold (`u`, `keryx cancel`).
    pub fn cancel(&mut self, id: &str, _who: Who) -> Result<Changeset> {
        let mut cs = self.pending_send(id)?;
        if !matches!(cs.status, Status::Holding | Status::Pending) {
            return Err(EngineError::BadRequest(format!(
                "{id} is {} — too late to cancel",
                cs.status.as_str()
            )));
        }
        cs.status = Status::Cancelled;
        self.db.save_changeset(&cs)?;
        Ok(cs)
    }

    /// The newest send still holding (for `u` during the hold).
    pub fn holding(&self) -> Result<Option<Changeset>> {
        Ok(self.db.changesets(Some(Status::Holding), 1)?.into_iter().next())
    }

    /// Alec edited a draft: it's replaced by his own send.
    pub fn replace(&mut self, id: &str, r: &SendReq, who: Who) -> Result<Changeset> {
        if !who.direct() {
            return Err(EngineError::Forbidden("only Alec edits drafts".into()));
        }
        let mut old = self.pending_send(id)?;
        if old.status != Status::Pending {
            return Err(EngineError::BadRequest(format!(
                "{id} is {}, not pending",
                old.status.as_str()
            )));
        }
        let new = self.send(r, who)?;
        old.status = Status::Rejected;
        old.error = Some(format!("replaced by Alec's edited version ({})", new.id));
        self.db.save_changeset(&old)?;
        Ok(new)
    }

    /// Send everything whose hold is over. Preconditions are checked again.
    pub fn process_holds(&mut self) -> Result<usize> {
        let now = (self.clock)();
        let mut sent = 0;
        for mut cs in self.db.changesets(Some(Status::Holding), 100)? {
            if cs.hold_until.is_some_and(|t| t > now) {
                continue;
            }
            let Some(o) = cs.outgoing().cloned() else { continue };
            let check = (|| -> std::result::Result<(), String> {
                let acc = self.cfg.account(&o.account).ok_or("account removed from the config")?;
                if !acc.writable {
                    return Err(format!("{} is read-only (writable = false)", acc.name));
                }
                let own = self.cfg.own_addresses();
                if !self.cfg.live && !o.recipients().iter().all(|r| own.contains(r)) {
                    return Err("live = false: keryx only sends to your own addresses".into());
                }
                for f in &o.attachments {
                    let (_, sha) = sha256_file(&f.path)?;
                    if sha != f.sha256 {
                        return Err(format!("{} changed after you confirmed: not sent", f.name));
                    }
                }
                Ok(())
            })();
            let result = check.and_then(|_| {
                let acc = self.cfg.account(&o.account).cloned().ok_or("account removed")?;
                self.act.send(&acc, &o)
            });
            match result {
                Ok(()) => {
                    cs.status = Status::Sent;
                    sent += 1;
                    // Replying clears the needs-reply tag.
                    if let Some(t) = &o.thread_id {
                        self.db.clear_tag(&o.account, t, crate::engine::NEEDS_REPLY)?;
                    }
                }
                Err(e) => {
                    cs.status = Status::Failed;
                    cs.error = Some(e);
                }
            }
            self.db.save_changeset(&cs)?;
        }
        Ok(sent)
    }

    /// `L`: unsubscribe from the thread's list. Alec only.
    pub fn unsubscribe(&mut self, r: &str, who: Who) -> Result<UnsubscribeOutcome> {
        let (a, t) = parse_thread_ref(r).ok_or_else(|| EngineError::BadRequest(format!("{r}: bad ref")))?;
        let msgs = self.db.thread(a, t)?;
        let Some(how) = msgs.iter().rev().find_map(|m| unsubscribe::parse(&m.list_headers)) else {
            return Ok(UnsubscribeOutcome::None);
        };
        let facts = Facts {
            author: who.author,
            interactive: who.interactive,
            kind: Kind::Unsubscribe,
            live: self.cfg.live,
            batch_threshold: self.cfg.risk.batch_threshold,
            threads: vec![],
            recipients_own: false,
        };
        if let Assessment::Forbidden(why) = risk::assess(&facts) {
            return Err(EngineError::Forbidden(why));
        }
        match how {
            Unsubscribe::Link { url } => Ok(UnsubscribeOutcome::Link { url }),
            Unsubscribe::Mailto { to, subject, body } => Ok(UnsubscribeOutcome::Mailto {
                account: a.to_string(),
                to,
                subject,
                body,
            }),
            Unsubscribe::OneClick { url } => {
                if !self.cfg.live
                    && !msgs
                        .iter()
                        .all(|m| keryx_core::reply::bare_subject(&m.subject).starts_with(risk::TEST_PREFIX))
                {
                    return Err(EngineError::Forbidden("live = false: only [keryx-test] threads".into()));
                }
                let acc = self
                    .cfg
                    .account(a)
                    .cloned()
                    .ok_or_else(|| EngineError::NotFound(a.to_string()))?;
                let mut ops = vec![Op::Unsubscribe {
                    account: a.to_string(),
                    thread_id: t.to_string(),
                    via: UnsubscribeVia::OneClick { url },
                }];
                for m in msgs
                    .iter()
                    .filter(|m| keryx_core::location::in_inbox(&acc, &m.location))
                {
                    ops.push(Op::State {
                        msg: keryx_core::changeset::MsgRef {
                            account: m.account.clone(),
                            thread_id: m.thread_id.clone(),
                            source_id: m.source_id.clone(),
                            message_id: m.message_id.clone(),
                        },
                        before: keryx_core::changeset::MsgState {
                            location: m.location.clone(),
                            read: m.read,
                            flagged: m.flagged,
                        },
                        after: keryx_core::changeset::MsgState {
                            location: keryx_core::location::archived(&acc, &m.location),
                            read: m.read,
                            flagged: m.flagged,
                        },
                    });
                }
                let mut cs = Changeset {
                    id: new_id(),
                    author: who.author,
                    interactive: who.interactive,
                    created_at: (self.clock)(),
                    ops,
                    risk: Risk::Low,
                    reasons: vec![],
                    status: Status::Pending,
                    summary: "unsubscribe and archive".into(),
                    review_id: None,
                    undoes: None,
                    error: None,
                    hold_until: None,
                    warnings: vec![],
                };
                self.apply(&mut cs)?;
                Ok(UnsubscribeOutcome::Done { changeset: cs })
            }
        }
    }
}

pub fn recipients_line(o: &Outgoing) -> String {
    let all: Vec<String> = o.to.iter().chain(&o.cc).map(|a| a.short().to_string()).collect();
    match all.len() {
        0 => "(bcc only)".into(),
        1..=3 => all.join(", "),
        n => format!("{}, {} and {} more", all[0], all[1], n - 2),
    }
}
