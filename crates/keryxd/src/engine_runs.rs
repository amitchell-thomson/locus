//! Scheduled runs, background cleanups and phone pushes (SPEC §8.4–8.5).
//!
//! A run gets a token that works only for that run, only on `/mcp`, and
//! only until it finishes or expires. Everything made with it is recorded
//! against the run, and the run ends with exactly one push.

use crate::engine::{Engine, EngineError, Result, Who};
use crate::notify::Push;
use keryx_core::changeset::{Author, Changeset, Status};
use keryx_core::model::Address;
use keryx_core::text::sanitize;
use keryx_store::runs::{Run, RunItem, RunKind, RunView};
use sha2::{Digest, Sha256};

pub fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn title_case(k: RunKind) -> &'static str {
    match k {
        RunKind::Morning => "Morning",
        RunKind::Evening => "Evening",
        RunKind::Cleanup => "Cleanup",
    }
}

impl Engine {
    /// Record something the current run made (no-op outside a run).
    pub(crate) fn note_run(
        &self,
        kind: &str,
        item_id: &str,
        account: Option<&str>,
        thread_id: Option<&str>,
    ) -> Result<()> {
        if let Some(r) = &self.current_run {
            self.db.add_run_item(&RunItem {
                run_id: r.id.clone(),
                kind: kind.into(),
                item_id: item_id.into(),
                account: account.map(str::to_string),
                thread_id: thread_id.map(str::to_string),
                created_at: self.now(),
            })?;
        }
        Ok(())
    }

    fn run_limit(&self, kind: RunKind) -> u32 {
        if kind.is_cleanup() {
            2000
        } else {
            self.cfg.runs.max_threads
        }
    }

    /// A run may act on a limited number of threads; the rest wait.
    pub(crate) fn run_room(&self, refs: &[String]) -> Result<()> {
        let Some(r) = &self.current_run else { return Ok(()) };
        let limit = self.run_limit(r.kind);
        let used = self.db.run_thread_count(&r.id)?;
        let new = refs.iter().collect::<std::collections::BTreeSet<_>>().len() as u32;
        if used + new > limit {
            return Err(EngineError::Forbidden(format!(
                "this run has reached its {limit}-thread limit ({used} so far): leave the rest for the next run"
            )));
        }
        Ok(())
    }

    /// Send a push; failures are logged, never fatal.
    pub(crate) fn push(&self, p: Push) {
        if let Some(n) = &self.notifier
            && let Err(e) = n.push(&p)
        {
            tracing::warn!(error = %e, title = %p.title, "push failed");
        }
    }

    /// A pending send outside a run: one push, recipients first (§8.4).
    pub(crate) fn push_draft(&self, cs: &Changeset) {
        let (Some(o), Some(links)) = (cs.outgoing(), &self.links) else {
            return;
        };
        let first = o.to.first().or(o.cc.first()).map(Address::display).unwrap_or_default();
        let others = o.to.len() + o.cc.len() + o.bcc.len() - usize::from(!first.is_empty());
        let title = if others > 0 {
            format!("Send to {first} (+{others})?")
        } else {
            format!("Send to {first}?")
        };
        let from = self
            .cfg
            .account(&o.account)
            .map(|a| a.address.clone())
            .unwrap_or_default();
        let lines: Vec<&str> = o.body.lines().filter(|l| !l.trim().is_empty()).take(3).collect();
        let message = sanitize(&format!(
            "From {from}\nTo {} · Cc {}\n{}\n\n{}",
            o.to.len(),
            o.cc.len(),
            o.subject,
            lines.join("\n")
        ));
        self.push(Push {
            title: sanitize(&title),
            message,
            click: Some(links.url(&cs.id, "view")),
            actions: vec![
                ("Approve".into(), links.url(&cs.id, "approve")),
                ("Reject".into(), links.url(&cs.id, "reject")),
                ("View".into(), links.url(&cs.id, "view")),
            ],
        });
    }

    /// Reviews Claude stopped adding to two minutes ago: one push each.
    pub fn push_quiet_reviews(&mut self) -> Result<usize> {
        let now = self.now();
        let mut n = 0;
        for id in self.db.quiet_reviews(now - 120)? {
            if !self.db.mark_pushed("review", &id, now)? {
                continue;
            }
            let v = self.review(&id)?;
            let open = v
                .items
                .iter()
                .filter(|i| i.proposal.status == keryx_store::state::ProposalStatus::Open)
                .count();
            if open == 0 {
                continue;
            }
            self.push(Push {
                title: format!("Cleanup ready: {open} proposal{}", if open == 1 { "" } else { "s" }),
                message: format!("{} · review in keryx (A)", sanitize(&v.review.title)),
                click: None,
                actions: vec![],
            });
            n += 1;
        }
        Ok(n)
    }

    // ---------------- runs ----------------

    /// Start a run: its id and a token for this run only. Runs never overlap.
    pub fn start_run(&mut self, kind: RunKind, scope: serde_json::Value, who: Who) -> Result<(Run, String)> {
        if who.author == Author::Claude {
            return Err(EngineError::Forbidden(
                "runs are started by keryx, not by Claude".into(),
            ));
        }
        if !kind.is_cleanup() && !self.cfg.runs.enabled {
            return Err(EngineError::Forbidden(
                "scheduled runs are off ([runs] enabled = false)".into(),
            ));
        }
        let now = self.now();
        self.expire_runs()?;
        if let Some(r) = self.db.active_run(now)? {
            return Err(EngineError::Conflict(vec![format!(
                "run {} ({}) is still going; runs never overlap",
                r.id,
                r.kind.as_str()
            )]));
        }
        let minutes = if kind.is_cleanup() {
            50
        } else {
            i64::from(self.cfg.runs.timeout_minutes).min(15) + 5
        };
        let token = format!("kr_{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
        let run = Run {
            id: format!("run-{}", &uuid::Uuid::new_v4().simple().to_string()[..10]),
            kind,
            started_at: now,
            finished_at: None,
            status: "running".into(),
            summary: String::new(),
            error: None,
            expires_at: now + minutes * 60,
            scope,
        };
        self.db.create_run(&run, &token_hash(&token))?;
        Ok((run, token))
    }

    /// The run a bearer token belongs to, if it's a live run token.
    pub fn run_for_token(&self, token: &str) -> Result<Option<Run>> {
        if !token.starts_with("kr_") {
            return Ok(None);
        }
        Ok(self.db.run_for_token(&token_hash(token), self.now())?)
    }

    pub fn run_view(&self, id: &str) -> Result<RunView> {
        self.db
            .run_view(id)?
            .ok_or_else(|| EngineError::NotFound(format!("run {id}")))
    }

    fn push_run(&self, v: &RunView) {
        if !self.db.mark_pushed("run", &v.run.id, self.now()).unwrap_or(false) {
            return;
        }
        let name = title_case(v.run.kind);
        let (title, message) = match v.run.status.as_str() {
            "done" => (format!("{name}: {}", v.counts()), sanitize(&v.run.summary)),
            _ => (
                format!("{name} run failed"),
                sanitize(v.run.error.as_deref().unwrap_or("no reason given")),
            ),
        };
        let link = self.links.as_ref().map(|l| l.run_url(&v.run.id));
        self.push(Push {
            title,
            message: message.chars().take(400).collect(),
            click: link.clone(),
            actions: link.map(|l| vec![("Open".to_string(), l)]).unwrap_or_default(),
        });
    }

    /// `record_run` (run token only): finish the run and push once.
    pub fn record_run(&mut self, run_id: &str, summary: &str) -> Result<RunView> {
        let Some(cur) = &self.current_run else {
            return Err(EngineError::Forbidden(
                "record_run works only inside a scheduled run".into(),
            ));
        };
        if cur.id != run_id {
            return Err(EngineError::Forbidden(format!(
                "this token belongs to run {}, not {run_id}",
                cur.id
            )));
        }
        let summary: String = sanitize(summary).chars().take(2000).collect();
        if !self.db.finish_run(run_id, "done", &summary, None, self.now())? {
            return Err(EngineError::BadRequest(format!("run {run_id} has already finished")));
        }
        let v = self.run_view(run_id)?;
        self.push_run(&v);
        Ok(v)
    }

    /// The launcher's report: Claude exited without `record_run`, or failed.
    pub fn fail_run(&mut self, run_id: &str, error: &str, who: Who) -> Result<RunView> {
        if who.author == Author::Claude {
            return Err(EngineError::Forbidden("Claude can't end a run this way".into()));
        }
        let error: String = sanitize(error).chars().take(1000).collect();
        if self.db.finish_run(run_id, "failed", "", Some(&error), self.now())? {
            let v = self.run_view(run_id)?;
            self.push_run(&v);
        }
        self.run_view(run_id)
    }

    /// Runs past their time are failed (and pushed once).
    pub fn expire_runs(&mut self) -> Result<usize> {
        let now = self.now();
        let mut n = 0;
        for r in self.db.expired_runs(now)? {
            if self.db.finish_run(&r.id, "failed", "", Some("timed out"), now)? {
                let v = self.run_view(&r.id)?;
                self.push_run(&v);
                n += 1;
            }
        }
        Ok(n)
    }

    // ---------------- phone ----------------

    /// A signed link's action (`approve` / `reject` / `cancel`). The
    /// signature was checked by the caller; GET never gets here.
    pub fn phone_action(&mut self, id: &str, action: &str) -> Result<Changeset> {
        match action {
            "approve" => self.approve_unchecked(id),
            "reject" => self.reject(id, Who::PHONE),
            "cancel" => self.cancel(id, Who::PHONE),
            a => Err(EngineError::BadRequest(format!("unknown action {a}"))),
        }
    }

    /// Changesets a run page lists (its drafts), newest state.
    pub fn run_drafts(&self, v: &RunView) -> Result<Vec<Changeset>> {
        let mut out = Vec::new();
        for id in &v.drafts {
            if let Some(cs) = self.db.changeset(id)?
                && cs.outgoing().is_some()
            {
                out.push(cs);
            }
        }
        Ok(out)
    }
}

impl Who {
    /// Alec on his phone, through a signed link.
    pub const PHONE: Who = Who {
        author: Author::Alec,
        interactive: true,
    };
}

/// Pending/holding states the phone page can still act on.
pub fn actionable(cs: &Changeset) -> bool {
    matches!(cs.status, Status::Pending | Status::Holding)
}
