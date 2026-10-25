use anyhow::{Context, Result, bail};
use clap::Parser;
use kairos_core::config::{Config, expand_home};
use kairos_core::constraints::Constraints;
use kairosd::engine::{Engine, ShellSyncer};
use kairosd::http::{self, AppState};
use kairosd::jobs::Jobs;
use kairosd::mcp;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// kairos server daemon: the single writer to iCloud.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Config file (default $KAIROS_CONFIG or ~/.config/kairos/config.toml).
    #[arg(long)]
    config: Option<PathBuf>,
    /// Scheduler constraints (default: constraints.toml next to the config).
    #[arg(long)]
    constraints: Option<PathBuf>,
    /// Listen address (overrides daemon.bind). Use the Tailscale IP.
    #[arg(long)]
    bind: Option<String>,
    /// Allow writes to real calendars (same as live = true in config).
    #[arg(long)]
    live: bool,
    /// Disable background jobs (sync, feeds, notifications).
    #[arg(long)]
    no_jobs: bool,
}

fn token(cfg: &Config) -> Result<String> {
    if let Ok(t) = std::env::var("KAIROS_TOKEN")
        && !t.trim().is_empty()
    {
        return Ok(t.trim().to_string());
    }
    if let Ok(dir) = std::env::var("CREDENTIALS_DIRECTORY") {
        let p = PathBuf::from(dir).join("kairos-token");
        if let Ok(t) = std::fs::read_to_string(&p) {
            return Ok(t.trim().to_string());
        }
    }
    if let Some(cmd) = &cfg.daemon.token_cmd {
        return kairos_store::run_secret_cmd(cmd).context("daemon.token_cmd");
    }
    bail!("no API token: set KAIROS_TOKEN, a systemd credential `kairos-token`, or daemon.token_cmd")
}

fn host_of(url: &str) -> Option<String> {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let host = rest.split(['/', ':']).next()?;
    (!host.is_empty()).then(|| host.to_string())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_target(false)
        // journald adds its own timestamps/colours; keep the stream plain there.
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .with_writer(std::io::stderr)
        .init();
    let args = Args::parse();
    let cfg_path = args.config.unwrap_or_else(Config::default_path);
    let mut cfg = Config::load(&cfg_path).with_context(|| format!("loading {}", cfg_path.display()))?;
    let errs = cfg.validate();
    if !errs.is_empty() {
        for e in &errs {
            tracing::error!("config: {e}");
        }
        bail!("invalid config ({} problem(s))", errs.len());
    }
    if args.live {
        cfg.live = true;
    }
    let cons_path = args.constraints.unwrap_or_else(|| {
        cfg_path
            .parent()
            .map_or_else(|| expand_home("~/.config/kairos"), PathBuf::from)
            .join("constraints.toml")
    });
    let cons_text = std::fs::read_to_string(&cons_path).with_context(|| format!("reading {}", cons_path.display()))?;
    let constraints =
        Constraints::from_toml(&cons_text).map_err(|e| anyhow::anyhow!("{}: {e}", cons_path.display()))?;
    let token = token(&cfg)?;
    if token.len() < 24 {
        bail!("API token is too short (need ≥ 24 chars)");
    }
    let db = kairos_store::db::Db::open(&cfg.data_dir().join("kairos.db"))?;

    let bind = args
        .bind
        .or_else(|| cfg.daemon.bind.clone())
        .unwrap_or_else(|| "127.0.0.1:7420".into());
    let mut hosts: Vec<String> = vec!["localhost".into(), "127.0.0.1".into(), "compute-node".into()];
    for u in [cfg.daemon.url.as_deref(), Some(bind.as_str())].into_iter().flatten() {
        if let Some(h) = host_of(u) {
            hosts.push(h);
        }
    }
    hosts.sort();
    hosts.dedup();
    tracing::info!(live = cfg.live, test_calendar = %cfg.test_calendar, %bind, "starting kairosd");

    let engine = Engine {
        syncer: Box::new(ShellSyncer {
            cmd: cfg.sync.collection_cmd.clone(),
        }),
        cfg: cfg.clone(),
        cfg_path: Some(cfg_path.clone()),
        constraints,
        db,
        clock: Arc::new(chrono::Utc::now),
        direct: false,
        replan_soon: true,
        // The locus registry (spec K7): kairos's projects are its repos.
        registry: kairos_store::taxonomy::local().ok().map(|s| s.registry),
    };
    let mut engine = engine;
    match engine.sync_registry() {
        Ok(Some(cs)) => tracing::info!("{}", cs.summary),
        Ok(None) => {}
        Err(e) => tracing::warn!("projects from the registry: {e}"),
    }
    // Project sessions were removed (2026-10-25): their events keep the link as what they are for.
    engine.retire_sessions().context("retiring project sessions")?;
    // ⚑ events from before due dates were task attributes (spec K5): proposed once for approval.
    if let Err(e) = engine.migrate_deadlines() {
        tracing::warn!("migrating deadlines: {e}");
    }
    let st = AppState {
        engine: Arc::new(Mutex::new(engine)),
        token: Arc::new(token),
    };
    let app = http::router(st.clone(), Some(mcp::router(st.clone(), hosts)));
    if !args.no_jobs {
        tokio::spawn(
            Jobs {
                engine: st.engine.clone(),
                sync_every_min: cfg.sync.every_minutes,
                feeds_every_min: 10,
            }
            .run(),
        );
    }
    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("binding {bind}"))?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let ctrl_c = tokio::signal::ctrl_c();
            #[cfg(unix)]
            {
                let mut term = match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                    Ok(s) => s,
                    Err(_) => {
                        let _ = ctrl_c.await;
                        return;
                    }
                };
                tokio::select! { _ = ctrl_c => {}, _ = term.recv() => {} }
            }
            #[cfg(not(unix))]
            {
                let _ = ctrl_c.await;
            }
            tracing::info!("shutting down");
        })
        .await?;
    Ok(())
}
