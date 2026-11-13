//! `locus-tui [home]` runs the window; `--dump` prints a screen as text.

use anyhow::{Context, Result, bail};
use clap::Parser;
use locus_tui::app::App;
use locus_tui::fixtures::{self, Fixtures};
use locus_tui::screen::ScreenId;
use locus_tui::uselog::{self, UseLog};
use locus_tui::{dump, ui};
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Sleep `d`, or less when `^r` moved `wake` since this thread last looked (`seen`); true when
/// woken early, so the thread fetches its slow sources too (cleanup C4).
fn nap(wake: &AtomicU64, seen: &mut u64, d: Duration) -> bool {
    let until = Instant::now() + d;
    loop {
        let now = wake.load(Ordering::Relaxed);
        if now != *seen {
            *seen = now;
            return true;
        }
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        std::thread::sleep(left.min(Duration::from_millis(200)));
    }
}

#[derive(Parser)]
#[command(about = "The locus window, on fixture data (Slice 1)")]
struct Cli {
    /// Which root to run: home (the only one since the Project window went, 13 Nov).
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
    /// With --dump: keys to press first, by name, space-separated: `tab left enter`, `p down`.
    #[arg(long, value_name = "KEYS")]
    press: Option<String>,
    /// With --dump: lay the live calendar over the fixtures first, as the window does (spec K8).
    #[arg(long)]
    live: bool,
    /// Print the interview dates found in the last N days of mail for each application (read-only;
    /// spec E6), past ones included, and exit.
    #[arg(long, value_name = "DAYS")]
    scan_mail: Option<i64>,
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
    if let Some(days) = cli.scan_mail {
        let shared = locus_contract::taxonomy::load(&locus_contract::taxonomy::root())?;
        for b in locus_tui::maildates::scan(&shared.registry, chrono::Utc::now(), days, true) {
            println!(
                "{:<14} {}  {} min  · from “{}” ({}, mail of {})",
                b.for_id,
                b.at.format("%a %-d %b %H:%M"),
                b.minutes,
                b.subject,
                b.thread,
                b.sent.format("%-d %b")
            );
        }
        return Ok(());
    }
    if cli.validate_fixtures {
        return validate();
    }
    if cli.timing {
        return timing(&cli.fixtures, &cli.size);
    }
    let mut fx = Fixtures::load(&fixtures::resolve(&cli.fixtures))?;
    if cli.live && cli.dump.is_some() {
        let live = locus_tui::calendar::Live::connect().context("no kairos config, or kairosd unreachable")?;
        let cal = live.fetch(chrono::Utc::now()).context("kairosd did not answer")?;
        locus_tui::calendar::apply(&mut fx, &cal, |k| live.area_of(k), chrono::Utc::now());
        locus_tui::calendar::apply_chores(&mut fx, &live.chores());
        locus_tui::calendar::apply_tasks(&mut fx, &live.tasks());
        let bookings = locus_tui::maildates::scan(&fx.registry, chrono::Utc::now(), 45, false);
        locus_tui::calendar::apply_dates(&mut fx, &bookings, &cal, &locus_tui::maildates::dismissed());
        locus_tui::calendar::apply_pending(&mut fx, &live.pending());
        if let Some(m) = locus_tui::mail::Live::connect() {
            let proposals = m.proposals();
            let previews = locus_tui::mail::Mirror::open()
                .map(|x| x.previews(&proposals))
                .unwrap_or_default();
            locus_tui::mail::apply(&mut fx, &proposals, &previews);
        }
    }
    if let Some(name) = cli.dump {
        let Some(screen) = ScreenId::from_name(&name) else {
            let all: Vec<_> = ScreenId::ALL.iter().map(|s| s.name()).collect();
            bail!("no screen {name:?}; screens: {}", all.join(", "));
        };
        let (w, h) = dump::parse_size(&cli.size).context("--size is WIDTHxHEIGHT, e.g. 95x38")?;
        let mut app = App::new(fx, ScreenId::Home, UseLog::off());
        if cli.live {
            // Home live, read once, as the window reads it on its threads.
            let mut prompts = locus_tui::live::Prompts::default();
            let sessions = locus_tui::live::scan_sessions(&app.fixtures.registry, &mut prompts);
            let commits = locus_tui::live::scan_commits(&app.fixtures.registry, &sessions);
            let facts = locus_tui::live::fetch_facts();
            let careers = facts
                .as_ref()
                .map_err(|e| e.clone())
                .and_then(|f| locus_tui::live::careers_from_facts(&f.current));
            app.go_live();
            if let Some(m) = locus_tui::mail::Mirror::open() {
                locus_tui::mail::apply_counts(&mut app.fixtures, m.need_reply());
            }
            apply_live(&mut app, Live::Projects(sessions, Some(commits), Some(careers)));
            if let Ok(f) = facts {
                apply_live(&mut app, Live::Facts(f));
            }
            apply_live(&mut app, Live::System(Box::new(locus_tui::live::fetch_system())));
            let others = locus_tui::live::scan_unregistered_repos(&app.fixtures.registry);
            apply_live(&mut app, Live::Others(others));
        }
        if screen != app.screen() {
            app.open(screen);
        }
        app.keys_open = cli.keys;
        // A first draw, so sections that record what they drew know it before keys arrive.
        dump::render(&app, w, h)?;
        for name in cli.press.as_deref().unwrap_or("").split_whitespace() {
            use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
            let code = match name {
                "enter" => KeyCode::Enter,
                "esc" => KeyCode::Esc,
                "tab" => KeyCode::Tab,
                "pgdn" => KeyCode::PageDown,
                "pgup" => KeyCode::PageUp,
                "home" => KeyCode::Home,
                "end" => KeyCode::End,
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
    if cli.view != "home" {
        bail!(
            "the only view is home, not {:?} (the Project window went 13 Nov)",
            cli.view
        );
    }
    let mut app = App::new(fx, ScreenId::Home, UseLog::new(uselog::default_path()));
    // Back on the screen it was on before a rebuild restarted it (C7).
    if let Some(s) = std::env::var("LOCUS_REEXEC_SCREEN")
        .ok()
        .and_then(|n| ScreenId::from_name(&n))
        && s != ScreenId::Home
    {
        app.open(s);
    }
    run(app)
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

/// What the Home-live threads send.
enum Live {
    Projects(
        Vec<locus_tui::live::Live>,
        Option<std::collections::BTreeMap<String, (locus_contract::Time, String)>>,
        Option<Result<std::collections::BTreeMap<String, locus_contract::Status>, String>>,
    ),
    System(Box<Result<locus_tui::live::SystemLive, String>>),
    /// The facts ledger's proposals and automatic writes (heuris H2.5).
    Facts(locus_tui::live::FactsLive),
    Others(Vec<(String, Option<(locus_contract::Time, String)>)>),
}

fn apply_live(app: &mut App, msg: Live) {
    // Live, the screen's clock is the real one: the fixtures' fixed moment made every session
    // "now" and every due date relative to 24 October.
    app.fixtures.manifest.now = chrono::Local::now().fixed_offset();
    // Live, Approve holds only what kairos, keryx and the ledger sent: an item with nothing to
    // act on is the fixtures' and goes. The log likewise holds only what the ledger wrote (H2.5).
    for g in &mut app.fixtures.approve.groups {
        g.items
            .retain(|i| i.date.is_some() || i.changeset.is_some() || i.mail.is_some() || i.fact.is_some());
    }
    app.fixtures.approve.groups.retain(|g| !g.items.is_empty());
    app.fixtures.autolog.entries.retain(|e| e.fact.is_some());
    match msg {
        Live::Projects(sessions, commits, careers) => {
            app.unregistered = locus_tui::live::apply_sessions(&mut app.fixtures, &sessions);
            if let Some(c) = commits {
                app.last_commit = c;
            }
            match careers {
                Some(Ok(c)) => {
                    locus_tui::live::apply_careers(&mut app.fixtures, &c);
                    app.careers_error = None;
                }
                Some(Err(e)) => app.careers_error = Some(e),
                None => {}
            }
        }
        Live::System(s) => app.system_live = Some(*s),
        Live::Facts(f) => locus_tui::live::apply_facts(&mut app.fixtures, &f),
        Live::Others(o) => app.other_repos = o,
    }
}

fn run(mut app: App) -> Result<()> {
    let started = Instant::now();
    let mut first_frame = None;
    let mut slowest = Duration::ZERO;
    app.in_session = std::env::var_os("LOCUS_IN_SESSION").is_some();
    // The chat runs only in the real window, never in dumps or tests.
    app.chat = locus_tui::chat::Chat::new(locus_contract::link::state_dir());
    // The calendar, live (spec K8): fetched from kairosd every minute on a thread of its own and
    // laid over the fixtures; the fixtures stand when kairosd does not answer.
    // ^r (cleanup C4): every fetching thread naps in short steps and wakes when this moves.
    let wake = std::sync::Arc::new(AtomicU64::new(0));
    let (cal_tx, cal_rx) = std::sync::mpsc::channel();
    if let Some(live) = locus_tui::calendar::Live::connect().map(std::sync::Arc::new) {
        app.calendar = Some(live.clone());
        let registry = app.fixtures.registry.clone();
        let wake = wake.clone();
        std::thread::spawn(move || {
            let mut bookings = Vec::new();
            let mut seen = 0;
            let mut woken = false;
            for minute in 0u64.. {
                // Dates from mail (spec E6): keryx's local copy, read every ten minutes.
                if minute % 10 == 0 || woken {
                    bookings = locus_tui::maildates::scan(&registry, chrono::Utc::now(), 45, false);
                }
                if let Some(cal) = live.fetch(chrono::Utc::now())
                    && cal_tx
                        .send((cal, live.chores(), live.tasks(), bookings.clone(), live.pending()))
                        .is_err()
                {
                    return;
                }
                woken = nap(&wake, &mut seen, Duration::from_secs(60));
            }
        });
    }
    // Mail to decide (keryx-joins-locus X2): keryxd's drafts and cleanup proposals, every minute.
    let (mail_tx, mail_rx) = std::sync::mpsc::channel();
    if let Some(m) = locus_tui::mail::Live::connect().map(std::sync::Arc::new) {
        app.mail = Some(m.clone());
        app.mail_queue = Some(locus_tui::mail::Queue::start(m.clone()));
        let wake = wake.clone();
        std::thread::spawn(move || {
            let mut seen = 0;
            loop {
                let mirror = locus_tui::mail::Mirror::open();
                // Unknown (no mirror) stays unknown: Home shows no count rather than 0.
                let need = mirror.as_ref().map(|x| x.need_reply());
                let proposals = m.proposals();
                let previews = mirror.as_ref().map(|x| x.previews(&proposals)).unwrap_or_default();
                if mail_tx.send((proposals, previews, need)).is_err() {
                    return;
                }
                nap(&wake, &mut seen, Duration::from_secs(60));
            }
        });
    }
    // Home live (home-live.md): sessions every 5 s, commits and the careers facts every minute,
    // on a thread of their own; the server's system section every 5 minutes on another, since
    // ssh can take seconds.
    app.go_live();
    let (live_tx, live_rx) = std::sync::mpsc::channel::<Live>();
    {
        let registry = app.fixtures.registry.clone();
        let tx = live_tx.clone();
        let wake1 = wake.clone();
        std::thread::spawn(move || {
            let mut prompts = locus_tui::live::Prompts::default();
            let mut seen = 0;
            let mut woken = false;
            for tick in 0u64.. {
                let sessions = locus_tui::live::scan_sessions(&registry, &mut prompts);
                let slow = tick % 12 == 0 || woken;
                let commits = slow.then(|| locus_tui::live::scan_commits(&registry, &sessions));
                // The facts, over ssh (H2.5): a second or two, once a minute, for careers, Approve
                // and the log at once.
                let facts = slow.then(locus_tui::live::fetch_facts);
                let careers = facts.as_ref().map(|f| {
                    f.as_ref()
                        .map_err(|e| e.clone())
                        .and_then(|f| locus_tui::live::careers_from_facts(&f.current))
                });
                if tx.send(Live::Projects(sessions, commits, careers)).is_err() {
                    return;
                }
                if let Some(Ok(f)) = facts
                    && tx.send(Live::Facts(f)).is_err()
                {
                    return;
                }
                // Every ten minutes: a git call per folder, for the projects view's last section.
                if tick % 120 == 0
                    && tx
                        .send(Live::Others(locus_tui::live::scan_unregistered_repos(&registry)))
                        .is_err()
                {
                    return;
                }
                woken = nap(&wake1, &mut seen, Duration::from_secs(5));
            }
        });
        let wake2 = wake.clone();
        std::thread::spawn(move || {
            let mut seen = 0;
            loop {
                if live_tx
                    .send(Live::System(Box::new(locus_tui::live::fetch_system())))
                    .is_err()
                {
                    return;
                }
                nap(&wake2, &mut seen, Duration::from_secs(300));
            }
        });
    }
    // Items kairos asks to open (spec K8): a registry entry as Enter on its row opens it, the
    // Tasks or Approve page, or a section of Home. Anything else (an old screen's name) is Home.
    let mut inbox = locus_contract::link::OpenFile::locus();
    let mut terminal = ratatui::try_init()?;
    let mut mouse = false;
    let mut rebuilt = locus_theme::rebuilt::Rebuilt::new();
    let mut reexec = false;
    let result = (|| -> Result<()> {
        while !app.quit {
            // A rebuilt binary takes over in place, once nothing is being typed (cleanup C7).
            if rebuilt.ready()
                && matches!(app.mode, locus_tui::app::Mode::Normal)
                && app.chat.busy.is_none()
                && app.last_key.is_none_or(|t| t.elapsed() > locus_theme::rebuilt::IDLE)
            {
                reexec = true;
                break;
            }
            if std::mem::take(&mut app.refresh_requested) {
                wake.fetch_add(1, Ordering::Relaxed);
            }
            app.chat.pump();
            if let Some(n) = app.chat.fact_note.take() {
                app.note = Some(n);
            }
            if let Ok((cal, chores, tasks, bookings, pending)) = cal_rx.try_recv()
                && let Some(live) = app.calendar.clone()
            {
                locus_tui::calendar::apply_pending(&mut app.fixtures, &pending);
                let dismissed = locus_tui::maildates::dismissed();
                locus_tui::calendar::apply_dates(&mut app.fixtures, &bookings, &cal, &dismissed);
                locus_tui::calendar::apply(&mut app.fixtures, &cal, |k| live.area_of(k), chrono::Utc::now());
                locus_tui::calendar::apply_chores(&mut app.fixtures, &chores);
                locus_tui::calendar::apply_tasks(&mut app.fixtures, &tasks);
            }
            if let Some(n) = app.mail_queue.as_ref().and_then(|q| q.take_note()) {
                app.note = Some(n);
            }
            while let Ok(msg) = live_rx.try_recv() {
                apply_live(&mut app, msg);
            }
            if let Ok((mut proposals, previews, need)) = mail_rx.try_recv() {
                // Decided here but not yet reached by keryxd: not shown again.
                if let Some(q) = &app.mail_queue {
                    proposals.retain(|p| !q.was_sent(&p.proposal.id));
                }
                locus_tui::mail::apply(&mut app.fixtures, &proposals, &previews);
                if let Some(n) = need {
                    locus_tui::mail::apply_counts(&mut app.fixtures, n);
                }
            }
            if let Some(req) = inbox.poll() {
                app.home();
                // A screen by name: the calendar's `A`, `T`, `P` and `L`, keryx's `A` (cleanup C4).
                if let Some(s) = ScreenId::from_name(&req) {
                    if s != ScreenId::Home {
                        app.open(s);
                    }
                } else if let Some(sec) = req
                    .strip_prefix("home:")
                    .and_then(|s| s.chars().next())
                    .filter(|&c| ScreenId::Home.sections().iter().any(|s| s.letter == c))
                {
                    app.focus = Some(sec);
                } else {
                    app.open_item(&req);
                }
            }
            app.frame = app.frame.wrapping_add(1);
            let t = Instant::now();
            terminal.draw(|f| ui::draw(f, &app))?;
            slowest = slowest.max(t.elapsed());
            first_frame.get_or_insert_with(|| started.elapsed());
            // Faster while the chat streams, so its words arrive as they are written.
            let wait = if app.chat.busy.is_some() { 40 } else { 250 };
            // The mouse wheel scrolls the chat (9 Nov). It is taken only while the chat is open:
            // everywhere else the mouse stays tmux's and the terminal's, so selecting text works
            // as it always did.
            let want = app.screen() == ScreenId::Chat;
            if want != mouse {
                mouse = want;
                if want {
                    ratatui::crossterm::execute!(std::io::stdout(), event::EnableMouseCapture)?;
                } else {
                    ratatui::crossterm::execute!(std::io::stdout(), event::DisableMouseCapture)?;
                }
            }
            if event::poll(Duration::from_millis(wait))? {
                match event::read()? {
                    Event::Key(k) if k.kind == KeyEventKind::Press => app.on_key(k),
                    Event::Mouse(m) if app.screen() == ScreenId::Chat => match m.kind {
                        event::MouseEventKind::ScrollUp => ui::chat::scroll(&mut app, 3),
                        event::MouseEventKind::ScrollDown => ui::chat::scroll(&mut app, -3),
                        _ => {}
                    },
                    _ => {}
                }
            }
        }
        Ok(())
    })();
    if mouse {
        let _ = ratatui::crossterm::execute!(std::io::stdout(), event::DisableMouseCapture);
    }
    ratatui::restore();
    if reexec && result.is_ok() {
        let err = locus_theme::rebuilt::reexec(&[("LOCUS_REEXEC_SCREEN", app.screen().name().to_string())]);
        // exec returns only on failure: say so, and stop as a quit would.
        eprintln!("locus-tui: could not restart after the rebuild: {err}");
    }
    if let Some(ff) = first_frame {
        eprintln!(
            "locus-tui: first frame {:.1} ms · slowest frame {:.1} ms",
            ff.as_secs_f64() * 1000.0,
            slowest.as_secs_f64() * 1000.0
        );
    }
    result
}
