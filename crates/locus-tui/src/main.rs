//! `locus-tui [home|project]` runs the window; `--dump` prints a screen as text.

use anyhow::{Context, Result, bail};
use clap::Parser;
use locus_tui::app::App;
use locus_tui::fixtures::{self, Fixtures};
use locus_tui::follow::{self, FocusFile};
use locus_tui::screen::ScreenId;
use locus_tui::uselog::{self, UseLog};
use locus_tui::{dump, ui};
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(about = "The locus window, on fixture data (Slice 1)")]
struct Cli {
    /// Which root to run: home or project.
    #[arg(default_value = "home")]
    view: String,
    /// Fixture set: a name under crates/locus-tui/fixtures/ or a path.
    #[arg(long, default_value = "default")]
    fixtures: String,
    /// Print this screen as text and exit.
    #[arg(long, value_name = "SCREEN")]
    dump: Option<String>,
    /// With --dump: the window size.
    #[arg(long, default_value = "95x38")]
    size: String,
    /// With --dump: draw the keys overlay on top.
    #[arg(long)]
    keys: bool,
    /// With --dump: the tmux session the project window is on.
    #[arg(long, value_name = "SESSION")]
    focus: Option<String>,
    /// Print what has been used, per screen and key, and exit.
    #[arg(long)]
    use_report: bool,
    /// Load every fixture set strictly, cross-check it, print each file's verdict and exit.
    #[arg(long)]
    validate_fixtures: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.use_report {
        let path = uselog::default_path().context("no HOME to find the use log under")?;
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        print!("{}", uselog::report(&text));
        return Ok(());
    }
    if cli.validate_fixtures {
        return validate();
    }
    let fx = Fixtures::load(&fixtures::resolve(&cli.fixtures))?;
    if let Some(name) = cli.dump {
        let Some(screen) = ScreenId::from_name(&name) else {
            let all: Vec<_> = ScreenId::ALL.iter().map(|s| s.name()).collect();
            bail!("no screen {name:?}; screens: {}", all.join(", "));
        };
        let (w, h) = dump::parse_size(&cli.size).context("--size is WIDTHxHEIGHT, e.g. 95x38")?;
        let root = if screen == ScreenId::Project {
            ScreenId::Project
        } else {
            ScreenId::Home
        };
        let mut app = App::new(fx, root, UseLog::off());
        if screen != root {
            app.open(screen);
        }
        app.keys_open = cli.keys;
        app.follow.session = cli.focus;
        print!("{}", dump::render(&app, w, h)?);
        return Ok(());
    }
    let root = match cli.view.as_str() {
        "home" => ScreenId::Home,
        "project" => ScreenId::Project,
        other => bail!("view is home or project, not {other:?}"),
    };
    run(App::new(fx, root, UseLog::new(uselog::default_path())))
}

/// Every set under fixtures/: a file that fails to parse is named by the loader's error; the
/// cross-file checks then run over the whole set.
fn validate() -> Result<()> {
    let mut failed = false;
    let mut sets: Vec<_> = std::fs::read_dir(fixtures::sets_dir())?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("manifest.json").is_file())
        .collect();
    sets.sort();
    for dir in sets {
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let files = Fixtures::files(&dir);
        match Fixtures::load(&dir) {
            Err(e) => {
                failed = true;
                println!("{name}: FAILED {e:#}");
            }
            Ok(fx) => {
                let read = fx.read_files();
                let mut bad = fx.check();
                for f in &files {
                    if read.contains(f) {
                        println!("{name}/{f}: OK");
                    } else {
                        bad.push(format!("{f} is not read by any screen"));
                    }
                }
                for b in &bad {
                    println!("{name}: {b}");
                }
                failed |= !bad.is_empty();
                println!(
                    "{name}: {} files, {} cross-file problems",
                    files.len(),
                    if bad.is_empty() {
                        "no".to_string()
                    } else {
                        bad.len().to_string()
                    }
                );
            }
        }
    }
    if failed {
        bail!("fixtures failed validation");
    }
    Ok(())
}

fn run(mut app: App) -> Result<()> {
    let mut focus = FocusFile::new(follow::state_dir());
    app.tick(focus.poll(), Instant::now());
    let mut terminal = ratatui::try_init()?;
    let result = (|| -> Result<()> {
        while !app.quit {
            app.tick(focus.poll(), Instant::now());
            terminal.draw(|f| ui::draw(f, &app))?;
            if event::poll(Duration::from_millis(250))?
                && let Event::Key(k) = event::read()?
                && k.kind == KeyEventKind::Press
            {
                app.on_key(k);
            }
        }
        Ok(())
    })();
    ratatui::restore();
    result
}
