//! keryxd: the only writer. Phase 1: keeps the mirror in sync with Mail.app
//! (and Gmail, once authorised). HTTP, MCP and actions arrive in later phases.

use anyhow::{Context, bail};
use clap::Parser;
use keryx_core::config::{Backend, Config};
use keryx_store::db::Db;
use keryx_store::ingest;
use keryx_store::mailapp::MailStore;
use std::path::PathBuf;
use std::time::{Duration, Instant};

mod gmail_sync;
mod jobs;

#[derive(Parser)]
#[command(
    name = "keryxd",
    version,
    about = "keryx daemon: mirror, changesets, actions, HTTP API and MCP"
)]
struct Args {
    /// Config file (default: $KERYX_CONFIG or ~/.config/keryx/config.toml).
    #[arg(long)]
    config: Option<PathBuf>,
    /// Check the config and exit.
    #[arg(long)]
    check: bool,
    /// Sync every account once, print what changed, and exit.
    #[arg(long)]
    once: bool,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("keryxd=info,keryx_store=info")),
        )
        .with_writer(std::io::stderr)
        .init();
    let args = Args::parse();
    let path = args.config.unwrap_or_else(Config::default_path);
    let cfg = Config::load(&path).with_context(|| format!("loading {}", path.display()))?;
    let errs = cfg.validate();
    if !errs.is_empty() {
        for e in &errs {
            eprintln!("config: {e}");
        }
        bail!("{} problem(s) in {}", errs.len(), path.display());
    }
    if args.check {
        println!(
            "config ok: {} account(s), {} key(s), live = {}",
            cfg.accounts.len(),
            cfg.keys.len(),
            cfg.live
        );
        return Ok(());
    }
    let mut db = Db::open(&cfg.db_path()).with_context(|| format!("opening {}", cfg.db_path().display()))?;
    match ingest::rederive(&mut db) {
        Ok(0) => {}
        Ok(n) => tracing::info!(messages = n, "re-derived text with the current rules"),
        Err(e) => tracing::warn!(error = %e, "re-deriving text"),
    }
    if args.once {
        let t = Instant::now();
        sync_mailapp(&cfg, &mut db)?;
        jobs::sync_gmail_all(&cfg, &mut db);
        println!(
            "synced in {:.1}s; mirror holds {} messages",
            t.elapsed().as_secs_f64(),
            db.count(None)?
        );
        return Ok(());
    }
    jobs::run(&cfg, db)
}

/// One pass over every Mail.app account. Errors on one account don't stop the rest.
pub fn sync_mailapp(cfg: &Config, db: &mut Db) -> anyhow::Result<()> {
    let accounts: Vec<_> = cfg.accounts.iter().filter(|a| a.backend == Backend::Mailapp).collect();
    if accounts.is_empty() {
        return Ok(());
    }
    let store = MailStore::open(&cfg.paths.mail_dir)?;
    for acc in accounts {
        let t = Instant::now();
        match ingest::mailapp_account(db, &store, acc, None) {
            Ok(r) => {
                if r.changed() || !r.skipped.is_empty() {
                    tracing::info!(
                        account = %acc.id, added = r.added, updated = r.updated, removed = r.removed,
                        skipped = r.skipped.len(), ms = t.elapsed().as_millis() as u64, "mail.app sync"
                    );
                }
                for s in r.skipped.iter().take(5) {
                    tracing::warn!(account = %acc.id, "skipped {s}");
                }
                db.set_sync_state(&acc.id, "{}", now())?;
            }
            Err(e) => tracing::error!(account = %acc.id, error = %e, "mail.app sync failed"),
        }
    }
    Ok(())
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Sleep in small steps so a shutdown signal is noticed quickly.
pub fn nap(d: Duration) {
    std::thread::sleep(d);
}
