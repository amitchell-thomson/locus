//! keryxd: the only writer. Phase 0 only loads and validates the config.

use anyhow::{Context, bail};
use clap::Parser;
use keryx_core::config::Config;
use std::path::PathBuf;

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
}

fn main() -> anyhow::Result<()> {
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
    bail!("keryxd isn't built yet (Phase 1); use --check to validate the config")
}
