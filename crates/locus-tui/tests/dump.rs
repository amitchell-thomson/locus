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
use unicode_width::UnicodeWidthStr;

fn app(set: &str, screen: ScreenId) -> App {
    let fx = Fixtures::load(&fixtures::resolve(set)).unwrap();
    let root = if screen == ScreenId::Project {
        ScreenId::Project
    } else {
        ScreenId::Home
    };
    let mut app = App::new(fx, root, UseLog::off());
    if screen != root {
        app.open(screen);
    }
    app
}

#[test]
fn every_screen_fits_the_window_on_every_fixture_set() {
    for set in ["default", "empty"] {
        for screen in ScreenId::ALL {
            let text = dump::render(&app(set, screen), 95, 38).unwrap();
            let lines: Vec<&str> = text.lines().collect();
            assert_eq!(lines.len(), 38, "{set}/{screen}");
            for (i, l) in lines.iter().enumerate() {
                assert!(l.width() <= 95, "{set}/{screen} line {i} is {} wide: {l}", l.width());
            }
        }
    }
}

#[test]
fn keys_overlay_fits_too() {
    let mut a = app("default", ScreenId::Home);
    a.keys_open = true;
    let text = dump::render(&a, 95, 38).unwrap();
    assert!(text.lines().all(|l| l.width() <= 95));
    assert!(text.contains("needs you"));
}

#[test]
fn home_snapshot() {
    insta::assert_snapshot!(dump::render(&app("default", ScreenId::Home), 95, 38).unwrap());
}
