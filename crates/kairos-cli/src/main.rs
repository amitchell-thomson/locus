mod render;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Duration, NaiveDate, NaiveTime, Utc};
use chrono_tz::Tz;
use clap::{Args, Parser, Subcommand};
use kairos_core::api;
use kairos_core::changeset::{Status, WriteResult};
use kairos_core::colour::Rgb;
use kairos_core::config::{Config, SourceKind};
use kairos_core::dates::{self, When};
use kairos_core::dedup::Cluster;
use kairos_core::model::{EventKind, Instance, Span};
use kairos_core::time::resolve_local;
use kairos_core::view::{Filter, view_with};
use kairos_store::cache::{self, Index};
use kairos_store::client::{Daemon, Rejected};
use render::{JSON_SCHEMA, JsonList, Paint};
use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;

/// kairos — calendar + task scheduler on top of iCloud.
#[derive(Parser)]
#[command(version, about, propagate_version = true)]
struct Cli {
    /// Config file (default $KAIROS_CONFIG or ~/.config/kairos/config.toml).
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Args, Clone, Default)]
struct ViewOpts {
    /// Only these overlays (personal, family, background, imported). Repeatable.
    #[arg(long = "overlay")]
    overlays: Vec<String>,
    /// Only these keys. Repeatable.
    #[arg(long = "key")]
    keys: Vec<String>,
    /// JSON output (schema in docs/json.md).
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Today's events.
    Today {
        /// One line, for shell startup.
        #[arg(long)]
        compact: bool,
        #[command(flatten)]
        v: ViewOpts,
    },
    /// The next 7 days.
    Week {
        #[command(flatten)]
        v: ViewOpts,
    },
    /// List events in a window.
    Ls {
        /// Start (date or time; default today).
        #[arg(long)]
        from: Option<String>,
        /// End (default from + 7 days).
        #[arg(long)]
        to: Option<String>,
        #[command(flatten)]
        v: ViewOpts,
    },
    /// Next event (or a deadline within 24 h).
    Next {
        /// tmux status-line segment.
        #[arg(long)]
        tmux: bool,
        #[arg(long)]
        json: bool,
    },
    /// Find events by title (shows UIDs for edit/rm/rekey).
    Find {
        text: String,
        #[arg(long)]
        json: bool,
    },
    /// Add an event: kairos add "Squash" tue 19:00 --dur 60 --key Sport
    Add {
        title: String,
        /// When, e.g. `tue 19:00`, `2026-10-23` (all-day), `tomorrow 9`.
        #[arg(required = true, num_args = 1..)]
        when: Vec<String>,
        #[arg(long)]
        key: Option<String>,
        /// Duration (90m, 1h30, 2h). Default 1h.
        #[arg(long)]
        dur: Option<String>,
        /// End time instead of a duration.
        #[arg(long)]
        end: Option<String>,
        #[arg(long = "loc")]
        location: Option<String>,
        /// Notes (shown as the event's notes on the phone).
        #[arg(long)]
        notes: Option<String>,
    },
    /// Edit an event.
    Edit {
        /// UID (or unique prefix).
        uid: String,
        #[arg(long)]
        title: Option<String>,
        /// New start.
        #[arg(long)]
        when: Option<String>,
        #[arg(long)]
        dur: Option<String>,
        #[arg(long = "loc")]
        location: Option<String>,
        /// Notes; "" clears them.
        #[arg(long)]
        notes: Option<String>,
        /// Family events: block the scheduler (true/false).
        #[arg(long)]
        blocking: Option<bool>,
    },
    /// Delete an event.
    Rm { uid: String },
    /// Move events to another key's calendar: kairos rekey <uid>... <KEY>.
    /// Several events (or --matching) become one changeset: one approval, one undo.
    Rekey {
        /// UIDs (or unique prefixes/titles), then the key.
        #[arg(required = true, num_args = 1..)]
        args: Vec<String>,
        /// Also every event in the default window whose title contains this text.
        #[arg(long)]
        matching: Option<String>,
    },
    /// Add a ⚑ deadline: kairos deadline "B4 sheet" fri 17:00 --key Engineering
    Deadline {
        title: String,
        #[arg(required = true, num_args = 1..)]
        when: Vec<String>,
        #[arg(long)]
        key: String,
        #[arg(long)]
        task: Option<String>,
    },
    /// Tasks.
    Task {
        #[command(subcommand)]
        cmd: TaskCmd,
    },
    /// Run the scheduler.
    Plan {
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        json: bool,
    },
    /// Changesets (what kairos/Claude changed or wants to change).
    Changes {
        #[arg(long)]
        pending: bool,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Show one changeset in detail.
    Show { id: String },
    /// Approve a pending changeset.
    Approve { id: String },
    /// Reject a pending changeset.
    Reject { id: String },
    /// Undo an applied changeset.
    Undo { id: String },
    /// Move an event into the Family calendar (needs approval).
    ShareFamily { uid: String },
    /// Copy an Imported event into a key calendar.
    Adopt {
        uid: String,
        #[arg(long)]
        key: String,
    },
    /// Import an .ics file into a key calendar (one undoable changeset).
    Import {
        file: PathBuf,
        #[arg(long)]
        key: String,
    },
    /// Evening check-in for today's task blocks.
    Checkin,
    /// Sync: vdirsyncer, read-only feeds, sidecar from kairosd, cache rebuild.
    Sync {
        /// Only rebuild the local cache.
        #[arg(long)]
        local: bool,
    },
    /// Check config, sync freshness, daemon reachability.
    Doctor,
    /// Full-screen TUI.
    Tui,
}

#[derive(Subcommand)]
enum TaskCmd {
    /// Add: kairos task add "B4 sheet" --key Engineering --est 4h --due "fri 17:00"
    Add {
        title: String,
        #[arg(long)]
        key: String,
        /// Estimate (90m, 4h, 1h30).
        #[arg(long)]
        est: String,
        #[arg(long)]
        due: Option<String>,
        /// 1 high … 3 low.
        #[arg(long)]
        prio: Option<u8>,
        /// morning | afternoon | evening
        #[arg(long)]
        window: Option<String>,
        /// Must be done in one sitting.
        #[arg(long)]
        no_split: bool,
        #[arg(long)]
        min_chunk: Option<String>,
        #[arg(long)]
        max_chunk: Option<String>,
        #[arg(long)]
        notes: Option<String>,
    },
    /// List tasks.
    Ls {
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
    /// Mark done.
    Done {
        id: String,
        /// Actual time spent.
        #[arg(long)]
        actual: Option<String>,
    },
    /// Edit a task.
    Edit {
        id: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        est: Option<String>,
        #[arg(long)]
        remaining: Option<String>,
        /// New deadline ("" clears).
        #[arg(long)]
        due: Option<String>,
        #[arg(long)]
        prio: Option<u8>,
        #[arg(long)]
        window: Option<String>,
        #[arg(long)]
        key: Option<String>,
    },
    /// Drop a task (needs approval).
    Drop { id: String },
}

struct Ctx {
    cfg: Config,
    path: PathBuf,
    tz: Tz,
    now: DateTime<Utc>,
}

impl Ctx {
    fn today(&self) -> NaiveDate {
        self.now.with_timezone(&self.tz).date_naive()
    }
    fn midnight(&self, d: NaiveDate) -> DateTime<Utc> {
        resolve_local(d.and_time(NaiveTime::MIN), self.tz)
    }
    fn index(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<Index> {
        Ok(cache::get(&self.cfg, Some(&self.path), self.now, from, to)?.0)
    }
    fn daemon(&self) -> Result<Daemon> {
        // Typed by a person on a terminal → their own edits apply directly.
        // Scripts and agents (no TTY) still go through the approval policy.
        let tty =
            std::io::IsTerminal::is_terminal(&std::io::stdin()) && std::io::IsTerminal::is_terminal(&std::io::stdout());
        Ok(Daemon::from_config(&self.cfg)?.interactive(tty))
    }
    fn filter(&self, v: &ViewOpts) -> Filter {
        let mut f = Filter::from_config(&self.cfg);
        if !v.overlays.is_empty() {
            let has = |o: &str| v.overlays.iter().any(|x| x.eq_ignore_ascii_case(o));
            f.personal = has("personal");
            f.family = has("family");
            f.background = has("background");
            f.imported = has("imported");
        }
        if !v.keys.is_empty() {
            // Keys are Personal calendars: `--key Engineering` alone means just those.
            if v.overlays.is_empty() {
                f.family = false;
                f.background = false;
                f.imported = false;
            }
            for k in &self.cfg.keys {
                if !v.keys.iter().any(|x| x.eq_ignore_ascii_case(&k.name)) {
                    f.hidden_keys.insert(k.name.to_lowercase());
                }
            }
        }
        f
    }
    fn clusters(&self, from: DateTime<Utc>, to: DateTime<Utc>, f: &Filter) -> Result<(Vec<Cluster>, Index)> {
        let idx = self.index(from, to)?;
        let inst = idx.between(from, to, self.tz);
        Ok((view_with(inst, |id| idx.kind_of(id), f, self.tz), idx))
    }
    /// Natural input → API time string (ambiguity is an error here, locally).
    fn when(&self, words: &str) -> Result<String> {
        Ok(
            match dates::parse_when(words, self.today()).map_err(|e| anyhow!("{e}"))? {
                When::Date(d) => d.to_string(),
                When::DateTime(n) => resolve_local(n, self.tz).to_rfc3339(),
            },
        )
    }
}

fn main() {
    if let Err(e) = run() {
        let p = Paint::auto();
        eprintln!("{} {e:#}", p.bold(Rgb::mocha("red"), "error:"));
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let path = cli.config.clone().unwrap_or_else(Config::default_path);
    let cfg =
        Config::load(&path).with_context(|| format!("loading {} (see config/config.example.toml)", path.display()))?;
    let errs = cfg.validate();
    if !errs.is_empty() && !matches!(cli.cmd, Some(Cmd::Doctor)) {
        bail!(
            "invalid config:\n  {}",
            errs.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n  ")
        );
    }
    let ctx = Ctx {
        tz: cfg.tz(),
        now: std::env::var("KAIROS_NOW")
            .ok()
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map_or_else(Utc::now, |t| t.with_timezone(&Utc)),
        cfg,
        path,
    };
    let p = Paint::auto();
    // Bare `kairos` in a terminal opens the calendar; piped, it prints today.
    let default_cmd = if std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        Cmd::Tui
    } else {
        Cmd::Today {
            compact: false,
            v: ViewOpts::default(),
        }
    };
    match cli.cmd.unwrap_or(default_cmd) {
        Cmd::Today { compact, v } => today(&ctx, &p, compact, &v),
        Cmd::Week { v } => {
            let from = ctx.midnight(ctx.today());
            agenda(&ctx, &p, from, from + Duration::days(7), &v)
        }
        Cmd::Ls { from, to, v } => {
            let from = match from {
                Some(s) => parse_instant(&ctx, &s)?,
                None => ctx.midnight(ctx.today()),
            };
            let to = match to {
                Some(s) => parse_instant(&ctx, &s)?,
                None => from + Duration::days(7),
            };
            if to <= from {
                bail!("--to must be after --from");
            }
            agenda(&ctx, &p, from, to, &v)
        }
        Cmd::Next { tmux, json } => next(&ctx, tmux, json),
        Cmd::Find { text, json } => find(&ctx, &p, &text, json),
        Cmd::Add {
            title,
            when,
            key,
            dur,
            end,
            location,
            notes,
        } => {
            let req = api::CreateEventReq {
                title,
                start: ctx.when(&when.join(" "))?,
                end: end.map(|e| ctx.when(&e)).transpose()?,
                duration_min: dur
                    .map(|d| dates::parse_minutes(&d))
                    .transpose()
                    .map_err(|e| anyhow!("{e}"))?,
                key,
                location,
                description: notes,
                ..Default::default()
            };
            let d = ctx.daemon()?;
            report(&p, &d, d.create_event(&req)?)
        }
        Cmd::Edit {
            uid,
            title,
            when,
            dur,
            location,
            notes,
            blocking,
        } => {
            let (inst, _) = resolve_uid(&ctx, &uid)?;
            let req = api::UpdateEventReq {
                title,
                start: when.map(|w| ctx.when(&w)).transpose()?,
                end: None,
                duration_min: dur
                    .map(|d| dates::parse_minutes(&d))
                    .transpose()
                    .map_err(|e| anyhow!("{e}"))?,
                location,
                description: notes,
                blocking,
            };
            let d = ctx.daemon()?;
            match d.update_event(&inst.source, &inst.uid, &req)? {
                Ok(v) => match serde_json::from_value::<WriteResult>(v.clone()) {
                    Ok(w) => report(&p, &d, Ok(w)),
                    Err(_) => {
                        println!(
                            "{} {}",
                            p.fg(Rgb::mocha("green"), "✓"),
                            v["summary"].as_str().unwrap_or("done")
                        );
                        Ok(())
                    }
                },
                Err(r) => Err(anyhow!("{r}")),
            }
        }
        Cmd::Rm { uid } => {
            let (inst, _) = resolve_uid(&ctx, &uid)?;
            let d = ctx.daemon()?;
            report(&p, &d, d.delete_event(&inst.source, &inst.uid)?)
        }
        Cmd::Rekey { mut args, matching } => {
            let key = args.pop().ok_or_else(|| anyhow!("missing KEY"))?;
            let mut evs: Vec<(String, String)> = Vec::new();
            for q in &args {
                let (inst, _) = resolve_uid(&ctx, q)?;
                evs.push((inst.source, inst.uid));
            }
            if let Some(m) = matching {
                let (from, to) = cache::default_window(&ctx.cfg, ctx.now);
                let ml = m.to_lowercase();
                for i in ctx.index(from, to)?.instances {
                    if i.title.to_lowercase().contains(&ml) {
                        evs.push((i.source, i.uid));
                    }
                }
            }
            evs.sort();
            evs.dedup();
            if evs.is_empty() {
                bail!("no events to re-key");
            }
            let d = ctx.daemon()?;
            if evs.len() == 1 {
                report(&p, &d, d.move_event(&evs[0].0, &evs[0].1, &key)?)
            } else {
                report(&p, &d, d.move_events(&evs, &key)?)
            }
        }
        Cmd::ShareFamily { uid } => {
            let (inst, _) = resolve_uid(&ctx, &uid)?;
            let d = ctx.daemon()?;
            report(&p, &d, d.share_family(&inst.source, &inst.uid)?)
        }
        Cmd::Adopt { uid, key } => {
            let (inst, _) = resolve_uid(&ctx, &uid)?;
            let d = ctx.daemon()?;
            report(&p, &d, d.adopt(&inst.source, &inst.uid, &key)?)
        }
        Cmd::Deadline { title, when, key, task } => {
            let req = api::DeadlineReq {
                title,
                due: ctx.when(&when.join(" "))?,
                key,
                task_id: task,
            };
            let d = ctx.daemon()?;
            report(&p, &d, d.deadline(&req)?)
        }
        Cmd::Task { cmd } => task(&ctx, &p, cmd),
        Cmd::Plan { dry_run, json } => {
            let d = ctx.daemon()?;
            let r = d.plan(dry_run)?.map_err(|e| anyhow!("{e}"))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&r)?);
                return Ok(());
            }
            print_plan(&p, &r, ctx.tz);
            if let Some(cs) = r.changeset {
                report(&p, &d, Ok(cs))?;
            }
            Ok(())
        }
        Cmd::Changes { pending, limit, json } => {
            let d = ctx.daemon()?;
            let v = d
                .changesets(pending.then_some("pending"), limit)?
                .map_err(|e| anyhow!("{e}"))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&v)?);
                return Ok(());
            }
            if v.is_empty() {
                println!(
                    "{}",
                    p.dim(if pending {
                        "Nothing waiting for approval."
                    } else {
                        "No changes yet."
                    })
                );
            }
            for c in v {
                let st = status_badge(&p, c.status);
                println!(
                    "{st} {} {} {}",
                    p.dim(&c.id),
                    p.dim(
                        &c.created_at
                            .with_timezone(&ctx.tz)
                            .format("%a %-d %b %H:%M")
                            .to_string()
                    ),
                    c.summary.lines().next().unwrap_or("")
                );
                if c.status == Status::Pending {
                    println!("    {}", p.dim(&c.reasons.join(" · ")));
                }
            }
            Ok(())
        }
        Cmd::Show { id } => {
            let d = ctx.daemon()?;
            let c = d.changeset(&id)?.map_err(|e| anyhow!("{e}"))?;
            println!(
                "{} {} by {} · risk {}",
                status_badge(&p, c.status),
                c.id,
                c.author.as_str(),
                c.risk.as_str()
            );
            println!("{}", c.summary);
            for r in &c.reasons {
                println!("  {} {r}", p.fg(Rgb::mocha("peach"), "!"));
            }
            for op in &c.ops {
                println!("  • {}", op.describe());
                if let kairos_core::changeset::Op::UpdateEvent { before, after, .. } = op {
                    for l in diff_lines(before, after) {
                        println!("      {l}");
                    }
                }
            }
            if let Some(e) = c.error {
                println!("  {} {e}", p.fg(Rgb::mocha("red"), "error:"));
            }
            Ok(())
        }
        Cmd::Approve { id } => decide(&ctx, &p, &id, "approve"),
        Cmd::Reject { id } => decide(&ctx, &p, &id, "reject"),
        Cmd::Undo { id } => decide(&ctx, &p, &id, "undo"),
        Cmd::Import { file, key } => {
            let ics = std::fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
            let name = file
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let d = ctx.daemon()?;
            report(&p, &d, d.import(&api::ImportReq { key, name, ics })?)
        }
        Cmd::Checkin => checkin(&ctx, &p),
        Cmd::Sync { local } => sync(&ctx, &p, local),
        Cmd::Doctor => doctor(&ctx, &p),
        Cmd::Tui => kairos_tui::run(ctx.cfg.clone(), ctx.path.clone()).map_err(|e| anyhow!("{e}")),
    }
}

fn parse_instant(ctx: &Ctx, s: &str) -> Result<DateTime<Utc>> {
    Ok(match api::parse_time(s, ctx.tz, ctx.now).map_err(|e| anyhow!("{e}"))? {
        api::TimeIn::At(t) => t,
        api::TimeIn::Date(d) => ctx.midnight(d),
    })
}

fn diff_lines(a: &str, b: &str) -> Vec<String> {
    let (a, b): (Vec<&str>, Vec<&str>) = (a.lines().collect(), b.lines().collect());
    let skip = |l: &str| l.starts_with("DTSTAMP") || l.starts_with("LAST-MODIFIED") || l.starts_with("SEQUENCE");
    let mut out = Vec::new();
    for l in &a {
        if !b.contains(l) && !skip(l) {
            out.push(format!("- {l}"));
        }
    }
    for l in &b {
        if !a.contains(l) && !skip(l) {
            out.push(format!("+ {l}"));
        }
    }
    out
}

fn status_badge(p: &Paint, s: Status) -> String {
    let (c, t) = match s {
        Status::Pending => ("peach", "pending "),
        Status::Applied => ("green", "applied "),
        Status::Rejected => ("overlay1", "rejected"),
        Status::Undone => ("mauve", "undone  "),
        Status::Failed => ("red", "failed  "),
    };
    p.fg(Rgb::mocha(c), t)
}

/// Print the outcome of a write; offer to approve interactively when pending.
fn report(p: &Paint, d: &Daemon, r: std::result::Result<WriteResult, Rejected>) -> Result<()> {
    let w = r.map_err(|e| anyhow!("{e}"))?;
    match w.status {
        Status::Applied => {
            background_pull();
            println!(
                "{} {}",
                p.fg(Rgb::mocha("green"), "✓"),
                w.summary.lines().next().unwrap_or("")
            );
            println!(
                "  {}",
                p.dim(&format!("{} · undo: kairos undo {}", w.changeset_id, w.changeset_id))
            );
        }
        Status::Pending => {
            println!(
                "{} {}",
                p.fg(Rgb::mocha("peach"), "◌ needs approval:"),
                w.summary.lines().next().unwrap_or("")
            );
            for r in &w.reasons {
                println!("  {} {r}", p.dim("·"));
            }
            if std::io::stdin().is_terminal() {
                print!("  Approve now? [y/N] ");
                let _ = std::io::stdout().flush();
                let mut line = String::new();
                std::io::stdin().lock().read_line(&mut line)?;
                if line.trim().eq_ignore_ascii_case("y") {
                    let a = d.decide(&w.changeset_id, "approve")?.map_err(|e| anyhow!("{e}"))?;
                    println!("{} {}", p.fg(Rgb::mocha("green"), "✓"), a.status.as_str());
                    return Ok(());
                }
            }
            println!(
                "  {}",
                p.dim(&format!("kairos approve {0} / kairos reject {0}", w.changeset_id))
            );
        }
        other => println!("{} {}", other.as_str(), w.summary),
    }
    Ok(())
}

/// After a write, pull from iCloud in the background so `today`/the TUI show it
/// without waiting for the next scheduled sync.
fn background_pull() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe)
            .arg("sync")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
}

fn decide(ctx: &Ctx, p: &Paint, id: &str, action: &str) -> Result<()> {
    let d = ctx.daemon()?;
    match d.decide(id, action)? {
        Ok(w) => {
            println!(
                "{} {} {}",
                status_badge(p, w.status),
                w.changeset_id,
                w.summary.lines().next().unwrap_or("")
            );
            Ok(())
        }
        Err(e) => Err(anyhow!("{e}")),
    }
}

/// Find an instance by UID / prefix / title in the next 60 days (and last 14).
fn resolve_uid(ctx: &Ctx, q: &str) -> Result<(Instance, Index)> {
    let (from, to) = cache::default_window(&ctx.cfg, ctx.now);
    let idx = ctx.index(from, to)?;
    let mut hits: Vec<&Instance> = idx.instances.iter().filter(|i| i.uid == q).collect();
    if hits.is_empty() {
        hits = idx.instances.iter().filter(|i| i.uid.starts_with(q)).collect();
    }
    if hits.is_empty() {
        let ql = q.to_lowercase();
        hits = idx
            .instances
            .iter()
            .filter(|i| i.title.to_lowercase().contains(&ql))
            .collect();
    }
    let mut uids: Vec<(&str, &str)> = hits.iter().map(|i| (i.uid.as_str(), i.source.as_str())).collect();
    uids.sort();
    uids.dedup();
    match uids.len() {
        0 => bail!("no event matches {q:?} (try `kairos find`)"),
        1 => {
            let inst = (*hits[0]).clone();
            Ok((inst, idx))
        }
        n => bail!(
            "{q:?} matches {n} events — use a UID:\n  {}",
            hits.iter()
                .take(10)
                .map(|i| format!("{}  {}  {}", i.uid, i.title, render::time_range(i, ctx.tz)))
                .collect::<Vec<_>>()
                .join("\n  ")
        ),
    }
}

fn find(ctx: &Ctx, p: &Paint, q: &str, json: bool) -> Result<()> {
    let (from, to) = cache::default_window(&ctx.cfg, ctx.now);
    let idx = ctx.index(from, to)?;
    let ql = q.to_lowercase();
    let hits: Vec<&Instance> = idx
        .instances
        .iter()
        .filter(|i| i.title.to_lowercase().contains(&ql) || i.uid.starts_with(q))
        .collect();
    if json {
        let v: Vec<_> = hits
            .iter()
            .map(|i| serde_json::json!({"uid": i.uid, "source": i.source, "title": i.title, "start": i.start(ctx.tz).with_timezone(&ctx.tz).to_rfc3339()}))
            .collect();
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    for i in hits {
        println!(
            "{} {} {}  {}",
            p.dim(&i.start(ctx.tz).with_timezone(&ctx.tz).format("%a %-d %b").to_string()),
            p.dim(&render::time_range(i, ctx.tz)),
            render::title_of(i),
            p.dim(&format!("{} · {}", i.uid, i.source))
        );
    }
    Ok(())
}

fn today(ctx: &Ctx, p: &Paint, compact: bool, v: &ViewOpts) -> Result<()> {
    let from = ctx.midnight(ctx.today());
    let to = ctx.midnight(ctx.today() + Duration::days(1));
    if !compact {
        return agenda(ctx, p, from, to, v);
    }
    let (cl, _) = ctx.clusters(from, to + Duration::days(6), &ctx.filter(v))?;
    let todays: Vec<&Cluster> = cl.iter().filter(|c| c.primary.start(ctx.tz) < to).collect();
    let mut parts = vec![p.bold(Rgb::mocha("blue"), &ctx.today().format("%a %-d %b").to_string())];
    let events = todays
        .iter()
        .filter(|c| c.primary.meta.kind == EventKind::Event)
        .count();
    let blocks = todays
        .iter()
        .filter(|c| c.primary.meta.kind == EventKind::TaskBlock)
        .count();
    parts.push(format!("{events} event{}", if events == 1 { "" } else { "s" }));
    if blocks > 0 {
        parts.push(format!("{blocks} work block{}", if blocks == 1 { "" } else { "s" }));
    }
    if let Some(n) = cl.iter().find(|c| {
        matches!(c.primary.span, Span::Timed { .. })
            && c.primary.start(ctx.tz) > ctx.now
            && c.primary.meta.kind != EventKind::Deadline
            && c.primary.start(ctx.tz) < to
    }) {
        parts.push(format!(
            "next {} {}",
            n.primary.title,
            n.primary.start(ctx.tz).with_timezone(&ctx.tz).format("%H:%M")
        ));
    }
    if let Some(dl) = cl
        .iter()
        .find(|c| c.primary.meta.kind == EventKind::Deadline && c.primary.end(ctx.tz) >= ctx.now)
    {
        let i = &dl.primary;
        let when = match i.span {
            Span::AllDay { start, .. } => start.format("%a").to_string(),
            Span::Timed { start, .. } => start.with_timezone(&ctx.tz).format("%a %H:%M").to_string(),
        };
        parts.push(p.fg(Rgb::mocha("red"), &format!("{} {when}", render::title_of(i))));
    }
    println!("{}", parts.join(&p.dim(" · ")));
    Ok(())
}

fn agenda(ctx: &Ctx, p: &Paint, from: DateTime<Utc>, to: DateTime<Utc>, v: &ViewOpts) -> Result<()> {
    let (cl, idx) = ctx.clusters(from, to, &ctx.filter(v))?;
    if v.json {
        let out = JsonList {
            schema: JSON_SCHEMA,
            timezone: ctx.tz.name().to_string(),
            from: from.with_timezone(&ctx.tz).to_rfc3339(),
            to: to.with_timezone(&ctx.tz).to_rfc3339(),
            events: cl.iter().map(|c| render::json_event(&ctx.cfg, c, ctx.tz)).collect(),
        };
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    let first = from.with_timezone(&ctx.tz).date_naive();
    let last = (to - Duration::seconds(1)).with_timezone(&ctx.tz).date_naive();
    let mut d = first;
    let mut printed_any = false;
    while d <= last {
        let mut day: Vec<&Cluster> = cl
            .iter()
            .filter(|c| c.primary.span.local_dates(ctx.tz).contains(&d))
            .collect();
        day.sort_by_key(|c| (!c.primary.span.is_all_day(), c.primary.start(ctx.tz)));
        if !day.is_empty() || first == last {
            if printed_any {
                println!();
            }
            println!("{}", render::day_header(p, d, ctx.today()));
            if day.is_empty() {
                println!(" {}", p.dim("nothing scheduled"));
            }
            for c in day {
                println!("{}", render::line(&ctx.cfg, p, c, ctx.tz));
            }
            printed_any = true;
        }
        d += Duration::days(1);
    }
    if !printed_any {
        println!("{}", p.dim("Nothing scheduled."));
    }
    if !idx.missing.is_empty() {
        let ids: Vec<&str> = idx.missing.iter().map(|(id, _)| id.as_str()).collect();
        eprintln!(
            "{}",
            p.dim(&format!(
                "({} source(s) not loaded: {} — see kairos doctor)",
                ids.len(),
                ids.join(", ")
            ))
        );
    }
    Ok(())
}

/// `next --tmux`: one status segment matching the tmux theme, or nothing.
fn next(ctx: &Ctx, tmux: bool, json: bool) -> Result<()> {
    let from = ctx.now - Duration::hours(12);
    let to = ctx.now + Duration::hours(36);
    let mut f = Filter::from_config(&ctx.cfg);
    f.background = false;
    f.imported = false;
    let (cl, _) = ctx.clusters(from, to, &f)?;
    let tz = ctx.tz;
    // A deadline within 24 h wins (the soonest-due one; all-day = end of that day).
    let due_of = |i: &Instance| match i.span {
        Span::Timed { start, .. } => start,
        Span::AllDay { end, .. } => ctx.midnight(end),
    };
    let deadline = cl
        .iter()
        .map(|c| &c.primary)
        .filter(|i| {
            i.meta.kind == EventKind::Deadline && due_of(i) >= ctx.now && due_of(i) <= ctx.now + Duration::hours(24)
        })
        .min_by_key(|i| due_of(i));
    let event = cl.iter().map(|c| &c.primary).find(|i| {
        matches!(i.span, Span::Timed { .. })
            && i.meta.kind != EventKind::Deadline
            && !i.transparent
            && i.end(tz) > ctx.now
            && i.start(tz) <= ctx.now + Duration::hours(24)
    });
    let (text, urgent, kind, inst) = if let Some(d) = deadline {
        let when = match d.span {
            Span::Timed { start, .. } => start.with_timezone(&tz).format("%H:%M").to_string(),
            Span::AllDay { start, .. } => start.format("%a").to_string(),
        };
        (format!("{} {when}", render::title_of(d)), true, "deadline", Some(d))
    } else if let Some(e) = event {
        let s = e.start(tz);
        let rel = if s <= ctx.now {
            format!("until {}", e.end(tz).with_timezone(&tz).format("%H:%M"))
        } else {
            render::humanise_until(ctx.now, s)
        };
        let title = render::title_of(e);
        let title: String = if title.chars().count() > 28 {
            format!("{}…", title.chars().take(27).collect::<String>())
        } else {
            title
        };
        (
            format!(
                "{} {title} {} ({rel})",
                render::CAL_ICON,
                s.with_timezone(&tz).format("%H:%M")
            ),
            false,
            "event",
            Some(e),
        )
    } else {
        (String::new(), false, "none", None)
    };
    if json {
        let v = serde_json::json!({
            "schema": JSON_SCHEMA,
            "kind": kind,
            "text": text,
            "uid": inst.map(|i| i.uid.clone()),
            "title": inst.map(|i| i.title.clone()),
            "start": inst.map(|i| i.start(tz).with_timezone(&tz).to_rfc3339()),
            "minutes_until": inst.map(|i| (i.start(tz) - ctx.now).num_minutes()),
        });
        println!("{v}");
        return Ok(());
    }
    if text.is_empty() {
        return Ok(());
    }
    if tmux {
        // Inline segment for ~/.config/tmux/tmux.conf status-right, placed right
        // after the surface0 date segment: a thin U+E0B3 separator in surface2,
        // like starship's bar. Urgent deadlines are red and bold.
        let fg = if urgent { "#f38ba8,bold" } else { "#cdd6f4" };
        println!("#[bg=#313244,fg=#585b70]\u{e0b3}#[bg=#313244,fg={fg}] {text} #[nobold]");
    } else {
        println!("{text}");
    }
    Ok(())
}

fn print_plan(p: &Paint, r: &api::PlanResp, tz: Tz) {
    println!(
        "{} {} new · {} removed · {} kept · {} moved{}",
        p.bold(Rgb::mocha("blue"), "plan"),
        r.created.len(),
        r.deleted.len(),
        r.kept,
        r.moved,
        if r.dry_run { p.dim(" (dry run)") } else { String::new() }
    );
    for b in &r.created {
        println!(
            "  {} {} {}–{}",
            p.fg(Rgb::mocha("green"), "+"),
            b.title,
            b.start.with_timezone(&tz).format("%a %-d %b %H:%M"),
            b.end.with_timezone(&tz).format("%H:%M")
        );
    }
    for b in &r.deleted {
        println!(
            "  {} {} {}–{}",
            p.fg(Rgb::mocha("red"), "-"),
            b.title,
            b.start.with_timezone(&tz).format("%a %-d %b %H:%M"),
            b.end.with_timezone(&tz).format("%H:%M")
        );
    }
    for s in &r.shortfalls {
        println!("  {} {}", p.bold(Rgb::mocha("peach"), "!"), s.message);
        for o in &s.options {
            println!("      {}", p.dim(&format!("· {o}")));
        }
    }
}

fn task(ctx: &Ctx, p: &Paint, cmd: TaskCmd) -> Result<()> {
    let mins = |s: &str| dates::parse_minutes(s).map_err(|e| anyhow!("{e}"));
    let d = ctx.daemon()?;
    match cmd {
        TaskCmd::Add {
            title,
            key,
            est,
            due,
            prio,
            window,
            no_split,
            min_chunk,
            max_chunk,
            notes,
        } => {
            let req = api::CreateTaskReq {
                title,
                key,
                estimate_min: mins(&est)?,
                due: due.map(|x| ctx.when(&x)).transpose()?,
                priority: prio,
                preferred_window: window,
                splittable: no_split.then_some(false),
                min_chunk_min: min_chunk.map(|x| mins(&x)).transpose()?,
                max_chunk_min: max_chunk.map(|x| mins(&x)).transpose()?,
                notes,
                ..Default::default()
            };
            let r = d.create_task(&req)?.map_err(|e| anyhow!("{e}"))?;
            println!(
                "{} task {} ({})",
                p.fg(Rgb::mocha("green"), "✓"),
                r.task.title,
                p.dim(&r.task.id)
            );
            for c in &r.changesets {
                if c.status == Status::Pending {
                    report(p, &d, Ok(c.clone()))?;
                }
            }
            if let Some(plan) = r.plan {
                print_plan(p, &plan, ctx.tz);
            }
        }
        TaskCmd::Ls { all, json } => {
            let v = d.tasks(if all { "all" } else { "open" })?.map_err(|e| anyhow!("{e}"))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&v)?);
                return Ok(());
            }
            if v.is_empty() {
                println!("{}", p.dim("No open tasks."));
            }
            for t in v {
                let col = ctx.cfg.key(&t.key).map_or(Rgb::mocha("text"), |k| k.colour);
                let due = t
                    .due
                    .map(|x| format!(" · due {}", x.describe(ctx.tz)))
                    .unwrap_or_default();
                let progress = format!(
                    "{}/{} min",
                    t.estimate_min - t.remaining_min.min(t.estimate_min),
                    t.estimate_min
                );
                println!(
                    "{} {} {}{}  {}",
                    p.fg(col, "●"),
                    t.title,
                    p.dim(&format!("P{} · {progress}", t.priority)),
                    p.dim(&due),
                    p.dim(&format!("{} · {}", t.id, t.status.as_str()))
                );
            }
        }
        TaskCmd::Done { id, actual } => {
            let r = d
                .complete_task(&id, actual.map(|a| mins(&a)).transpose()?)?
                .map_err(|e| anyhow!("{e}"))?;
            println!("{} done: {}", p.fg(Rgb::mocha("green"), "✓"), r.task.title);
            if let Some(plan) = r.plan {
                print_plan(p, &plan, ctx.tz);
            }
        }
        TaskCmd::Edit {
            id,
            title,
            est,
            remaining,
            due,
            prio,
            window,
            key,
        } => {
            let req = api::UpdateTaskReq {
                title,
                key,
                estimate_min: est.map(|x| mins(&x)).transpose()?,
                remaining_min: remaining.map(|x| mins(&x)).transpose()?,
                due: due
                    .map(|x| if x.is_empty() { Ok(String::new()) } else { ctx.when(&x) })
                    .transpose()?,
                priority: prio,
                preferred_window: window,
                ..Default::default()
            };
            let r = d.update_task(&id, &req)?.map_err(|e| anyhow!("{e}"))?;
            println!("{} updated {}", p.fg(Rgb::mocha("green"), "✓"), r.task.title);
            if let Some(plan) = r.plan {
                print_plan(p, &plan, ctx.tz);
            }
        }
        TaskCmd::Drop { id } => {
            let r = d.drop_task(&id)?.map_err(|e| anyhow!("{e}"))?;
            for c in r.changesets {
                report(p, &d, Ok(c))?;
            }
        }
    }
    Ok(())
}

fn checkin(ctx: &Ctx, p: &Paint) -> Result<()> {
    let d = ctx.daemon()?;
    let items = d.checkin_items()?.map_err(|e| anyhow!("{e}"))?;
    if items.is_empty() {
        println!("{}", p.dim("No task blocks today."));
        return Ok(());
    }
    let stdin = std::io::stdin();
    for it in items {
        let when = format!(
            "{}–{}",
            it.start.with_timezone(&ctx.tz).format("%H:%M"),
            it.end.with_timezone(&ctx.tz).format("%H:%M")
        );
        if let Some(o) = &it.outcome {
            println!("{} {} {}", p.dim(&when), it.title, p.dim(&format!("({o})")));
            continue;
        }
        print!(
            "{} {} — [d]one / [p]artial / [m]issed / [s]kip? ",
            p.dim(&when),
            it.title
        );
        std::io::stdout().flush()?;
        let mut line = String::new();
        stdin.lock().read_line(&mut line)?;
        let outcome = match line.trim().chars().next() {
            Some('d') => "done",
            Some('p') => "partial",
            Some('m') => "missed",
            _ => continue,
        };
        let actual = if outcome == "partial" {
            print!("  minutes actually spent? ");
            std::io::stdout().flush()?;
            let mut l = String::new();
            stdin.lock().read_line(&mut l)?;
            dates::parse_minutes(l.trim()).ok()
        } else {
            None
        };
        let r = d
            .checkin(&api::CheckinReq {
                block_uid: it.block_uid.clone(),
                outcome: outcome.into(),
                actual_min: actual,
            })?
            .map_err(|e| anyhow!("{e}"))?;
        if !r.created.is_empty() || !r.shortfalls.is_empty() {
            print_plan(p, &r, ctx.tz);
        }
    }
    Ok(())
}

fn sync(ctx: &Ctx, p: &Paint, local: bool) -> Result<()> {
    let ok = p.fg(Rgb::mocha("green"), "✓");
    let warn = p.fg(Rgb::mocha("peach"), "!");
    if !local {
        let cmd = ctx.cfg.sync.full_cmd.trim();
        if !cmd.is_empty() {
            let r = kairos_store::sync::run_full(&ctx.cfg);
            for pc in &r.followed_empty {
                println!("{ok} {pc} was emptied on iCloud; local copy emptied too");
            }
            match r.error {
                None => println!("{ok} {cmd}"),
                Some(e) => println!("{warn} {cmd} failed: {e}"),
            }
        }
        for (id, r) in kairos_store::feeds::fetch_all(&ctx.cfg) {
            match r {
                Ok(kairos_store::feeds::FetchOutcome::Updated { events, .. }) => {
                    println!("{ok} feed {id}: {events} events")
                }
                Ok(kairos_store::feeds::FetchOutcome::NotModified) => println!("{ok} feed {id}: unchanged"),
                Err(e) => println!("{warn} feed {id}: {e}"),
            }
        }
        match ctx.daemon().and_then(|d| Ok(d.sidecar()?)) {
            Ok(Ok(rows)) => {
                let side = rows.into_iter().map(|(s, u, m)| ((s, u), m)).collect();
                kairos_store::load::write_sidecar_json(&ctx.cfg, &side)?;
                println!("{ok} sidecar from kairosd");
            }
            Ok(Err(e)) => println!("{warn} sidecar: {e}"),
            Err(e) => println!("{warn} sidecar: {e}"),
        }
    }
    let (from, to) = cache::default_window(&ctx.cfg, ctx.now);
    let side = kairos_store::load::read_sidecar_json(&ctx.cfg);
    let idx = cache::build(&ctx.cfg, Some(&ctx.path), &side, from, to)?;
    cache::write(&ctx.cfg, &idx)?;
    println!(
        "{ok} cache: {} instances from {} sources",
        idx.instances.len(),
        idx.sources.len()
    );
    for (id, why) in &idx.missing {
        println!("{warn} {id}: {why}");
    }
    if !idx.warnings.is_empty() {
        println!(
            "{warn} {} parse warning(s); first: {}",
            idx.warnings.len(),
            idx.warnings[0]
        );
    }
    Ok(())
}

fn doctor(ctx: &Ctx, p: &Paint) -> Result<()> {
    let ok = p.fg(Rgb::mocha("green"), "✓");
    let bad = p.fg(Rgb::mocha("red"), "✗");
    let warn = p.fg(Rgb::mocha("peach"), "!");
    let mut problems = 0;
    println!("{}", p.bold(Rgb::mocha("blue"), "kairos doctor"));
    let errs = ctx.cfg.validate();
    if errs.is_empty() {
        println!("{ok} config {}", p.dim(&ctx.path.display().to_string()));
    } else {
        problems += errs.len();
        for e in errs {
            println!("{bad} config: {e}");
        }
    }
    println!(
        "{} live = {} {}",
        if ctx.cfg.live { warn.clone() } else { ok.clone() },
        ctx.cfg.live,
        p.dim(&if ctx.cfg.live {
            "writes go to real calendars".to_string()
        } else {
            format!("writes only go to {}", ctx.cfg.test_calendar)
        })
    );
    let cons = ctx.path.parent().map(|d| d.join("constraints.toml"));
    match cons.as_ref().map(std::fs::read_to_string) {
        Some(Ok(t)) => match kairos_core::constraints::Constraints::from_toml(&t) {
            Ok(_) => println!("{ok} constraints.toml"),
            Err(e) => {
                problems += 1;
                println!("{bad} constraints.toml: {e}");
            }
        },
        _ => println!(
            "{} no constraints.toml next to config {}",
            p.dim("·"),
            p.dim("(only kairosd needs it)")
        ),
    }
    let vd = ctx.cfg.vdir();
    let cols = kairos_store::vdir::collections(&vd).unwrap_or_default();
    println!(
        "{} vdir {} ({} collections)",
        if cols.is_empty() { bad.clone() } else { ok.clone() },
        p.dim(&vd.display().to_string()),
        cols.len()
    );
    let stale = Duration::minutes(i64::from(ctx.cfg.sync.stale_after_minutes));
    if ctx.cfg.sources.iter().any(|s| s.kind != SourceKind::IcsReadonly) {
        match kairos_store::sync::last_success(&ctx.cfg) {
            Some(t) if ctx.now - t <= stale => println!("{ok} iCloud synced {} ago", fmt_age(ctx.now - t)),
            Some(t) => println!(
                "{warn} iCloud last synced {} ago {}",
                fmt_age(ctx.now - t),
                p.dim("(check vdirsyncer / the sync job)")
            ),
            None => println!("{warn} iCloud never synced by kairos {}", p.dim("(run kairos sync)")),
        }
    }
    for s in &ctx.cfg.sources {
        match kairos_store::vdir::source_path(&ctx.cfg, s, &cols) {
            Ok(path) if path.exists() => {
                let newest = if s.kind == SourceKind::IcsReadonly {
                    kairos_store::feeds::state(&ctx.cfg, &s.id).fetched_at
                } else {
                    std::fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .ok()
                        .map(DateTime::<Utc>::from)
                };
                let age = newest.map(|t| ctx.now - t);
                // Calendars that simply haven't changed aren't stale; only feeds are judged by age.
                let mark = if s.kind == SourceKind::IcsReadonly && age.is_some_and(|a| a > stale * 4) {
                    warn.clone()
                } else {
                    ok.clone()
                };
                let err = if s.kind == SourceKind::IcsReadonly {
                    kairos_store::feeds::state(&ctx.cfg, &s.id)
                        .last_error
                        .map(|e| format!(" — last error: {e}"))
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                println!(
                    "{mark} {:<14} {}{}",
                    s.id,
                    p.dim(&age.map_or("never".into(), |a| format!("changed {} ago", fmt_age(a)))),
                    p.dim(&err)
                );
            }
            Ok(path) => {
                problems += 1;
                println!("{bad} {:<14} missing {}", s.id, p.dim(&path.display().to_string()));
            }
            Err(e) => {
                problems += 1;
                println!("{bad} {:<14} {e}", s.id);
            }
        }
    }
    let last_sync = std::fs::metadata(cache::path(&ctx.cfg))
        .and_then(|m| m.modified())
        .ok()
        .map(DateTime::<Utc>::from);
    match last_sync {
        Some(t) if ctx.now - t <= stale => println!("{ok} cache rebuilt {} ago", fmt_age(ctx.now - t)),
        Some(t) => println!(
            "{warn} cache last rebuilt {} ago {}",
            fmt_age(ctx.now - t),
            p.dim("(run kairos sync)")
        ),
        None => println!("{warn} no cache yet {}", p.dim("(run kairos sync)")),
    }
    let have_vdirsyncer = std::process::Command::new("sh")
        .arg("-c")
        .arg("command -v vdirsyncer")
        .output()
        .is_ok_and(|o| o.status.success());
    println!(
        "{} vdirsyncer {}",
        if have_vdirsyncer { ok.clone() } else { warn.clone() },
        if have_vdirsyncer { "installed" } else { "not on PATH" }
    );
    match ctx.daemon() {
        Ok(d) => {
            if d.healthy() {
                match d.status()? {
                    Ok(s) => println!(
                        "{ok} kairosd {} {}",
                        p.dim(d.base()),
                        p.dim(&format!(
                            "· live={} · {} pending · {} open tasks",
                            s["live"], s["pending_changesets"], s["open_tasks"]
                        ))
                    ),
                    Err(e) => {
                        problems += 1;
                        println!("{bad} kairosd auth: {e}");
                    }
                }
            } else {
                println!(
                    "{warn} kairosd unreachable at {} {}",
                    d.base(),
                    p.dim("(reads still work; writes need it)")
                );
            }
        }
        Err(e) => println!("{warn} kairosd: {e}"),
    }
    if problems > 0 {
        bail!("{problems} problem(s)");
    }
    Ok(())
}

fn fmt_age(d: Duration) -> String {
    let m = d.num_minutes();
    if m < 1 {
        "<1 min".into()
    } else if m < 120 {
        format!("{m} min")
    } else if m < 48 * 60 {
        format!("{} h", m / 60)
    } else {
        format!("{} days", m / 1440)
    }
}
