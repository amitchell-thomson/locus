//! `keryx` (bare): the ratatui front end. Reads the mirror read-only (works
//! without keryxd); writes will go to keryxd (Phase 4).

pub mod app;
pub mod drafts;
pub mod file_for;
pub mod filed;
pub mod form;
mod kairos;
pub mod pane;
pub mod pick;
mod runs;
pub mod send;
pub mod theme;
pub mod ui;
mod ui_kairos;
mod ui_runs;
mod ui_send;
mod ui_write;
pub mod write;
pub mod writing;

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
        KeyCode::Tab if k.modifiers.contains(KeyModifiers::ALT) => Key::AltTab,
        KeyCode::Tab => Key::Tab,
        KeyCode::BackTab => Key::BackTab,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Up => Key::Up,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
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
    run_at(cfg, None)
}

/// The TUI, opened on a thread (`keryx open keryx://thread/…`).
pub fn run_at(cfg: Config, thread: Option<(String, String)>) -> Result<(), String> {
    let path = cfg.db_path();
    if !path.exists() {
        return Err(format!(
            "no mirror at {} yet: start keryxd (deploy/keryx/install-daemon.sh) or run `keryxd --once`",
            path.display()
        ));
    }
    let db = Db::open_readonly(&path).map_err(|e| e.to_string())?;
    let mut app = App::new(cfg.clone(), db, now(&cfg));
    app.load_seen();
    app.drafts_dir = locus_contract::link::state_dir().map(|d| d.join("keryx-drafts"));
    // What mail is filed for (X7): `d` archives plainly when the taxonomy cannot be read.
    app.shared = locus_contract::taxonomy::load(&locus_contract::taxonomy::root()).ok();
    let editor = writing::embeddable_editor();
    let mut session: Option<writing::Session> = None;
    let mut cursor_shape: u16 = 0;
    app.daemon = keryx_store::client::Client::from_config(&cfg)
        .ok()
        .map(|c| Box::new(c.interactive(true)) as Box<dyn keryx_store::client::Daemon>);
    if let Some((a, t)) = thread
        && !app.open_thread(&a, &t)
    {
        return Err(format!("no thread {a}:{t} in the mirror"));
    }
    let mut terminal = ratatui::try_init().map_err(|e| e.to_string())?;
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste);
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
            let full = ratatui::layout::Rect::new(0, 0, size.width, size.height);
            if let Some(s) = &mut session {
                let r = ui::writing_rect(&app, full);
                s.pane.resize(r.height, r.width);
            }
            terminal
                .draw(|f| ui::draw_writing(f, &app, session.as_ref().map(|s| &s.pane)))
                .map_err(|e| e.to_string())?;
            // Your terminal's cursor takes the shape Neovim asks for (block, bar), and your own
            // default again once the editor closes.
            let shape = session.as_ref().map_or(0, |s| s.pane.cursor_shape());
            if shape != cursor_shape {
                let _ = crossterm::execute!(std::io::stdout(), cursor_style(shape));
                cursor_shape = shape;
            }
            if let Some(req) = app.editor_request.take() {
                match &editor {
                    // Your Neovim in the right pane, the list beside it (X6).
                    Some(ed) => {
                        app.writing_started(req.purpose.clone());
                        let r = ui::writing_rect(&app, full);
                        let draft = app.draft_path(&req.purpose);
                        let accounts: Vec<String> = app.cfg.accounts.iter().map(|a| app.from_line(&a.id)).collect();
                        match writing::Session::start(
                            ed,
                            &req.text,
                            req.purpose,
                            draft,
                            &accounts,
                            (r.height, r.width),
                            &base_hex(),
                        ) {
                            Ok(s) => session = Some(s),
                            Err(e) => {
                                app.writing = None;
                                app.message = Some((format!("the editor did not start: {e}"), true));
                            }
                        }
                    }
                    // Any other editor: hand it the terminal, then take it back.
                    None => {
                        ratatui::restore();
                        let result = send::run_editor(&req.text);
                        terminal = ratatui::try_init().map_err(|e| e.to_string())?;
                        app.editor_done(req.purpose, result);
                    }
                }
                continue;
            }
            if let Some(how) = session.as_mut().and_then(writing::Session::finished) {
                session = None;
                app.writing_finished(how);
                continue;
            }
            // keryxd writes the mirror; pick changes up within ~2 s.
            if last_check.elapsed() > Duration::from_secs(2) {
                let s = db_stamp(&cfg);
                if s != stamp {
                    stamp = s;
                    app.reload();
                }
                if app.hold.is_some() || app.sending.is_some() {
                    app.refresh_hold();
                }
                last_check = Instant::now();
            }
            if let Some(s) = &mut session {
                // Writing: keys go to the editor, except ^a (the picker) and the picker's own.
                if !event::poll(Duration::from_millis(16)).map_err(|e| e.to_string())? {
                    continue;
                }
                match event::read().map_err(|e| e.to_string())? {
                    Event::Key(k) if k.kind == KeyEventKind::Press => {
                        let picking = app.writing.as_ref().is_some_and(|w| w.picker.is_some());
                        if picking {
                            if let Some(path) = map_key(k).and_then(|key| app.picker_key(key)) {
                                let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
                                let shown = pick::tilde(&path, home.as_deref());
                                // ^\ ^n reaches normal mode from any mode.
                                s.pane.send(format!("\x1c\x0e:KeryxAttach {shown}\r").as_bytes());
                            }
                        } else if k.modifiers == KeyModifiers::CONTROL && k.code == KeyCode::Char('a') {
                            let home = std::env::var_os("HOME")
                                .map(std::path::PathBuf::from)
                                .unwrap_or_default();
                            app.open_picker(pick::Picker::scan(&home));
                        } else {
                            s.pane.key(k);
                        }
                    }
                    Event::Paste(t) => s.pane.paste(&t),
                    _ => {}
                }
                continue;
            }
            if event::poll(Duration::from_millis(500)).map_err(|e| e.to_string())?
                && let Event::Key(k) = event::read().map_err(|e| e.to_string())?
                && let Some(key) = map_key(k)
            {
                app.key(key);
                if app.quit {
                    return Ok(());
                }
                if !app.deferred.is_empty() {
                    // Show "archiving…" first, then talk to keryxd.
                    terminal.draw(|f| ui::draw(f, &app)).map_err(|e| e.to_string())?;
                    app.run_deferred();
                }
            }
        }
    })();
    drop(session);
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::event::DisableBracketedPaste,
        crossterm::cursor::SetCursorStyle::DefaultUserShape
    );
    ratatui::restore();
    result
}

/// DECSCUSR's number as crossterm's cursor style (0: your terminal's default).
fn cursor_style(n: u16) -> crossterm::cursor::SetCursorStyle {
    use crossterm::cursor::SetCursorStyle as S;
    match n {
        1 => S::BlinkingBlock,
        2 => S::SteadyBlock,
        3 => S::BlinkingUnderScore,
        4 => S::SteadyUnderScore,
        5 => S::BlinkingBar,
        6 => S::SteadyBar,
        _ => S::DefaultUserShape,
    }
}

/// The theme's background, as `#rrggbb`, told to the editor when it asks.
fn base_hex() -> String {
    match locus_theme::c("base") {
        ratatui::style::Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        _ => "#1e1e2e".into(),
    }
}
