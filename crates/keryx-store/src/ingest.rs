//! Bringing a source into the mirror. Each pass diffs the source against
//! the mirror's fingerprints: new or re-indexed messages are parsed and
//! upserted, moved or re-flagged ones get a state update, vanished ones are
//! removed. Work is committed in batches, so an interrupted pass leaves a
//! consistent mirror and the next pass converges to the same result.

use crate::db::{Db, DbError, Fingerprint, Raw};
use crate::mailapp::{self, MailError, MailRow, MailStore};
use keryx_core::build::{self, SourceFacts, is_hidden_mailbox};
use keryx_core::config::Account;
use keryx_core::mime::{self, Parsed};
use keryx_core::model::Location;
use keryx_core::text::TEXT_VERSION;
use std::collections::HashSet;

pub const BATCH: usize = 200;

#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("mirror: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Mail(#[from] MailError),
    #[error("account {0:?} isn't a Mail.app account (no mailapp_account_id)")]
    NotMailapp(String),
    #[error("ingest stopped after {done} messages (test hook)")]
    Interrupted { done: usize },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    /// Messages whose file couldn't be read or parsed (kept out of the mirror).
    pub skipped: Vec<String>,
}

impl Report {
    pub fn changed(&self) -> bool {
        self.added + self.updated + self.removed > 0
    }
}

/// Where a Mail.app row lives, with hidden mailboxes (trash, spam, …) dropped.
/// `None` if the message is only in hidden mailboxes.
pub fn mailapp_location(row: &MailRow) -> Option<Location> {
    if is_hidden_mailbox(&row.physical) {
        return None;
    }
    Some(Location::Mailboxes {
        physical: row.physical.clone(),
        labels: row.labels.iter().filter(|l| !is_hidden_mailbox(l)).cloned().collect(),
    })
}

/// Mirror one Mail.app account. `stop_after` interrupts after that many
/// parsed messages (tests only).
pub fn mailapp_account(
    db: &mut Db,
    store: &MailStore,
    account: &Account,
    stop_after: Option<usize>,
) -> Result<Report, IngestError> {
    let uuid = account
        .mailapp_account_id
        .as_deref()
        .ok_or_else(|| IngestError::NotMailapp(account.id.clone()))?;
    let inbox = account.inbox_mailbox.clone().unwrap_or_default();
    let rows = store.scan(uuid)?;
    let current = db.fingerprints(&account.id)?;
    let mut report = Report::default();
    let mut seen = HashSet::new();

    enum Work {
        Parse(MailRow, Location),
        State(MailRow, Location),
    }
    let mut work = Vec::new();
    for row in rows {
        let Some(location) = mailapp_location(&row) else {
            continue;
        };
        let sid = row.rowid.to_string();
        seen.insert(sid.clone());
        match current.get(&sid) {
            Some(fp) if same_message(fp, &row.message_id) => {
                if fp.location != location || fp.read != row.read || fp.flagged != row.flagged {
                    work.push(Work::State(row, location));
                }
            }
            _ => work.push(Work::Parse(row, location)),
        }
    }

    let needs_files = work.iter().any(|w| matches!(w, Work::Parse(..)));
    let index = if needs_files {
        store.emlx_index(uuid)?
    } else {
        Default::default()
    };
    let mut parsed = 0usize;
    for chunk in work.chunks(BATCH) {
        db.transaction(|db| {
            for w in chunk {
                match w {
                    Work::State(row, location) => {
                        let in_inbox = location.contains(&inbox);
                        db.update_state(
                            &account.id,
                            &row.rowid.to_string(),
                            location,
                            in_inbox,
                            row.read,
                            row.flagged,
                        )?;
                        report.updated += 1;
                    }
                    Work::Parse(row, location) => {
                        if stop_after.is_some_and(|n| parsed >= n) {
                            return Err(DbError::Aborted);
                        }
                        parsed += 1;
                        let sid = row.rowid.to_string();
                        let path = index.get(&row.rowid);
                        let raw = match path.map(|p| mailapp::read_emlx(p)) {
                            Some(Ok(raw)) => raw,
                            Some(Err(e)) => {
                                report.skipped.push(format!("{sid}: {e}"));
                                continue;
                            }
                            None => Vec::new(),
                        };
                        // Mail hasn't downloaded the body yet (no file, or a
                        // 0-byte placeholder): mirror the envelope it keeps and
                        // retry every pass (text_version 0) until it arrives.
                        let header_only = raw.is_empty();
                        let mut p = if header_only {
                            envelope_only(row)
                        } else {
                            match mime::parse(&raw) {
                                Ok(p) => p,
                                Err(e) => {
                                    report.skipped.push(format!("{sid}: {e}"));
                                    continue;
                                }
                            }
                        };
                        if let Some(path) = path.filter(|p| mailapp::is_partial(p)) {
                            for a in &mut p.attachments {
                                if let Some(f) = mailapp::attachment_file(path, row.rowid, &a.name)
                                    && let Ok(meta) = std::fs::metadata(f)
                                {
                                    a.size = meta.len();
                                }
                            }
                        }
                        let thread_id = if row.conversation_id != 0 {
                            row.conversation_id.to_string()
                        } else {
                            format!("m{sid}")
                        };
                        let was = current.contains_key(&sid);
                        let m = build::message(
                            &p,
                            SourceFacts {
                                account: account.id.clone(),
                                source_id: sid.clone(),
                                thread_id,
                                in_inbox: location.contains(&inbox),
                                location: location.clone(),
                                read: row.read,
                                flagged: row.flagged,
                                size: row.size,
                                received: row.date_received,
                            },
                        );
                        let raw_ref = path.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
                        let version = if header_only { 0 } else { TEXT_VERSION };
                        let changed = !was || current.get(&sid).is_some_and(|fp| fp.text_version != 0 || !header_only);
                        db.upsert(&m, Raw::Path(&raw_ref), version)?;
                        if !changed {
                            // Still waiting for Mail's download: not news.
                        } else if was {
                            report.updated += 1;
                        } else {
                            report.added += 1;
                        }
                    }
                }
            }
            Ok(())
        })
        .map_err(|e| match e {
            DbError::Aborted => IngestError::Interrupted { done: parsed },
            e => IngestError::Db(e),
        })?;
    }

    let gone: Vec<String> = current.keys().filter(|k| !seen.contains(*k)).cloned().collect();
    for chunk in gone.chunks(BATCH) {
        db.transaction(|db| {
            for sid in chunk {
                if db.delete(&account.id, sid)? {
                    report.removed += 1;
                }
            }
            Ok(())
        })?;
    }
    Ok(report)
}

/// Re-derive text for messages stored under an older [`TEXT_VERSION`],
/// from the raw bytes already in the mirror (Gmail) or the `.emlx` path
/// (Mail.app) — no network. Header-only rows (version 0) are left to ingest.
pub fn rederive(db: &mut Db) -> Result<usize, IngestError> {
    let stale: Vec<(String, String)> = {
        let mut st = db
            .conn()
            .prepare("SELECT account, source_id FROM messages WHERE text_version > 0 AND text_version < ?1")?;
        let rows = st.query_map([TEXT_VERSION], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<Result<_, _>>()?
    };
    let mut done = 0;
    for chunk in stale.chunks(BATCH) {
        db.transaction(|db| {
            for (account, sid) in chunk {
                let Some(old) = db.message(account, sid)? else { continue };
                let (bytes, path) = db.raw(account, sid)?;
                let raw = match (&bytes, &path) {
                    (Some(b), _) => b.clone(),
                    (None, Some(p)) if !p.is_empty() => match mailapp::read_emlx(std::path::Path::new(p)) {
                        Ok(r) => r,
                        Err(_) => continue, // moved or gone: the next ingest pass fixes it
                    },
                    _ => continue,
                };
                let Ok(p) = mime::parse(&raw) else { continue };
                let mut m = build::message(
                    &p,
                    SourceFacts {
                        account: old.account.clone(),
                        source_id: old.source_id.clone(),
                        thread_id: old.thread_id.clone(),
                        location: old.location.clone(),
                        in_inbox: old.in_inbox,
                        read: old.read,
                        flagged: old.flagged,
                        size: old.size,
                        received: old.date,
                    },
                );
                // Attachment sizes from disk (partial .emlx) are kept.
                if m.attachments.len() == old.attachments.len() {
                    for (new, prev) in m.attachments.iter_mut().zip(&old.attachments) {
                        new.size = new.size.max(prev.size);
                    }
                }
                let raw_arg = match (&bytes, &path) {
                    (Some(b), _) => Raw::Bytes(b),
                    (None, Some(p)) => Raw::Path(p),
                    _ => Raw::None,
                };
                db.upsert(&m, raw_arg, TEXT_VERSION)?;
                done += 1;
            }
            Ok(())
        })?;
    }
    Ok(done)
}

/// A message built from what Mail's Envelope Index knows, for bodies Mail
/// hasn't downloaded.
fn envelope_only(row: &MailRow) -> Parsed {
    Parsed {
        from: row.sender.clone(),
        to: row.to.clone(),
        cc: row.cc.clone(),
        subject: row.subject.clone(),
        date: row.date_received,
        message_id: row.message_id.clone(),
        ..Parsed::default()
    }
}

/// Same message, and its text is current? A different Message-ID at the same
/// ROWID means Mail rebuilt its index: treat it as a new message.
fn same_message(fp: &Fingerprint, message_id: &str) -> bool {
    fp.text_version == TEXT_VERSION && (message_id.is_empty() || fp.message_id == message_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use keryx_core::config::Config;
    use std::path::{Path, PathBuf};

    fn fixture_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/mailapp/V10")
    }

    fn cfg() -> Config {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config/config.test.toml");
        Config::load(&path).unwrap()
    }

    /// A private copy of the fixture store that tests may modify.
    fn copy_store() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("V10");
        copy_dir(&fixture_root(), &dst);
        (dir, dst)
    }

    fn copy_dir(src: &Path, dst: &Path) {
        std::fs::create_dir_all(dst).unwrap();
        for e in std::fs::read_dir(src).unwrap() {
            let e = e.unwrap();
            let to = dst.join(e.file_name());
            if e.file_type().unwrap().is_dir() {
                copy_dir(&e.path(), &to);
            } else {
                std::fs::copy(e.path(), to).unwrap();
            }
        }
    }

    #[test]
    fn mirrors_all_mail_accounts() {
        let cfg = cfg();
        let store = MailStore::open(&fixture_root()).unwrap();
        let mut db = Db::open_in_memory().unwrap();
        for id in ["oxford", "oqts", "oqts-shared"] {
            let r = mailapp_account(&mut db, &store, cfg.account(id).unwrap(), None).unwrap();
            assert!(r.skipped.is_empty(), "{id}: {:?}", r.skipped);
        }
        assert_eq!(db.count(Some("oxford")).unwrap(), 20);
        assert_eq!(db.count(Some("oqts")).unwrap(), 2);
        assert_eq!(db.count(Some("oqts-shared")).unwrap(), 1);
        // Mail's copy of the personal Gmail account is never read.
        assert_eq!(db.count(Some("personal")).unwrap(), 0);

        let m = db.message("oxford", "12345").unwrap().unwrap();
        assert_eq!(m.subject, "B4 problem sheet 3");
        assert!(m.in_inbox && !m.read);
        assert_eq!(m.attachments.len(), 1);
        assert!(m.attachments[0].size > 100, "partial attachment size comes from disk");
        let oqts = db.message("oqts", "19").unwrap().unwrap();
        assert!(oqts.in_inbox, "Gmail-over-IMAP INBOX label counts as inbox");
        // Not downloaded by Mail yet: envelope only, still listed.
        let pending = db.message("oxford", "23").unwrap().unwrap();
        assert_eq!(pending.subject, "Your e-ticket: Oxford to London, Fri 23 Oct");
        assert_eq!(pending.from.as_ref().unwrap().email, "no-reply@trains.example.co.uk");
        assert!(pending.in_inbox && pending.body_text.is_empty());
        let empty = db.message("oqts", "24").unwrap().unwrap();
        assert_eq!(empty.subject, "Training schedule");
        assert!(!empty.in_inbox);
        let sent = db.message("oxford", "11").unwrap().unwrap();
        assert!(!sent.in_inbox);
        assert_eq!(sent.thread_id, db.message("oxford", "10").unwrap().unwrap().thread_id);
    }

    #[test]
    fn counts_match_mail() {
        let cfg = cfg();
        let store = MailStore::open(&fixture_root()).unwrap();
        let mut db = Db::open_in_memory().unwrap();
        let acc = cfg.account("oxford").unwrap();
        mailapp_account(&mut db, &store, acc, None).unwrap();
        let ours = db.counts_by_mailbox("oxford").unwrap();
        let theirs = store
            .mailbox_totals(acc.mailapp_account_id.as_deref().unwrap())
            .unwrap();
        for (mailbox, n) in theirs {
            if !is_hidden_mailbox(&mailbox) {
                assert_eq!(ours.get(&mailbox).copied().unwrap_or(0), n, "{mailbox}");
            }
        }
    }

    #[test]
    fn second_pass_is_a_no_op() {
        let cfg = cfg();
        let store = MailStore::open(&fixture_root()).unwrap();
        let mut db = Db::open_in_memory().unwrap();
        let acc = cfg.account("oxford").unwrap();
        let first = mailapp_account(&mut db, &store, acc, None).unwrap();
        assert_eq!(first.added, 20);
        let second = mailapp_account(&mut db, &store, acc, None).unwrap();
        assert!(!second.changed(), "{second:?}");
    }

    #[test]
    fn interrupted_pass_converges() {
        let cfg = cfg();
        let store = MailStore::open(&fixture_root()).unwrap();
        let acc = cfg.account("oxford").unwrap();

        let mut clean = Db::open_in_memory().unwrap();
        mailapp_account(&mut clean, &store, acc, None).unwrap();

        let mut db = Db::open_in_memory().unwrap();
        let r = mailapp_account(&mut db, &store, acc, Some(7));
        assert!(matches!(r, Err(IngestError::Interrupted { .. })));
        mailapp_account(&mut db, &store, acc, None).unwrap();
        assert_eq!(db.dump("oxford").unwrap(), clean.dump("oxford").unwrap());
    }

    #[test]
    fn follows_moves_flags_and_deletes() {
        let cfg = cfg();
        let (_dir, root) = copy_store();
        let acc = cfg.account("oxford").unwrap();
        let mut db = Db::open_in_memory().unwrap();
        mailapp_account(&mut db, &MailStore::open(&root).unwrap(), acc, None).unwrap();

        {
            let c = rusqlite::Connection::open(root.join("MailData/Envelope Index")).unwrap();
            let archive: i64 = c
                .query_row("SELECT ROWID FROM mailboxes WHERE url LIKE '%/Archive'", [], |r| {
                    r.get(0)
                })
                .unwrap();
            c.execute("UPDATE messages SET mailbox = ?1 WHERE ROWID = 1", [archive])
                .unwrap();
            c.execute("UPDATE messages SET flagged = 1, read = 1 WHERE ROWID = 2", [])
                .unwrap();
            c.execute("UPDATE messages SET deleted = 1 WHERE ROWID = 3", [])
                .unwrap();
        }
        let r = mailapp_account(&mut db, &MailStore::open(&root).unwrap(), acc, None).unwrap();
        assert_eq!((r.added, r.updated, r.removed), (0, 2, 1), "{r:?}");
        assert!(!db.message("oxford", "1").unwrap().unwrap().in_inbox);
        let m2 = db.message("oxford", "2").unwrap().unwrap();
        assert!(m2.flagged && m2.read);
        assert!(db.message("oxford", "3").unwrap().is_none());
    }

    #[test]
    fn index_rebuild_is_reingested() {
        let cfg = cfg();
        let (_dir, root) = copy_store();
        let acc = cfg.account("oxford").unwrap();
        let mut db = Db::open_in_memory().unwrap();
        mailapp_account(&mut db, &MailStore::open(&root).unwrap(), acc, None).unwrap();
        let before = db.message("oxford", "12").unwrap().unwrap();
        assert_eq!(before.subject, "Hello");

        // Mail rebuilt its index: ROWID 12 and 13 swapped messages.
        let idx = MailStore::open(&root)
            .unwrap()
            .emlx_index(acc.mailapp_account_id.as_deref().unwrap())
            .unwrap();
        let (p12, p13) = (idx[&12].clone(), idx[&13].clone());
        let tmp = p12.with_extension("tmp");
        std::fs::rename(&p12, &tmp).unwrap();
        std::fs::rename(&p13, &p12).unwrap();
        std::fs::rename(&tmp, &p13).unwrap();
        {
            let c = rusqlite::Connection::open(root.join("MailData/Envelope Index")).unwrap();
            c.execute_batch(
                "UPDATE message_global_data SET message_id_header = '<swap>' WHERE ROWID = 12;
                 UPDATE message_global_data SET message_id_header = '<hello-1@example.com>' WHERE ROWID = 13;
                 UPDATE message_global_data SET message_id_header = '<hello-2@example.org>' WHERE ROWID = 12;",
            )
            .unwrap();
        }
        let r = mailapp_account(&mut db, &MailStore::open(&root).unwrap(), acc, None).unwrap();
        assert_eq!(r.updated, 2, "{r:?}");
        assert_eq!(
            db.message("oxford", "12").unwrap().unwrap().message_id,
            "hello-2@example.org"
        );
        assert_eq!(
            db.message("oxford", "13").unwrap().unwrap().message_id,
            "hello-1@example.com"
        );
    }

    #[test]
    fn body_arriving_later_replaces_the_envelope() {
        let cfg = cfg();
        let (_dir, root) = copy_store();
        let acc = cfg.account("oxford").unwrap();
        let mut db = Db::open_in_memory().unwrap();
        mailapp_account(&mut db, &MailStore::open(&root).unwrap(), acc, None).unwrap();
        assert!(db.message("oxford", "23").unwrap().unwrap().body_text.is_empty());

        // Mail downloads the body.
        let idx = MailStore::open(&root)
            .unwrap()
            .emlx_index(acc.mailapp_account_id.as_deref().unwrap())
            .unwrap();
        let dir = idx[&1].parent().unwrap().to_path_buf();
        let raw =
            std::fs::read(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/eml/17-noreply-receipt.eml"))
                .unwrap();
        let mut emlx = format!("{}\n", raw.len()).into_bytes();
        emlx.extend_from_slice(&raw);
        std::fs::write(dir.join("23.emlx"), emlx).unwrap();

        let r = mailapp_account(&mut db, &MailStore::open(&root).unwrap(), acc, None).unwrap();
        assert_eq!(r.updated, 1, "{r:?}");
        assert!(
            db.message("oxford", "23")
                .unwrap()
                .unwrap()
                .body_text
                .contains("Ref ABC123")
        );
        let r = mailapp_account(&mut db, &MailStore::open(&root).unwrap(), acc, None).unwrap();
        assert!(!r.changed());
    }

    #[test]
    fn rederive_upgrades_old_text_without_touching_state() {
        let cfg = cfg();
        let store = MailStore::open(&fixture_root()).unwrap();
        let mut db = Db::open_in_memory().unwrap();
        mailapp_account(&mut db, &store, cfg.account("oxford").unwrap(), None).unwrap();
        // Pretend message 2 was stored by an older keryx, and flagged since.
        db.conn()
            .execute(
                "UPDATE messages SET text_version = 1, body_text = 'old', flagged = 1 WHERE source_id = '2'",
                [],
            )
            .unwrap();
        assert_eq!(rederive(&mut db).unwrap(), 1);
        let m = db.message("oxford", "2").unwrap().unwrap();
        assert!(m.body_text.starts_with("Submission portal"), "{}", m.body_text);
        assert!(m.flagged, "state is kept");
        assert_eq!(rederive(&mut db).unwrap(), 0);
        // Header-only rows (version 0) are ingest's business.
        assert!(db.message("oxford", "23").unwrap().unwrap().body_text.is_empty());
    }

    #[test]
    fn location_drops_hidden_mailboxes() {
        let row = MailRow {
            rowid: 1,
            physical: "[Gmail]/All Mail".into(),
            labels: vec!["INBOX".into(), "[Gmail]/Spam".into()],
            read: true,
            flagged: false,
            conversation_id: 1,
            message_id: "x".into(),
            date_received: 0,
            size: 0,
            subject: String::new(),
            sender: None,
            to: vec![],
            cc: vec![],
        };
        assert_eq!(
            mailapp_location(&row),
            Some(Location::Mailboxes {
                physical: "[Gmail]/All Mail".into(),
                labels: vec!["INBOX".into()]
            })
        );
        let bin = MailRow {
            physical: "[Gmail]/Bin".into(),
            ..row
        };
        assert_eq!(mailapp_location(&bin), None);
    }
}
