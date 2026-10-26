//! keryxd's own state next to the mirror: changesets, snoozes, tags, cleanup
//! reviews and their proposals (SPEC §4). Same database, same single writer.

use crate::db::{Db, DbError};
use keryx_core::changeset::{Changeset, Status};
use keryx_core::model::Message;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

pub const STATE_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS changesets (
  id TEXT PRIMARY KEY,
  created_at INTEGER NOT NULL,
  author TEXT NOT NULL,
  status TEXT NOT NULL,
  review_id TEXT,
  json TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS changesets_status ON changesets(status, created_at);
CREATE TABLE IF NOT EXISTS snoozes (
  account TEXT NOT NULL,
  thread_id TEXT NOT NULL,
  until INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  created_by TEXT NOT NULL,
  changeset_id TEXT,
  PRIMARY KEY (account, thread_id)
);
CREATE TABLE IF NOT EXISTS tags (
  account TEXT NOT NULL,
  thread_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  reason TEXT NOT NULL,
  author TEXT NOT NULL,
  run_id TEXT,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (account, thread_id, kind)
);
CREATE TABLE IF NOT EXISTS links (
  account TEXT NOT NULL,
  thread_id TEXT NOT NULL,
  app TEXT NOT NULL,
  kind TEXT NOT NULL,
  target TEXT NOT NULL,
  title TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (account, thread_id, app, target)
);
CREATE TABLE IF NOT EXISTS suggestions (
  id TEXT PRIMARY KEY,
  account TEXT NOT NULL,
  thread_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  proposal TEXT NOT NULL,
  reason TEXT NOT NULL,
  author TEXT NOT NULL,
  run_id TEXT,
  status TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS reviews (
  id TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  author TEXT NOT NULL,
  run_id TEXT,
  created_at INTEGER NOT NULL,
  status TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS proposals (
  id TEXT PRIMARY KEY,
  review_id TEXT NOT NULL REFERENCES reviews(id),
  position INTEGER NOT NULL,
  account TEXT NOT NULL,
  thread_id TEXT NOT NULL,
  action TEXT NOT NULL,
  reason TEXT NOT NULL,
  fingerprint TEXT NOT NULL,
  status TEXT NOT NULL,
  decided_at INTEGER,
  changeset_id TEXT,
  UNIQUE (review_id, account, thread_id)
);
";

/// What a cleanup proposes for a thread (SPEC §4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    Archive,
    File {
        key: String,
    },
    Snooze {
        until: i64,
    },
    Keep {
        why: Keep,
        #[serde(default)]
        needs_reply: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Keep {
    New,
    Action,
    Soon,
}

impl Action {
    /// The group a proposal is listed under.
    pub fn group(&self) -> String {
        match self {
            Action::Archive => "Archive".into(),
            Action::File { key } => format!("File → {key}"),
            Action::Snooze { .. } => "Snooze".into(),
            Action::Keep { .. } => "Keep".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    Open,
    Accepted,
    Declined,
    Changed,
}

impl ProposalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ProposalStatus::Open => "open",
            ProposalStatus::Accepted => "accepted",
            ProposalStatus::Declined => "declined",
            ProposalStatus::Changed => "changed",
        }
    }
    fn parse(s: &str) -> ProposalStatus {
        match s {
            "accepted" => ProposalStatus::Accepted,
            "declined" => ProposalStatus::Declined,
            "changed" => ProposalStatus::Changed,
            _ => ProposalStatus::Open,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Proposal {
    pub id: String,
    pub review_id: String,
    pub position: i64,
    pub account: String,
    pub thread_id: String,
    pub action: Action,
    pub reason: String,
    pub fingerprint: String,
    pub status: ProposalStatus,
    pub decided_at: Option<i64>,
    pub changeset_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Review {
    pub id: String,
    pub title: String,
    pub author: String,
    pub run_id: Option<String>,
    pub created_at: i64,
    /// open | done
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snooze {
    pub account: String,
    pub thread_id: String,
    pub until: i64,
    pub created_at: i64,
    pub created_by: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tag {
    pub account: String,
    pub thread_id: String,
    pub kind: String,
    pub reason: String,
    pub author: String,
    pub created_at: i64,
}

/// A thread linked to something in another app (a kairos task or event).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Link {
    pub account: String,
    pub thread_id: String,
    pub app: String,
    /// task | event
    pub kind: String,
    /// The other app's id (kairos task id / changeset id for events).
    pub target: String,
    pub title: String,
    pub created_at: i64,
}

/// Claude's idea for kairos; nothing reaches kairos until Alec saves the form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Suggestion {
    pub id: String,
    pub account: String,
    pub thread_id: String,
    /// task | deadline | event
    pub kind: String,
    /// Prefill for the form (title, due, estimate_min, kind / start, end, location).
    pub proposal: serde_json::Value,
    pub reason: String,
    pub author: String,
    pub run_id: Option<String>,
    /// open | accepted | dismissed
    pub status: String,
    pub created_at: i64,
}

/// A thread's state, for stale-proposal detection: messages, where they
/// are, read/flag. A new message or any move/flag changes it.
pub fn fingerprint(msgs: &[Message]) -> String {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    for m in msgs {
        h.update(m.source_id.as_bytes());
        h.update(serde_json::to_string(&m.location).unwrap_or_default().as_bytes());
        h.update([u8::from(m.read), u8::from(m.flagged)]);
    }
    hex::encode(&h.finalize()[..12])
}

impl Db {
    pub fn init_state(&self) -> Result<(), DbError> {
        self.conn().execute_batch(STATE_SCHEMA)?;
        self.conn().execute_batch(crate::runs::RUNS_SCHEMA)?;
        Ok(())
    }

    pub fn save_changeset(&self, cs: &Changeset) -> Result<(), DbError> {
        self.conn().execute(
            "INSERT INTO changesets (id, created_at, author, status, review_id, json) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET status = excluded.status, json = excluded.json",
            params![
                cs.id,
                cs.created_at,
                cs.author.as_str(),
                cs.status.as_str(),
                cs.review_id,
                serde_json::to_string(cs)?
            ],
        )?;
        Ok(())
    }

    pub fn changeset(&self, id: &str) -> Result<Option<Changeset>, DbError> {
        let s: Option<String> = self
            .conn()
            .query_row("SELECT json FROM changesets WHERE id = ?1", [id], |r| r.get(0))
            .optional()?;
        Ok(s.map(|s| serde_json::from_str(&s)).transpose()?)
    }

    /// Newest first; `status` filters.
    pub fn changesets(&self, status: Option<Status>, limit: u32) -> Result<Vec<Changeset>, DbError> {
        let mut st = self.conn().prepare(
            "SELECT json FROM changesets WHERE (?1 IS NULL OR status = ?1) ORDER BY created_at DESC, rowid DESC LIMIT ?2",
        )?;
        let rows = st.query_map(params![status.map(|s| s.as_str()), limit], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(serde_json::from_str(&r?)?);
        }
        Ok(out)
    }

    pub fn snooze(&self, account: &str, thread_id: &str) -> Result<Option<Snooze>, DbError> {
        Ok(self
            .conn()
            .query_row(
                "SELECT account, thread_id, until, created_at, created_by FROM snoozes WHERE account = ?1 AND thread_id = ?2",
                params![account, thread_id],
                |r| {
                    Ok(Snooze {
                        account: r.get(0)?,
                        thread_id: r.get(1)?,
                        until: r.get(2)?,
                        created_at: r.get(3)?,
                        created_by: r.get(4)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn snoozes(&self) -> Result<Vec<Snooze>, DbError> {
        let mut st = self
            .conn()
            .prepare("SELECT account, thread_id, until, created_at, created_by FROM snoozes ORDER BY until")?;
        let rows = st.query_map([], |r| {
            Ok(Snooze {
                account: r.get(0)?,
                thread_id: r.get(1)?,
                until: r.get(2)?,
                created_at: r.get(3)?,
                created_by: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn set_snooze(&self, s: &Snooze, changeset_id: &str) -> Result<(), DbError> {
        self.conn().execute(
            "INSERT INTO snoozes (account, thread_id, until, created_at, created_by, changeset_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(account, thread_id) DO UPDATE SET until = excluded.until, created_at = excluded.created_at,
               created_by = excluded.created_by, changeset_id = excluded.changeset_id",
            params![
                s.account,
                s.thread_id,
                s.until,
                s.created_at,
                s.created_by,
                changeset_id
            ],
        )?;
        Ok(())
    }

    pub fn clear_snooze(&self, account: &str, thread_id: &str) -> Result<bool, DbError> {
        Ok(self.conn().execute(
            "DELETE FROM snoozes WHERE account = ?1 AND thread_id = ?2",
            params![account, thread_id],
        )? > 0)
    }

    pub fn set_tag(&self, t: &Tag) -> Result<(), DbError> {
        self.conn().execute(
            "INSERT INTO tags (account, thread_id, kind, reason, author, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(account, thread_id, kind) DO UPDATE SET reason = excluded.reason, author = excluded.author,
               created_at = excluded.created_at",
            params![t.account, t.thread_id, t.kind, t.reason, t.author, t.created_at],
        )?;
        Ok(())
    }

    pub fn clear_tag(&self, account: &str, thread_id: &str, kind: &str) -> Result<bool, DbError> {
        Ok(self.conn().execute(
            "DELETE FROM tags WHERE account = ?1 AND thread_id = ?2 AND kind = ?3",
            params![account, thread_id, kind],
        )? > 0)
    }

    pub fn tags(&self, kind: &str) -> Result<Vec<Tag>, DbError> {
        let mut st = self.conn().prepare(
            "SELECT account, thread_id, kind, reason, author, created_at FROM tags WHERE kind = ?1 ORDER BY created_at DESC",
        )?;
        let rows = st.query_map([kind], |r| {
            Ok(Tag {
                account: r.get(0)?,
                thread_id: r.get(1)?,
                kind: r.get(2)?,
                reason: r.get(3)?,
                author: r.get(4)?,
                created_at: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn tag(&self, account: &str, thread_id: &str, kind: &str) -> Result<Option<Tag>, DbError> {
        Ok(self
            .tags(kind)?
            .into_iter()
            .find(|t| t.account == account && t.thread_id == thread_id))
    }

    pub fn add_link(&self, l: &Link) -> Result<(), DbError> {
        self.conn().execute(
            "INSERT OR REPLACE INTO links (account, thread_id, app, kind, target, title, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![l.account, l.thread_id, l.app, l.kind, l.target, l.title, l.created_at],
        )?;
        Ok(())
    }

    pub fn links(&self, account: &str, thread_id: &str) -> Result<Vec<Link>, DbError> {
        let mut st = self.conn().prepare(
            "SELECT account, thread_id, app, kind, target, title, created_at FROM links
             WHERE account = ?1 AND thread_id = ?2 ORDER BY created_at",
        )?;
        let rows = st.query_map(params![account, thread_id], |r| {
            Ok(Link {
                account: r.get(0)?,
                thread_id: r.get(1)?,
                app: r.get(2)?,
                kind: r.get(3)?,
                target: r.get(4)?,
                title: r.get(5)?,
                created_at: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Every thread with a link.
    pub fn linked_threads(&self) -> Result<std::collections::HashSet<(String, String)>, DbError> {
        let mut st = self.conn().prepare("SELECT DISTINCT account, thread_id FROM links")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn add_suggestion(&self, s: &Suggestion) -> Result<(), DbError> {
        self.conn().execute(
            "INSERT INTO suggestions (id, account, thread_id, kind, proposal, reason, author, run_id, status, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                s.id,
                s.account,
                s.thread_id,
                s.kind,
                serde_json::to_string(&s.proposal)?,
                s.reason,
                s.author,
                s.run_id,
                s.status,
                s.created_at
            ],
        )?;
        Ok(())
    }

    /// Suggestions (newest first); `status` filters, `thread` narrows.
    pub fn suggestions(&self, status: Option<&str>, thread: Option<(&str, &str)>) -> Result<Vec<Suggestion>, DbError> {
        let mut st = self.conn().prepare(
            "SELECT id, account, thread_id, kind, proposal, reason, author, run_id, status, created_at FROM suggestions
             WHERE (?1 IS NULL OR status = ?1) AND (?2 IS NULL OR (account = ?2 AND thread_id = ?3))
             ORDER BY created_at DESC, rowid DESC",
        )?;
        let (a, t) = thread.map(|(a, t)| (Some(a), Some(t))).unwrap_or((None, None));
        let rows = st.query_map(params![status, a, t], |r| {
            Ok((
                Suggestion {
                    id: r.get(0)?,
                    account: r.get(1)?,
                    thread_id: r.get(2)?,
                    kind: r.get(3)?,
                    proposal: serde_json::Value::Null,
                    reason: r.get(5)?,
                    author: r.get(6)?,
                    run_id: r.get(7)?,
                    status: r.get(8)?,
                    created_at: r.get(9)?,
                },
                r.get::<_, String>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (mut s, p) = row?;
            s.proposal = serde_json::from_str(&p)?;
            out.push(s);
        }
        Ok(out)
    }

    pub fn set_suggestion_status(&self, id: &str, status: &str) -> Result<bool, DbError> {
        Ok(self
            .conn()
            .execute("UPDATE suggestions SET status = ?2 WHERE id = ?1", params![id, status])?
            > 0)
    }

    pub fn create_review(&self, r: &Review) -> Result<(), DbError> {
        self.conn().execute(
            "INSERT INTO reviews (id, title, author, run_id, created_at, status) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![r.id, r.title, r.author, r.run_id, r.created_at, r.status],
        )?;
        Ok(())
    }

    pub fn review(&self, id: &str) -> Result<Option<Review>, DbError> {
        Ok(self
            .conn()
            .query_row(
                "SELECT id, title, author, run_id, created_at, status FROM reviews WHERE id = ?1",
                [id],
                review_row,
            )
            .optional()?)
    }

    pub fn reviews(&self, status: Option<&str>) -> Result<Vec<Review>, DbError> {
        let mut st = self.conn().prepare(
            "SELECT id, title, author, run_id, created_at, status FROM reviews
             WHERE (?1 IS NULL OR status = ?1) ORDER BY created_at DESC, rowid DESC",
        )?;
        let rows = st.query_map([status], review_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn set_review_status(&self, id: &str, status: &str) -> Result<(), DbError> {
        self.conn()
            .execute("UPDATE reviews SET status = ?2 WHERE id = ?1", params![id, status])?;
        Ok(())
    }

    /// Insert proposals (a thread already in the review is replaced while open).
    pub fn add_proposal(&self, p: &Proposal) -> Result<(), DbError> {
        self.conn().execute(
            "INSERT INTO proposals (id, review_id, position, account, thread_id, action, reason, fingerprint, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'open')
             ON CONFLICT(review_id, account, thread_id) DO UPDATE SET action = excluded.action, reason = excluded.reason,
               fingerprint = excluded.fingerprint
             WHERE proposals.status = 'open'",
            params![
                p.id,
                p.review_id,
                p.position,
                p.account,
                p.thread_id,
                serde_json::to_string(&p.action)?,
                p.reason,
                p.fingerprint
            ],
        )?;
        Ok(())
    }

    pub fn proposals(&self, review_id: &str) -> Result<Vec<Proposal>, DbError> {
        let mut st = self.conn().prepare(
            "SELECT id, review_id, position, account, thread_id, action, reason, fingerprint, status, decided_at, changeset_id
             FROM proposals WHERE review_id = ?1 ORDER BY position",
        )?;
        let rows = st.query_map([review_id], proposal_row)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r??);
        }
        Ok(out)
    }

    pub fn proposal(&self, id: &str) -> Result<Option<Proposal>, DbError> {
        let r = self
            .conn()
            .query_row(
                "SELECT id, review_id, position, account, thread_id, action, reason, fingerprint, status, decided_at, changeset_id
                 FROM proposals WHERE id = ?1",
                [id],
                proposal_row,
            )
            .optional()?;
        r.transpose()
    }

    pub fn decide_proposal(
        &self,
        id: &str,
        status: ProposalStatus,
        action: Option<&Action>,
        changeset_id: Option<&str>,
        at: i64,
    ) -> Result<(), DbError> {
        match action {
            Some(a) => self.conn().execute(
                "UPDATE proposals SET status = ?2, action = ?3, changeset_id = ?4, decided_at = ?5 WHERE id = ?1",
                params![id, status.as_str(), serde_json::to_string(a)?, changeset_id, at],
            )?,
            None => self.conn().execute(
                "UPDATE proposals SET status = ?2, changeset_id = ?3, decided_at = ?4 WHERE id = ?1",
                params![id, status.as_str(), changeset_id, at],
            )?,
        };
        Ok(())
    }

    /// Re-open a decided proposal (its decision was undone).
    pub fn reopen_proposal(&self, id: &str) -> Result<(), DbError> {
        self.conn().execute(
            "UPDATE proposals SET status = 'open', changeset_id = NULL, decided_at = NULL WHERE id = ?1",
            [id],
        )?;
        Ok(())
    }
}

/// A review with each proposal's thread and whether it went stale
/// (the thread changed after the proposal was made).
pub fn review_view(db: &Db, id: &str) -> Result<Option<crate::api::ReviewView>, DbError> {
    let Some(review) = db.review(id)? else { return Ok(None) };
    let mut items = Vec::new();
    for p in db.proposals(id)? {
        let msgs = db.thread(&p.account, &p.thread_id)?;
        let latest = msgs.last();
        items.push(crate::api::ReviewItem {
            stale: p.status == ProposalStatus::Open && fingerprint(&msgs) != p.fingerprint,
            subject: latest.map(|m| m.subject.clone()).unwrap_or_default(),
            who: latest
                .and_then(|m| m.from.as_ref())
                .map(|a| a.short().to_string())
                .unwrap_or_default(),
            date: latest.map(|m| m.date).unwrap_or(0),
            r#ref: keryx_core::model::thread_ref(&p.account, &p.thread_id),
            group: p.action.group(),
            proposal: p,
        });
    }
    Ok(Some(crate::api::ReviewView { review, items }))
}

/// Open proposals across open reviews (for the status bar).
pub fn open_proposals(db: &Db) -> Result<usize, DbError> {
    let n: i64 = db.conn().query_row(
        "SELECT count(*) FROM proposals p JOIN reviews r ON r.id = p.review_id WHERE p.status = 'open' AND r.status = 'open'",
        [],
        |r| r.get(0),
    )?;
    Ok(n as usize)
}

fn review_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Review> {
    Ok(Review {
        id: r.get(0)?,
        title: r.get(1)?,
        author: r.get(2)?,
        run_id: r.get(3)?,
        created_at: r.get(4)?,
        status: r.get(5)?,
    })
}

#[allow(clippy::type_complexity)]
fn proposal_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Result<Proposal, DbError>> {
    let action: String = r.get(5)?;
    let status: String = r.get(8)?;
    Ok(serde_json::from_str(&action)
        .map_err(DbError::from)
        .map(|action| Proposal {
            id: r.get(0).unwrap_or_default(),
            review_id: r.get(1).unwrap_or_default(),
            position: r.get(2).unwrap_or_default(),
            account: r.get(3).unwrap_or_default(),
            thread_id: r.get(4).unwrap_or_default(),
            action,
            reason: r.get(6).unwrap_or_default(),
            fingerprint: r.get(7).unwrap_or_default(),
            status: ProposalStatus::parse(&status),
            decided_at: r.get(9).ok().flatten(),
            changeset_id: r.get(10).ok().flatten(),
        }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use keryx_core::changeset::{Author, Risk};

    fn db() -> Db {
        let db = Db::open_in_memory().unwrap();
        db.init_state().unwrap();
        db
    }

    #[test]
    fn changesets_round_trip() {
        let db = db();
        let mut cs = Changeset {
            id: "cs-1".into(),
            author: Author::Claude,
            interactive: false,
            created_at: 10,
            ops: vec![],
            risk: Risk::Low,
            reasons: vec![],
            status: Status::Applied,
            summary: "x".into(),
            review_id: None,
            undoes: None,
            error: None,
            hold_until: None,
            warnings: vec![],
        };
        db.save_changeset(&cs).unwrap();
        cs.status = Status::Undone;
        db.save_changeset(&cs).unwrap();
        assert_eq!(db.changeset("cs-1").unwrap().unwrap().status, Status::Undone);
        assert_eq!(db.changesets(Some(Status::Undone), 10).unwrap().len(), 1);
        assert!(db.changesets(Some(Status::Applied), 10).unwrap().is_empty());
    }

    #[test]
    fn snoozes_tags_reviews() {
        let db = db();
        let s = Snooze {
            account: "a".into(),
            thread_id: "t".into(),
            until: 100,
            created_at: 1,
            created_by: "alec".into(),
        };
        db.set_snooze(&s, "cs").unwrap();
        db.set_snooze(
            &Snooze {
                until: 200,
                ..s.clone()
            },
            "cs2",
        )
        .unwrap();
        assert_eq!(db.snooze("a", "t").unwrap().unwrap().until, 200);
        assert!(db.clear_snooze("a", "t").unwrap());
        assert!(db.snoozes().unwrap().is_empty());

        let t = Tag {
            account: "a".into(),
            thread_id: "t".into(),
            kind: "needs_reply".into(),
            reason: "asks about Thursday".into(),
            author: "claude".into(),
            created_at: 1,
        };
        db.set_tag(&t).unwrap();
        assert_eq!(
            db.tag("a", "t", "needs_reply").unwrap().unwrap().reason,
            "asks about Thursday"
        );
        assert!(db.clear_tag("a", "t", "needs_reply").unwrap());

        db.create_review(&Review {
            id: "rv-1".into(),
            title: "Oxford cleanup".into(),
            author: "claude".into(),
            run_id: None,
            created_at: 1,
            status: "open".into(),
        })
        .unwrap();
        let p = Proposal {
            id: "p-1".into(),
            review_id: "rv-1".into(),
            position: 0,
            account: "a".into(),
            thread_id: "t".into(),
            action: Action::File { key: "Admin".into() },
            reason: "receipt".into(),
            fingerprint: "f".into(),
            status: ProposalStatus::Open,
            decided_at: None,
            changeset_id: None,
        };
        db.add_proposal(&p).unwrap();
        // Re-proposing the same thread replaces the open proposal.
        db.add_proposal(&Proposal {
            id: "p-2".into(),
            action: Action::Archive,
            ..p.clone()
        })
        .unwrap();
        let ps = db.proposals("rv-1").unwrap();
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].action, Action::Archive);
        db.decide_proposal("p-1", ProposalStatus::Declined, None, None, 5)
            .unwrap();
        // A decided proposal isn't overwritten by a later proposal.
        db.add_proposal(&Proposal {
            id: "p-3".into(),
            action: Action::Archive,
            ..p
        })
        .unwrap();
        assert_eq!(db.proposal("p-1").unwrap().unwrap().status, ProposalStatus::Declined);
        db.reopen_proposal("p-1").unwrap();
        assert_eq!(db.proposal("p-1").unwrap().unwrap().status, ProposalStatus::Open);
    }

    #[test]
    fn action_json_shape() {
        let a = Action::Keep {
            why: Keep::Soon,
            needs_reply: false,
        };
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            r#"{"action":"keep","why":"soon","needs_reply":false}"#
        );
        let b: Action = serde_json::from_str(r#"{"action":"snooze","until":5}"#).unwrap();
        assert_eq!(b, Action::Snooze { until: 5 });
        assert_eq!(Action::File { key: "Admin".into() }.group(), "File → Admin");
    }
}
