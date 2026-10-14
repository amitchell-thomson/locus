//! `keryx` (bare): the ratatui front end. Reads the mirror read-only (works
//! without keryxd); writes will go to keryxd (Phase 4).

pub mod app;
pub mod theme;
pub mod ui;

use app::{App, Key};
use chrono::Utc;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use keryx_core::config::Config;
use keryx_store::db::Db;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use std::time::{Duration, Instant, SystemTime};

/// Render one frame to text (snapshot tests, perf budget).
pub fn render_to_string(app: &mut App, w: u16, h: u16) -> String {
    app.width = w;
    if app.split() {
        app.ensure_preview();
    }
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
    if k.modifiers.contains(KeyModifiers::CONTROL)
        && let KeyCode::Char(c) = k.code
    {
        return Some(if c == 'c' { Key::Char('q') } else { Key::Ctrl(c) });
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
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        _ => return None,
    })
}

/// `KERYX_NOW=<rfc3339>` previews the TUI at another time.
pub fn now(cfg: &Config) -> chrono::DateTime<chrono_tz::Tz> {
    let t = std::env::var("KERYX_NOW")
        .ok()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
        .map(|t| t.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);
    t.with_timezone(&cfg.tz())
}

/// When the mirror last changed (WAL or main file).
fn db_stamp(cfg: &Config) -> Option<SystemTime> {
    let main = cfg.db_path();
    let wal = main.with_file_name("keryx.db-wal");
    [main, wal]
        .iter()
        .filter_map(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
        .max()
}

pub fn run(cfg: Config) -> Result<(), String> {
    let path = cfg.db_path();
    if !path.exists() {
        return Err(format!(
            "no mirror at {} yet: start keryxd (scripts/install-daemon.sh) or run `keryxd --once`",
            path.display()
        ));
    }
    let db = Db::open_readonly(&path).map_err(|e| e.to_string())?;
    let mut app = App::new(cfg.clone(), db, now(&cfg));
    let mut terminal = ratatui::try_init().map_err(|e| e.to_string())?;
    let result = (|| -> Result<(), String> {
        let mut stamp = db_stamp(&cfg);
        let mut last_check = Instant::now();
        loop {
            app.now = now(&cfg);
            let size = terminal.size().map_err(|e| e.to_string())?;
            app.width = size.width;
            if app.split() {
                app.ensure_preview();
            }
            terminal.draw(|f| ui::draw(f, &app)).map_err(|e| e.to_string())?;
            // keryxd writes the mirror; pick changes up within ~2 s.
            if last_check.elapsed() > Duration::from_secs(2) {
                let s = db_stamp(&cfg);
                if s != stamp {
                    stamp = s;
                    app.reload();
                }
                last_check = Instant::now();
            }
            if event::poll(Duration::from_millis(500)).map_err(|e| e.to_string())?
                && let Event::Key(k) = event::read().map_err(|e| e.to_string())?
                && let Some(key) = map_key(k)
            {
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
