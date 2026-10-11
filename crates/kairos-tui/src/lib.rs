//! `kairos tui`: ratatui front end. Reads the local cache index (fast cold
//! start, works offline); writes go to kairosd.

pub mod app;
pub mod form;
pub mod ui;

use app::{App, Key, Mode};
use chrono::Utc;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use kairos_core::config::Config;
use kairos_store::cache;
use kairos_store::client::Daemon;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Load the app state the way the TUI does at startup (no terminal).
pub fn load(cfg: Config, path: &std::path::Path, now: chrono::DateTime<Utc>) -> Result<App, String> {
    let (from, to) = cache::default_window(&cfg, now);
    let (idx, _) = cache::get(&cfg, Some(path), now, from, to).map_err(|e| e.to_string())?;
    Ok(App::new(cfg, idx, now, None))
}

/// Render one frame to text (snapshot tests, perf budget).
pub fn render_to_string(app: &App, w: u16, h: u16) -> String {
    let Ok(mut term) = Terminal::new(TestBackend::new(w, h));
    let _ = term.draw(|f| ui::draw(f, app));
    let buf = term.backend().buffer();
    let mut out = String::new();
    for y in 0..h {
        let mut line = String::new();
        let mut skip = 0;
        for x in 0..w {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let s = buf[(x, y)].symbol();
            line.push_str(s);
            skip = unicode_width::UnicodeWidthStr::width(s).saturating_sub(1);
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

fn map_key(k: event::KeyEvent) -> Option<Key> {
    if k.kind != KeyEventKind::Press {
        return None;
    }
    if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
        return Some(Key::Char('q'));
    }
    Some(match k.code {
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Esc,
        KeyCode::Tab => Key::Tab,
        KeyCode::BackTab => Key::BackTab,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        _ => return None,
    })
}

/// Background: run the configured sync command (vdirsyncer pull on the Mac)
/// so writes made through kairosd show up quickly.
fn spawn_sync(cfg: &Config, done: mpsc::Sender<()>) {
    let cfg = cfg.clone();
    std::thread::spawn(move || {
        // kairosd pushes to iCloud in the background after a write; pulling at
        // once would usually miss it, so pull a little later and once more.
        for wait in [4, 11] {
            std::thread::sleep(Duration::from_secs(wait));
            let _ = kairos_store::sync::run_full(&cfg);
            let _ = done.send(());
        }
    });
}

pub fn run(cfg: Config, path: PathBuf) -> Result<(), String> {
    let now = Utc::now();
    let mut app = load(cfg.clone(), &path, now)?;
    app.daemon = Daemon::from_config(&cfg).ok().map(|d| d.interactive(true));
    let (tx, rx) = mpsc::channel::<()>();
    let mut terminal = ratatui::try_init().map_err(|e| e.to_string())?;
    let result = (|| -> Result<(), String> {
        let mut last_remote = Instant::now() - Duration::from_secs(3600);
        let mut last_reload = Instant::now();
        let mut syncing = false;
        loop {
            app.now = Utc::now();
            terminal.draw(|f| ui::draw(f, &app)).map_err(|e| e.to_string())?;
            // Remote state (pending approvals, tasks) every 30 s; first time right after the first frame.
            if last_remote.elapsed() > Duration::from_secs(30) {
                app.refresh_remote();
                last_remote = Instant::now();
                maybe_prompt_checkin(&mut app);
                continue;
            }
            if app.wants_sync && !syncing {
                app.wants_sync = false;
                syncing = true;
                spawn_sync(&app.cfg, tx.clone());
            }
            if rx.try_recv().is_ok() {
                syncing = false;
                last_reload = Instant::now() - Duration::from_secs(60);
            }
            // Reload the index if the vdir changed (cheap fingerprint check).
            if last_reload.elapsed() > Duration::from_secs(2) {
                let (from, to) = cache::default_window(&app.cfg, app.now);
                if let Ok((idx, rebuilt)) = cache::get(&app.cfg, Some(&path), app.now, from, to)
                    && rebuilt
                {
                    app.index = idx;
                }
                last_reload = Instant::now();
            }
            if event::poll(Duration::from_millis(500)).map_err(|e| e.to_string())?
                && let Event::Key(k) = event::read().map_err(|e| e.to_string())?
                && let Some(key) = map_key(k)
            {
                if app.message.is_some() && matches!(app.mode, Mode::Normal) {
                    app.message = None;
                }
                app.key(key);
                if app.quit {
                    return Ok(());
                }
            }
        }
    })();
    ratatui::restore();
    result
}

/// After the evening check-in time, show the check-in once per day.
fn maybe_prompt_checkin(app: &mut App) {
    let Some(hm) = kairos_core::constraints::Hm::parse(&app.cfg.notify.evening_checkin) else {
        return;
    };
    let local = app.now.with_timezone(&app.tz);
    let mins = (chrono::Timelike::hour(&local) * 60 + chrono::Timelike::minute(&local)) as u16;
    if mins < hm.minutes() || !matches!(app.mode, Mode::Normal) {
        return;
    }
    let marker = app.cfg.cache_dir().join("checkin-prompted");
    let today = local.date_naive().to_string();
    if std::fs::read_to_string(&marker).is_ok_and(|s| s.trim() == today) {
        return;
    }
    if app.checkins.iter().any(|c| c.outcome.is_none()) {
        app.mode = Mode::Checkin(0);
        let _ = std::fs::write(&marker, &today);
    }
}
