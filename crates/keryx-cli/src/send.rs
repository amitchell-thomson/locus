//! Sending from the command line: `reply`, `reply-all`, `forward`,
//! `compose` (need a terminal: $EDITOR, then a y/N confirm), and
//! `approve` / `reject` / `cancel` / `unsubscribe`.

use crate::write::{client, on_tty};
use anyhow::{Context, bail};
use keryx_core::compose::{self, ComposeError};
use keryx_core::config::Config;
use keryx_core::model::Address;
use keryx_store::api::{SendReq, UnsubscribeOutcome};
use keryx_store::client::Daemon;
use std::io::Write;

pub enum Kind {
    Reply,
    ReplyAll,
    Forward,
    Compose(String),
}

fn ask(q: &str) -> anyhow::Result<bool> {
    print!("{q} [y/N] ");
    std::io::stdout().flush()?;
    let mut s = String::new();
    std::io::stdin().read_line(&mut s)?;
    Ok(matches!(s.trim(), "y" | "Y" | "yes"))
}

pub fn compose(cfg: &Config, kind: Kind, thread: Option<&str>) -> anyhow::Result<()> {
    if !on_tty() {
        bail!(
            "sending from the CLI needs a terminal ($EDITOR and a confirm); scripts and Claude draft through MCP instead"
        );
    }
    let (account, thread_ref, mut text) = match kind {
        Kind::Compose(account) => {
            let acc = cfg
                .account(&account)
                .with_context(|| format!("no account {account:?}"))?;
            (
                acc.id.clone(),
                None,
                compose::file(&[], &[], "", acc.signature.as_deref(), ""),
            )
        }
        k => {
            let t = thread.context("which thread? account:thread or @N")?;
            let (a, tid) = crate::read::resolve_ref(cfg, t)?;
            let db = crate::read::open_db(cfg)?;
            let msgs = db.thread(&a, &tid)?;
            let d = match k {
                Kind::Forward => keryx_core::reply::forward(cfg, &msgs)?,
                Kind::ReplyAll => keryx_core::reply::reply(cfg, &msgs, true)?,
                _ => keryx_core::reply::reply(cfg, &msgs, false)?,
            };
            let sig = cfg.account(&d.account).and_then(|x| x.signature.clone());
            let thread = (!matches!(k, Kind::Forward)).then(|| keryx_core::model::thread_ref(&a, &tid));
            (
                d.account.clone(),
                thread,
                compose::file(&d.to, &d.cc, &d.subject, sig.as_deref(), &d.quote),
            )
        }
    };
    let sig = cfg.account(&account).and_then(|a| a.signature.clone());
    let c = loop {
        text = keryx_tui::send::run_editor(&text).map_err(anyhow::Error::msg)?;
        match compose::parse(&text, sig.as_deref(), cfg.send.max_attachments_mb) {
            Ok(c) => break c,
            Err(ComposeError::Empty) => {
                println!("Nothing sent");
                return Ok(());
            }
            Err(e) => {
                eprintln!("{e}");
                text = compose::with_error(&text, &e);
            }
        }
    };
    let list = |v: &[Address]| v.iter().map(Address::display).collect::<Vec<_>>().join("\n         ");
    let from = cfg.account(&account).map(|a| a.address.clone()).unwrap_or_default();
    println!("\nFrom     {from}\nTo       {}", list(&c.to));
    if !c.cc.is_empty() {
        println!("Cc       {}", list(&c.cc));
    }
    if !c.bcc.is_empty() {
        println!("Bcc      {}", list(&c.bcc));
    }
    for p in &c.attach {
        println!("Attach   {}", p.display());
    }
    println!("Subject  {}\n", c.subject);
    if !ask(&format!("Send? ({} s to cancel afterwards)", cfg.send.hold_seconds))? {
        println!("Nothing sent");
        return Ok(());
    }
    let cs = client(cfg)?.send(&SendReq {
        account,
        thread: thread_ref,
        to: c.to,
        cc: c.cc,
        bcc: c.bcc,
        subject: c.subject,
        body: c.body,
        attachments: c.attach.iter().map(|p| p.to_string_lossy().into_owned()).collect(),
    })?;
    for w in &cs.warnings {
        println!("{w}");
    }
    println!(
        "sending in {} s — `keryx cancel {}` to stop it",
        cfg.send.hold_seconds, cs.id
    );
    Ok(())
}

pub fn approve(cfg: &Config, id: &str) -> anyhow::Result<()> {
    let cs = client(cfg)?.approve(id)?;
    println!(
        "approved: sending in {} s — `keryx cancel {}` to stop it",
        cfg.send.hold_seconds, cs.id
    );
    Ok(())
}

pub fn reject(cfg: &Config, id: &str) -> anyhow::Result<()> {
    client(cfg)?.reject(id)?;
    println!("rejected: nothing sent");
    Ok(())
}

pub fn cancel(cfg: &Config, id: &str) -> anyhow::Result<()> {
    client(cfg)?.cancel(id)?;
    println!("cancelled: nothing sent");
    Ok(())
}

pub fn unsubscribe(cfg: &Config, thread: &str) -> anyhow::Result<()> {
    let (a, t) = crate::read::resolve_ref(cfg, thread)?;
    let r = keryx_core::model::thread_ref(&a, &t);
    if on_tty() && !ask("Unsubscribe? (this contacts the sender)")? {
        return Ok(());
    }
    match client(cfg)?.unsubscribe(&r)? {
        UnsubscribeOutcome::Done { .. } => println!("unsubscribed and archived"),
        UnsubscribeOutcome::Mailto { to, .. } => {
            println!("this list unsubscribes by email: `keryx compose {a}` to {to}")
        }
        UnsubscribeOutcome::Link { url } => println!("open this page to unsubscribe: {url}"),
        UnsubscribeOutcome::None => println!("this thread has no unsubscribe link"),
    }
    Ok(())
}
