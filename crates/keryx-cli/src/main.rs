//! `keryx`: bare command opens the TUI; subcommands for everything else.

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use keryx_core::config::Config;
use std::path::PathBuf;

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
    /// Check the config and report each problem.
    Doctor,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let path = cli.config.unwrap_or_else(Config::default_path);
    match cli.cmd {
        Some(Cmd::Doctor) => {
            let cfg = Config::load(&path).with_context(|| format!("loading {}", path.display()))?;
            let errs = cfg.validate();
            if errs.is_empty() {
                println!(
                    "✓ config {} ({} accounts, {} keys, live = {})",
                    path.display(),
                    cfg.accounts.len(),
                    cfg.keys.len(),
                    cfg.live
                );
                Ok(())
            } else {
                for e in &errs {
                    println!("✗ {e}");
                }
                bail!("{} problem(s) in {}", errs.len(), path.display())
            }
        }
        None => bail!("the TUI arrives in Phase 3; try `keryx doctor`"),
    }
}
