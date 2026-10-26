//! Scheduled runs and background cleanups (SPEC §8.5).
//!
//! A run gets a token that works only for that run, only on `/mcp`, and
//! only until it finishes or expires. Everything made with it is recorded
//! against the run. Nothing is pushed to the phone (removed 26 Oct 2026,
//! plan B.4): what a run made waits in locus's Approve.

use crate::engine::{Engine, EngineError, Result, Who};
use keryx_core::changeset::Author;
use keryx_core::text::sanitize;
use keryx_store::runs::{Run, RunItem, RunKind, RunView};
use sha2::{Digest, Sha256};

pub fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
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
        // A cleanup scoped to some accounts stays on them.
        let scope: Vec<&str> = r.scope["accounts"]
            .as_array()
            .map(|v| v.iter().filter_map(|a| a.as_str()).collect())
            .unwrap_or_default();
        if !scope.is_empty()
            && let Some(out) = refs
                .iter()
                .find(|x| keryx_core::model::parse_thread_ref(x).is_none_or(|(a, _)| !scope.contains(&a)))
        {
            return Err(EngineError::Forbidden(format!(
                "{out} is outside this run's accounts ({})",
                scope.join(", ")
            )));
        }
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

    /// `record_run` (run token only): finish the run.
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
        self.run_view(run_id)
    }

    /// The launcher's report: Claude exited without `record_run`, or failed.
    pub fn fail_run(&mut self, run_id: &str, error: &str, who: Who) -> Result<RunView> {
        if who.author == Author::Claude {
            return Err(EngineError::Forbidden("Claude can't end a run this way".into()));
        }
        let error: String = sanitize(error).chars().take(1000).collect();
        self.db.finish_run(run_id, "failed", "", Some(&error), self.now())?;
        self.run_view(run_id)
    }

    /// Runs past their time are failed.
    pub fn expire_runs(&mut self) -> Result<usize> {
        let now = self.now();
        let mut n = 0;
        for r in self.db.expired_runs(now)? {
            if self.db.finish_run(&r.id, "failed", "", Some("timed out"), now)? {
                n += 1;
            }
        }
        Ok(n)
    }
}
