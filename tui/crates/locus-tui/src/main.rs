//! `locus-tui [home|project]` runs the window; `--dump` prints a screen as text.

use anyhow::{Context, Result, bail};
use clap::Parser;
use locus_tui::app::App;
use locus_tui::fixtures::{self, Fixtures};
use locus_tui::screen::ScreenId;
use locus_tui::uselog::{self, UseLog};
use locus_tui::{dump, ui};
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use std::time::Duration;

#[derive(Parser)]
#[command(about = "The locus window, on fixture data (Slice 1)")]
struct Cli {
    /// Which root to run: home or project.
    #[arg(default_value = "home")]
    view: String,
    /// Fixture set: a name under tui/fixtures/ or a path.
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
    /// Print what has been used, per screen and key, and exit.
    #[arg(long)]
    use_report: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.use_report {
        let path = uselog::default_path().context("no HOME to find the use log under")?;
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        print!("{}", uselog::report(&text));
        return Ok(());
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

fn run(mut app: App) -> Result<()> {
    let mut terminal = ratatui::try_init()?;
    let result = (|| -> Result<()> {
        while !app.quit {
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
