//! `kairos tui`: ratatui front end. Draws kairosd's own index (polled every
//! few seconds, at once after a write), so changes show as soon as they're
//! made; starts from the local cache (fast first frame) and falls back to it
//! (pulled by `sync.full_cmd`) when kairosd can't be reached. Writes go to
//! kairosd.

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
        KeyCode::Up if k.modifiers.contains(KeyModifiers::SHIFT) => Key::ShiftUp,
        KeyCode::Down if k.modifiers.contains(KeyModifiers::SHIFT) => Key::ShiftDown,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left if k.modifiers.contains(KeyModifiers::SHIFT) => Key::ShiftLeft,
        KeyCode::Right if k.modifiers.contains(KeyModifiers::SHIFT) => Key::ShiftRight,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        _ => return None,
    })
}

/// Background (offline only): run the configured sync command (the pull of
/// kairosd's vdir on the Mac) so the local copy stays fresh.
fn spawn_sync(cfg: &Config, done: mpsc::Sender<()>) {
    let cfg = cfg.clone();
    std::thread::spawn(move || {
        let _ = kairos_store::sync::run_full(&cfg);
        kairos_store::sync::pull_sidecar(&cfg);
        kairos_store::sync::pull_tasks(&cfg);
        let _ = done.send(());
    });
}

pub fn run(cfg: Config, path: PathBuf) -> Result<(), String> {
    // KAIROS_NOW=2026-10-12T09:40:00+01:00 previews the TUI at another time
    // (the clock keeps running from there). Display only; writes are real.
    let skew = std::env::var("KAIROS_NOW")
        .ok()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
        .map(|t| t.with_timezone(&Utc) - Utc::now())
        .unwrap_or_else(chrono::Duration::zero);
    let now = Utc::now() + skew;
    let mut app = load(cfg.clone(), &path, now)?;
    app.daemon = Daemon::from_config(&cfg).ok().map(|d| d.interactive(true));
    let (tx, rx) = mpsc::channel::<()>();
    // kairosd's index, fetched in the background: (requested at, result).
    type Fetched = (chrono::DateTime<Utc>, Result<Option<(cache::Index, String)>, String>);
    let poller = Daemon::from_config(&cfg).ok().map(std::sync::Arc::new);
    let (itx, irx) = mpsc::channel::<Fetched>();
    let mut terminal = ratatui::try_init().map_err(|e| e.to_string())?;
    let result = (|| -> Result<(), String> {
        let long_ago = Instant::now() - Duration::from_secs(3600);
        let mut last_remote = long_ago;
        let mut last_reload = Instant::now();
        let mut syncing = false;
        let mut last_pull = Instant::now();
        let mut last_fetch = long_ago;
        let mut fetching = false;
        let mut etag: Option<String> = None;
        let mut server_idx: Option<cache::Index> = None;
        // Until the first fetch answers, assume kairosd is there.
        let mut online = poller.is_some();
        let mut write_done = chrono::DateTime::<Utc>::MIN_UTC;
        // Days locus asks to open (spec K8).
        let mut inbox = locus_contract::link::OpenFile::kairos();
        loop {
            if let Some(req) = inbox.poll()
                && let Some((day, uid)) = locus_contract::link::parse_kairos_request(&req)
            {
                app.open_day(day, uid.as_deref());
            }
            app.now = Utc::now() + skew;
            terminal.draw(|f| ui::draw(f, &app)).map_err(|e| e.to_string())?;
            // Remote state (pending approvals, tasks) every 30 s; first time right after the first frame.
            if last_remote.elapsed() > Duration::from_secs(30) {
                app.refresh_remote();
                last_remote = Instant::now();
                maybe_prompt_checkin(&mut app);
                continue;
            }
            // A write just finished: fetch the index now (and ignore one
            // requested before the write, which can't show it).
            if app.wants_sync {
                app.wants_sync = false;
                write_done = Utc::now();
                last_fetch = long_ago;
            }
            if let Some(d) = &poller
                && !fetching
                && last_fetch.elapsed() > Duration::from_secs(3)
            {
                fetching = true;
                last_fetch = Instant::now();
                let (from, to) = cache::default_window(&app.cfg, app.now);
                let (d, tag, tx) = (d.clone(), etag.clone(), itx.clone());
                std::thread::spawn(move || {
                    let requested = Utc::now();
                    let r = d.index(from, to, tag.as_deref()).map_err(|e| e.to_string());
                    let _ = tx.send((requested, r));
                });
            }
            if let Ok((requested, r)) = irx.try_recv() {
                fetching = false;
                match r {
                    Ok(_) if requested < write_done => last_fetch = long_ago,
                    Ok(Some((idx, tag))) => {
                        online = true;
                        etag = Some(tag);
                        server_idx = Some(idx.clone());
                        app.set_server_index(idx, requested);
                    }
                    Ok(None) => {
                        online = true;
                        if app.has_patches_before(requested)
                            && let Some(idx) = &server_idx
                        {
                            app.set_server_index(idx.clone(), requested);
                        }
                    }
                    Err(_) => online = false,
                }
            }
            if !online {
                // Offline: keep the local copy fresh and draw that.
                if !syncing && last_pull.elapsed() > Duration::from_secs(20) {
                    last_pull = Instant::now();
                    syncing = true;
                    spawn_sync(&app.cfg, tx.clone());
                }
                if rx.try_recv().is_ok() {
                    syncing = false;
                    last_reload = Instant::now() - Duration::from_secs(60);
                }
                // Reload the index if the vdir changed (cheap fingerprint check).
                // Another process (`kairos sync` from launchd, `kairos today`) may
                // have rebuilt the cache first, so compare with what's shown
                // rather than asking whether this call rebuilt it.
                if last_reload.elapsed() > Duration::from_secs(2) {
                    let (from, to) = cache::default_window(&app.cfg, app.now);
                    if let Ok((idx, rebuilt)) = cache::get(&app.cfg, Some(&path), app.now, from, to)
                        && (rebuilt || idx.built_at != app.index.built_at)
                    {
                        app.set_index(idx);
                    }
                    last_reload = Instant::now();
                }
            }
            if event::poll(Duration::from_millis(250)).map_err(|e| e.to_string())?
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
                if !app.deferred.is_empty() {
                    // Draw the optimistic change first, then do the round trip.
                    terminal.draw(|f| ui::draw(f, &app)).map_err(|e| e.to_string())?;
                    app.run_deferred();
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
