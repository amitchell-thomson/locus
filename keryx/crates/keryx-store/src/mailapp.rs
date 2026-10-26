//! Reading Mac Mail's local store (`~/Library/Mail/V10`), read-only.
//!
//! - `MailData/Envelope Index` (SQLite, opened `mode=ro`): one row per
//!   message in `messages`, its mailbox in `mailboxes`, Gmail-over-IMAP label
//!   membership in `labels` (virtual mailboxes have `source` set).
//! - `.emlx` files: a byte-count line, the RFC 5322 message, then a plist.
//!   `.partial.emlx` keep attachment bodies under `Data/Attachments/<ROWID>/`.
//!
//! Layout findings: CLAUDE.md "Environment findings".

use keryx_core::model::Address;
use rusqlite::{Connection, OpenFlags, params};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum MailError {
    #[error("can't open Mail's Envelope Index at {path}: {msg} (does keryxd have Full Disk Access?)")]
    Open { path: String, msg: String },
    #[error("Mail's Envelope Index: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("reading {path}: {msg}")]
    Io { path: String, msg: String },
    #[error("{0} isn't a valid .emlx file")]
    BadEmlx(String),
}

pub type Result<T> = std::result::Result<T, MailError>;

/// One message as Mail knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailRow {
    pub rowid: i64,
    /// Physical mailbox, as a path below the account (`Inbox`, `[Gmail]/All Mail`).
    pub physical: String,
    /// Virtual mailboxes it also appears in (`INBOX`, `[Gmail]/Sent Mail`).
    pub labels: Vec<String>,
    pub read: bool,
    pub flagged: bool,
    pub conversation_id: i64,
    /// Without angle brackets; "" if Mail doesn't have it.
    pub message_id: String,
    pub date_received: i64,
    pub size: u64,
    /// Envelope fields Mail keeps itself (used when the body isn't on disk).
    pub subject: String,
    pub sender: Option<Address>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
}

/// `scheme://ACCOUNT/percent-encoded/path` → (account, decoded path).
pub fn parse_mailbox_url(url: &str) -> Option<(String, String)> {
    let rest = url.split_once("://")?.1;
    let (account, path) = rest.split_once('/')?;
    Some((account.to_string(), percent_decode(path)))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
}

/// Strip `<` `>` and whitespace from a Message-ID.
fn bare_id(s: &str) -> String {
    s.trim().trim_start_matches('<').trim_end_matches('>').to_string()
}

pub struct MailStore {
    root: PathBuf,
    conn: Connection,
}

impl MailStore {
    /// `root` is the `V10` directory.
    pub fn open(root: &Path) -> Result<MailStore> {
        let path = root.join("MailData").join("Envelope Index");
        let conn = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(|e| MailError::Open {
            path: path.display().to_string(),
            msg: e.to_string(),
        })?;
        // Probe now so a missing grant shows up as one clear error.
        conn.query_row("SELECT count(*) FROM mailboxes", [], |r| r.get::<_, i64>(0))
            .map_err(|e| MailError::Open {
                path: path.display().to_string(),
                msg: e.to_string(),
            })?;
        Ok(MailStore {
            root: root.to_path_buf(),
            conn,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Every message of one Mail account (by its UUID), excluding deleted rows.
    pub fn scan(&self, account_uuid: &str) -> Result<Vec<MailRow>> {
        let prefix_like = format!("%://{account_uuid}/%");
        let mut labels: HashMap<i64, Vec<String>> = HashMap::new();
        {
            let mut st = self.conn.prepare(
                "SELECT l.message_id, mb.url FROM labels l JOIN mailboxes mb ON mb.ROWID = l.mailbox_id
                 WHERE mb.url LIKE ?1",
            )?;
            let rows = st.query_map([&prefix_like], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
            for row in rows {
                let (id, url) = row?;
                if let Some((_, path)) = parse_mailbox_url(&url) {
                    labels.entry(id).or_default().push(path);
                }
            }
        }
        let mut recipients: HashMap<i64, (Vec<Address>, Vec<Address>)> = HashMap::new();
        {
            let mut st = self.conn.prepare(
                "SELECT r.message, r.type, a.address, a.comment FROM recipients r
                 JOIN addresses a ON a.ROWID = r.address
                 JOIN messages m ON m.ROWID = r.message
                 JOIN mailboxes mb ON mb.ROWID = m.mailbox
                 WHERE mb.url LIKE ?1 ORDER BY r.message, r.type, r.position",
            )?;
            let rows = st.query_map([&prefix_like], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Option<i64>>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })?;
            for row in rows {
                let (id, kind, addr, name) = row?;
                let e = recipients.entry(id).or_default();
                let a = Address::new(Some(&name), &addr);
                match kind {
                    Some(1) => e.1.push(a),
                    _ => e.0.push(a),
                }
            }
        }
        let mut st = self.conn.prepare(
            "SELECT m.ROWID, mb.url, m.read, m.flagged, m.conversation_id, g.message_id_header,
                    COALESCE(m.date_received, m.date_sent, 0), m.size,
                    COALESCE(m.subject_prefix, '') || COALESCE(s.subject, ''), a.address, a.comment
             FROM messages m
             JOIN mailboxes mb ON mb.ROWID = m.mailbox
             LEFT JOIN message_global_data g ON g.ROWID = m.global_message_id
             LEFT JOIN subjects s ON s.ROWID = m.subject
             LEFT JOIN addresses a ON a.ROWID = m.sender
             WHERE mb.url LIKE ?1 AND m.deleted = 0
             ORDER BY m.ROWID",
        )?;
        let rows = st.query_map([&prefix_like], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, bool>(2)?,
                r.get::<_, bool>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, i64>(7)?,
                r.get::<_, String>(8)?,
                r.get::<_, Option<String>>(9)?,
                r.get::<_, Option<String>>(10)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (rowid, url, read, flagged, conversation_id, mid, date, size, subject, s_addr, s_name) = row?;
            let Some((_, physical)) = parse_mailbox_url(&url) else {
                continue;
            };
            let mut l = labels.remove(&rowid).unwrap_or_default();
            l.sort();
            let (to, cc) = recipients.remove(&rowid).unwrap_or_default();
            out.push(MailRow {
                rowid,
                physical,
                labels: l,
                read,
                flagged,
                conversation_id,
                message_id: mid.as_deref().map(bare_id).unwrap_or_default(),
                date_received: date,
                size: size.max(0) as u64,
                subject,
                sender: s_addr.map(|a| Address::new(s_name.as_deref(), &a)),
                to,
                cc,
            });
        }
        Ok(out)
    }

    /// Mail's own total per mailbox (path → total_count) for an account.
    pub fn mailbox_totals(&self, account_uuid: &str) -> Result<HashMap<String, u64>> {
        let mut st = self
            .conn
            .prepare("SELECT url, total_count FROM mailboxes WHERE url LIKE ?1")?;
        let rows = st.query_map([format!("%://{account_uuid}/%")], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        let mut out = HashMap::new();
        for row in rows {
            let (url, n) = row?;
            if let Some((_, path)) = parse_mailbox_url(&url) {
                out.insert(path, n.max(0) as u64);
            }
        }
        Ok(out)
    }

    /// ROWID → `.emlx` path for every message file under an account.
    pub fn emlx_index(&self, account_uuid: &str) -> Result<HashMap<i64, PathBuf>> {
        let mut out = HashMap::new();
        let dir = self.root.join(account_uuid);
        if dir.is_dir() {
            walk(&dir, &mut out)?;
        }
        Ok(out)
    }
}

fn walk(dir: &Path, out: &mut HashMap<i64, PathBuf>) -> Result<()> {
    let rd = std::fs::read_dir(dir).map_err(|e| MailError::Io {
        path: dir.display().to_string(),
        msg: e.to_string(),
    })?;
    for entry in rd.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            if path.file_name().is_some_and(|n| n == "Attachments") {
                continue;
            }
            walk(&path, out)?;
        } else if let Some(name) = path.file_name().and_then(|n| n.to_str())
            && let Some(stem) = name.strip_suffix(".emlx")
        {
            let stem = stem.strip_suffix(".partial").unwrap_or(stem);
            if let Ok(rowid) = stem.parse::<i64>() {
                // A full copy wins over a partial one.
                let partial = name.ends_with(".partial.emlx");
                if !partial || !out.contains_key(&rowid) {
                    out.insert(rowid, path.clone());
                }
            }
        }
    }
    Ok(())
}

pub fn is_partial(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(".partial.emlx"))
}

/// The RFC 5322 bytes inside an `.emlx`.
pub fn read_emlx(path: &Path) -> Result<Vec<u8>> {
    let data = std::fs::read(path).map_err(|e| MailError::Io {
        path: path.display().to_string(),
        msg: e.to_string(),
    })?;
    let bad = || MailError::BadEmlx(path.display().to_string());
    let nl = data.iter().position(|&b| b == b'\n').ok_or_else(bad)?;
    let len: usize = std::str::from_utf8(&data[..nl])
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .ok_or_else(bad)?;
    let start = nl + 1;
    let end = start.checked_add(len).filter(|&e| e <= data.len()).ok_or_else(bad)?;
    Ok(data[start..end].to_vec())
}

/// For a `.partial.emlx`, the on-disk file of the attachment called `name`
/// (`Data/Attachments/<ROWID>/<part>/<name>`).
pub fn attachment_file(emlx: &Path, rowid: i64, name: &str) -> Option<PathBuf> {
    let data_dir = emlx.ancestors().find(|p| p.file_name().is_some_and(|n| n == "Data"))?;
    let dir = data_dir.join("Attachments").join(rowid.to_string());
    let rd = std::fs::read_dir(dir).ok()?;
    for part in rd.flatten() {
        let candidate = part.path().join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Mail accounts known to macOS (`~/Library/Accounts/Accounts4.sqlite`):
/// (account UUID used by Mail, address, description).
pub fn discover_accounts(accounts_db: &Path) -> Result<Vec<(String, String, String)>> {
    let conn = Connection::open_with_flags(
        accounts_db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| MailError::Open {
        path: accounts_db.display().to_string(),
        msg: e.to_string(),
    })?;
    // Mail's account id is a child row whose parent carries the username.
    let mut st = conn.prepare(
        "SELECT a.ZIDENTIFIER, COALESCE(p.ZUSERNAME, a.ZUSERNAME, ''),
                COALESCE(p.ZACCOUNTDESCRIPTION, a.ZACCOUNTDESCRIPTION, '')
         FROM ZACCOUNT a LEFT JOIN ZACCOUNT p ON a.ZPARENTACCOUNT = p.Z_PK
         WHERE a.ZIDENTIFIER IS NOT NULL",
    )?;
    let rows = st.query_map(params![], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OXFORD: &str = "74EA59BE-FE1D-4D49-8712-D8A2AB402605";
    const OQTS: &str = "09F30390-18E1-4550-AF71-D9A1004D8C0F";

    fn fixture() -> MailStore {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/mailapp/V10");
        MailStore::open(&root).unwrap()
    }

    #[test]
    fn urls() {
        assert_eq!(
            parse_mailbox_url(&format!("imap://{OQTS}/%5BGmail%5D/All%20Mail")),
            Some((OQTS.to_string(), "[Gmail]/All Mail".to_string()))
        );
        assert_eq!(
            parse_mailbox_url(&format!("ews://{OXFORD}/Sent%20Items")),
            Some((OXFORD.to_string(), "Sent Items".to_string()))
        );
        assert_eq!(parse_mailbox_url("nonsense"), None);
        assert_eq!(percent_decode("a%2"), "a%2");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%C3%A9t%C3%A9%e2"), "été%e2".replace("%e2", "\u{fffd}"));
        assert_eq!(percent_decode("ü%"), "ü%");
    }

    #[test]
    fn scans_exchange_account() {
        let s = fixture();
        let rows = s.scan(OXFORD).unwrap();
        assert_eq!(rows.len(), 20);
        let inbox = rows.iter().filter(|r| r.physical == "Inbox").count();
        assert_eq!(inbox, 17);
        let r1 = rows.iter().find(|r| r.rowid == 1).unwrap();
        assert_eq!(r1.subject, "Lab partner for Thursday?");
        assert_eq!(r1.sender.as_ref().unwrap().display(), "Jane Doe <jane.doe@example.com>");
        assert_eq!(r1.to[0].email, "newc7370@ox.ac.uk");
        let r11 = rows.iter().find(|r| r.rowid == 11).unwrap();
        assert_eq!(r11.subject, "Re: Group project plan");
        assert_eq!(r11.cc.len(), 2);
        let r = rows.iter().find(|r| r.rowid == 8).unwrap();
        assert!(r.flagged && r.read);
        assert_eq!(r.message_id, "outlook-15@jcr.example.ac.uk");
        // Thread a/b share a conversation across Inbox and Sent Items.
        let a = rows.iter().find(|r| r.rowid == 10).unwrap();
        let b = rows.iter().find(|r| r.rowid == 11).unwrap();
        assert_eq!(a.conversation_id, b.conversation_id);
        assert_eq!(b.physical, "Sent Items");
    }

    #[test]
    fn gmail_imap_inbox_is_a_label() {
        let s = fixture();
        let rows = s.scan(OQTS).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].physical, "[Gmail]/All Mail");
        assert_eq!(rows[0].labels, vec!["INBOX".to_string()]);
        assert!(!rows[0].read);
    }

    #[test]
    fn emlx_index_and_reading() {
        let s = fixture();
        let idx = s.emlx_index(OXFORD).unwrap();
        assert_eq!(idx.len(), 19, "ROWID 23 has no file (not downloaded)");
        let p = &idx[&12345];
        assert!(is_partial(p));
        assert!(p.to_string_lossy().contains("/Data/2/1/Messages/"));
        let raw = read_emlx(&idx[&1]).unwrap();
        assert!(raw.starts_with(b"From: Jane Doe"));
        assert!(raw.ends_with(b"Jane\n"));
        let f = attachment_file(p, 12345, "sheet3.pdf").unwrap();
        assert!(std::fs::read(f).unwrap().starts_with(b"%PDF"));
        assert!(attachment_file(p, 12345, "nope.pdf").is_none());
    }

    #[test]
    fn bad_emlx_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("1.emlx");
        std::fs::write(&p, b"99999\nshort").unwrap();
        assert!(matches!(read_emlx(&p), Err(MailError::BadEmlx(_))));
        std::fs::write(&p, b"not a number\n").unwrap();
        assert!(matches!(read_emlx(&p), Err(MailError::BadEmlx(_))));
    }

    #[test]
    fn totals_match_fixture() {
        let s = fixture();
        let t = s.mailbox_totals(OXFORD).unwrap();
        assert_eq!(t["Inbox"], 17);
        assert_eq!(t["Sent Items"], 1);
    }

    #[test]
    fn missing_store_is_a_clear_error() {
        let e = MailStore::open(Path::new("/nonexistent/V10")).err().unwrap();
        assert!(e.to_string().contains("Full Disk Access"), "{e}");
    }
}
