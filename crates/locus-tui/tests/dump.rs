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

const PROJECT_SCREENS: [ScreenId; 6] = [
    ScreenId::Project,
    ScreenId::Application,
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

/// ← walks the branch graph back through history: on a small window, nabla's oldest branch is
/// not drawn until the selection reaches it.
#[test]
fn left_scrolls_the_branch_graph_back_through_history() {
    let mut a = app("default", ScreenId::Project);
    a.follow.session = Some("nabla".into());
    a.focus = Some('g');
    let n = locus_tui::ui::section_len(&a, 'g');
    a.sel.insert((ScreenId::Project, 'g'), 0);
    let oldest_name = a.fixtures.projects["nabla"].view.graph.as_ref().unwrap().lanes[0]
        .branch
        .trim_start_matches("worktree-")
        .to_string();
    let newest = dump::render(&a, 95, 38).unwrap();
    assert!(
        !newest.contains(&oldest_name),
        "the oldest branch is off screen at first"
    );
    for _ in 0..n {
        a.on_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    }
    let oldest = dump::render(&a, 95, 38).unwrap();
    assert!(oldest.contains(&oldest_name), "{oldest}");
    assert!(
        oldest.contains("start,"),
        "the first branch's date is marked as the start"
    );
    assert!(!newest.contains("start,"));
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
                for sec in ['y', 'g', 'd', 's', 'c', 'a', 'p', 'e', 't'] {
                    a.focus = Some(sec);
                    dump::render(&a, w, h).unwrap();
                }
            }
        }
    }
}

/// ↓ walks Home in the order it is drawn, and inside Explore only over the finds drawn; past
/// the last drawn find it moves on to the next section (round 2g: it went over hidden finds,
/// and broke across "waiting for you").
#[test]
fn home_arrows_follow_what_is_drawn() {
    let mut a = app("default", ScreenId::Home);
    let order: Vec<char> = ScreenId::Home.sections().iter().map(|s| s.letter).collect();
    assert_eq!(
        order,
        vec!['w', 'p', 'c', 's', 'e', 'a', 't', 'y'],
        "the drawn order, top to bottom"
    );
    dump::render(&a, 95, 38).unwrap();
    let drawn = a.shown.borrow().get(&(ScreenId::Home, 'e')).map_or(0, Vec::len);
    assert!(
        drawn < a.fixtures.explore.queue.len(),
        "at 95 x 38 some finds are not drawn"
    );
    a.focus = Some('e');
    for _ in 0..drawn {
        a.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }
    assert_eq!(a.focus, Some('a'), "past the last drawn find, into approve");
    a.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(a.focus, Some('t'), "then tasks, beside it");
    a.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(a.focus, Some('y'));
}

/// Enter on a branch opens its change records under the graph; ↑ ↓ scroll them, Esc closes.
#[test]
fn enter_opens_a_branchs_records() {
    let mut a = app("default", ScreenId::Project);
    a.focus = Some('g');
    let press = |a: &mut App, c| a.on_key(KeyEvent::new(c, KeyModifiers::NONE));
    press(&mut a, KeyCode::Left); // barrier-control, the one with 40 records
    press(&mut a, KeyCode::Enter);
    let open = dump::render(&a, 155, 44).unwrap();
    assert!(open.contains("change records · 40"), "{open}");
    press(&mut a, KeyCode::Down);
    assert_eq!(a.records_at, 1);
    assert_eq!(a.focus, Some('g'), "↓ scrolls while open, it does not leave the graph");
    press(&mut a, KeyCode::Esc);
    assert!(!a.lane_open);
    assert!(!dump::render(&a, 155, 44).unwrap().contains("change records"));
}

/// The Decisions page never shows a decision's number (round 2: "no decision numbers"), in the
/// list or the entry, whatever is focused; and Enter on a replaced decision's link reaches the
/// decision that replaced it (round 2q).
#[test]
fn decisions_page_names_decisions_by_title_and_follows_links() {
    for id in ["quast", "nabla", "locus"] {
        let mut a = viewing("default", id, ScreenId::Decisions);
        for sec in [None, Some('r'), Some('n'), Some('h'), Some('x'), Some('e')] {
            a.focus = sec;
            for w in [95u16, 160] {
                let text = dump::render(&a, w, 45).unwrap();
                for bad in ["§8", "ADR-"] {
                    assert!(!text.contains(bad), "{id} {sec:?} at {w} shows {bad}:\n{text}");
                }
            }
        }
    }
    // nabla: the power decision was replaced by the breadth one; from it, Enter on its link
    // ("replaced by") lands on the breadth decision.
    let mut a = viewing("default", "nabla", ScreenId::Decisions);
    a.focus = Some('x');
    let text = dump::render(&a, 160, 45).unwrap();
    assert!(text.contains("replaced by"), "{text}");
    press(&mut a, KeyCode::Enter);
    assert_eq!(a.focus, Some('e'));
    for _ in 0..5 {
        let text = dump::render(&a, 160, 45).unwrap();
        if text.contains("▌") && text.lines().any(|l| l.contains("▌") && l.contains("Abandon breadth")) {
            break;
        }
        press(&mut a, KeyCode::Down);
    }
    press(&mut a, KeyCode::Enter);
    assert_eq!(a.focus, Some('h'));
    let text = dump::render(&a, 160, 45).unwrap();
    assert!(
        text.lines().any(|l| l.contains("▌") && l.contains("Abandon breadth")),
        "{text}"
    );
}

fn press(a: &mut App, code: KeyCode) {
    a.on_key(KeyEvent::new(code, KeyModifiers::NONE));
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

/// L4: where tasks are, a shared key means what it means in kairos (d done, x remove, + add).
/// Other screens' letters (d for decisions on the Project view, d discuss in the review, x a
/// note in ink confirm) came from the design rounds and are raised with you, not changed here.
#[test]
fn shared_keys_keep_their_meaning_where_tasks_are() {
    for screen in [ScreenId::Tasks] {
        for (k, what) in locus_tui::ui::screen_keys(screen) {
            assert!(
                locus_theme::keeps_shared_meaning(k, what),
                "{screen}: {k} means {what:?}, not what it means everywhere"
            );
        }
    }
}
