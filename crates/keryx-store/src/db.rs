//! The mirror: `~/.local/share/keryx/keryx.db` (SQLite, WAL, FTS5).
//! Only keryxd opens it for writing ([`Db::open`]); the TUI and CLI use
//! [`Db::open_readonly`].

use keryx_core::model::{Address, Attachment, Location, Message};
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, params};
use std::collections::HashMap;
use std::path::Path;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("database: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("database row isn't valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("can't create {path}: {msg}")]
    Dir { path: String, msg: String },
    /// A caller abandoned a transaction on purpose (rolled back).
    #[error("transaction aborted")]
    Aborted,
    #[error("mirror schema {found} is newer than this keryx ({SCHEMA_VERSION}); update keryx")]
    TooNew { found: u32 },
}

pub type Result<T> = std::result::Result<T, DbError>;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS messages (
  id INTEGER PRIMARY KEY,
  account TEXT NOT NULL,
  source_id TEXT NOT NULL,
  message_id TEXT NOT NULL,
  thread_id TEXT NOT NULL,
  from_json TEXT,
  to_json TEXT NOT NULL,
  cc_json TEXT NOT NULL,
  bcc_json TEXT NOT NULL,
  reply_to_json TEXT NOT NULL,
  subject TEXT NOT NULL,
  date INTEGER NOT NULL,
  location_json TEXT NOT NULL,
  in_inbox INTEGER NOT NULL,
  read INTEGER NOT NULL,
  flagged INTEGER NOT NULL,
  size INTEGER NOT NULL,
  attachments_json TEXT NOT NULL,
  body_text TEXT NOT NULL,
  hidden_text_removed INTEGER NOT NULL,
  snippet TEXT NOT NULL,
  in_reply_to_json TEXT NOT NULL,
  references_json TEXT NOT NULL,
  list_headers_json TEXT NOT NULL,
  raw_ref TEXT,
  text_version INTEGER NOT NULL,
  UNIQUE(account, source_id)
);
CREATE INDEX IF NOT EXISTS messages_thread ON messages(account, thread_id);
CREATE INDEX IF NOT EXISTS messages_inbox ON messages(in_inbox, date);
CREATE INDEX IF NOT EXISTS messages_date ON messages(date);
CREATE INDEX IF NOT EXISTS messages_msgid ON messages(message_id);
CREATE TABLE IF NOT EXISTS raws (
  message INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
  raw BLOB NOT NULL
);
CREATE VIRTUAL TABLE IF NOT EXISTS fts USING fts5(
  subject, people, body, content='', contentless_delete=1, tokenize='unicode61 remove_diacritics 2'
);
CREATE TABLE IF NOT EXISTS sync_state (
  account TEXT PRIMARY KEY,
  state TEXT NOT NULL,
  synced_at INTEGER NOT NULL
);
";

/// Where a message's raw bytes come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Raw<'a> {
    /// Stored in the mirror (Gmail).
    Bytes(&'a [u8]),
    /// A path to the `.emlx` (Mail.app; read on demand).
    Path(&'a str),
    /// Not kept (too large); fetched from the source when needed.
    None,
}

/// What ingest compares to decide whether a message changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    pub message_id: String,
    pub location: Location,
    pub read: bool,
    pub flagged: bool,
    pub text_version: u32,
}

pub struct Db {
    conn: Connection,
}

fn json<T: serde::Serialize>(v: &T) -> Result<String> {
    Ok(serde_json::to_string(v)?)
}

fn people(m: &Message) -> String {
    m.from
        .iter()
        .chain(&m.to)
        .chain(&m.cc)
        .chain(&m.bcc)
        .map(|a| format!("{} {}", a.name.as_deref().unwrap_or_default(), a.email))
        .collect::<Vec<_>>()
        .join(" ")
}

impl Db {
    /// Open for writing (keryxd only), creating the schema.
    pub fn open(path: &Path) -> Result<Db> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| DbError::Dir {
                path: dir.display().to_string(),
                msg: e.to_string(),
            })?;
        }
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    /// An in-memory mirror (tests).
    pub fn open_in_memory() -> Result<Db> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Db> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(SCHEMA)?;
        let db = Db { conn };
        match db.meta("schema_version")? {
            Some(v) => {
                let found: u32 = v.parse().unwrap_or(0);
                if found > SCHEMA_VERSION {
                    return Err(DbError::TooNew { found });
                }
            }
            None => db.set_meta("schema_version", &SCHEMA_VERSION.to_string())?,
        }
        #[cfg(unix)]
        if let Some(p) = db.conn.path() {
            use std::os::unix::fs::PermissionsExt;
            if !p.is_empty() {
                let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600));
            }
        }
        Ok(db)
    }

    /// Open read-only (TUI, CLI). Never blocks keryxd (WAL readers).
    pub fn open_readonly(path: &Path) -> Result<Db> {
        let conn =
            Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        Ok(Db { conn })
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Run `f` in one transaction (an ingest batch is all-or-nothing).
    pub fn transaction<T>(&mut self, f: impl FnOnce(&Db) -> Result<T>) -> Result<T> {
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

    /// Insert or replace a message (by account + source id), with its FTS row.
    pub fn upsert(&self, m: &Message, raw: Raw<'_>, text_version: u32) -> Result<i64> {
        let raw_ref = match raw {
            Raw::Path(p) => Some(p),
            _ => None,
        };
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM messages WHERE account = ?1 AND source_id = ?2",
                params![m.account, m.source_id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = existing {
            self.conn.execute("DELETE FROM fts WHERE rowid = ?1", [id])?;
            self.conn.execute("DELETE FROM messages WHERE id = ?1", [id])?;
        }
        self.conn.execute(
            "INSERT INTO messages (id, account, source_id, message_id, thread_id, from_json, to_json, cc_json,
               bcc_json, reply_to_json, subject, date, location_json, in_inbox, read, flagged, size,
               attachments_json, body_text, hidden_text_removed, snippet, in_reply_to_json, references_json,
               list_headers_json, raw_ref, text_version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20,
               ?21, ?22, ?23, ?24, ?25, ?26)",
            params![
                existing,
                m.account,
                m.source_id,
                m.message_id,
                m.thread_id,
                m.from.as_ref().map(json).transpose()?,
                json(&m.to)?,
                json(&m.cc)?,
                json(&m.bcc)?,
                json(&m.reply_to)?,
                m.subject,
                m.date,
                json(&m.location)?,
                m.in_inbox,
                m.read,
                m.flagged,
                m.size as i64,
                json(&m.attachments)?,
                m.body_text,
                m.hidden_text_removed,
                m.snippet,
                json(&m.in_reply_to)?,
                json(&m.references)?,
                json(&m.list_headers)?,
                raw_ref,
                text_version,
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        if let Raw::Bytes(b) = raw {
            self.conn.execute(
                "INSERT OR REPLACE INTO raws (message, raw) VALUES (?1, ?2)",
                params![id, b],
            )?;
        }
        self.conn.execute(
            "INSERT INTO fts (rowid, subject, people, body) VALUES (?1, ?2, ?3, ?4)",
            params![id, m.subject, people(m), m.body_text],
        )?;
        Ok(id)
    }

    /// Update only where a message is and its flags (no re-parse needed).
    pub fn update_state(
        &self,
        account: &str,
        source_id: &str,
        location: &Location,
        in_inbox: bool,
        read: bool,
        flagged: bool,
    ) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE messages SET location_json = ?3, in_inbox = ?4, read = ?5, flagged = ?6
             WHERE account = ?1 AND source_id = ?2",
            params![account, source_id, json(location)?, in_inbox, read, flagged],
        )?;
        Ok(n > 0)
    }

    pub fn delete(&self, account: &str, source_id: &str) -> Result<bool> {
        let id: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM messages WHERE account = ?1 AND source_id = ?2",
                params![account, source_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(id) = id else { return Ok(false) };
        self.conn.execute("DELETE FROM fts WHERE rowid = ?1", [id])?;
        self.conn.execute("DELETE FROM messages WHERE id = ?1", [id])?;
        Ok(true)
    }

    /// source_id → fingerprint, for diffing an account against its source.
    pub fn fingerprints(&self, account: &str) -> Result<HashMap<String, Fingerprint>> {
        let mut st = self.conn.prepare(
            "SELECT source_id, message_id, location_json, read, flagged, text_version
             FROM messages WHERE account = ?1",
        )?;
        let rows = st.query_map([account], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, bool>(3)?,
                r.get::<_, bool>(4)?,
                r.get::<_, u32>(5)?,
            ))
        })?;
        let mut out = HashMap::new();
        for row in rows {
            let (sid, message_id, loc, read, flagged, text_version) = row?;
            out.insert(
                sid,
                Fingerprint {
                    message_id,
                    location: serde_json::from_str(&loc)?,
                    read,
                    flagged,
                    text_version,
                },
            );
        }
        Ok(out)
    }

    pub fn message(&self, account: &str, source_id: &str) -> Result<Option<Message>> {
        let mut st = self
            .conn
            .prepare(&format!("{SELECT_MESSAGE} WHERE account = ?1 AND source_id = ?2"))?;
        let mut rows = st.query(params![account, source_id])?;
        match rows.next()? {
            Some(r) => Ok(Some(row_to_message(r)?)),
            None => Ok(None),
        }
    }

    /// Every message of a thread, oldest first.
    pub fn thread(&self, account: &str, thread_id: &str) -> Result<Vec<Message>> {
        let mut st = self.conn.prepare(&format!(
            "{SELECT_MESSAGE} WHERE account = ?1 AND thread_id = ?2 ORDER BY date, id"
        ))?;
        let mut rows = st.query(params![account, thread_id])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(row_to_message(r)?);
        }
        Ok(out)
    }

    /// Stored raw bytes (Gmail), or the `.emlx` path (Mail.app).
    pub fn raw(&self, account: &str, source_id: &str) -> Result<(Option<Vec<u8>>, Option<String>)> {
        Ok(self
            .conn
            .query_row(
                "SELECT r.raw, m.raw_ref FROM messages m LEFT JOIN raws r ON r.message = m.id
                 WHERE m.account = ?1 AND m.source_id = ?2",
                params![account, source_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .unwrap_or((None, None)))
    }

    pub fn count(&self, account: Option<&str>) -> Result<u64> {
        let n: i64 = match account {
            Some(a) => self
                .conn
                .query_row("SELECT count(*) FROM messages WHERE account = ?1", [a], |r| r.get(0))?,
            None => self.conn.query_row("SELECT count(*) FROM messages", [], |r| r.get(0))?,
        };
        Ok(n as u64)
    }

    /// (mailbox or label, total) for an account, from each message's location.
    pub fn counts_by_mailbox(&self, account: &str) -> Result<HashMap<String, u64>> {
        let mut st = self
            .conn
            .prepare("SELECT location_json FROM messages WHERE account = ?1")?;
        let rows = st.query_map([account], |r| r.get::<_, String>(0))?;
        let mut out: HashMap<String, u64> = HashMap::new();
        for row in rows {
            let loc: Location = serde_json::from_str(&row?)?;
            for m in loc.all() {
                *out.entry(m.to_string()).or_default() += 1;
            }
        }
        Ok(out)
    }

    /// Full-text search over subject, people and body; ids by rank.
    pub fn fts(&self, query: &str, limit: u32) -> Result<Vec<i64>> {
        let mut st = self
            .conn
            .prepare("SELECT rowid FROM fts WHERE fts MATCH ?1 ORDER BY rank LIMIT ?2")?;
        let rows = st.query_map(params![query, limit], |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn sync_state(&self, account: &str) -> Result<Option<(String, i64)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT state, synced_at FROM sync_state WHERE account = ?1",
                [account],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    pub fn set_sync_state(&self, account: &str, state: &str, synced_at: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO sync_state (account, state, synced_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(account) DO UPDATE SET state = excluded.state, synced_at = excluded.synced_at",
            params![account, state, synced_at],
        )?;
        Ok(())
    }

    /// A deterministic dump of an account's mirror (idempotence tests).
    pub fn dump(&self, account: &str) -> Result<Vec<Message>> {
        let mut st = self
            .conn
            .prepare(&format!("{SELECT_MESSAGE} WHERE account = ?1 ORDER BY source_id"))?;
        let mut rows = st.query([account])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(row_to_message(r)?);
        }
        Ok(out)
    }
}

pub const SELECT_MESSAGE: &str = "SELECT account, source_id, message_id, thread_id, from_json, to_json, cc_json,
  bcc_json, reply_to_json, subject, date, location_json, in_inbox, read, flagged, size, attachments_json,
  body_text, hidden_text_removed, snippet, in_reply_to_json, references_json, list_headers_json FROM messages";

pub fn row_to_message(r: &Row<'_>) -> Result<Message> {
    let from: Option<String> = r.get(4)?;
    let j = |i: usize| -> Result<String> { Ok(r.get::<_, String>(i)?) };
    Ok(Message {
        account: r.get(0)?,
        source_id: r.get(1)?,
        message_id: r.get(2)?,
        thread_id: r.get(3)?,
        from: from.map(|f| serde_json::from_str::<Address>(&f)).transpose()?,
        to: serde_json::from_str(&j(5)?)?,
        cc: serde_json::from_str(&j(6)?)?,
        bcc: serde_json::from_str(&j(7)?)?,
        reply_to: serde_json::from_str(&j(8)?)?,
        subject: r.get(9)?,
        date: r.get(10)?,
        location: serde_json::from_str(&j(11)?)?,
        in_inbox: r.get(12)?,
        read: r.get(13)?,
        flagged: r.get(14)?,
        size: r.get::<_, i64>(15)? as u64,
        attachments: serde_json::from_str::<Vec<Attachment>>(&j(16)?)?,
        body_text: r.get(17)?,
        hidden_text_removed: r.get(18)?,
        snippet: r.get(19)?,
        in_reply_to: serde_json::from_str(&j(20)?)?,
        references: serde_json::from_str(&j(21)?)?,
        list_headers: serde_json::from_str(&j(22)?)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn msg(account: &str, sid: &str, subject: &str) -> Message {
        Message {
            account: account.into(),
            source_id: sid.into(),
            message_id: format!("{sid}@x"),
            thread_id: sid.into(),
            from: Some(Address::new(Some("Jane"), "jane@example.com")),
            to: vec![Address::new(None, "me@example.com")],
            cc: vec![],
            bcc: vec![],
            reply_to: vec![],
            subject: subject.into(),
            date: 1_760_000_000,
            location: Location::Labels {
                labels: vec!["INBOX".into()],
            },
            in_inbox: true,
            read: false,
            flagged: false,
            size: 100,
            attachments: vec![],
            body_text: format!("body of {subject}"),
            hidden_text_removed: false,
            snippet: String::new(),
            in_reply_to: vec![],
            references: vec![],
            list_headers: vec![],
        }
    }

    #[test]
    fn upsert_roundtrip_and_replace() {
        let db = Db::open_in_memory().unwrap();
        let m = msg("personal", "a1", "Lab partner");
        db.upsert(&m, Raw::Bytes(b"raw"), 1).unwrap();
        assert_eq!(db.message("personal", "a1").unwrap().unwrap(), m);
        assert_eq!(db.raw("personal", "a1").unwrap().0.as_deref(), Some(&b"raw"[..]));
        let mut m2 = m.clone();
        m2.subject = "Changed".into();
        m2.body_text = "new body".into();
        db.upsert(&m2, Raw::Bytes(b"raw2"), 1).unwrap();
        assert_eq!(db.count(None).unwrap(), 1);
        assert_eq!(db.message("personal", "a1").unwrap().unwrap().subject, "Changed");
        // FTS follows the replacement.
        assert!(db.fts("Lab", 10).unwrap().is_empty());
        assert_eq!(db.fts("Changed", 10).unwrap().len(), 1);
    }

    #[test]
    fn state_updates_and_deletes() {
        let db = Db::open_in_memory().unwrap();
        db.upsert(&msg("oxford", "7", "Hi"), Raw::Path("/x/7.emlx"), 1).unwrap();
        let loc = Location::Mailboxes {
            physical: "Archive".into(),
            labels: vec![],
        };
        assert!(db.update_state("oxford", "7", &loc, false, true, true).unwrap());
        let m = db.message("oxford", "7").unwrap().unwrap();
        assert!(!m.in_inbox && m.read && m.flagged);
        assert_eq!(db.raw("oxford", "7").unwrap().1.as_deref(), Some("/x/7.emlx"));
        assert!(db.delete("oxford", "7").unwrap());
        assert!(!db.delete("oxford", "7").unwrap());
        assert_eq!(db.fts("Hi", 10).unwrap(), Vec::<i64>::new());
    }

    #[test]
    fn fingerprints_and_counts() {
        let db = Db::open_in_memory().unwrap();
        db.upsert(&msg("personal", "a", "One"), Raw::None, 1).unwrap();
        db.upsert(&msg("personal", "b", "Two"), Raw::None, 1).unwrap();
        db.upsert(&msg("oxford", "c", "Three"), Raw::None, 1).unwrap();
        let f = db.fingerprints("personal").unwrap();
        assert_eq!(f.len(), 2);
        assert_eq!(f["a"].message_id, "a@x");
        assert_eq!(db.counts_by_mailbox("personal").unwrap()["INBOX"], 2);
    }

    #[test]
    fn fts_matches_people_and_diacritics() {
        let db = Db::open_in_memory().unwrap();
        let mut m = msg("personal", "a", "Café on Sunday");
        m.from = Some(Address::new(Some("Jürgen"), "juergen@example.de"));
        db.upsert(&m, Raw::None, 1).unwrap();
        assert_eq!(db.fts("cafe", 10).unwrap().len(), 1);
        assert_eq!(db.fts("jurgen", 10).unwrap().len(), 1);
        assert_eq!(db.fts("people:juergen", 10).unwrap().len(), 1);
    }

    #[test]
    fn readonly_open_sees_writes_and_cannot_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keryx.db");
        let db = Db::open(&path).unwrap();
        db.upsert(&msg("personal", "a", "One"), Raw::None, 1).unwrap();
        let ro = Db::open_readonly(&path).unwrap();
        assert_eq!(ro.count(None).unwrap(), 1);
        assert!(ro.set_meta("x", "y").is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn transaction_rolls_back() {
        let mut db = Db::open_in_memory().unwrap();
        let r: Result<()> = db.transaction(|db| {
            db.upsert(&msg("personal", "a", "One"), Raw::None, 1)?;
            Err(DbError::TooNew { found: 99 })
        });
        assert!(r.is_err());
        assert_eq!(db.count(None).unwrap(), 0);
    }
}
