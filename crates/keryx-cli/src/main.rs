//! `keryx`: bare command opens the TUI; subcommands for everything else.

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use keryx_core::build::is_hidden_mailbox;
use keryx_core::config::{Backend, Config};
use keryx_store::api::Intent;
use keryx_store::db::Db;
use keryx_store::mailapp::{self, MailStore};
use std::path::PathBuf;

mod auth;
mod read;
mod runs;
mod send;
mod write;

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
    /// Inbox threads, newest first (`@N` refers to row N afterwards).
    Inbox {
        /// One account (id or name).
        #[arg(long)]
        account: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: u32,
        #[arg(long)]
        json: bool,
    },
    /// Search: free text plus from: to: subject: account: key: in: is: has: before: after:
    Search {
        query: String,
        #[arg(long, default_value_t = 50)]
        limit: u32,
        #[arg(long)]
        json: bool,
    },
    /// Print a thread (`account:thread` or `@N`).
    Show {
        thread: String,
        #[arg(long)]
        json: bool,
    },
    /// Archive threads (`@N` or account:thread; several at once).
    Archive { threads: Vec<String> },
    /// Back to the inbox.
    Unarchive { threads: Vec<String> },
    /// Flag threads.
    Flag { threads: Vec<String> },
    /// Remove the flag.
    Unflag { threads: Vec<String> },
    /// Mark threads read.
    Read { threads: Vec<String> },
    /// Mark threads unread.
    Unread { threads: Vec<String> },
    /// Snooze until a time (off for now: [snooze] enabled = false).
    #[command(hide = true)]
    Snooze { when: String, threads: Vec<String> },
    /// File into a key (and out of the inbox).
    File { key: String, threads: Vec<String> },
    /// Undo the last change, or a given changeset.
    Undo { id: Option<String> },
    /// Recent changes (changesets).
    Changes {
        /// pending | applied | undone | failed …
        #[arg(long)]
        status: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: u32,
        #[arg(long)]
        json: bool,
    },
    /// Ask keryxd to sync every account now.
    Sync,
    /// Reply in $EDITOR (needs a terminal), then confirm.
    Reply { thread: String },
    /// Reply to everyone in $EDITOR, then confirm.
    ReplyAll { thread: String },
    /// Forward in $EDITOR, then confirm.
    Forward { thread: String },
    /// Write a new message from an account (personal, oxford, oqts, oqts-shared).
    Compose { account: String },
    /// Approve a draft waiting for you (`keryx changes --status pending`).
    Approve { id: String },
    /// Reject a draft: nothing is sent.
    Reject { id: String },
    /// Stop a send during its hold.
    Cancel { id: String },
    /// Unsubscribe from a thread's mailing list (asks first).
    Unsubscribe { thread: String },
    /// Make a kairos task from a thread (its notes link back here).
    Task {
        thread: String,
        /// Title (default: the thread's subject).
        title: Option<String>,
        /// Deadline: "fri 17:00", "23 oct", 2026-10-23T17:00.
        #[arg(long)]
        due: Option<String>,
        /// Estimate: 2h, 90m, 1h30.
        #[arg(long)]
        estimate: Option<String>,
        /// Task type: sheet | coursework | admin.
        #[arg(long)]
        kind: Option<String>,
        /// kairos calendar (default: the thread's key's, else Personal).
        #[arg(long)]
        key: Option<String>,
    },
    /// Claude's kairos suggestions waiting for you (accept them in the TUI with y).
    Suggestions {
        /// open | accepted | dismissed (default: open).
        #[arg(long, default_value = "open")]
        status: String,
        #[arg(long)]
        json: bool,
    },
    /// Open the TUI on a thread: `keryx open keryx://thread/<account>/<thread_id>`.
    Open { url: String },
    /// A scheduled Claude run now: morning, evening, or auto (by the hour).
    Run {
        kind: String,
        /// Print the prompt and tool list instead of running.
        #[arg(long)]
        dry_run: bool,
    },
    /// A background cleanup: Claude proposes, you review with A in keryx.
    Cleanup {
        /// Only these accounts (repeat for several).
        #[arg(long)]
        account: Vec<String>,
        /// Extra instructions ("be strict with OQTS newsletters").
        instructions: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Claude's runs: when, what they did, what failed.
    Runs {
        #[arg(long)]
        json: bool,
    },
    /// People you correspond with (for address completion).
    Addresses {
        #[arg(default_value = "")]
        prefix: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
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
        Some(Cmd::Inbox { account, limit, json }) => read::inbox(&load(&path)?, account.as_deref(), limit, json),
        Some(Cmd::Search { query, limit, json }) => read::search(&load(&path)?, &query, limit, json),
        Some(Cmd::Show { thread, json }) => read::show(&load(&path)?, &thread, json),
        Some(Cmd::Addresses { prefix, limit, json }) => read::addresses(&load(&path)?, &prefix, limit, json),
        Some(Cmd::Archive { threads }) => write::triage(&load(&path)?, Intent::Archive, &threads),
        Some(Cmd::Unarchive { threads }) => write::triage(&load(&path)?, Intent::Unarchive, &threads),
        Some(Cmd::Flag { threads }) => write::triage(&load(&path)?, Intent::Flag { on: true }, &threads),
        Some(Cmd::Unflag { threads }) => write::triage(&load(&path)?, Intent::Flag { on: false }, &threads),
        Some(Cmd::Read { threads }) => write::triage(&load(&path)?, Intent::Read { on: true }, &threads),
        Some(Cmd::Unread { threads }) => write::triage(&load(&path)?, Intent::Read { on: false }, &threads),
        Some(Cmd::Snooze { when, threads }) => write::snooze(&load(&path)?, &when, &threads),
        Some(Cmd::File { key, threads }) => write::triage(&load(&path)?, Intent::File { key }, &threads),
        Some(Cmd::Undo { id }) => write::undo(&load(&path)?, id.as_deref()),
        Some(Cmd::Changes { status, limit, json }) => write::changes(&load(&path)?, status.as_deref(), limit, json),
        Some(Cmd::Sync) => write::sync(&load(&path)?),
        Some(Cmd::Reply { thread }) => send::compose(&load(&path)?, send::Kind::Reply, Some(&thread)),
        Some(Cmd::ReplyAll { thread }) => send::compose(&load(&path)?, send::Kind::ReplyAll, Some(&thread)),
        Some(Cmd::Forward { thread }) => send::compose(&load(&path)?, send::Kind::Forward, Some(&thread)),
        Some(Cmd::Compose { account }) => send::compose(&load(&path)?, send::Kind::Compose(account), None),
        Some(Cmd::Approve { id }) => send::approve(&load(&path)?, &id),
        Some(Cmd::Reject { id }) => send::reject(&load(&path)?, &id),
        Some(Cmd::Cancel { id }) => send::cancel(&load(&path)?, &id),
        Some(Cmd::Unsubscribe { thread }) => send::unsubscribe(&load(&path)?, &thread),
        Some(Cmd::Task {
            thread,
            title,
            due,
            estimate,
            kind,
            key,
        }) => write::task(&load(&path)?, &thread, title, due, estimate, kind, key),
        Some(Cmd::Suggestions { status, json }) => read::suggestions(&load(&path)?, &status, json),
        Some(Cmd::Run { kind, dry_run }) => {
            let cfg = load(&path)?;
            use chrono::Timelike;
            let evening_h = cfg
                .runs
                .evening
                .split(':')
                .next()
                .and_then(|h| h.parse().ok())
                .unwrap_or(18);
            let k = runs::kind_for(&kind, keryx_tui::now(&cfg).hour(), evening_h)?;
            runs::run(&cfg, k, dry_run, &[], "")
        }
        Some(Cmd::Cleanup {
            account,
            instructions,
            dry_run,
        }) => {
            let cfg = load(&path)?;
            for a in &account {
                if cfg.account(a).is_none() {
                    bail!("no account {a:?}");
                }
            }
            let ids: Vec<String> = account
                .iter()
                .filter_map(|a| cfg.account(a).map(|x| x.id.clone()))
                .collect();
            runs::run(
                &cfg,
                keryx_store::runs::RunKind::Cleanup,
                dry_run,
                &ids,
                instructions.as_deref().unwrap_or(""),
            )
        }
        Some(Cmd::Runs { json }) => runs::list(&load(&path)?, json),
        Some(Cmd::Open { url }) => {
            let (a, t) = keryx_core::model::parse_keryx_url(&url)
                .with_context(|| format!("{url}: expected keryx://thread/<account>/<thread_id>"))?;
            keryx_tui::run_at(load(&path)?, Some((a, t))).map_err(anyhow::Error::msg)
        }
        None => keryx_tui::run(load(&path)?).map_err(anyhow::Error::msg),
    }
}

/// Load and validate the config (every problem listed).
fn load(path: &std::path::Path) -> anyhow::Result<Config> {
    let cfg = Config::load(path).with_context(|| format!("loading {}", path.display()))?;
    let errs = cfg.validate();
    if !errs.is_empty() {
        for e in &errs {
            eprintln!("config: {e}");
        }
        bail!(
            "{} problem(s) in {}; `keryx doctor` explains",
            errs.len(),
            path.display()
        );
    }
    Ok(cfg)
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

    // Phone pushes and scheduled runs.
    if keryx_store::keychain::get("keryx-ntfy-topic").is_ok() {
        r.ok("ntfy topic set (phone pushes on)");
    } else {
        r.note("no keryx-ntfy-topic in the Keychain: phone pushes are off (docs/setup.md §7)");
    }
    if cfg.runs.enabled {
        let claude = std::process::Command::new(&cfg.runs.claude_bin)
            .arg("--version")
            .output();
        match claude {
            Ok(o) if o.status.success() => r.ok(format!(
                "Claude Code for runs: {}",
                String::from_utf8_lossy(&o.stdout).trim()
            )),
            _ => r.bad(
                format!("{} isn't runnable", cfg.runs.claude_bin),
                "install Claude Code, or set [runs] claude_bin to its full path",
            ),
        }
        let agent = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join("Library/LaunchAgents/com.keryx.runs.plist");
        if agent.exists() {
            r.ok("scheduled runs installed (com.keryx.runs)");
        } else {
            r.note("scheduled runs aren't installed yet: scripts/install-runs.sh");
        }
        if let Ok(db) = Db::open_readonly(&cfg.db_path())
            && let Some(last) = db.runs(1).ok().and_then(|v| v.into_iter().next())
        {
            let when = keryx_core::display::date_full(last.started_at, cfg.tz());
            match last.status.as_str() {
                "failed" => r.bad(
                    format!(
                        "last run ({} {when}) failed: {}",
                        last.kind.as_str(),
                        last.error.unwrap_or_default()
                    ),
                    "see ~/Library/Logs/keryx-runs.log and the Runs view (g c)",
                ),
                s => r.ok(format!("last run: {} {when}, {s}", last.kind.as_str())),
            }
        }
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
