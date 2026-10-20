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

const PROJECT_SCREENS: [ScreenId; 5] = [
    ScreenId::Project,
    ScreenId::Decisions,
    ScreenId::Answered,
    ScreenId::Changes,
    ScreenId::Prep,
];

/// A screen as the window would show it: project screens on the Project view following
/// `quast`'s tmux session, the rest opened from Home.
fn app(set: &str, screen: ScreenId) -> App {
    let fx = Fixtures::load(&fixtures::resolve(set)).unwrap();
    let project = PROJECT_SCREENS.contains(&screen);
    let mut app = App::new(
        fx,
        if project { ScreenId::Project } else { ScreenId::Home },
        UseLog::off(),
    );
    if project {
        app.follow.session = Some("quast".into());
    }
    if screen != app.screen() {
        app.open(screen);
    }
    app
}

/// A project screen for a project opened from Home's list (most have no tmux session).
fn viewing(set: &str, id: &str, screen: ScreenId) -> App {
    let fx = Fixtures::load(&fixtures::resolve(set)).unwrap();
    let mut app = App::new(fx, ScreenId::Home, UseLog::off());
    app.viewing = Some(id.into());
    app.open(ScreenId::Project);
    if screen != ScreenId::Project {
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
fn every_project_screen_fits_for_every_active_project() {
    for set in ["default", "empty"] {
        let ids: Vec<String> = Fixtures::load(&fixtures::resolve(set))
            .unwrap()
            .projects
            .keys()
            .cloned()
            .collect();
        for id in ids {
            for screen in PROJECT_SCREENS {
                fits(&format!("{set}/{id}/{screen}"), &viewing(set, &id, screen));
            }
        }
    }
}

#[test]
fn each_review_step_and_explore_tab_fits() {
    for set in ["default", "empty"] {
        for step in 1..=5 {
            let mut a = app(set, ScreenId::Review);
            a.review_step = step;
            fits(&format!("{set}/review step {step}"), &a);
        }
        for tab in ['q', 'i', 'r'] {
            let mut a = app(set, ScreenId::Explore);
            a.focus = Some(tab);
            fits(&format!("{set}/explore {tab}"), &a);
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
    let mut keys: Vec<KeyCode> = "abcdefghijklmnopqrstuvwxyzLRWY0123456789?/+"
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
        let a = if screen == ScreenId::Prep {
            viewing("default", "jump", screen)
        } else {
            app("default", screen)
        };
        insta::assert_snapshot!(screen.name(), dump::render(&a, 95, 38).unwrap());
    }
}
