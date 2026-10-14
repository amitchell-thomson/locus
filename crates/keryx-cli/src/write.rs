//! Triage from the command line. On a terminal these are Alec's own edits
//! (applied at once); from scripts or Claude via Bash (no TTY) they get the
//! full policy, so people mail becomes proposals.

use anyhow::{Context, bail};
use keryx_core::config::Config;
use keryx_store::api::{Intent, TriageReq};
use keryx_store::client::{Client, Daemon};
use std::io::IsTerminal;

pub fn on_tty() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

pub fn client(cfg: &Config) -> anyhow::Result<Client> {
    Ok(Client::from_config(cfg)
        .context("can't talk to keryxd")?
        .interactive(on_tty()))
}

/// Resolve `@N` / `account:thread` refs.
fn refs(cfg: &Config, threads: &[String]) -> anyhow::Result<Vec<String>> {
    if threads.is_empty() {
        bail!("which threads? give account:thread or @N (from `keryx inbox` / `keryx search`)");
    }
    threads
        .iter()
        .map(|t| crate::read::resolve_ref(cfg, t).map(|(a, th)| keryx_core::model::thread_ref(&a, &th)))
        .collect()
}

pub fn triage(cfg: &Config, intent: Intent, threads: &[String]) -> anyhow::Result<()> {
    let req = TriageReq {
        intent,
        refs: refs(cfg, threads)?,
        summary: None,
        review_id: None,
    };
    let out = client(cfg)?.triage(&req)?;
    println!("{}", out.summary);
    for (r, why) in &out.skipped {
        println!("  skipped {r}: {why}");
    }
    if out.applied.as_ref().is_some_and(|c| c.error.is_some()) {
        bail!("not applied");
    }
    Ok(())
}

pub fn snooze(cfg: &Config, when: &str, threads: &[String]) -> anyhow::Result<()> {
    let until = keryx_tui::write::parse_when(cfg, keryx_tui::now(cfg), when).map_err(anyhow::Error::msg)?;
    println!("snoozing until {}", keryx_core::display::date_full(until, cfg.tz()));
    triage(cfg, Intent::Snooze { until }, threads)
}

pub fn undo(cfg: &Config, id: Option<&str>) -> anyhow::Result<()> {
    let c = client(cfg)?;
    let cs = match id {
        Some(id) => c.undo(id)?,
        None => c.undo_last()?,
    };
    println!("{} ({})", cs.summary, cs.id);
    Ok(())
}

pub fn changes(cfg: &Config, status: Option<&str>, limit: u32, json: bool) -> anyhow::Result<()> {
    let list = client(cfg)?.changesets(status, limit)?;
    if json {
        println!("{:#}", serde_json::json!({ "schema": 1, "changesets": list }));
        return Ok(());
    }
    for c in list {
        println!(
            "{}  {:<9} {:<7} {}{}",
            c.id,
            c.status.as_str(),
            c.author.as_str(),
            c.summary,
            c.error.map(|e| format!(" — {e}")).unwrap_or_default()
        );
    }
    Ok(())
}

pub fn sync(cfg: &Config) -> anyhow::Result<()> {
    client(cfg)?.sync()?;
    println!("keryxd is syncing every account now");
    Ok(())
}
