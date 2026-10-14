//! Read model for the TUI and CLI: thread rows per view, search, and the
//! address book. Reads the mirror only (works without keryxd).

use crate::db::{Db, DbError, SELECT_MESSAGE, row_to_message};
use keryx_core::config::{Backend, Config};
use keryx_core::model::{Address, Location, Message};
use keryx_core::query::{Has, In, Is, Query};
use rusqlite::types::Value;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Which list the TUI shows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum View {
    #[default]
    Inbox,
    Flagged,
    Sent,
    /// Everything not in the inbox.
    Archive,
    All,
    /// Threads filed under a key.
    Key(String),
    Snoozed,
    NeedsReply,
}

impl View {
    pub fn title(&self) -> String {
        match self {
            View::Inbox => "Inbox".into(),
            View::Flagged => "Flagged".into(),
            View::Sent => "Sent".into(),
            View::Archive => "Archive".into(),
            View::All => "All mail".into(),
            View::Key(k) => k.clone(),
            View::Snoozed => "Snoozed".into(),
            View::NeedsReply => "Needs reply".into(),
        }
    }
}

/// One line of a list: a thread, described by its latest message.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ThreadRow {
    pub account: String,
    pub thread_id: String,
    /// `account:thread_id`.
    pub r#ref: String,
    pub subject: String,
    /// Who the thread is with (latest other person; "me" if only Alec).
    pub who: String,
    pub snippet: String,
    /// Unix seconds of the latest message.
    pub date: i64,
    pub count: u32,
    pub unread: bool,
    pub flagged: bool,
    pub has_attachment: bool,
    pub in_inbox: bool,
    /// Key names the thread is filed under.
    pub keys: Vec<String>,
    /// Latest message's source id (for opening / previews).
    pub latest: String,
    /// Snoozed until (Unix seconds).
    pub snoozed_until: Option<i64>,
    /// Why it needs a reply (Claude's or Alec's one line).
    pub needs_reply: Option<String>,
}

/// Gmail label id → name per account, from sync state.
pub fn label_names(db: &Db, cfg: &Config) -> HashMap<String, BTreeMap<String, String>> {
    let mut out = HashMap::new();
    for a in cfg.accounts.iter().filter(|a| a.backend == Backend::Gmail) {
        if let Ok(Some((s, _))) = db.sync_state(&a.id)
            && let Ok(st) = serde_json::from_str::<crate::gmail::State>(&s)
        {
            out.insert(a.id.clone(), st.labels);
        }
    }
    out
}

/// Names of the keys a location is filed under.
pub fn keys_of(cfg: &Config, account: &str, loc: &Location, labels: Option<&BTreeMap<String, String>>) -> Vec<String> {
    let names: Vec<String> = loc
        .all()
        .iter()
        .map(|m| labels.and_then(|l| l.get(*m)).cloned().unwrap_or_else(|| m.to_string()))
        .collect();
    cfg.keys
        .iter()
        .filter(|k| k.folders.get(account).is_some_and(|f| names.iter().any(|n| n == f)))
        .map(|k| k.name.clone())
        .collect()
}

/// Lowercased addresses Alec has sent to (for automated detection and the
/// new-recipient warning).
pub fn sent_to(db: &Db, cfg: &Config) -> Result<HashSet<String>, DbError> {
    let own = cfg.own_addresses();
    let mut out = HashSet::new();
    let mut st = db
        .conn()
        .prepare("SELECT from_json, to_json, cc_json, bcc_json FROM messages WHERE from_json IS NOT NULL")?;
    let rows = st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })?;
    for row in rows {
        let (from, to, cc, bcc) = row?;
        let from: Address = serde_json::from_str(&from)?;
        if !own.contains(&from.key()) {
            continue;
        }
        for list in [to, cc, bcc] {
            let v: Vec<Address> = serde_json::from_str(&list)?;
            out.extend(v.iter().map(Address::key).filter(|k| !own.contains(k)));
        }
    }
    Ok(out)
}

/// Filters applied to every listing.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    /// Only these accounts (empty = all).
    pub accounts: Vec<String>,
    pub limit: u32,
}

fn account_clause(f: &Filter, args: &mut Vec<Value>) -> String {
    if f.accounts.is_empty() {
        return "1".into();
    }
    let marks: Vec<String> = f
        .accounts
        .iter()
        .map(|a| {
            args.push(Value::Text(a.clone()));
            "?".to_string()
        })
        .collect();
    format!("account IN ({})", marks.join(","))
}

/// SQL selecting the (account, thread_id) pairs that belong in a view.
fn view_threads(cfg: &Config, view: &View, f: &Filter, args: &mut Vec<Value>) -> String {
    let acc = account_clause(f, args);
    match view {
        View::Inbox => format!("SELECT account, thread_id FROM messages WHERE {acc} AND in_inbox = 1"),
        View::Flagged => format!("SELECT account, thread_id FROM messages WHERE {acc} AND flagged = 1"),
        View::All => format!("SELECT account, thread_id FROM messages WHERE {acc}"),
        View::Archive => format!(
            "SELECT account, thread_id FROM messages WHERE {acc} GROUP BY account, thread_id HAVING max(in_inbox) = 0"
        ),
        View::Sent => {
            let own: Vec<String> = cfg.own_addresses().into_iter().collect();
            let marks: Vec<String> = own
                .iter()
                .map(|a| {
                    args.push(Value::Text(a.clone()));
                    "?".to_string()
                })
                .collect();
            format!(
                "SELECT account, thread_id FROM messages WHERE {acc}
                 AND lower(json_extract(from_json, '$.email')) IN ({})",
                if marks.is_empty() { "''".into() } else { marks.join(",") }
            )
        }
        View::Key(k) => {
            // Matched in Rust (Gmail labels need id → name); select all and filter later.
            let _ = k;
            format!("SELECT account, thread_id FROM messages WHERE {acc}")
        }
        View::Snoozed => format!("SELECT account, thread_id FROM snoozes WHERE {acc}"),
        View::NeedsReply => format!("SELECT account, thread_id FROM tags WHERE kind = 'needs_reply' AND {acc}"),
    }
}

/// Rows for a view, newest first.
pub fn list(db: &Db, cfg: &Config, view: &View, f: &Filter) -> Result<Vec<ThreadRow>, DbError> {
    let mut args = Vec::new();
    let threads = view_threads(cfg, view, f, &mut args);
    let rows = rows_for(
        db,
        cfg,
        &threads,
        args,
        if matches!(view, View::Key(_)) { 0 } else { f.limit },
    )?;
    Ok(match view {
        View::Key(k) => {
            let mut v: Vec<ThreadRow> = rows.into_iter().filter(|r| r.keys.iter().any(|x| x == k)).collect();
            if f.limit > 0 {
                v.truncate(f.limit as usize);
            }
            v
        }
        _ => rows,
    })
}

/// Aggregate the threads selected by `threads_sql` into rows.
fn rows_for(db: &Db, cfg: &Config, threads_sql: &str, args: Vec<Value>, limit: u32) -> Result<Vec<ThreadRow>, DbError> {
    let own = cfg.own_addresses();
    let labels = label_names(db, cfg);
    let snoozes: HashMap<(String, String), i64> = db
        .snoozes()?
        .into_iter()
        .map(|s| ((s.account, s.thread_id), s.until))
        .collect();
    let needs: HashMap<(String, String), String> = db
        .tags("needs_reply")?
        .into_iter()
        .map(|t| ((t.account, t.thread_id), t.reason))
        .collect();
    let sql = format!(
        "WITH t AS (SELECT DISTINCT account, thread_id FROM ({threads_sql})),
         agg AS (
           SELECT m.account, m.thread_id, max(m.date) AS last, count(*) AS n,
                  max(m.read = 0) AS unread, max(m.flagged) AS flagged, max(m.in_inbox) AS inbox,
                  max(m.attachments_json != '[]') AS att
           FROM messages m JOIN t ON t.account = m.account AND t.thread_id = m.thread_id
           GROUP BY m.account, m.thread_id
         )
         SELECT agg.account, agg.thread_id, agg.last, agg.n, agg.unread, agg.flagged, agg.inbox, agg.att
         FROM agg ORDER BY agg.last DESC, agg.account, agg.thread_id {}",
        if limit > 0 {
            format!("LIMIT {limit}")
        } else {
            String::new()
        }
    );
    let mut st = db.conn().prepare(&sql)?;
    let heads = st
        .query_map(rusqlite::params_from_iter(args), |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, u32>(3)?,
                r.get::<_, bool>(4)?,
                r.get::<_, bool>(5)?,
                r.get::<_, bool>(6)?,
                r.get::<_, bool>(7)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut msg_st = db.conn().prepare(&format!(
        "{SELECT_MESSAGE} WHERE account = ?1 AND thread_id = ?2 ORDER BY date DESC, id DESC"
    ))?;
    let mut out = Vec::with_capacity(heads.len());
    for (account, thread_id, date, count, unread, flagged, in_inbox, has_attachment) in heads {
        let mut rows = msg_st.query(rusqlite::params![account, thread_id])?;
        let mut msgs: Vec<Message> = Vec::new();
        while let Some(r) = rows.next()? {
            msgs.push(row_to_message(r)?);
        }
        let Some(latest) = msgs.first() else { continue };
        let who = msgs
            .iter()
            .filter_map(|m| m.from.as_ref())
            .find(|a| !own.contains(&a.key()))
            .map(|a| a.short().to_string())
            .or_else(|| {
                // Only Alec in the thread: show who he wrote to.
                latest.to.first().map(|a| format!("to {}", a.short()))
            })
            .unwrap_or_else(|| "me".into());
        let mut keys: Vec<String> = Vec::new();
        for m in &msgs {
            for k in keys_of(cfg, &account, &m.location, labels.get(&account)) {
                if !keys.contains(&k) {
                    keys.push(k);
                }
            }
        }
        out.push(ThreadRow {
            r#ref: keryx_core::model::thread_ref(&account, &thread_id),
            subject: if latest.subject.is_empty() {
                "(no subject)".into()
            } else {
                latest.subject.clone()
            },
            who,
            snippet: latest.snippet.clone(),
            date,
            count,
            unread,
            flagged,
            has_attachment,
            in_inbox,
            keys,
            latest: latest.source_id.clone(),
            snoozed_until: snoozes.get(&(account.clone(), thread_id.clone())).copied(),
            needs_reply: needs.get(&(account.clone(), thread_id.clone())).cloned(),
            account,
            thread_id,
        });
    }
    Ok(out)
}

const SNOOZED: &str =
    "EXISTS (SELECT 1 FROM snoozes s WHERE s.account = messages.account AND s.thread_id = messages.thread_id)";

/// Search with the §7 syntax. `in:snoozed`, `is:snoozed` and `has:task`
/// match nothing until snoozing (Phase 4) and kairos links (Phase 7) exist.
pub fn search(db: &Db, cfg: &Config, q: &Query, f: &Filter) -> Result<Vec<ThreadRow>, DbError> {
    let mut args: Vec<Value> = Vec::new();
    let mut wh: Vec<String> = vec![account_clause(f, &mut args)];
    if let Some(fts) = q.fts() {
        args.push(Value::Text(fts));
        wh.push("id IN (SELECT rowid FROM fts WHERE fts MATCH ?)".into());
    }
    for (field, vals) in [("from_json", &q.from), ("to_json || cc_json || bcc_json", &q.to)] {
        for v in vals {
            args.push(Value::Text(format!("%{}%", v.to_lowercase())));
            wh.push(format!("lower({field}) LIKE ?"));
        }
    }
    if !q.account.is_empty() {
        let marks: Vec<String> = q
            .account
            .iter()
            .map(|a| {
                // Accept ids or display names.
                let id = cfg
                    .accounts
                    .iter()
                    .find(|x| x.id == *a || x.name.eq_ignore_ascii_case(a))
                    .map(|x| x.id.clone())
                    .unwrap_or_else(|| a.clone());
                args.push(Value::Text(id));
                "?".to_string()
            })
            .collect();
        wh.push(format!("account IN ({})", marks.join(",")));
    }
    if let Some(b) = q.before {
        args.push(Value::Integer(b));
        wh.push("date < ?".into());
    }
    if let Some(a) = q.after {
        args.push(Value::Integer(a));
        wh.push("date >= ?".into());
    }
    let own: Vec<String> = cfg.own_addresses().into_iter().collect();
    for i in &q.in_ {
        match i {
            In::Inbox => wh.push("in_inbox = 1".into()),
            In::Archive => wh.push("in_inbox = 0".into()),
            In::Flagged => wh.push("flagged = 1".into()),
            In::Snoozed => wh.push(SNOOZED.into()),
            In::Sent => {
                let marks: Vec<String> = own
                    .iter()
                    .map(|a| {
                        args.push(Value::Text(a.clone()));
                        "?".to_string()
                    })
                    .collect();
                wh.push(format!(
                    "lower(json_extract(from_json, '$.email')) IN ({})",
                    marks.join(",")
                ));
            }
        }
    }
    for i in &q.is {
        wh.push(
            match i {
                Is::Unread => "read = 0",
                Is::Read => "read = 1",
                Is::Flagged => "flagged = 1",
                Is::Snoozed => SNOOZED,
            }
            .into(),
        );
    }
    for h in &q.has {
        wh.push(
            match h {
                Has::Attachment => "attachments_json != '[]'",
                Has::Task => "0",
            }
            .into(),
        );
    }
    let threads = format!("SELECT account, thread_id FROM messages WHERE {}", wh.join(" AND "));
    let rows = rows_for(db, cfg, &threads, args, if q.key.is_empty() { f.limit } else { 0 })?;
    if q.key.is_empty() {
        return Ok(rows);
    }
    let mut v: Vec<ThreadRow> = rows
        .into_iter()
        .filter(|r| q.key.iter().all(|k| r.keys.iter().any(|x| x.eq_ignore_ascii_case(k))))
        .collect();
    if f.limit > 0 {
        v.truncate(f.limit as usize);
    }
    Ok(v)
}

/// An attachment's bytes, from the stored raw message (Gmail), the
/// `.emlx` (Mail.app), or Mail's `Attachments/` folder for partial ones.
pub fn attachment_bytes(db: &Db, m: &Message, a: &keryx_core::model::Attachment) -> Option<Vec<u8>> {
    match db.raw(&m.account, &m.source_id).ok()? {
        (Some(raw), _) => keryx_core::mime::attachment_bytes(&raw, a.id),
        (None, Some(path)) if !path.is_empty() => {
            let p = std::path::Path::new(&path);
            if crate::mailapp::is_partial(p) {
                let rowid = m.source_id.parse::<i64>().ok()?;
                std::fs::read(crate::mailapp::attachment_file(p, rowid, &a.name)?).ok()
            } else {
                crate::mailapp::read_emlx(p)
                    .ok()
                    .and_then(|raw| keryx_core::mime::attachment_bytes(&raw, a.id))
            }
        }
        _ => None,
    }
}

/// One address-book entry.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Contact {
    pub name: Option<String>,
    pub email: String,
    /// Messages Alec sent to them.
    pub sent: u32,
    /// Messages from them.
    pub received: u32,
    pub last: i64,
}

/// People Alec corresponds with, most-written-to first; `prefix` matches the
/// start of the name, any name word, or the address.
pub fn addresses(db: &Db, cfg: &Config, prefix: &str, limit: usize) -> Result<Vec<Contact>, DbError> {
    let own = cfg.own_addresses();
    let mut book: HashMap<String, Contact> = HashMap::new();
    let mut st = db
        .conn()
        .prepare("SELECT from_json, to_json, cc_json, date, list_headers_json FROM messages")?;
    let rows = st.query_map([], |r| {
        Ok((
            r.get::<_, Option<String>>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, String>(4)?,
        ))
    })?;
    for row in rows {
        let (from, to, cc, date, lh) = row?;
        let Some(from) = from else { continue };
        let from: Address = serde_json::from_str(&from)?;
        let mut touch = |a: &Address, sent: bool| {
            let k = a.key();
            if own.contains(&k) {
                return;
            }
            let e = book.entry(k).or_insert_with(|| Contact {
                name: a.name.clone(),
                email: a.email.clone(),
                sent: 0,
                received: 0,
                last: 0,
            });
            if sent {
                e.sent += 1;
            } else {
                e.received += 1;
            }
            if date > e.last {
                e.last = date;
                if a.name.is_some() {
                    e.name = a.name.clone();
                }
            }
        };
        if own.contains(&from.key()) {
            let to: Vec<Address> = serde_json::from_str(&to)?;
            let cc: Vec<Address> = serde_json::from_str(&cc)?;
            for a in to.iter().chain(&cc) {
                touch(a, true);
            }
        } else if lh == "[]" {
            // People mail only: newsletters don't belong in the address book.
            touch(&from, false);
        }
    }
    let p = prefix.trim().to_lowercase();
    let mut v: Vec<Contact> = book
        .into_values()
        .filter(|c| {
            p.is_empty()
                || c.email.to_lowercase().starts_with(&p)
                || c.name.as_ref().is_some_and(|n| {
                    let n = n.to_lowercase();
                    n.starts_with(&p) || n.split_whitespace().any(|w| w.starts_with(&p))
                })
        })
        .collect();
    v.sort_by(|a, b| {
        b.sent
            .cmp(&a.sent)
            .then(b.received.cmp(&a.received))
            .then(b.last.cmp(&a.last))
    });
    v.truncate(limit);
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    use crate::testkit::fixture_db;

    fn subjects(rows: &[ThreadRow]) -> Vec<&str> {
        rows.iter().map(|r| r.subject.as_str()).collect()
    }

    #[test]
    fn inbox_is_newest_first_across_accounts() {
        let (cfg, db) = fixture_db();
        let rows = list(&db, &cfg, &View::Inbox, &Filter::default()).unwrap();
        assert!(rows.windows(2).all(|w| w[0].date >= w[1].date));
        let accounts: HashSet<&str> = rows.iter().map(|r| r.account.as_str()).collect();
        assert_eq!(accounts, ["oxford", "oqts", "oqts-shared", "personal"].into());
        // Thread a/b is one row with 2 messages; Alec's reply is the latest.
        let t = rows.iter().find(|r| r.subject == "Re: Group project plan").unwrap();
        assert_eq!(t.count, 2);
        assert_eq!(t.who, "Jane Doe");
        assert!(rows.iter().all(|r| r.in_inbox));
        assert!(!subjects(&rows).contains(&"Room booking"), "archived");
    }

    #[test]
    fn account_filter_and_limit() {
        let (cfg, db) = fixture_db();
        let f = Filter {
            accounts: vec!["personal".into()],
            limit: 2,
        };
        let rows = list(&db, &cfg, &View::Inbox, &f).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.account == "personal"));
    }

    #[test]
    fn other_views() {
        let (cfg, db) = fixture_db();
        let flagged = list(&db, &cfg, &View::Flagged, &Filter::default()).unwrap();
        assert!(subjects(&flagged).contains(&"RE: Reimbursement"));
        assert!(subjects(&flagged).contains(&"Your account needs attention"));
        let sent = list(&db, &cfg, &View::Sent, &Filter::default()).unwrap();
        assert_eq!(subjects(&sent), vec!["Re: Group project plan"]);
        let archive = list(&db, &cfg, &View::Archive, &Filter::default()).unwrap();
        assert!(subjects(&archive).contains(&"Fwd: Room booking"));
        assert!(archive.iter().all(|r| !r.in_inbox));
        // Keys: Oxford "Admin" mailbox, Gmail "Admin & Receipts" label (Label_1).
        let admin = list(&db, &cfg, &View::Key("Admin".into()), &Filter::default()).unwrap();
        let mut s = subjects(&admin);
        s.sort();
        assert_eq!(s, vec!["Your appointment is confirmed", "Your receipt"]);
    }

    #[test]
    fn search_operators() {
        let (cfg, db) = fixture_db();
        let now = chrono_tz::Europe::London
            .with_ymd_and_hms(2026, 10, 14, 15, 0, 0)
            .unwrap();
        let s = |q: &str| {
            let q = keryx_core::query::parse(q, now).unwrap();
            subjects(&search(&db, &cfg, &q, &Filter::default()).unwrap())
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            s("munchen"),
            vec!["Grüße"],
            "diacritics folded; Mail's copy of Gmail is ignored"
        );
        assert_eq!(s("from:jane subject:project"), vec!["Re: Group project plan"]);
        assert_eq!(s("to:team@example.com"), vec!["Re: Group project plan"]);
        assert!(s("has:attachment").contains(&"B4 problem sheet 3".to_string()));
        assert_eq!(s("account:OQTS is:unread"), vec!["Poster for the talk"]);
        assert_eq!(s("key:admin"), s("key:Admin"));
        assert!(s("in:sent").contains(&"Re: Group project plan".to_string()));
        assert!(s("after:2026-10-14").contains(&"Your e-ticket: Oxford to London, Fri 23 Oct".to_string()));
        assert!(!s("before:2026-10-14").contains(&"Your e-ticket: Oxford to London, Fri 23 Oct".to_string()));
        assert!(s("in:snoozed").is_empty());
        assert!(s("\"no such phrase anywhere\"").is_empty());
    }

    #[test]
    fn snoozed_and_needs_reply_views() {
        let (cfg, db) = fixture_db();
        let rows = list(&db, &cfg, &View::Inbox, &Filter::default()).unwrap();
        let (a, b) = (&rows[0], &rows[1]);
        db.set_snooze(
            &crate::state::Snooze {
                account: a.account.clone(),
                thread_id: a.thread_id.clone(),
                until: 99,
                created_at: 1,
                created_by: "alec".into(),
            },
            "cs",
        )
        .unwrap();
        db.set_tag(&crate::state::Tag {
            account: b.account.clone(),
            thread_id: b.thread_id.clone(),
            kind: "needs_reply".into(),
            reason: "asks about Thursday".into(),
            author: "claude".into(),
            created_at: 1,
        })
        .unwrap();
        let s = list(&db, &cfg, &View::Snoozed, &Filter::default()).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].snoozed_until, Some(99));
        let n = list(&db, &cfg, &View::NeedsReply, &Filter::default()).unwrap();
        assert_eq!(n[0].needs_reply.as_deref(), Some("asks about Thursday"));
        let now = chrono_tz::Europe::London
            .with_ymd_and_hms(2026, 10, 14, 15, 0, 0)
            .unwrap();
        let q = keryx_core::query::parse("is:snoozed", now).unwrap();
        assert_eq!(search(&db, &cfg, &q, &Filter::default()).unwrap().len(), 1);
    }

    #[test]
    fn address_book() {
        let (cfg, db) = fixture_db();
        let all = addresses(&db, &cfg, "", 100).unwrap();
        // Alec wrote to Jane, Sam and Team; they rank first.
        let top: Vec<&str> = all.iter().take(3).map(|c| c.email.as_str()).collect();
        assert!(
            top.contains(&"jane.doe@example.com") && top.contains(&"sam@example.com"),
            "{top:?}"
        );
        assert!(
            all.iter()
                .all(|c| !cfg.own_addresses().contains(&c.email.to_lowercase()))
        );
        assert!(
            !all.iter().any(|c| c.email == "newsletter@weekly.example.com"),
            "lists excluded"
        );
        assert_eq!(
            addresses(&db, &cfg, "doe", 10).unwrap()[0].email,
            "jane.doe@example.com"
        );
        assert_eq!(addresses(&db, &cfg, "SAM", 10).unwrap()[0].email, "sam@example.com");
        assert_eq!(sent_to(&db, &cfg).unwrap().len(), 3);
    }
}
