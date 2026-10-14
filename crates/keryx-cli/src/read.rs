//! Read commands: `inbox`, `search`, `show`, `addresses` (text or `--json`,
//! schema in docs/json.md). They read the mirror directly.

use anyhow::{Context, anyhow, bail};
use chrono::TimeZone;
use keryx_core::config::Config;
use keryx_core::display::{date_full, date_label};
use keryx_core::model::{Address, Message, parse_thread_ref};
use keryx_store::db::Db;
use keryx_store::view::{self, Filter, ThreadRow, View};
use serde_json::{Value, json};

pub const SCHEMA: u32 = 1;

pub fn open_db(cfg: &Config) -> anyhow::Result<Db> {
    let path = cfg.db_path();
    if !path.exists() {
        bail!(
            "no mirror at {} yet: start keryxd (scripts/install-daemon.sh) or run `keryxd --once`",
            path.display()
        );
    }
    Db::open_readonly(&path).with_context(|| format!("opening {}", path.display()))
}

fn iso(cfg: &Config, ts: i64) -> String {
    cfg.tz()
        .timestamp_opt(ts, 0)
        .single()
        .map(|t| t.to_rfc3339())
        .unwrap_or_default()
}

fn row_json(cfg: &Config, r: &ThreadRow, n: usize) -> Value {
    json!({
        "n": n,
        "ref": r.r#ref,
        "account": r.account,
        "thread_id": r.thread_id,
        "subject": r.subject,
        "who": r.who,
        "snippet": r.snippet,
        "date": iso(cfg, r.date),
        "count": r.count,
        "unread": r.unread,
        "flagged": r.flagged,
        "has_attachment": r.has_attachment,
        "in_inbox": r.in_inbox,
        "keys": r.keys,
    })
}

/// Remember the last listing so `@3` refers to its third row.
fn save_listing(cfg: &Config, rows: &[ThreadRow]) {
    let refs: Vec<&str> = rows.iter().map(|r| r.r#ref.as_str()).collect();
    if let Ok(s) = serde_json::to_string(&refs) {
        let _ = std::fs::write(cfg.paths.data_dir.join("cli-last-listing.json"), s);
    }
}

/// `account:thread` or `@N` (from the last listing).
pub fn resolve_ref(cfg: &Config, r: &str) -> anyhow::Result<(String, String)> {
    if let Some(n) = r.strip_prefix('@') {
        let n: usize = n
            .parse()
            .map_err(|_| anyhow!("{r}: use @1, @2 … from the last listing"))?;
        let s = std::fs::read_to_string(cfg.paths.data_dir.join("cli-last-listing.json"))
            .map_err(|_| anyhow!("{r}: no previous listing; run `keryx inbox` or `keryx search` first"))?;
        let refs: Vec<String> = serde_json::from_str(&s)?;
        let full = refs
            .get(n.wrapping_sub(1))
            .ok_or_else(|| anyhow!("{r}: the last listing had {} rows", refs.len()))?;
        return resolve_ref(cfg, full);
    }
    let (a, t) = parse_thread_ref(r).ok_or_else(|| anyhow!("{r}: expected account:thread or @N"))?;
    if cfg.account(a).is_none() {
        bail!("{r}: no account {a:?}");
    }
    Ok((a.to_string(), t.to_string()))
}

fn print_rows(cfg: &Config, rows: &[ThreadRow], json: bool, title: &str, now: chrono::DateTime<chrono_tz::Tz>) {
    save_listing(cfg, rows);
    if json {
        let v = json!({
            "schema": SCHEMA,
            "title": title,
            "threads": rows.iter().enumerate().map(|(i, r)| row_json(cfg, r, i + 1)).collect::<Vec<_>>(),
        });
        println!("{v:#}");
        return;
    }
    if rows.is_empty() {
        println!("Nothing here.");
        return;
    }
    let name = |id: &str| cfg.account(id).map(|a| a.name.clone()).unwrap_or_else(|| id.into());
    for (i, r) in rows.iter().enumerate() {
        let who = if r.count > 1 {
            format!("{} ({})", r.who, r.count)
        } else {
            r.who.clone()
        };
        let mut flags = String::new();
        if r.unread {
            flags.push('●');
        }
        if r.flagged {
            flags.push('⚑');
        }
        println!(
            "@{:<3} {:<11.11} {:<20.20} {:<2} {}  {}",
            i + 1,
            name(&r.account),
            who,
            flags,
            r.subject,
            date_label(r.date, now)
        );
    }
}

pub fn inbox(cfg: &Config, account: Option<&str>, limit: u32, json: bool) -> anyhow::Result<()> {
    let db = open_db(cfg)?;
    let accounts = match account {
        Some(a) => vec![account_id(cfg, a)?],
        None => vec![],
    };
    let rows = view::list(&db, cfg, &View::Inbox, &Filter { accounts, limit })?;
    print_rows(cfg, &rows, json, "inbox", keryx_tui::now(cfg));
    Ok(())
}

pub fn search(cfg: &Config, q: &str, limit: u32, json: bool) -> anyhow::Result<()> {
    let db = open_db(cfg)?;
    let now = keryx_tui::now(cfg);
    let query = keryx_core::query::parse(q, now)?;
    let rows = view::search(
        &db,
        cfg,
        &query,
        &Filter {
            accounts: vec![],
            limit,
        },
    )?;
    print_rows(cfg, &rows, json, &format!("search: {q}"), now);
    Ok(())
}

fn account_id(cfg: &Config, a: &str) -> anyhow::Result<String> {
    cfg.accounts
        .iter()
        .find(|x| x.id == a || x.name.eq_ignore_ascii_case(a))
        .map(|x| x.id.clone())
        .ok_or_else(|| {
            anyhow!(
                "no account {a:?}; accounts: {}",
                cfg.accounts
                    .iter()
                    .map(|x| x.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

fn msg_json(cfg: &Config, m: &Message) -> Value {
    json!({
        "source_id": m.source_id,
        "message_id": m.message_id,
        "from": m.from,
        "to": m.to,
        "cc": m.cc,
        "reply_to": m.reply_to,
        "subject": m.subject,
        "date": iso(cfg, m.date),
        "read": m.read,
        "flagged": m.flagged,
        "in_inbox": m.in_inbox,
        "attachments": m.attachments,
        "hidden_text_removed": m.hidden_text_removed,
        "body": m.body_text,
    })
}

pub fn show(cfg: &Config, r: &str, json: bool) -> anyhow::Result<()> {
    let (account, thread) = resolve_ref(cfg, r)?;
    let db = open_db(cfg)?;
    let msgs = db.thread(&account, &thread)?;
    if msgs.is_empty() {
        bail!("{r}: no such thread in the mirror");
    }
    if json {
        let v = json!({
            "schema": SCHEMA,
            "ref": keryx_core::model::thread_ref(&account, &thread),
            "account": account,
            "messages": msgs.iter().map(|m| msg_json(cfg, m)).collect::<Vec<_>>(),
        });
        println!("{v:#}");
        return Ok(());
    }
    let list = |v: &[Address]| v.iter().map(Address::display).collect::<Vec<_>>().join(", ");
    for (i, m) in msgs.iter().enumerate() {
        if i > 0 {
            println!("\n{}\n", "─".repeat(60));
        }
        println!("From:    {}", m.from.as_ref().map(Address::display).unwrap_or_default());
        println!("To:      {}", list(&m.to));
        if !m.cc.is_empty() {
            println!("Cc:      {}", list(&m.cc));
        }
        println!("Date:    {}", date_full(m.date, cfg.tz()));
        println!("Subject: {}", m.subject);
        if m.hidden_text_removed {
            println!("⚠ hidden text was removed from this message");
        }
        println!();
        println!("{}", m.body_text);
        for a in &m.attachments {
            println!("[attachment] {} ({} bytes)", a.name, a.size);
        }
    }
    Ok(())
}

pub fn addresses(cfg: &Config, prefix: &str, limit: usize, json: bool) -> anyhow::Result<()> {
    let db = open_db(cfg)?;
    let book = view::addresses(&db, cfg, prefix, limit)?;
    if json {
        let v = json!({
            "schema": SCHEMA,
            "contacts": book.iter().map(|c| json!({
                "name": c.name,
                "email": c.email,
                "display": Address { name: c.name.clone(), email: c.email.clone() }.display(),
                "sent": c.sent,
                "received": c.received,
                "last": iso(cfg, c.last),
            })).collect::<Vec<_>>(),
        });
        println!("{v:#}");
    } else {
        for c in book {
            println!(
                "{}",
                Address {
                    name: c.name,
                    email: c.email
                }
                .display()
            );
        }
    }
    Ok(())
}
