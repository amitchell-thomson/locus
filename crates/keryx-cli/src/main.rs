//! `keryx`: bare command opens the TUI; subcommands for everything else.

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use keryx_core::build::is_hidden_mailbox;
use keryx_core::config::{Backend, Config};
use keryx_store::db::Db;
use keryx_store::mailapp::{self, MailStore};
use std::path::PathBuf;

mod auth;

#[derive(Parser)]
#[command(
    name = "keryx",
    version,
    about = "Terminal email for all your accounts, with Claude on a leash"
)]
struct Cli {
    /// Config file (default: $KERYX_CONFIG or ~/.config/keryx/config.toml).
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Check config, permissions, the mirror and connections; each problem with a fix.
    Doctor {
        /// List the accounts Mac Mail knows (ids for `mailapp_account_id`).
        #[arg(long)]
        discover: bool,
    },
    /// Authorise keryx for your Gmail (opens the browser once).
    Auth {
        #[command(subcommand)]
        what: AuthWhat,
    },
}

#[derive(Subcommand)]
enum AuthWhat {
    /// Gmail API (scope gmail.modify; never permanent delete).
    Gmail,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let path = cli.config.unwrap_or_else(Config::default_path);
    match cli.cmd {
        Some(Cmd::Doctor { discover: true }) => discover(),
        Some(Cmd::Doctor { discover: false }) => doctor(&path),
        Some(Cmd::Auth { what: AuthWhat::Gmail }) => auth::gmail(),
        None => bail!("the TUI arrives in Phase 3; try `keryx doctor`"),
    }
}

fn discover() -> anyhow::Result<()> {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let accounts = mailapp::discover_accounts(&home.join("Library/Accounts/Accounts4.sqlite"))
        .context("reading macOS accounts (needs Full Disk Access for this terminal)")?;
    let mail_dir = home.join("Library/Mail/V10");
    println!("Mail accounts with a local store (use the id as mailapp_account_id):");
    for (id, user, desc) in accounts {
        if mail_dir.join(&id).is_dir() {
            println!("  {id}  {user:<34} {desc}");
        }
    }
    Ok(())
}

struct Report {
    problems: usize,
}

impl Report {
    fn ok(&self, msg: impl AsRef<str>) {
        println!("✓ {}", msg.as_ref());
    }
    fn bad(&mut self, msg: impl AsRef<str>, fix: impl AsRef<str>) {
        self.problems += 1;
        println!("✗ {}\n    → {}", msg.as_ref(), fix.as_ref());
    }
    fn note(&self, msg: impl AsRef<str>) {
        println!("· {}", msg.as_ref());
    }
}

fn doctor(path: &std::path::Path) -> anyhow::Result<()> {
    let mut r = Report { problems: 0 };
    let cfg = Config::load(path).with_context(|| format!("loading {}", path.display()))?;
    let errs = cfg.validate();
    if errs.is_empty() {
        r.ok(format!(
            "config {} ({} accounts, {} keys, live = {})",
            path.display(),
            cfg.accounts.len(),
            cfg.keys.len(),
            cfg.live
        ));
    } else {
        for e in &errs {
            r.bad(e.to_string(), format!("edit {}", path.display()));
        }
    }

    let mail_accounts: Vec<_> = cfg.accounts.iter().filter(|a| a.backend == Backend::Mailapp).collect();
    let store = if mail_accounts.is_empty() {
        None
    } else {
        match MailStore::open(&cfg.paths.mail_dir) {
            Ok(s) => {
                r.ok("Mail's store is readable (Full Disk Access)");
                Some(s)
            }
            Err(e) => {
                r.bad(
                    e.to_string(),
                    "System Settings → Privacy & Security → Full Disk Access → add your terminal and ~/.cargo/bin/keryxd",
                );
                None
            }
        }
    };
    let mail_running = std::process::Command::new("pgrep")
        .args(["-x", "Mail"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if mail_running {
        r.ok("Mail is running");
    } else {
        r.note("Mail isn't running (fine for reading; keryxd starts it when it needs to act)");
    }

    let db_path = cfg.db_path();
    let db = if db_path.exists() {
        match Db::open_readonly(&db_path) {
            Ok(db) => Some(db),
            Err(e) => {
                r.bad(format!("mirror {}: {e}", db_path.display()), "check keryxd's log");
                None
            }
        }
    } else {
        r.bad(
            format!("no mirror yet at {}", db_path.display()),
            "start keryxd (or run `keryxd --once`)",
        );
        None
    };

    if let Some(db) = &db {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        for acc in &cfg.accounts {
            let n = db.count(Some(&acc.id)).unwrap_or(0);
            match db.sync_state(&acc.id).ok().flatten() {
                Some((_, at)) => {
                    let mins = (now - at) / 60;
                    let line = format!("{}: {n} messages, synced {}", acc.name, ago(mins));
                    if mins > 10 {
                        r.bad(
                            line,
                            "is keryxd running? `launchctl print gui/$(id -u)/com.keryx.daemon`",
                        );
                    } else {
                        r.ok(line);
                    }
                }
                None if acc.backend == Backend::Gmail => {
                    r.bad(
                        format!("{}: never synced", acc.name),
                        "run `keryx auth gmail`, then keryxd syncs it",
                    );
                }
                None => r.bad(format!("{}: never synced", acc.name), "start keryxd"),
            }
        }
        if let Some(store) = &store {
            for acc in &mail_accounts {
                let Some(uuid) = acc.mailapp_account_id.as_deref() else {
                    continue;
                };
                let (Ok(theirs), Ok(ours)) = (store.mailbox_totals(uuid), db.counts_by_mailbox(&acc.id)) else {
                    continue;
                };
                let mut diffs = Vec::new();
                for (mailbox, n) in &theirs {
                    if is_hidden_mailbox(mailbox) {
                        continue;
                    }
                    let mine = ours.get(mailbox).copied().unwrap_or(0);
                    if mine != *n {
                        diffs.push(format!("{mailbox}: Mail {n}, keryx {mine}"));
                    }
                }
                if diffs.is_empty() {
                    r.ok(format!("{}: every mailbox count matches Mail", acc.name));
                } else {
                    r.bad(
                        format!("{}: counts differ from Mail ({})", acc.name, diffs.join("; ")),
                        "wait for the next sync; if it persists, report it",
                    );
                }
            }
        }
    }

    for acc in cfg.accounts.iter().filter(|a| a.backend == Backend::Gmail) {
        let client = keryx_store::keychain::get("keryx-gmail-client").is_ok();
        let refresh = keryx_store::keychain::get("keryx-gmail-refresh").is_ok();
        match (client, refresh) {
            (true, true) => r.ok(format!("{}: Gmail authorised", acc.name)),
            (false, _) => r.bad(
                format!("{}: no Gmail OAuth client in the Keychain", acc.name),
                "docs/setup.md §2",
            ),
            (true, false) => r.bad(format!("{}: Gmail not authorised", acc.name), "run `keryx auth gmail`"),
        }
    }

    match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .and_then(|c| c.get(format!("{}/healthz", cfg.kairos.url)).send())
    {
        Ok(resp) if resp.status().is_success() => r.ok(format!("kairos reachable at {}", cfg.kairos.url)),
        Ok(resp) => r.bad(
            format!("kairos at {} answered {}", cfg.kairos.url, resp.status()),
            "check kairosd",
        ),
        Err(_) => r.bad(
            format!("kairos isn't reachable at {}", cfg.kairos.url),
            "is Tailscale up and kairosd running on compute-node?",
        ),
    }

    if r.problems == 0 {
        println!("\nall good");
        Ok(())
    } else {
        bail!("{} problem(s)", r.problems)
    }
}

fn ago(mins: i64) -> String {
    match mins {
        m if m < 1 => "just now".into(),
        m if m < 60 => format!("{m} min ago"),
        m if m < 60 * 48 => format!("{} h ago", m / 60),
        m => format!("{} days ago", m / 60 / 24),
    }
}
