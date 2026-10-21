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
    /// With --dump: keys to press first, by name, space-separated: `tab left enter`, `p down`.
    #[arg(long, value_name = "KEYS")]
    press: Option<String>,
    /// With --dump: open this project as Home's project list does (for one with no tmux session).
    #[arg(long, value_name = "ID")]
    project: Option<String>,
    /// Print what has been used, per screen and key, and exit.
    #[arg(long)]
    use_report: bool,
    /// Print how long loading and drawing each screen take (R5: first frame under 100 ms, every
    /// frame under 16 ms) and exit.
    #[arg(long)]
    timing: bool,
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
    if cli.timing {
        return timing(&cli.fixtures, &cli.size);
    }
    let fx = Fixtures::load(&fixtures::resolve(&cli.fixtures))?;
    if let Some(name) = cli.dump {
        let Some(screen) = ScreenId::from_name(&name) else {
            let all: Vec<_> = ScreenId::ALL.iter().map(|s| s.name()).collect();
            bail!("no screen {name:?}; screens: {}", all.join(", "));
        };
        let (w, h) = dump::parse_size(&cli.size).context("--size is WIDTHxHEIGHT, e.g. 95x38")?;
        // The project screens sit on the Project view: the root one following --focus, or one
        // opened from Home with --project.
        let project_screen = matches!(
            screen,
            ScreenId::Project | ScreenId::Decisions | ScreenId::Answered | ScreenId::Changes | ScreenId::Prep
        );
        let root = if project_screen && cli.project.is_none() {
            ScreenId::Project
        } else {
            ScreenId::Home
        };
        let mut app = App::new(fx, root, UseLog::off());
        if let Some(id) = cli.project {
            if app.fixtures.registry.get(&id).is_none() {
                bail!("no project {id:?} in the registry");
            }
            app.viewing = Some(id);
            if project_screen {
                app.open(ScreenId::Project);
            }
        }
        if screen != app.screen() {
            app.open(screen);
        }
        app.keys_open = cli.keys;
        app.follow.session = cli.focus;
        // A first draw, so sections that record what they drew know it before keys arrive.
        dump::render(&app, w, h)?;
        for name in cli.press.as_deref().unwrap_or("").split_whitespace() {
            use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
            let code = match name {
                "enter" => KeyCode::Enter,
                "esc" => KeyCode::Esc,
                "tab" => KeyCode::Tab,
                "up" => KeyCode::Up,
                "down" => KeyCode::Down,
                "left" => KeyCode::Left,
                "right" => KeyCode::Right,
                "space" => KeyCode::Char(' '),
                c if c.chars().count() == 1 => KeyCode::Char(c.chars().next().unwrap_or(' ')),
                other => bail!("--press: no key {other:?}"),
            };
            app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
            dump::render(&app, w, h)?;
        }
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

/// Load the set and draw every screen, best of five draws each, into an in-memory terminal: the
/// work a frame costs before the terminal writes it out.
fn timing(set: &str, size: &str) -> Result<()> {
    let (w, h) = dump::parse_size(size).context("--size is WIDTHxHEIGHT, e.g. 95x38")?;
    let t0 = Instant::now();
    let fx = Fixtures::load(&fixtures::resolve(set))?;
    let load = t0.elapsed();
    let mut slowest = (Duration::ZERO, "");
    let mut first = Duration::ZERO;
    for screen in ScreenId::ALL {
        let mut app = App::new(fx.clone(), ScreenId::Home, UseLog::off());
        app.follow.session = Some("quast".into());
        app.viewing = Some("jump".into());
        if screen != ScreenId::Home {
            app.open(screen);
        }
        let mut best = Duration::MAX;
        for _ in 0..5 {
            let t = Instant::now();
            dump::render(&app, w, h)?;
            best = best.min(t.elapsed());
        }
        if screen == ScreenId::Home {
            first = load + best;
        }
        println!("{:<10} {:>7.2} ms", screen.name(), best.as_secs_f64() * 1000.0);
        if best > slowest.0 {
            slowest = (best, screen.name());
        }
    }
    println!("load fixtures {:.2} ms", load.as_secs_f64() * 1000.0);
    println!(
        "first frame (load + home) {:.2} ms, target under 100",
        first.as_secs_f64() * 1000.0
    );
    println!(
        "slowest frame {} {:.2} ms, target under 16",
        slowest.1,
        slowest.0.as_secs_f64() * 1000.0
    );
    Ok(())
}

fn run(mut app: App) -> Result<()> {
    let started = Instant::now();
    let mut first_frame = None;
    let mut slowest = Duration::ZERO;
    let mut asked = false;
    app.in_session = std::env::var_os("LOCUS_IN_SESSION").is_some();
    app.peek_file = Some(follow::PeekFile::new(follow::state_dir()));
    let mut peek = follow::PeekFile::new(follow::state_dir());
    let mut focus = FocusFile::new(follow::state_dir());
    app.tick(focus.poll(), Instant::now());
    let mut terminal = ratatui::try_init()?;
    let result = (|| -> Result<()> {
        while !app.quit {
            app.tick(focus.poll(), Instant::now());
            if let Some(change) = peek.poll()
                && app.stack.first() == Some(&ScreenId::Project)
            {
                app.peek = change;
                app.stack.truncate(1);
                app.focus = None;
            }
            app.frame = app.frame.wrapping_add(1);
            // Ask the terminal which image protocol it speaks (D6: kitty in Ghostty, passed
            // through tmux) only when a screen first needs it: a terminal that never answers
            // costs the whole timeout, and at startup that was 2 s before the first frame
            // (measured on a tmux server with no terminal behind it).
            if app.screen() == ScreenId::InkConfirm && !asked {
                asked = true;
                let options = ratatui_image::picker::cap_parser::QueryStdioOptions {
                    timeout: Duration::from_millis(500),
                    ..Default::default()
                };
                match ratatui_image::picker::Picker::from_query_stdio_with_options(options) {
                    Ok(p) => app.picker = Some(p),
                    Err(e) => app.note = Some(format!("no image protocol ({e}); the crop is in half blocks")),
                }
            }
            let t = Instant::now();
            terminal.draw(|f| ui::draw(f, &app))?;
            slowest = slowest.max(t.elapsed());
            first_frame.get_or_insert_with(|| started.elapsed());
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
    if let Some(ff) = first_frame {
        eprintln!(
            "locus-tui: first frame {:.1} ms · slowest frame {:.1} ms",
            ff.as_secs_f64() * 1000.0,
            slowest.as_secs_f64() * 1000.0
        );
    }
    result
}
