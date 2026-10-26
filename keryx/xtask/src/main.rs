//! `cargo keryx-xtask mailapp-fixture [out]`: build a synthetic Mac Mail store
//! (`V10/…`, `MailData/Envelope Index`, `.emlx` / `.partial.emlx`,
//! `Attachments/`) from `fixtures/eml`, using the real Envelope Index column
//! layout (checked against macOS 26, 2026-10-14). No real mail is involved.
//!
//! Layout reproduced:
//! - Exchange (Oxford): messages live in real mailboxes (Inbox, Archive, …).
//! - Gmail over IMAP (OQTS accounts): messages live in `[Gmail]/All Mail`;
//!   INBOX / Sent are virtual mailboxes (`mailboxes.source` = All Mail) with
//!   membership in the `labels` table.
//! - `.emlx` path: `Data/<reversed digits of ROWID/1000>/Messages/<ROWID>.emlx`
//!   (`Data/Messages/` below 1000).

use anyhow::{Context, Result, bail};
use mail_parser::MimeHeaders;
use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};

const STORE_UUID: &str = "87651D8E-EBB1-47B3-8D73-7AE35252A961";
const OXFORD: &str = "74EA59BE-FE1D-4D49-8712-D8A2AB402605";
const OQTS: &str = "09F30390-18E1-4550-AF71-D9A1004D8C0F";
const OQTS_SHARED: &str = "428452FB-23CB-4AFF-B98C-CFBB01D1B7D1";
const PERSONAL: &str = "1FD73A3D-D50A-49FE-B98E-E87E389ED2E0";

/// Real column layout, minus Mail's triggers and indexes.
const SCHEMA: &str = "
CREATE TABLE mailboxes (ROWID INTEGER PRIMARY KEY AUTOINCREMENT, url TEXT COLLATE BINARY NOT NULL,
  total_count INTEGER NOT NULL DEFAULT 0, unread_count INTEGER NOT NULL DEFAULT 0,
  deleted_count INTEGER NOT NULL DEFAULT 0, unseen_count INTEGER NOT NULL DEFAULT 0,
  unread_count_adjusted_for_duplicates INTEGER NOT NULL DEFAULT 0, change_identifier TEXT COLLATE BINARY,
  source INTEGER, alleged_change_identifier TEXT COLLATE BINARY, UNIQUE(url) ON CONFLICT ABORT);
CREATE TABLE messages (ROWID INTEGER PRIMARY KEY AUTOINCREMENT, message_id INTEGER NOT NULL DEFAULT 0,
  global_message_id INTEGER NOT NULL, remote_id INTEGER, document_id TEXT COLLATE BINARY, sender INTEGER,
  subject_prefix TEXT COLLATE BINARY, subject INTEGER NOT NULL, summary INTEGER, date_sent INTEGER,
  date_received INTEGER, mailbox INTEGER NOT NULL, remote_mailbox INTEGER, flags INTEGER NOT NULL DEFAULT 0,
  read INTEGER NOT NULL DEFAULT 0, flagged INTEGER NOT NULL DEFAULT 0, deleted INTEGER NOT NULL DEFAULT 0,
  size INTEGER NOT NULL DEFAULT 0, conversation_id INTEGER NOT NULL DEFAULT 0, date_last_viewed INTEGER,
  list_id_hash INTEGER, unsubscribe_type INTEGER, searchable_message INTEGER, brand_indicator INTEGER,
  display_date INTEGER, flag_color INTEGER, is_urgent INTEGER NOT NULL DEFAULT 0, color TEXT COLLATE BINARY,
  type INTEGER, fuzzy_ancestor INTEGER, automated_conversation INTEGER DEFAULT 0, root_status INTEGER DEFAULT -1);
CREATE TABLE subjects (ROWID INTEGER PRIMARY KEY AUTOINCREMENT, subject TEXT COLLATE RTRIM NOT NULL,
  UNIQUE(subject) ON CONFLICT ABORT);
CREATE TABLE addresses (ROWID INTEGER PRIMARY KEY AUTOINCREMENT, address TEXT COLLATE NOCASE NOT NULL,
  comment TEXT COLLATE BINARY NOT NULL, UNIQUE(address, comment) ON CONFLICT ABORT);
CREATE TABLE recipients (ROWID INTEGER PRIMARY KEY, message INTEGER NOT NULL, address INTEGER NOT NULL,
  type INTEGER, position INTEGER, UNIQUE(message, type, position) ON CONFLICT ABORT);
CREATE TABLE message_global_data (ROWID INTEGER PRIMARY KEY AUTOINCREMENT, message_id INTEGER,
  follow_up_start_date INTEGER, follow_up_end_date INTEGER,
  follow_up_jsonstringformodelevaluationforsuggestions TEXT COLLATE BINARY,
  download_state INTEGER NOT NULL DEFAULT 0, read_later_date INTEGER, send_later_date INTEGER,
  validation_state INTEGER NOT NULL DEFAULT 0, model_category INTEGER, model_subcategory INTEGER,
  category_model_version INTEGER, category_is_temporary INTEGER, model_analytics TEXT COLLATE BINARY,
  model_high_impact INTEGER NOT NULL DEFAULT 0, generated_summary INTEGER, urgent INTEGER,
  message_id_header TEXT COLLATE BINARY, UNIQUE(message_id) ON CONFLICT ABORT);
CREATE TABLE conversations (conversation_id INTEGER PRIMARY KEY AUTOINCREMENT, flags INTEGER NOT NULL DEFAULT 0,
  sync_key TEXT COLLATE BINARY);
CREATE TABLE labels (message_id INTEGER REFERENCES messages(ROWID) ON DELETE CASCADE,
  mailbox_id INTEGER REFERENCES mailboxes(ROWID) ON DELETE CASCADE, PRIMARY KEY(message_id, mailbox_id)) WITHOUT ROWID;
";

struct Mbox {
    account: &'static str,
    scheme: &'static str,
    /// Path below the account, `/`-separated, unescaped.
    path: &'static str,
    /// Physical mailbox this virtual one draws from (index into the list).
    source: Option<usize>,
}

const MAILBOXES: &[Mbox] = &[
    Mbox {
        account: OXFORD,
        scheme: "ews",
        path: "Inbox",
        source: None,
    },
    Mbox {
        account: OXFORD,
        scheme: "ews",
        path: "Archive",
        source: None,
    },
    Mbox {
        account: OXFORD,
        scheme: "ews",
        path: "Sent Items",
        source: None,
    },
    Mbox {
        account: OXFORD,
        scheme: "ews",
        path: "Admin",
        source: None,
    },
    Mbox {
        account: OQTS,
        scheme: "imap",
        path: "[Gmail]/All Mail",
        source: None,
    },
    Mbox {
        account: OQTS,
        scheme: "imap",
        path: "INBOX",
        source: Some(4),
    },
    Mbox {
        account: OQTS,
        scheme: "imap",
        path: "[Gmail]/Sent Mail",
        source: Some(4),
    },
    Mbox {
        account: OQTS_SHARED,
        scheme: "imap",
        path: "[Gmail]/All Mail",
        source: None,
    },
    Mbox {
        account: OQTS_SHARED,
        scheme: "imap",
        path: "INBOX",
        source: Some(7),
    },
    Mbox {
        account: PERSONAL,
        scheme: "imap",
        path: "[Gmail]/All Mail",
        source: None,
    },
    Mbox {
        account: PERSONAL,
        scheme: "imap",
        path: "INBOX",
        source: Some(9),
    },
];

struct Msg {
    rowid: i64,
    eml: &'static str,
    /// Physical mailbox (index into MAILBOXES).
    mailbox: usize,
    /// Virtual mailboxes it also appears in (Gmail labels).
    labels: &'static [usize],
    read: bool,
    flagged: bool,
    conversation: i64,
    /// Store attachments separately (`.partial.emlx` + `Attachments/`).
    partial: bool,
    /// Body not downloaded by Mail: no file, or a 0-byte placeholder.
    body: Body,
}

#[derive(Clone, Copy, PartialEq)]
enum Body {
    Full,
    NoFile,
    Empty,
}

const fn m(rowid: i64, eml: &'static str, mailbox: usize, conversation: i64) -> Msg {
    Msg {
        rowid,
        eml,
        mailbox,
        labels: &[],
        read: true,
        flagged: false,
        conversation,
        partial: false,
        body: Body::Full,
    }
}

fn messages() -> Vec<Msg> {
    vec![
        Msg {
            read: false,
            ..m(1, "01-plain.eml", 0, 1)
        },
        m(2, "03-html-only.eml", 0, 2),
        m(3, "08-encoded-words.eml", 0, 3),
        Msg {
            read: false,
            ..m(4, "11-invite-request.eml", 0, 4)
        },
        m(5, "12-invite-cancel.eml", 0, 4),
        m(6, "14-injection.eml", 0, 5),
        m(7, "14b-escapes.eml", 0, 6),
        Msg {
            flagged: true,
            ..m(8, "15-outlook-quote.eml", 0, 7)
        },
        m(9, "18-mailto-unsubscribe.eml", 0, 8),
        m(10, "19-thread-a.eml", 0, 9),
        m(11, "20-thread-b.eml", 2, 9),
        m(12, "21-hello-1.eml", 0, 10),
        m(13, "22-hello-2.eml", 0, 11),
        m(14, "24-docx-attachment.eml", 0, 12),
        m(15, "25-corrupt-pdf.eml", 0, 13),
        m(16, "06-latin1.eml", 0, 14),
        m(17, "09-forward-rfc822.eml", 1, 15),
        m(18, "02-multipart-alternative.eml", 3, 16),
        Msg {
            labels: &[5],
            read: false,
            ..m(19, "10-inline-image.eml", 4, 17)
        },
        Msg {
            labels: &[8],
            ..m(20, "16-newsletter.eml", 7, 18)
        },
        Msg {
            labels: &[10],
            ..m(21, "05-base64-utf8.eml", 9, 19)
        },
        // A large ROWID exercises the nested Data/<digits>/Messages path.
        Msg {
            partial: true,
            read: false,
            ..m(12345, "23-pdf-attachment.eml", 0, 20)
        },
        // Bodies Mail hasn't downloaded yet: mirrored header-only.
        Msg {
            body: Body::NoFile,
            read: false,
            ..m(23, "17-noreply-receipt.eml", 0, 21)
        },
        Msg {
            body: Body::Empty,
            ..m(24, "18-mailto-unsubscribe.eml", 4, 22)
        },
    ]
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("mailapp-fixture") => {
            let root = workspace_root()?;
            let out = args
                .next()
                .map(PathBuf::from)
                .unwrap_or_else(|| root.join("fixtures/mailapp"));
            build(&root.join("fixtures/eml"), &out)?;
            println!("wrote {}", out.join("V10").display());
            Ok(())
        }
        _ => bail!("usage: cargo keryx-xtask mailapp-fixture [out-dir]"),
    }
}

fn workspace_root() -> Result<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    dir.parent().map(Path::to_path_buf).context("xtask has no parent dir")
}

fn url(mb: &Mbox) -> String {
    let enc: String = mb
        .path
        .split('/')
        .map(|seg| {
            seg.replace('%', "%25")
                .replace('[', "%5B")
                .replace(']', "%5D")
                .replace(' ', "%20")
        })
        .collect::<Vec<_>>()
        .join("/");
    format!("{}://{}/{}", mb.scheme, mb.account, enc)
}

/// `<account>/<a>.mbox/<b>.mbox/<store>/Data/…`
fn mbox_dir(v10: &Path, mb: &Mbox) -> PathBuf {
    let mut p = v10.join(mb.account);
    for seg in mb.path.split('/') {
        p = p.join(format!("{seg}.mbox"));
    }
    p.join(STORE_UUID).join("Data")
}

/// Mail's sharding: the digits of ROWID/1000, least significant first.
pub fn shard(rowid: i64) -> PathBuf {
    let mut p = PathBuf::new();
    let mut n = rowid / 1000;
    while n > 0 {
        p = p.join((n % 10).to_string());
        n /= 10;
    }
    p.join("Messages")
}

struct Hdr {
    from: Option<(String, String)>,
    to: Vec<(String, String)>,
    cc: Vec<(String, String)>,
    subject: String,
    message_id: String,
    date: i64,
}

fn headers(raw: &[u8]) -> Result<Hdr> {
    let msg = mail_parser::MessageParser::default()
        .parse(raw)
        .context("fixture doesn't parse")?;
    let addrs = |a: Option<&mail_parser::Address>| -> Vec<(String, String)> {
        a.map(|a| {
            a.iter()
                .filter_map(|x| {
                    x.address()
                        .map(|ad| (ad.to_string(), x.name().unwrap_or_default().to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
    };
    Ok(Hdr {
        from: addrs(msg.from()).into_iter().next(),
        to: addrs(msg.to()),
        cc: addrs(msg.cc()),
        subject: msg.subject().unwrap_or_default().to_string(),
        message_id: msg.message_id().unwrap_or_default().to_string(),
        date: msg.date().map(|d| d.to_timestamp()).unwrap_or_default(),
    })
}

/// Mail splits "Re: " / "Fwd: " into `subject_prefix`.
fn split_prefix(subject: &str) -> (&str, &str) {
    for p in ["Re: ", "RE: ", "Fwd: ", "FW: "] {
        if let Some(rest) = subject.strip_prefix(p) {
            return (p, rest);
        }
    }
    ("", subject)
}

/// Remove attachment bodies (parts with `Content-Disposition: attachment`),
/// returning the stripped message and the (part number, filename, bytes).
/// A stripped attachment: (MIME part number, filename, bytes).
type Stripped = (usize, String, Vec<u8>);

fn strip_attachments(raw: &[u8]) -> Result<(Vec<u8>, Vec<Stripped>)> {
    let msg = mail_parser::MessageParser::default()
        .parse(raw)
        .context("fixture doesn't parse")?;
    let mut atts = Vec::new();
    let mut ranges = Vec::new();
    for (i, part) in msg.parts.iter().enumerate() {
        if let Some(name) = part.attachment_name() {
            atts.push((i, name.to_string(), part.contents().to_vec()));
            ranges.push((part.offset_body as usize, part.offset_end as usize));
        }
    }
    let mut stripped = raw.to_vec();
    // Back to front, so earlier offsets stay valid.
    for (start, end) in ranges.into_iter().rev() {
        stripped.splice(start..end, std::iter::empty());
    }
    Ok((stripped, atts))
}

fn emlx(body: &[u8], flags: i64, rowid: i64) -> Vec<u8> {
    let mut out = format!("{:<10}\n", body.len()).into_bytes();
    out.extend_from_slice(body);
    out.extend_from_slice(
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n\t<key>date-last-viewed</key>\n\t<integer>0</integer>\n\t<key>date-received</key>\n\t<integer>0</integer>\n\t<key>flags</key>\n\t<integer>{flags}</integer>\n\t<key>remote-id</key>\n\t<string>{rowid}</string>\n</dict>\n</plist>\n"
        )
        .as_bytes(),
    );
    out
}

fn build(eml_dir: &Path, out: &Path) -> Result<()> {
    let v10 = out.join("V10");
    if v10.exists() {
        std::fs::remove_dir_all(&v10).with_context(|| format!("clearing {}", v10.display()))?;
    }
    let data = v10.join("MailData");
    std::fs::create_dir_all(&data)?;
    let db = Connection::open(data.join("Envelope Index"))?;
    db.execute_batch(SCHEMA)?;

    for (i, mb) in MAILBOXES.iter().enumerate() {
        let source = mb.source.map(|s| s as i64 + 1);
        db.execute(
            "INSERT INTO mailboxes (ROWID, url, source) VALUES (?1, ?2, ?3)",
            params![i as i64 + 1, url(mb), source],
        )?;
    }

    let address_id = |db: &Connection, addr: &str, name: &str| -> Result<i64> {
        db.execute(
            "INSERT OR IGNORE INTO addresses (address, comment) VALUES (?1, ?2)",
            params![addr, name],
        )?;
        Ok(db.query_row(
            "SELECT ROWID FROM addresses WHERE address = ?1 AND comment = ?2",
            params![addr, name],
            |r| r.get(0),
        )?)
    };

    for msg in messages() {
        let raw = std::fs::read(eml_dir.join(msg.eml)).with_context(|| format!("reading {}", msg.eml))?;
        let h = headers(&raw)?;
        let (prefix, subject) = split_prefix(&h.subject);
        db.execute("INSERT OR IGNORE INTO subjects (subject) VALUES (?1)", params![subject])?;
        let subject_id: i64 = db.query_row("SELECT ROWID FROM subjects WHERE subject = ?1", params![subject], |r| {
            r.get(0)
        })?;
        db.execute(
            "INSERT OR IGNORE INTO conversations (conversation_id) VALUES (?1)",
            params![msg.conversation],
        )?;
        db.execute(
            "INSERT INTO message_global_data (ROWID, message_id, message_id_header) VALUES (?1, ?1, ?2)",
            params![msg.rowid, format!("<{}>", h.message_id)],
        )?;
        let sender = match &h.from {
            Some((a, n)) => Some(address_id(&db, a, n)?),
            None => None,
        };
        let flags = i64::from(msg.read) | (i64::from(msg.flagged) << 4);
        db.execute(
            "INSERT INTO messages (ROWID, message_id, global_message_id, remote_id, sender, subject_prefix,
               subject, date_sent, date_received, mailbox, flags, read, flagged, size, conversation_id)
             VALUES (?1, ?1, ?1, ?1, ?2, ?3, ?4, ?5, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                msg.rowid,
                sender,
                prefix,
                subject_id,
                h.date,
                msg.mailbox as i64 + 1,
                flags,
                msg.read,
                msg.flagged,
                raw.len() as i64,
                msg.conversation
            ],
        )?;
        for (kind, list) in [(0, &h.to), (1, &h.cc)] {
            for (pos, (a, n)) in list.iter().enumerate() {
                let id = address_id(&db, a, n)?;
                db.execute(
                    "INSERT INTO recipients (message, address, type, position) VALUES (?1, ?2, ?3, ?4)",
                    params![msg.rowid, id, kind, pos as i64],
                )?;
            }
        }
        for l in msg.labels {
            db.execute(
                "INSERT INTO labels (message_id, mailbox_id) VALUES (?1, ?2)",
                params![msg.rowid, *l as i64 + 1],
            )?;
        }

        let dir = mbox_dir(&v10, &MAILBOXES[msg.mailbox]);
        let msg_dir = dir.join(shard(msg.rowid));
        std::fs::create_dir_all(&msg_dir)?;
        if msg.body == Body::NoFile {
            continue;
        }
        if msg.body == Body::Empty {
            std::fs::write(msg_dir.join(format!("{}.emlx", msg.rowid)), emlx(b"", flags, msg.rowid))?;
            continue;
        }
        if msg.partial {
            let (stripped, atts) = strip_attachments(&raw)?;
            std::fs::write(
                msg_dir.join(format!("{}.partial.emlx", msg.rowid)),
                emlx(&stripped, flags, msg.rowid),
            )?;
            // Beside the message's own Messages/ folder, as Mail keeps them
            // (`Data/2/1/Attachments/12345/…` for `Data/2/1/Messages/12345.partial.emlx`).
            let beside = msg_dir.parent().unwrap_or(&dir).to_path_buf();
            for (part, name, bytes) in atts {
                let adir = beside
                    .join("Attachments")
                    .join(msg.rowid.to_string())
                    .join(part.to_string());
                std::fs::create_dir_all(&adir)?;
                std::fs::write(adir.join(name), bytes)?;
            }
        } else {
            std::fs::write(
                msg_dir.join(format!("{}.emlx", msg.rowid)),
                emlx(&raw, flags, msg.rowid),
            )?;
        }
    }
    // Mail keeps counts in mailboxes; the reader compares against them.
    db.execute_batch(
        "UPDATE mailboxes SET
           total_count = (SELECT count(*) FROM messages WHERE mailbox = mailboxes.ROWID)
                       + (SELECT count(*) FROM labels WHERE mailbox_id = mailboxes.ROWID),
           unread_count = (SELECT count(*) FROM messages WHERE mailbox = mailboxes.ROWID AND read = 0)
                        + (SELECT count(*) FROM labels JOIN messages ON messages.ROWID = labels.message_id
                           WHERE mailbox_id = mailboxes.ROWID AND read = 0);",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shard_matches_mail() {
        assert_eq!(shard(133), PathBuf::from("Messages"));
        assert_eq!(shard(7293), PathBuf::from("7/Messages"));
        assert_eq!(shard(12345), PathBuf::from("2/1/Messages"));
    }

    #[test]
    fn urls_are_escaped_like_mail() {
        assert_eq!(url(&MAILBOXES[4]), format!("imap://{OQTS}/%5BGmail%5D/All%20Mail"));
        assert_eq!(url(&MAILBOXES[2]), format!("ews://{OXFORD}/Sent%20Items"));
    }

    #[test]
    fn builds() {
        let root = workspace_root().unwrap();
        let tmp = std::env::temp_dir().join(format!("keryx-xtask-{}", std::process::id()));
        build(&root.join("fixtures/eml"), &tmp).unwrap();
        let db = Connection::open(tmp.join("V10/MailData/Envelope Index")).unwrap();
        let n: i64 = db.query_row("SELECT count(*) FROM messages", [], |r| r.get(0)).unwrap();
        assert_eq!(n as usize, messages().len());
        let partial = mbox_dir(&tmp.join("V10"), &MAILBOXES[0]).join("2/1/Messages/12345.partial.emlx");
        assert!(partial.exists());
        let att = mbox_dir(&tmp.join("V10"), &MAILBOXES[0]).join("2/1/Attachments/12345");
        assert!(att.exists());
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
