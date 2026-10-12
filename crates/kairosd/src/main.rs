use anyhow::{Context, Result, bail};
use clap::Parser;
use kairos_core::config::{Config, expand_home};
use kairos_core::constraints::Constraints;
use kairosd::engine::{Engine, NoNotifier, Notifier, ShellSyncer};
use kairosd::http::{self, AppState};
use kairosd::jobs::Jobs;
use kairosd::mcp;
use kairosd::notify::Ntfy;
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

fn sign_key(cfg: &Config) -> Result<Vec<u8>> {
    let p = cfg.data_dir().join("sign.key");
    if let Ok(k) = std::fs::read_to_string(&p) {
        return hex::decode(k.trim()).context("sign.key is not hex");
    }
    std::fs::create_dir_all(cfg.data_dir())?;
    let k = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
    std::fs::write(&p, &k)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(hex::decode(k)?)
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
    let key = sign_key(&cfg)?;
    let db = kairos_store::db::Db::open(&cfg.data_dir().join("kairos.db"))?;

    let ntfy = match (&cfg.notify.topic_cmd, &cfg.notify.public_url) {
        (Some(tc), Some(url)) => match kairos_store::run_secret_cmd(tc) {
            Ok(topic) if !topic.is_empty() => Some(Ntfy {
                server: cfg.notify.server.clone(),
                topic,
                token: cfg
                    .notify
                    .token_cmd
                    .as_deref()
                    .and_then(|c| kairos_store::run_secret_cmd(c).ok()),
                public_url: url.clone(),
                sign_key: key.clone(),
            }),
            Ok(_) | Err(_) => {
                tracing::warn!("notify.topic_cmd failed; notifications disabled");
                None
            }
        },
        _ => None,
    };
    let notifier: Box<dyn Notifier> = match &ntfy {
        Some(n) => Box::new(n.clone()),
        None => Box::new(NoNotifier),
    };
    let bind = args
        .bind
        .or_else(|| cfg.daemon.bind.clone())
        .unwrap_or_else(|| "127.0.0.1:7420".into());
    let mut hosts: Vec<String> = vec!["localhost".into(), "127.0.0.1".into(), "compute-node".into()];
    for u in [
        cfg.daemon.url.as_deref(),
        cfg.notify.public_url.as_deref(),
        Some(bind.as_str()),
    ]
    .into_iter()
    .flatten()
    {
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
        notifier,
        direct: false,
        replan_soon: true,
    };
    let st = AppState {
        engine: Arc::new(Mutex::new(engine)),
        token: Arc::new(token),
        sign_key: Arc::new(key),
    };
    let app = http::router(st.clone(), Some(mcp::router(st.clone(), hosts)));
    if !args.no_jobs {
        tokio::spawn(
            Jobs {
                engine: st.engine.clone(),
                ntfy,
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
