//! Every screen renders at the window size (95 × 38, plan B.4) on both fixture sets, and no
//! line is wider than the window by DISPLAY width: a glyph the terminal draws two cells wide
//! would push a border off the edge while the buffer still looked the right width.

// A test helper is not a #[test], so clippy.toml's allowance does not reach it.
#![allow(clippy::unwrap_used)]

use locus_tui::app::App;
use locus_tui::dump;
use locus_tui::fixtures::{self, Fixtures};
use locus_tui::screen::ScreenId;
use locus_tui::uselog::UseLog;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthStr;

/// A screen as the window would show it, opened from Home.
fn app(set: &str, screen: ScreenId) -> App {
    let fx = Fixtures::load(&fixtures::resolve(set)).unwrap();
    let mut app = App::new(fx, ScreenId::Home, UseLog::off());
    if screen != app.screen() {
        app.open(screen);
    }
    app
}

fn fits(what: &str, a: &App) -> String {
    let text = dump::render(a, 95, 38).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 38, "{what}");
    for (i, l) in lines.iter().enumerate() {
        assert!(l.width() <= 95, "{what} line {i} is {} wide: {l}", l.width());
    }
    text
}

#[test]
fn every_screen_fits_the_window_on_every_fixture_set() {
    for set in ["default", "empty"] {
        for screen in ScreenId::ALL {
            fits(&format!("{set}/{screen}"), &app(set, screen));
        }
    }
}

#[test]
fn keys_overlay_fits_on_every_screen() {
    for screen in ScreenId::ALL {
        let mut a = app("default", screen);
        a.keys_open = true;
        let text = fits(&format!("keys on {screen}"), &a);
        assert!(text.contains("everywhere"), "{screen}");
    }
}

/// Every key the screens use, pressed on every screen in turn, drawing after each: no key
/// may panic or push a line past the window, whatever state the last one left.
#[test]
fn every_key_on_every_screen_is_safe() {
    let mut keys: Vec<KeyCode> = "abcdefghijklmnopqrstuvwxyzLY0123456789?/+"
        .chars()
        .map(KeyCode::Char)
        .collect();
    keys.extend([
        KeyCode::Enter,
        KeyCode::Tab,
        KeyCode::Backspace,
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::Esc,
    ]);
    for set in ["default", "empty"] {
        for screen in ScreenId::ALL {
            let mut a = app(set, screen);
            for k in &keys {
                if *k == KeyCode::Char('q') {
                    continue;
                }
                a.on_key(KeyEvent::new(*k, KeyModifiers::NONE));
                a.quit = false;
                fits(&format!("{set}/{screen} after {k:?}"), &a);
            }
        }
    }
}

#[test]
fn snapshots_of_every_screen() {
    for screen in ScreenId::ALL {
        let a = app("default", screen);
        insta::assert_snapshot!(screen.name(), dump::render(&a, 95, 38).unwrap());
    }
}

/// Every screen at sizes other than the design size, including small and odd ones: a layout
/// that subtracts rows it was not given panics here before it panics in a window (the locus
/// session's second status line took two rows and found one, 22 Oct).
#[test]
fn every_screen_survives_other_sizes() {
    for (w, h) in [(60u16, 20u16), (80, 24), (95, 36), (120, 30), (155, 44), (200, 60)] {
        for set in ["default", "empty"] {
            for screen in ScreenId::ALL {
                let mut a = app(set, screen);
                let text = dump::render(&a, w, h).unwrap();
                assert_eq!(text.lines().count(), usize::from(h), "{set}/{screen} at {w}x{h}");
                for sec in ['m', 'g', 'd', 's', 'c', 'a', 'p', 'r', 'h', 'b', 't'] {
                    a.focus = Some(sec);
                    dump::render(&a, w, h).unwrap();
                }
            }
        }
    }
}

/// ↓ walks Home in the order it is drawn, each section's rows, then on into the next (round
/// 2g).
#[test]
fn home_arrows_follow_what_is_drawn() {
    let mut a = app("default", ScreenId::Home);
    let order: Vec<char> = ScreenId::Home.sections().iter().map(|s| s.letter).collect();
    assert_eq!(
        order,
        vec!['w', 'p', 'c', 's', 'a', 't', 'm'],
        "the drawn order, top to bottom"
    );
    dump::render(&a, 95, 38).unwrap();
    a.focus = Some('s');
    for _ in 0..locus_tui::ui::section_len(&a, 's') {
        a.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }
    assert_eq!(a.focus, Some('a'), "past studies' last row, into approve");
    a.focus = Some('t');
    a.sel
        .insert((ScreenId::Home, 't'), locus_tui::ui::section_len(&a, 't') - 1);
    a.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(a.focus, Some('m'));
}

/// The Tasks page is the one task list (L7): e asks kairos for its edit form, x asks before it
/// drops, and anything else in between forgets the x.
#[test]
fn tasks_page_edits_in_kairos_and_asks_before_dropping() {
    let key = |a: &mut App, c: char| a.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    let mut a = app("default", ScreenId::Tasks);
    let first = locus_tui::ui::tasks::open(&a)[0].clone();
    let n = locus_tui::ui::tasks::open(&a).len();
    key(&mut a, 'e');
    assert_eq!(
        a.note.as_deref(),
        Some(format!("would ask kairos: edit-task {}", first.id).as_str())
    );
    key(&mut a, 'x');
    assert!(
        a.note.as_deref().unwrap_or("").contains("x again drops it"),
        "{:?}",
        a.note
    );
    key(&mut a, 'e');
    key(&mut a, 'x');
    assert_eq!(
        locus_tui::ui::tasks::open(&a).len(),
        n,
        "e in between: the first x is forgotten"
    );
    key(&mut a, 'x');
    assert_eq!(locus_tui::ui::tasks::open(&a).len(), n - 1, "x twice drops it");
}

/// L4: a shared key means what it means in kairos on every screen (d done, x remove, + add,
/// ? keys).
#[test]
fn shared_keys_keep_their_meaning_on_every_screen() {
    for screen in ScreenId::ALL {
        for (k, what) in locus_tui::ui::screen_keys(screen) {
            assert!(
                locus_theme::keeps_shared_meaning(k, what),
                "{screen}: {k} means {what:?}, not what it means everywhere"
            );
        }
    }
}
