//! SQLite: sidecar metadata, tasks, changesets, check-ins, estimate log.
//! Rows are mostly JSON blobs with a few indexed columns, so the schema can
//! evolve without migrations for every field.

use crate::Result;
use crate::load::Sidecar;
use chrono::{DateTime, NaiveDate, Utc};
use kairos_core::changeset::{Changeset, Status};
use kairos_core::model::Meta;
use kairos_core::project::Project;
use kairos_core::task::{Task, TaskStatus};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub struct Db {
    conn: Connection,
}

const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS meta (
    source TEXT NOT NULL,
    uid    TEXT NOT NULL,
    json   TEXT NOT NULL,
    PRIMARY KEY (source, uid)
);
CREATE TABLE IF NOT EXISTS tasks (
    id         TEXT PRIMARY KEY,
    status     TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    json       TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS changesets (
    id         TEXT PRIMARY KEY,
    created_at TEXT NOT NULL,
    status     TEXT NOT NULL,
    json       TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS changesets_status ON changesets(status, created_at);
CREATE TABLE IF NOT EXISTS checkins (
    block_uid   TEXT PRIMARY KEY,
    task_id     TEXT NOT NULL,
    day         TEXT NOT NULL,
    planned_min INTEGER NOT NULL,
    outcome     TEXT,
    actual_min  INTEGER,
    answered_at TEXT
);
CREATE TABLE IF NOT EXISTS estimate_log (
    task_id      TEXT NOT NULL,
    title        TEXT NOT NULL,
    estimate_min INTEGER NOT NULL,
    actual_min   INTEGER NOT NULL,
    completed_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS proposals (
    id     TEXT PRIMARY KEY,
    start  TEXT NOT NULL,
    end    TEXT NOT NULL,
    status TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS skips (
    changeset TEXT NOT NULL,
    start     TEXT NOT NULL,
    end       TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS projects (
    id   TEXT PRIMARY KEY,
    json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS project_sessions (
    event_uid  TEXT PRIMARY KEY,
    project_id TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS kv (
    k TEXT PRIMARY KEY,
    v TEXT NOT NULL
);
"#;

/// A skipped work-block slot: `(changeset, start, end)`.
pub type SkipRow = (String, DateTime<Utc>, DateTime<Utc>);

/// Proposed project time: `(id, start, end, status)`.
pub type ProposalRow = (String, DateTime<Utc>, DateTime<Utc>, String);

/// Outcome of a planned block, from the evening check-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Done,
    Partial,
    Missed,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Done => "done",
            Outcome::Partial => "partial",
            Outcome::Missed => "missed",
        }
    }
    pub fn parse(s: &str) -> Option<Outcome> {
        match s {
            "done" => Some(Outcome::Done),
            "partial" => Some(Outcome::Partial),
            "missed" => Some(Outcome::Missed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Checkin {
    pub block_uid: String,
    pub task_id: String,
    pub day: NaiveDate,
    pub planned_min: u32,
    pub outcome: Option<Outcome>,
    pub actual_min: Option<u32>,
    pub answered_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EstimateRow {
    pub task_id: String,
    pub title: String,
    pub estimate_min: u32,
    pub actual_min: u32,
    pub completed_at: DateTime<Utc>,
}

impl Db {
    pub fn open(path: &Path) -> Result<Db> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(crate::io_err(dir))?;
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(SCHEMA)?;
        Ok(Db { conn })
    }

    pub fn open_in_memory() -> Result<Db> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Db { conn })
    }

    /// Run `f` in a transaction.
    pub fn tx<T>(&mut self, f: impl FnOnce(&Db) -> Result<T>) -> Result<T> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        match f(self) {
            Ok(v) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(v)
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    // ---------------------------------------------------------------- meta

    pub fn meta_all(&self) -> Result<Sidecar> {
        let mut st = self.conn.prepare("SELECT source, uid, json FROM meta")?;
        let rows = st.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })?;
        let mut out = Sidecar::new();
        for row in rows {
            let (s, u, j) = row?;
            out.insert((s, u), serde_json::from_str(&j)?);
        }
        Ok(out)
    }

    pub fn meta_get(&self, source: &str, uid: &str) -> Result<Option<Meta>> {
        let j: Option<String> = self
            .conn
            .query_row(
                "SELECT json FROM meta WHERE source = ?1 AND uid = ?2",
                params![source, uid],
                |r| r.get(0),
            )
            .optional()?;
        Ok(match j {
            Some(j) => Some(serde_json::from_str(&j)?),
            None => None,
        })
    }

    pub fn meta_set(&self, source: &str, uid: &str, m: &Meta) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta(source, uid, json) VALUES (?1, ?2, ?3)
             ON CONFLICT(source, uid) DO UPDATE SET json = excluded.json",
            params![source, uid, serde_json::to_string(m)?],
        )?;
        Ok(())
    }

    pub fn meta_delete(&self, source: &str, uid: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM meta WHERE source = ?1 AND uid = ?2", params![source, uid])?;
        Ok(())
    }

    // ---------------------------------------------------------------- tasks

    pub fn task_put(&self, t: &Task) -> Result<()> {
        self.conn.execute(
            "INSERT INTO tasks(id, status, updated_at, json) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET status = excluded.status, updated_at = excluded.updated_at, json = excluded.json",
            params![t.id, t.status.as_str(), t.updated_at.to_rfc3339(), serde_json::to_string(t)?],
        )?;
        Ok(())
    }

    pub fn task_get(&self, id: &str) -> Result<Option<Task>> {
        let j: Option<String> = self
            .conn
            .query_row("SELECT json FROM tasks WHERE id = ?1", params![id], |r| r.get(0))
            .optional()?;
        Ok(match j {
            Some(j) => Some(serde_json::from_str(&j)?),
            None => None,
        })
    }

    pub fn tasks(&self, status: Option<TaskStatus>) -> Result<Vec<Task>> {
        let mut st = self
            .conn
            .prepare("SELECT json FROM tasks WHERE ?1 IS NULL OR status = ?1")?;
        let rows = st.query_map(params![status.map(|s| s.as_str())], |r| r.get::<_, String>(0))?;
        let mut out: Vec<Task> = Vec::new();
        for r in rows {
            out.push(serde_json::from_str(&r?)?);
        }
        out.sort_by(|a, b| {
            let key = |t: &Task| t.due.map(|d| d.instant(chrono_tz::UTC));
            (key(a).is_none(), key(a), a.priority, &a.id).cmp(&(key(b).is_none(), key(b), b.priority, &b.id))
        });
        Ok(out)
    }

    /// Find a task by id or unique case-insensitive title prefix.
    pub fn task_find(&self, q: &str) -> Result<Vec<Task>> {
        if let Some(t) = self.task_get(q)? {
            return Ok(vec![t]);
        }
        let ql = q.to_lowercase();
        Ok(self
            .tasks(None)?
            .into_iter()
            .filter(|t| t.status == TaskStatus::Open && t.title.to_lowercase().starts_with(&ql))
            .collect())
    }

    // ---------------------------------------------------------------- changesets

    pub fn cs_put(&self, c: &Changeset) -> Result<()> {
        self.conn.execute(
            "INSERT INTO changesets(id, created_at, status, json) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET status = excluded.status, json = excluded.json",
            params![
                c.id,
                c.created_at.to_rfc3339(),
                c.status.as_str(),
                serde_json::to_string(c)?
            ],
        )?;
        Ok(())
    }

    pub fn cs_get(&self, id: &str) -> Result<Option<Changeset>> {
        let j: Option<String> = self
            .conn
            .query_row("SELECT json FROM changesets WHERE id = ?1", params![id], |r| r.get(0))
            .optional()?;
        Ok(match j {
            Some(j) => Some(serde_json::from_str(&j)?),
            None => None,
        })
    }

    /// Newest first.
    pub fn cs_list(&self, status: Option<Status>, limit: usize) -> Result<Vec<Changeset>> {
        let mut st = self
            .conn
            .prepare("SELECT json FROM changesets WHERE ?1 IS NULL OR status = ?1 ORDER BY created_at DESC LIMIT ?2")?;
        let rows = st.query_map(params![status.map(|s| s.as_str()), limit as i64], |r| {
            r.get::<_, String>(0)
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(serde_json::from_str(&r?)?);
        }
        Ok(out)
    }

    pub fn cs_since(&self, since: DateTime<Utc>) -> Result<Vec<Changeset>> {
        let mut st = self
            .conn
            .prepare("SELECT json FROM changesets WHERE created_at >= ?1 ORDER BY created_at")?;
        let rows = st.query_map(params![since.to_rfc3339()], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(serde_json::from_str(&r?)?);
        }
        Ok(out)
    }

    // ---------------------------------------------------------------- check-ins

    pub fn checkin_upsert_planned(&self, c: &Checkin) -> Result<()> {
        self.conn.execute(
            "INSERT INTO checkins(block_uid, task_id, day, planned_min) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(block_uid) DO UPDATE SET planned_min = excluded.planned_min, day = excluded.day
             WHERE checkins.outcome IS NULL",
            params![c.block_uid, c.task_id, c.day.to_string(), c.planned_min],
        )?;
        Ok(())
    }

    pub fn checkin_answer(
        &self,
        block_uid: &str,
        outcome: Outcome,
        actual_min: u32,
        at: DateTime<Utc>,
    ) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE checkins SET outcome = ?2, actual_min = ?3, answered_at = ?4 WHERE block_uid = ?1",
            params![block_uid, outcome.as_str(), actual_min, at.to_rfc3339()],
        )?;
        Ok(n > 0)
    }

    pub fn checkins(&self, unanswered_only: bool) -> Result<Vec<Checkin>> {
        let mut st = self.conn.prepare(
            "SELECT block_uid, task_id, day, planned_min, outcome, actual_min, answered_at FROM checkins
             WHERE (?1 = 0 OR outcome IS NULL) ORDER BY day, block_uid",
        )?;
        let rows = st.query_map(params![unanswered_only as i64], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<i64>>(5)?,
                r.get::<_, Option<String>>(6)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (block_uid, task_id, day, planned, outcome, actual, at) = r?;
            out.push(Checkin {
                block_uid,
                task_id,
                day: day.parse().unwrap_or_default(),
                planned_min: planned.max(0) as u32,
                outcome: outcome.as_deref().and_then(Outcome::parse),
                actual_min: actual.map(|a| a.max(0) as u32),
                answered_at: at
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|t| t.with_timezone(&Utc)),
            });
        }
        Ok(out)
    }

    // ---------------------------------------------------------------- estimates

    pub fn estimate_log(&self, row: &EstimateRow) -> Result<()> {
        self.conn.execute(
            "INSERT INTO estimate_log(task_id, title, estimate_min, actual_min, completed_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![row.task_id, row.title, row.estimate_min, row.actual_min, row.completed_at.to_rfc3339()],
        )?;
        Ok(())
    }

    /// Forget a task's estimate-log rows (it was reopened).
    pub fn estimate_unlog(&self, task_id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM estimate_log WHERE task_id = ?1", params![task_id])?;
        Ok(())
    }

    pub fn estimates(&self) -> Result<Vec<EstimateRow>> {
        let mut st = self.conn.prepare(
            "SELECT task_id, title, estimate_min, actual_min, completed_at FROM estimate_log ORDER BY completed_at",
        )?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (task_id, title, e, a, at) = r?;
            out.push(EstimateRow {
                task_id,
                title,
                estimate_min: e.max(0) as u32,
                actual_min: a.max(0) as u32,
                completed_at: DateTime::parse_from_rfc3339(&at)
                    .map(|t| t.with_timezone(&Utc))
                    .unwrap_or_default(),
            });
        }
        Ok(out)
    }

    // ---------------------------------------------------------------- proposals

    /// Status `proposed` | `rejected` | `accepted`.
    pub fn proposals(&self) -> Result<Vec<ProposalRow>> {
        let mut st = self
            .conn
            .prepare("SELECT id, start, end, status FROM proposals ORDER BY start")?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        let parse = |s: &str| {
            DateTime::parse_from_rfc3339(s)
                .map(|t| t.with_timezone(&Utc))
                .unwrap_or_default()
        };
        let mut out = Vec::new();
        for r in rows {
            let (id, a, b, s) = r?;
            out.push((id, parse(&a), parse(&b), s));
        }
        Ok(out)
    }

    /// Replace the open proposals with `spans`, keeping rejected and accepted
    /// ones (never re-proposed); forget anything that has ended.
    pub fn proposals_replace(
        &self,
        spans: &[(String, DateTime<Utc>, DateTime<Utc>)],
        now: DateTime<Utc>,
    ) -> Result<()> {
        self.conn
            .execute("DELETE FROM proposals WHERE status = 'proposed'", [])?;
        self.conn
            .execute("DELETE FROM proposals WHERE end <= ?1", params![now.to_rfc3339()])?;
        for (id, a, b) in spans {
            self.conn.execute(
                "INSERT OR IGNORE INTO proposals(id, start, end, status) VALUES (?1, ?2, ?3, 'proposed')",
                params![id, a.to_rfc3339(), b.to_rfc3339()],
            )?;
        }
        Ok(())
    }

    pub fn proposal_set_status(&self, id: &str, status: &str) -> Result<bool> {
        let n = self
            .conn
            .execute("UPDATE proposals SET status = ?2 WHERE id = ?1", params![id, status])?;
        Ok(n > 0)
    }

    // ---------------------------------------------------------------- projects

    pub fn projects(&self) -> Result<Vec<Project>> {
        let mut st = self.conn.prepare("SELECT json FROM projects")?;
        let rows = st.query_map([], |r| r.get::<_, String>(0))?;
        let mut out: Vec<Project> = Vec::new();
        for r in rows {
            out.push(serde_json::from_str(&r?)?);
        }
        out.sort_by(|a, b| (a.status, a.created_at, &a.id).cmp(&(b.status, b.created_at, &b.id)));
        Ok(out)
    }

    pub fn project_get(&self, id: &str) -> Result<Option<Project>> {
        let j: Option<String> = self
            .conn
            .query_row("SELECT json FROM projects WHERE id = ?1", params![id], |r| r.get(0))
            .optional()?;
        Ok(match j {
            Some(j) => Some(serde_json::from_str(&j)?),
            None => None,
        })
    }

    pub fn project_put(&self, p: &Project) -> Result<()> {
        self.conn.execute(
            "INSERT INTO projects(id, json) VALUES (?1, ?2)
             ON CONFLICT(id) DO UPDATE SET json = excluded.json",
            params![p.id, serde_json::to_string(p)?],
        )?;
        Ok(())
    }

    /// Delete a project (its calendar sessions stay; they just aren't linked).
    pub fn project_delete(&self, id: &str) -> Result<bool> {
        self.conn
            .execute("DELETE FROM project_sessions WHERE project_id = ?1", params![id])?;
        Ok(self.conn.execute("DELETE FROM projects WHERE id = ?1", params![id])? > 0)
    }

    /// Link an accepted session (calendar event) to a project.
    pub fn project_session_link(&self, event_uid: &str, project_id: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO project_sessions(event_uid, project_id) VALUES (?1, ?2)
             ON CONFLICT(event_uid) DO UPDATE SET project_id = excluded.project_id",
            params![event_uid, project_id],
        )?;
        Ok(())
    }

    /// `(event_uid, project_id)` for every linked session.
    pub fn project_sessions(&self) -> Result<Vec<(String, String)>> {
        let mut st = self
            .conn
            .prepare("SELECT event_uid, project_id FROM project_sessions")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    // ---------------------------------------------------------------- skips

    /// Times Alec skipped a work block: `(changeset, start, end)`.
    pub fn skips(&self) -> Result<Vec<SkipRow>> {
        let mut st = self.conn.prepare("SELECT changeset, start, end FROM skips")?;
        let rows = st.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })?;
        let parse = |s: &str| {
            DateTime::parse_from_rfc3339(s)
                .map(|t| t.with_timezone(&Utc))
                .unwrap_or_default()
        };
        let mut out = Vec::new();
        for r in rows {
            let (c, a, b) = r?;
            out.push((c, parse(&a), parse(&b)));
        }
        Ok(out)
    }

    pub fn skip_add(&self, changeset: &str, start: DateTime<Utc>, end: DateTime<Utc>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO skips(changeset, start, end) VALUES (?1, ?2, ?3)",
            params![changeset, start.to_rfc3339(), end.to_rfc3339()],
        )?;
        Ok(())
    }

    /// Forget the skips a changeset made (it was undone).
    pub fn skips_forget(&self, changeset: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM skips WHERE changeset = ?1", params![changeset])?;
        Ok(())
    }

    // ---------------------------------------------------------------- kv

    pub fn kv_get(&self, k: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT v FROM kv WHERE k = ?1", params![k], |r| r.get(0))
            .optional()?)
    }

    pub fn kv_set(&self, k: &str, v: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO kv(k, v) VALUES (?1, ?2) ON CONFLICT(k) DO UPDATE SET v = excluded.v",
            params![k, v],
        )?;
        Ok(())
    }
}
