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

/// A calendar change is decided in kairosd or not at all (15 Nov: Y turned it green and kairos
/// never heard). With no kairosd in the window, y and Y say so and the change stays.
#[test]
fn a_calendar_change_is_never_marked_decided_without_kairosd() {
    let mut a = app("default", ScreenId::Home);
    let cs = kairos_core::changeset::Changeset {
        id: "cs-1".into(),
        author: kairos_core::model::Author::Claude,
        created_at: "2026-11-15T01:00:00Z".parse().unwrap(),
        ops: Vec::new(),
        risk: kairos_core::changeset::Risk::High,
        reasons: Vec::new(),
        status: kairos_core::changeset::Status::Pending,
        summary: "Task \"3YP Logbook\"".into(),
        error: None,
        decided_at: None,
    };
    locus_tui::calendar::apply_pending(&mut a.fixtures, &[cs]);
    a.open(ScreenId::Approve);
    for c in ['Y', 'y'] {
        a.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        assert!(
            a.note.as_deref().unwrap_or("").contains("kairosd isn't connected"),
            "{c}: {:?}",
            a.note
        );
        let g = &a.fixtures.approve.groups[0];
        assert_eq!(g.items[0].changeset.as_deref(), Some("cs-1"), "{c}");
    }
    assert!(!dump::render(&a, 95, 38).unwrap().is_empty());
}

/// What the stand-in server was asked, in order.
static ASKED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn fake_server(args: &[String]) -> Result<String, String> {
    ASKED.lock().unwrap().push(args.join(" "));
    Ok(match args.get(1).map(String::as_str) {
        Some("reopen") => "Back in Approve: me · education:b14".into(),
        _ => "Written: me · education:b14".into(),
    })
}

#[test]
fn a_decided_fact_comes_back_with_u_whether_it_was_yes_or_no() {
    // 16 Nov: "either way yes or no should put it back in the list so I can change my answer"
    use locus_contract::{ApproveGroup, ApproveItem, ApproveKind};
    *locus_tui::live::SERVER_FAKE.lock().unwrap() = Some(fake_server);
    let mut a = app("default", ScreenId::Home);
    a.home_live = true;
    let fact = |id: u64, text: &str| ApproveItem {
        text: text.into(),
        to: None,
        from: "heuris".into(),
        new: false,
        date: None,
        changeset: None,
        mail: None,
        fact: Some(id),
        detail: None,
    };
    a.fixtures.approve.groups = vec![ApproveGroup {
        kind: ApproveKind::Fact,
        label: None,
        items: vec![fact(375, "education:b14"), fact(376, "education:b15")],
    }];
    a.open(ScreenId::Approve);
    let press = |a: &mut App, c: char| a.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    for c in ['y', 'n'] {
        press(&mut a, c);
        assert_eq!(a.fixtures.approve.groups[0].items.len(), 1, "{c} takes it off the list");
        press(&mut a, 'u');
        let items = &a.fixtures.approve.groups[0].items;
        assert_eq!(items.len(), 2, "{c} then u puts it back: {:?}", a.note);
        assert_eq!(items[0].fact, Some(375), "back where it was");
    }
    // the last of a group: the group comes back with it
    press(&mut a, 'y');
    press(&mut a, 'y');
    assert!(a.fixtures.approve.groups.iter().all(|g| g.kind != ApproveKind::Fact));
    press(&mut a, 'u');
    press(&mut a, 'u');
    assert_eq!(a.fixtures.approve.groups[0].items.len(), 2);
    *locus_tui::live::SERVER_FAKE.lock().unwrap() = None;
    let asked = ASKED.lock().unwrap().clone();
    assert!(asked.contains(&"facts reopen --id 375".to_string()), "{asked:?}");
    assert!(asked.contains(&"facts reject --id 375".to_string()), "{asked:?}");
}
