//! Scheduled Claude runs and background cleanups (SPEC §8.5), what each one
//! made, and which pushes have gone out (one per run / review).

use crate::db::{Db, DbError};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

pub const RUNS_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS runs (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  started_at INTEGER NOT NULL,
  finished_at INTEGER,
  status TEXT NOT NULL,
  summary TEXT NOT NULL DEFAULT '',
  error TEXT,
  token_hash TEXT NOT NULL,
  expires_at INTEGER NOT NULL,
  scope TEXT NOT NULL DEFAULT '{}'
);
CREATE TABLE IF NOT EXISTS run_items (
  run_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  item_id TEXT NOT NULL,
  account TEXT,
  thread_id TEXT,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (run_id, kind, item_id)
);
CREATE TABLE IF NOT EXISTS pushes (
  kind TEXT NOT NULL,
  id TEXT NOT NULL,
  at INTEGER NOT NULL,
  PRIMARY KEY (kind, id)
);
CREATE TABLE IF NOT EXISTS review_touch (
  review_id TEXT PRIMARY KEY,
  at INTEGER NOT NULL
);
";

/// The keryx MCP tools a morning / evening run may use (SPEC §8.5): read,
/// clear automated mail, tag, draft replies, suggest, tidy, record. keryxd
/// enforces the same list for run tokens; `keryx run` passes it to Claude.
pub const RUN_TOOLS: &[&str] = &[
    "search",
    "list_inbox",
    "read_thread",
    "read_threads",
    "read_attachment",
    "style_samples",
    "list_keys",
    "inbox_policy",
    "list_tags",
    "list_suggestions",
    "cleanup_feedback",
    "archive",
    "file",
    "tag_needs_reply",
    "draft_reply",
    "suggest_task",
    "suggest_event",
    "propose_cleanup",
    "record_run",
];

/// A background cleanup only reads, proposes and tags: no triage tools.
pub const CLEANUP_TOOLS: &[&str] = &[
    "search",
    "list_inbox",
    "read_thread",
    "read_threads",
    "read_attachment",
    "list_keys",
    "inbox_policy",
    "list_tags",
    "cleanup_feedback",
    "list_reviews",
    "get_review",
    "propose_cleanup",
    "tag_needs_reply",
    "record_run",
];

/// morning | evening | cleanup
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunKind {
    Morning,
    Evening,
    Cleanup,
}

impl RunKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RunKind::Morning => "morning",
            RunKind::Evening => "evening",
            RunKind::Cleanup => "cleanup",
        }
    }
    pub fn parse(s: &str) -> Option<RunKind> {
        match s {
            "morning" => Some(RunKind::Morning),
            "evening" => Some(RunKind::Evening),
            "cleanup" => Some(RunKind::Cleanup),
            _ => None,
        }
    }
    /// Cleanups only propose: no triage tools, more threads, more time.
    pub fn is_cleanup(self) -> bool {
        self == RunKind::Cleanup
    }
    pub fn tools(self) -> &'static [&'static str] {
        if self.is_cleanup() { CLEANUP_TOOLS } else { RUN_TOOLS }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub kind: RunKind,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    /// running | done | failed
    pub status: String,
    pub summary: String,
    pub error: Option<String>,
    pub expires_at: i64,
    /// Accounts and Alec's instructions (cleanups).
    pub scope: serde_json::Value,
}

/// Something a run made: changeset | draft | tag | suggestion | review.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunItem {
    pub run_id: String,
    pub kind: String,
    pub item_id: String,
    pub account: Option<String>,
    pub thread_id: Option<String>,
    pub created_at: i64,
}

fn run_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<(Run, String)> {
    Ok((
        Run {
            id: r.get(0)?,
            kind: RunKind::parse(&r.get::<_, String>(1)?).unwrap_or(RunKind::Morning),
            started_at: r.get(2)?,
            finished_at: r.get(3)?,
            status: r.get(4)?,
            summary: r.get(5)?,
            error: r.get(6)?,
            expires_at: r.get(7)?,
            scope: serde_json::Value::Null,
        },
        r.get(8)?,
    ))
}

const RUN_COLS: &str = "id, kind, started_at, finished_at, status, summary, error, expires_at, scope";

impl Db {
    pub fn create_run(&self, r: &Run, token_hash: &str) -> Result<(), DbError> {
        self.conn().execute(
            "INSERT INTO runs (id, kind, started_at, finished_at, status, summary, error, token_hash, expires_at, scope)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                r.id,
                r.kind.as_str(),
                r.started_at,
                r.finished_at,
                r.status,
                r.summary,
                r.error,
                token_hash,
                r.expires_at,
                serde_json::to_string(&r.scope)?
            ],
        )?;
        Ok(())
    }

    fn runs_where(&self, clause: &str, p: impl rusqlite::Params) -> Result<Vec<Run>, DbError> {
        let mut st = self.conn().prepare(&format!("SELECT {RUN_COLS} FROM runs {clause}"))?;
        let rows = st.query_map(p, run_row)?;
        let mut out = Vec::new();
        for row in rows {
            let (mut r, scope) = row?;
            r.scope = serde_json::from_str(&scope).unwrap_or(serde_json::Value::Null);
            out.push(r);
        }
        Ok(out)
    }

    pub fn run(&self, id: &str) -> Result<Option<Run>, DbError> {
        Ok(self.runs_where("WHERE id = ?1", [id])?.into_iter().next())
    }

    /// Newest first.
    pub fn runs(&self, limit: u32) -> Result<Vec<Run>, DbError> {
        self.runs_where("ORDER BY started_at DESC, rowid DESC LIMIT ?1", [limit])
    }

    /// The run a token belongs to (only while it runs and hasn't expired).
    pub fn run_for_token(&self, token_hash: &str, now: i64) -> Result<Option<Run>, DbError> {
        Ok(self
            .runs_where(
                "WHERE token_hash = ?1 AND status = 'running' AND expires_at > ?2",
                params![token_hash, now],
            )?
            .into_iter()
            .next())
    }

    /// A run still going (not finished, not past its time).
    pub fn active_run(&self, now: i64) -> Result<Option<Run>, DbError> {
        Ok(self
            .runs_where(
                "WHERE status = 'running' AND expires_at > ?1 ORDER BY started_at DESC",
                [now],
            )?
            .into_iter()
            .next())
    }

    pub fn finish_run(
        &self,
        id: &str,
        status: &str,
        summary: &str,
        error: Option<&str>,
        now: i64,
    ) -> Result<bool, DbError> {
        Ok(self.conn().execute(
            "UPDATE runs SET status = ?2, summary = ?3, error = ?4, finished_at = ?5 WHERE id = ?1 AND status = 'running'",
            params![id, status, summary, error, now],
        )? > 0)
    }

    /// Runs past their time that never finished.
    pub fn expired_runs(&self, now: i64) -> Result<Vec<Run>, DbError> {
        self.runs_where("WHERE status = 'running' AND expires_at <= ?1", [now])
    }

    pub fn add_run_item(&self, it: &RunItem) -> Result<(), DbError> {
        self.conn().execute(
            "INSERT OR IGNORE INTO run_items (run_id, kind, item_id, account, thread_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![it.run_id, it.kind, it.item_id, it.account, it.thread_id, it.created_at],
        )?;
        Ok(())
    }

    pub fn run_items(&self, run_id: &str) -> Result<Vec<RunItem>, DbError> {
        let mut st = self.conn().prepare(
            "SELECT run_id, kind, item_id, account, thread_id, created_at FROM run_items
             WHERE run_id = ?1 ORDER BY created_at, rowid",
        )?;
        let rows = st.query_map([run_id], |r| {
            Ok(RunItem {
                run_id: r.get(0)?,
                kind: r.get(1)?,
                item_id: r.get(2)?,
                account: r.get(3)?,
                thread_id: r.get(4)?,
                created_at: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Distinct threads a run has acted on (for its thread limit).
    pub fn run_thread_count(&self, run_id: &str) -> Result<u32, DbError> {
        Ok(self.conn().query_row(
            "SELECT count(*) FROM (SELECT DISTINCT account, thread_id FROM run_items
             WHERE run_id = ?1 AND thread_id IS NOT NULL)",
            [run_id],
            |r| r.get(0),
        )?)
    }

    /// Record a push once; false if it was already sent.
    pub fn mark_pushed(&self, kind: &str, id: &str, now: i64) -> Result<bool, DbError> {
        Ok(self.conn().execute(
            "INSERT OR IGNORE INTO pushes (kind, id, at) VALUES (?1, ?2, ?3)",
            params![kind, id, now],
        )? > 0)
    }

    pub fn pushed(&self, kind: &str, id: &str) -> Result<bool, DbError> {
        Ok(self
            .conn()
            .query_row("SELECT 1 FROM pushes WHERE kind = ?1 AND id = ?2", [kind, id], |_| {
                Ok(())
            })
            .optional()?
            .is_some())
    }

    pub fn touch_review(&self, id: &str, now: i64) -> Result<(), DbError> {
        self.conn().execute(
            "INSERT INTO review_touch (review_id, at) VALUES (?1, ?2)
             ON CONFLICT(review_id) DO UPDATE SET at = excluded.at",
            params![id, now],
        )?;
        Ok(())
    }

    /// Reviews whose proposals stopped arriving before `quiet_since` and
    /// haven't been pushed yet.
    pub fn quiet_reviews(&self, quiet_since: i64) -> Result<Vec<String>, DbError> {
        let mut st = self.conn().prepare(
            "SELECT t.review_id FROM review_touch t JOIN reviews r ON r.id = t.review_id
             WHERE t.at <= ?1 AND r.status = 'open'
               AND NOT EXISTS (SELECT 1 FROM pushes p WHERE p.kind = 'review' AND p.id = t.review_id)",
        )?;
        let rows = st.query_map([quiet_since], |r| r.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

/// A run as the Runs view and the run page show it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunView {
    pub run: Run,
    pub cleared: usize,
    pub tagged: usize,
    pub drafts: Vec<String>,
    pub suggestions: usize,
    pub proposals: usize,
    pub reviews: Vec<String>,
    pub items: Vec<RunItem>,
}

impl RunView {
    /// "23 cleared · 4 need reply · 2 drafts to approve · 1 task suggestion".
    pub fn counts(&self) -> String {
        let n = |k: usize, one: &str, many: &str| format!("{k} {}", if k == 1 { one } else { many });
        let mut v = Vec::new();
        if self.cleared > 0 {
            v.push(format!("{} cleared", self.cleared));
        }
        if self.tagged > 0 {
            v.push(format!("{} need reply", self.tagged));
        }
        if !self.drafts.is_empty() {
            v.push(n(self.drafts.len(), "draft to approve", "drafts to approve"));
        }
        if self.suggestions > 0 {
            v.push(n(self.suggestions, "task suggestion", "task suggestions"));
        }
        if self.proposals > 0 {
            v.push(n(self.proposals, "proposal to review", "proposals to review"));
        }
        if v.is_empty() {
            "nothing to do".into()
        } else {
            v.join(" · ")
        }
    }
}

impl Db {
    /// A run with what it made, counted.
    pub fn run_view(&self, id: &str) -> Result<Option<RunView>, DbError> {
        let Some(run) = self.run(id)? else { return Ok(None) };
        let items = self.run_items(id)?;
        let count = |k: &str| items.iter().filter(|i| i.kind == k).count();
        let ids = |k: &str| {
            items
                .iter()
                .filter(|i| i.kind == k)
                .map(|i| i.item_id.clone())
                .collect::<Vec<_>>()
        };
        Ok(Some(RunView {
            cleared: count("changeset"),
            tagged: count("tag"),
            drafts: ids("draft"),
            suggestions: count("suggestion"),
            proposals: count("proposal"),
            reviews: ids("review"),
            items,
            run,
        }))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn run(id: &str, status: &str, expires: i64) -> Run {
        Run {
            id: id.into(),
            kind: RunKind::Morning,
            started_at: 100,
            finished_at: None,
            status: status.into(),
            summary: String::new(),
            error: None,
            expires_at: expires,
            scope: serde_json::json!({}),
        }
    }

    #[test]
    fn tokens_work_only_while_running() {
        let db = Db::open_in_memory().unwrap();
        db.init_state().unwrap();
        db.create_run(&run("r1", "running", 1000), "h1").unwrap();
        assert_eq!(db.run_for_token("h1", 500).unwrap().unwrap().id, "r1");
        assert!(db.run_for_token("h1", 1000).unwrap().is_none(), "expired");
        assert!(db.run_for_token("h2", 500).unwrap().is_none());
        assert!(db.active_run(500).unwrap().is_some());
        assert!(db.finish_run("r1", "done", "ok", None, 600).unwrap());
        assert!(!db.finish_run("r1", "done", "again", None, 601).unwrap(), "once");
        assert!(db.run_for_token("h1", 700).unwrap().is_none(), "finished");
        assert!(db.active_run(700).unwrap().is_none());
    }

    #[test]
    fn items_pushes_and_quiet_reviews() {
        let db = Db::open_in_memory().unwrap();
        db.init_state().unwrap();
        for (i, t) in [("a", "1"), ("b", "1"), ("c", "2")] {
            db.add_run_item(&RunItem {
                run_id: "r".into(),
                kind: "tag".into(),
                item_id: i.into(),
                account: Some("oxford".into()),
                thread_id: Some(t.into()),
                created_at: 1,
            })
            .unwrap();
        }
        assert_eq!(db.run_thread_count("r").unwrap(), 2);
        assert_eq!(db.run_items("r").unwrap().len(), 3);
        assert!(db.mark_pushed("run", "r", 5).unwrap());
        assert!(!db.mark_pushed("run", "r", 6).unwrap());
        assert!(db.pushed("run", "r").unwrap());
    }
}
