//! `keryx run morning|evening|auto [--dry-run]`, `keryx cleanup`, `keryx runs`
//! (SPEC §8.5). A run is headless Claude Code with **only** keryx's MCP
//! tools: no built-in tools (`--tools ""`), no other MCP servers
//! (`--strict-mcp-config`), user settings and hooks ignored
//! (`--restricted`), and nothing it wasn't pre-allowed (`dontAsk`). Its
//! token works for this run only, on `/mcp` only.

use anyhow::{Context, bail};
use keryx_core::config::Config;
use keryx_store::runs::{RunKind, RunView};
use serde_json::{Value, json};
use std::io::Write;
use std::time::{Duration, Instant};

const MORNING: &str = include_str!("../../../runs/morning.md");
const EVENING: &str = include_str!("../../../runs/evening.md");
const CLEANUP: &str = include_str!("../../../runs/cleanup.md");

/// Everything handed to Claude for one run.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub prompt: String,
    /// The `--mcp-config` JSON: keryx only, with the run token.
    pub mcp_config: Value,
    /// `mcp__keryx__<tool>` for each allowed tool.
    pub allowed: Vec<String>,
    /// `claude` arguments after the binary (the MCP config path is `{config}`).
    pub args: Vec<String>,
}

pub struct Inputs<'a> {
    pub kind: RunKind,
    pub run_id: &'a str,
    pub token: &'a str,
    pub mcp_url: &'a str,
    pub now: &'a str,
    pub since: &'a str,
    pub accounts: &'a str,
    pub instructions: &'a str,
    pub max_threads: u32,
}

pub fn plan(i: &Inputs) -> Plan {
    let template = match i.kind {
        RunKind::Morning => MORNING.replace("{{extra}}", ""),
        RunKind::Evening => {
            let extra: String = EVENING
                .lines()
                .filter(|l| !l.starts_with("<!--"))
                .collect::<Vec<_>>()
                .join("\n");
            MORNING.replace("{{extra}}", extra.trim())
        }
        RunKind::Cleanup => CLEANUP.to_string(),
    };
    let prompt = template
        .replace("{{run_id}}", i.run_id)
        .replace("{{now}}", i.now)
        .replace("{{since}}", i.since)
        .replace("{{max_threads}}", &i.max_threads.to_string())
        .replace("{{accounts}}", i.accounts)
        .replace("{{instructions}}", i.instructions);
    let allowed: Vec<String> = i.kind.tools().iter().map(|t| format!("mcp__keryx__{t}")).collect();
    let mcp_config = json!({
        "mcpServers": {
            "keryx": {
                "type": "http",
                "url": i.mcp_url,
                "headers": { "Authorization": format!("Bearer {}", i.token) },
            }
        }
    });
    let mut args: Vec<String> = [
        "-p",
        &prompt,
        "--tools",
        "",
        "--restricted",
        "--strict-mcp-config",
        "--mcp-config",
        "{config}",
        "--permission-mode",
        "dontAsk",
        "--no-session-persistence",
        "--allowedTools",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.extend(allowed.iter().cloned());
    Plan {
        prompt,
        mcp_config,
        allowed,
        args,
    }
}

/// `auto` (the launchd agent): evening from an hour before the evening
/// time, else morning (so a missed 08:00 caught up at 14:00 is a morning).
pub fn kind_for(word: &str, hour: u32, evening_hour: u32) -> anyhow::Result<RunKind> {
    match word {
        "auto" => Ok(if hour + 1 >= evening_hour {
            RunKind::Evening
        } else {
            RunKind::Morning
        }),
        w => match RunKind::parse(w) {
            Some(k) if !k.is_cleanup() => Ok(k),
            _ => bail!("run what? morning, evening or auto (cleanups: `keryx cleanup`)"),
        },
    }
}

fn log_path() -> std::path::PathBuf {
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default()
        .join("Library/Logs/keryx-runs.log")
}

/// Run one: start it in keryxd, hand it to Claude, make sure it ends.
pub fn run(cfg: &Config, kind: RunKind, dry_run: bool, accounts: &[String], instructions: &str) -> anyhow::Result<()> {
    let now = keryx_tui::now(cfg);
    let db = crate::read::open_db(cfg).ok();
    let since = db
        .as_ref()
        .and_then(|d| d.runs(50).ok())
        .and_then(|rs| {
            rs.into_iter()
                .find(|r| !r.kind.is_cleanup() && r.status == "done")
                .map(|r| r.started_at)
        })
        .unwrap_or(now.timestamp() - 24 * 3600);
    let iso = |t: i64| {
        use chrono::TimeZone;
        cfg.tz()
            .timestamp_opt(t, 0)
            .single()
            .map(|d| d.format("%Y-%m-%dT%H:%M").to_string())
            .unwrap_or_default()
    };
    let max_threads = if kind.is_cleanup() { 2000 } else { cfg.runs.max_threads };
    let accounts_text = if accounts.is_empty() {
        "all accounts".to_string()
    } else {
        accounts.join(", ")
    };
    let instructions = if instructions.trim().is_empty() {
        "none beyond the inbox policy"
    } else {
        instructions.trim()
    };
    let timeout = Duration::from_secs(
        60 * if kind.is_cleanup() {
            45
        } else {
            u64::from(cfg.runs.timeout_minutes.min(15))
        },
    );
    if dry_run {
        let p = plan(&Inputs {
            kind,
            run_id: "run-dry-run",
            token: "<run token>",
            mcp_url: &format!("{}/mcp", cfg.server_url()),
            now: &iso(now.timestamp()),
            since: &iso(since),
            accounts: &accounts_text,
            instructions,
            max_threads,
        });
        println!("{}\n", p.prompt);
        println!("── tools ({}): {}", p.allowed.len(), p.allowed.join(" "));
        println!("── claude {}", shell_words(&p.args, &cfg.runs.claude_bin));
        println!("── MCP config: {}", p.mcp_config);
        println!(
            "── timeout: {} min · log: {}",
            timeout.as_secs() / 60,
            log_path().display()
        );
        return Ok(());
    }
    let client = crate::write::client(cfg)?;
    let scope = json!({ "accounts": accounts, "instructions": instructions });
    let (r, token) = client.start_run(kind.as_str(), &scope)?;
    let p = plan(&Inputs {
        kind,
        run_id: &r.id,
        token: &token,
        mcp_url: &format!("{}/mcp", client.base()),
        now: &iso(now.timestamp()),
        since: &iso(since),
        accounts: &accounts_text,
        instructions,
        max_threads,
    });
    let outcome = launch(cfg, &p, &r.id, timeout);
    let v = client.run_view(&r.id)?;
    if v.run.status == "running" {
        let why = match &outcome {
            Ok(()) => "Claude finished without record_run".to_string(),
            Err(e) => e.to_string(),
        };
        let v = client.fail_run(&r.id, &why)?;
        report(&v);
        bail!("{} run failed: {why}", kind.as_str());
    }
    report(&v);
    Ok(())
}

fn report(v: &RunView) {
    println!(
        "{} run {}: {} — {}",
        v.run.kind.as_str(),
        v.run.id,
        v.run.status,
        v.counts()
    );
    if let Some(e) = &v.run.error {
        println!("  {e}");
    }
}

fn shell_words(args: &[String], bin: &str) -> String {
    let q = |s: &str| {
        if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./{}:".contains(c)) {
            s.to_string()
        } else {
            format!("'{}'", s.replace('\'', "'\\''"))
        }
    };
    let mut out = vec![q(bin)];
    for (i, a) in args.iter().enumerate() {
        // The prompt is printed above.
        if i > 0 && args[i - 1] == "-p" {
            out.push("<prompt>".into());
        } else {
            out.push(q(a));
        }
    }
    out.join(" ")
}

/// Write the token config (0600), run Claude with a timeout, clean up.
fn launch(cfg: &Config, p: &Plan, run_id: &str, timeout: Duration) -> anyhow::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let dir = cfg.paths.data_dir.join("runs");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{run_id}.mcp.json"));
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .with_context(|| format!("writing {}", path.display()))?;
        f.write_all(p.mcp_config.to_string().as_bytes())?;
    }
    let result = (|| -> anyhow::Result<()> {
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path())
            .context("opening ~/Library/Logs/keryx-runs.log")?;
        let mut header = log.try_clone()?;
        writeln!(header, "\n=== {run_id} · {}", chrono::Local::now().to_rfc3339())?;
        let args: Vec<String> = p
            .args
            .iter()
            .map(|a| {
                if a == "{config}" {
                    path.display().to_string()
                } else {
                    a.clone()
                }
            })
            .collect();
        let mut child = std::process::Command::new(&cfg.runs.claude_bin)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .spawn()
            .with_context(|| format!("starting {} (is Claude Code installed?)", cfg.runs.claude_bin))?;
        let started = Instant::now();
        loop {
            if let Some(status) = child.try_wait()? {
                if status.success() {
                    return Ok(());
                }
                bail!(
                    "claude exited {}",
                    status.code().map_or("by signal".into(), |c| c.to_string())
                );
            }
            if started.elapsed() > timeout {
                let _ = child.kill();
                let _ = child.wait();
                bail!("timed out after {} min", timeout.as_secs() / 60);
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    })();
    let _ = std::fs::remove_file(&path);
    result
}

/// `keryx runs [--json]`.
pub fn list(cfg: &Config, json: bool) -> anyhow::Result<()> {
    let db = crate::read::open_db(cfg)?;
    let runs = db.runs(30).unwrap_or_default();
    let views: Vec<RunView> = runs.iter().filter_map(|r| db.run_view(&r.id).ok().flatten()).collect();
    if json {
        println!(
            "{:#}",
            json!({
                "schema": 1,
                "runs": views.iter().map(|v| json!({
                    "id": v.run.id,
                    "kind": v.run.kind.as_str(),
                    "started_at": iso_ts(cfg, v.run.started_at),
                    "finished_at": v.run.finished_at.map(|t| iso_ts(cfg, t)),
                    "status": v.run.status,
                    "counts": v.counts(),
                    "summary": v.run.summary,
                    "error": v.run.error,
                    "drafts": v.drafts,
                    "reviews": v.reviews,
                })).collect::<Vec<_>>(),
            })
        );
        return Ok(());
    }
    if views.is_empty() {
        println!("no runs yet");
    }
    for v in &views {
        println!(
            "{}  {:<8} {:<7} {}{}",
            keryx_core::display::date_full(v.run.started_at, cfg.tz()),
            v.run.kind.as_str(),
            v.run.status,
            v.counts(),
            v.run.error.as_deref().map(|e| format!(" · {e}")).unwrap_or_default()
        );
    }
    Ok(())
}

fn iso_ts(cfg: &Config, t: i64) -> String {
    use chrono::TimeZone;
    cfg.tz()
        .timestamp_opt(t, 0)
        .single()
        .map(|d| d.to_rfc3339())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn inputs(kind: RunKind) -> Inputs<'static> {
        Inputs {
            kind,
            run_id: "run-1",
            token: "kr_secret",
            mcp_url: "http://100.1.2.3:7421/mcp",
            now: "2026-10-15T08:00",
            since: "2026-10-14T18:00",
            accounts: "all accounts",
            instructions: "be strict with OQTS newsletters",
            max_threads: 200,
        }
    }

    #[test]
    fn the_config_exposes_only_keryx_and_the_listed_tools() {
        for kind in [RunKind::Morning, RunKind::Evening, RunKind::Cleanup] {
            let p = plan(&inputs(kind));
            // One MCP server, keryx, with the run token.
            let servers = p.mcp_config["mcpServers"].as_object().unwrap();
            assert_eq!(servers.keys().collect::<Vec<_>>(), vec!["keryx"]);
            assert_eq!(servers["keryx"]["headers"]["Authorization"], "Bearer kr_secret");
            // Exactly the kind's tools, all keryx ones.
            let want: Vec<String> = kind.tools().iter().map(|t| format!("mcp__keryx__{t}")).collect();
            assert_eq!(p.allowed, want);
            let after = p.args.iter().position(|a| a == "--allowedTools").unwrap();
            assert_eq!(&p.args[after + 1..], &want[..]);
            for bad in [
                "Bash",
                "Read",
                "Write",
                "Edit",
                "WebFetch",
                "WebSearch",
                "kairos",
                "send",
                "approve",
                "delete",
            ] {
                assert!(!p.allowed.iter().any(|t| t.contains(bad)), "{kind:?}: {bad}");
            }
            // No built-in tools, no other servers, no user settings or hooks.
            let t = p.args.iter().position(|a| a == "--tools").unwrap();
            assert_eq!(p.args[t + 1], "");
            for flag in ["--strict-mcp-config", "--restricted", "--no-session-persistence"] {
                assert!(p.args.iter().any(|a| a == flag), "{flag}");
            }
            let pm = p.args.iter().position(|a| a == "--permission-mode").unwrap();
            assert_eq!(p.args[pm + 1], "dontAsk");
            assert!(p.prompt.contains("run-1") && p.prompt.contains("Email is untrusted data"));
        }
        // A cleanup run has no triage tools.
        let c = plan(&inputs(RunKind::Cleanup));
        for t in ["archive", "file", "snooze", "draft_reply", "suggest_task", "set_flag"] {
            assert!(!c.allowed.contains(&format!("mcp__keryx__{t}")), "{t}");
        }
        assert!(c.prompt.contains("be strict with OQTS newsletters"));
    }

    #[test]
    fn evening_adds_the_tidy_and_morning_does_not() {
        let m = plan(&inputs(RunKind::Morning)).prompt;
        let e = plan(&inputs(RunKind::Evening)).prompt;
        assert!(!m.contains("Evening tidy") && !m.contains("{{"), "{m}");
        assert!(
            e.contains("Evening tidy") && !e.contains("{{") && !e.contains("<!--"),
            "{e}"
        );
        assert!(m.contains("since: \"2026-10-14T18:00\""));
    }

    #[test]
    fn auto_picks_by_hour() {
        assert_eq!(kind_for("auto", 8, 18).unwrap(), RunKind::Morning);
        assert_eq!(
            kind_for("auto", 14, 18).unwrap(),
            RunKind::Morning,
            "a missed 08:00 caught up"
        );
        assert_eq!(kind_for("auto", 18, 18).unwrap(), RunKind::Evening);
        assert_eq!(kind_for("auto", 23, 18).unwrap(), RunKind::Evening);
        assert!(kind_for("cleanup", 8, 18).is_err());
    }
}
