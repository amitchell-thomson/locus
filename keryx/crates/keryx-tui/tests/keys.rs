#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! keryx on locus's one key table (cleanup C4, D1): keryx's own keys never shadow a table key,
//! and every key the help or a footer hint names does something.

mod common;
use common::*;
use keryx_tui::app::{App, Key, Mode};
use keryx_tui::render_to_string;
use locus_theme::keys::{TABLE, app_key_is_free};

/// Each key a help line names: "j k ↑ ↓" is four keys.
fn tokens(shown: &str) -> Vec<&str> {
    shown.split_whitespace().collect()
}

#[test]
fn keryxs_own_keys_are_all_ones_the_table_leaves_free() {
    for (k, what) in keryx_tui::keys::OWN {
        for t in tokens(k) {
            assert!(app_key_is_free(t), "{t} ({what}) is a table key");
        }
    }
    // g is mail's own (its lists), caught before the table: whether or not the table still has
    // a "go to" action, keryx never asks it about g.
    assert_eq!(keryx_tui::keys::action(Key::Char('g')), None);
    let mut seen = std::collections::HashSet::new();
    assert!(keryx_tui::keys::GO.iter().all(|(k, _)| seen.insert(*k)));
    // h is no longer back, and n/p are not message keys (n is No).
    assert!(keryx_tui::keys::OWN.iter().all(|(k, _)| !tokens(k).contains(&"h")));
}

/// What a key can change, seen from outside.
fn state(a: &App) -> String {
    let o = a
        .open
        .as_ref()
        .map(|o| format!("{}:{:?}:{}:{:?}:{:?}", o.thread_id, o.sel, o.off, o.toggled, o.raw));
    format!(
        "{:?}|{:?}|{}|{}|{:?}|{}|{}|{:?}|{}|{}|{}|{}|{:?}|{}",
        a.mode,
        a.message,
        a.deferred.len(),
        a.editor_request.is_some(),
        o,
        a.quit,
        a.marked.len(),
        a.view,
        a.sel,
        a.account,
        a.list_hidden,
        a.opened.len(),
        a.also,
        a.help_scroll,
    )
}

/// The places a key is tried: the list, a thread at its top, a thread scrolled into it, and a
/// thread on a wide screen (where the list can hide).
fn places() -> Vec<App> {
    let list = || {
        let (mut a, _f) = app();
        a.width = 80;
        a
    };
    let thread = |w: u16, down: bool| {
        let (mut a, _f) = app();
        let r = select(&mut a, "B4 problem sheet 3");
        let (acc, t) = keryx_core::model::parse_thread_ref(&r).unwrap();
        // A thread with an attachment, a link and a page or more to scroll.
        a.open_thread(acc, t);
        let _ = render_to_string(&mut a, w, 8);
        if down {
            a.key(Key::Char('j'));
            let _ = render_to_string(&mut a, w, 8);
        }
        a
    };
    let mut v = vec![list(), thread(80, false), thread(80, true), thread(140, false)];
    // The list at its end, so ↑ and ⇧↑ have somewhere to go.
    let mut end = list();
    end.key(Key::Bottom);
    v.push(end);
    v
}

fn does_something(key: Key) -> bool {
    places().into_iter().any(|mut a| {
        let w = a.width;
        let _ = render_to_string(&mut a, w, 8);
        let before = state(&a);
        a.key(key);
        state(&a) != before
    })
}

#[test]
fn every_key_the_help_names_does_something() {
    let mut keys: Vec<(String, String)> = keryx_tui::keys::help();
    keys.retain(|(k, _)| !k.starts_with("g ") && !k.starts_with('^'));
    for (shown, what) in keys {
        for t in tokens(&shown) {
            let key = keryx_tui::keys::parse(t).unwrap_or_else(|| panic!("help names {t:?}, not a key"));
            assert!(does_something(key), "{t} ({what}) does nothing anywhere");
        }
    }
    // ^r: a sync.
    assert!(does_something(Key::Ctrl('r')));
}

#[test]
fn every_g_letter_the_help_names_goes_somewhere() {
    for (shown, what) in keryx_tui::keys::help().into_iter().filter(|(k, _)| k.starts_with("g ")) {
        let c = shown.chars().nth(2).unwrap();
        let (mut a, _f) = app();
        a.view = if c == 'v' {
            keryx_store::view::View::Sent
        } else {
            keryx_store::view::View::Archive
        };
        let before = state(&a);
        a.key(Key::Char('g'));
        assert_eq!(a.mode, Mode::GoTo);
        a.key(Key::Char(c));
        assert_ne!(state(&a), before, "g {c} ({what})");
    }
}

#[test]
fn every_hint_names_a_key_the_help_has() {
    let help: Vec<String> = keryx_tui::keys::help()
        .into_iter()
        .flat_map(|(k, _)| k.split_whitespace().map(str::to_string).collect::<Vec<_>>())
        .collect();
    let (mut a, _f) = app();
    let mut screens: Vec<App> = places();
    // The confirm screen, the drafts, and rows marked in a thread.
    a.key(Key::Char('R'));
    let req = a.editor_request.take().unwrap();
    a.editor_done(req.purpose, Ok(req.text.replacen("\n\n\n", "\n\nhi\n", 1)));
    screens.push(a);
    let (mut d, _f) = app();
    d.key(Key::Char('g'));
    d.key(Key::Char('d'));
    screens.push(d);
    let (mut m, _f) = app();
    m.key(Key::Char('v'));
    m.key(Key::Enter);
    screens.push(m);
    for s in &screens {
        for (k, what) in keryx_tui::ui::hints(s) {
            for t in tokens(k) {
                assert!(
                    help.iter().any(|h| h == t) || t == "1…4",
                    "the hint {t} ({what}) on {:?} is not a key the help has",
                    s.mode
                );
            }
        }
    }
    assert!(TABLE.iter().all(|(_, k, _)| help.iter().any(|h| h == k)));
}

#[test]
fn esc_is_back_q_quits_only_at_the_top_and_h_is_not_back() {
    let (mut a, _f) = app();
    let r = select(&mut a, "Lab partner for Thursday?");
    let (acc, t) = keryx_core::model::parse_thread_ref(&r).unwrap();
    a.open_thread(acc, t);
    a.key(Key::Char('h'));
    assert_eq!(a.mode, Mode::Thread, "h is not back any more");
    a.key(Key::Char('q'));
    assert_eq!((a.mode.clone(), a.quit), (Mode::List, false), "q in a thread is back");
    a.key(Key::Char('/'));
    a.key(Key::Char('j'));
    a.key(Key::Enter);
    assert!(a.search.is_some());
    a.key(Key::Char('q'));
    assert!(!a.quit && a.search.is_none(), "q in a search is back");
    a.key(Key::Esc);
    assert!(!a.quit, "esc never quits");
    a.key(Key::Char('q'));
    assert!(a.quit, "q at the top quits");
}

#[test]
fn x_archives_after_asking_once() {
    let (mut a, f) = app();
    let r = select(&mut a, "Lab partner for Thursday?");
    keys(&mut a, &[Key::Char('x')]);
    assert!(f.take().is_empty());
    assert!(a.message.as_ref().unwrap().0.contains("x again archives"));
    // Another key in between: the question is forgotten.
    keys(&mut a, &[Key::Char('j'), Key::Char('k'), Key::Char('x')]);
    assert!(f.take().is_empty());
    keys(&mut a, &[Key::Char('x')]);
    assert_eq!(f.take(), vec![format!(r#"triage {{"action":"archive"}} ["{r}"]"#)]);
}

#[test]
fn with_rows_marked_d_in_a_thread_acts_on_them_and_says_so() {
    let (mut a, f) = app();
    let lab = select(&mut a, "Lab partner for Thursday?");
    keys(&mut a, &[Key::Char('v')]);
    let other = a.rows[a.sel].r#ref.clone();
    keys(&mut a, &[Key::Char('v'), Key::Char('k'), Key::Enter]);
    assert_eq!(a.mode, Mode::Thread);
    let s = render_to_string(&mut a, 100, 20);
    assert!(s.contains("2 marked") && s.contains("archive marked"), "{s}");
    a.key(Key::Char('a'));
    assert!(
        a.message.as_ref().unwrap().0.contains("2 marked threads"),
        "{:?}",
        a.message
    );
    a.run_deferred();
    let call = f.take().remove(0);
    assert!(call.contains(&lab) && call.contains(&other), "{call}");
}

/// Alec, 13 Nov: "g only makes sense in mail for going to its own lists". Its letters are the
/// ones they had before C4, and none of them leaves this window; A is still locus's Approve.
#[test]
fn g_goes_to_mails_own_lists_on_their_old_letters_and_never_to_another_window() {
    use keryx_store::view::View;
    for (c, view) in [
        ('i', View::Inbox),
        ('r', View::NeedsReply),
        ('s', View::Sent),
        ('a', View::All),
        ('v', View::Archive),
    ] {
        let (mut a, _f) = app();
        a.view = View::Sent;
        if c == 's' {
            a.view = View::Archive;
        }
        keys(&mut a, &[Key::Char('g'), Key::Char(c)]);
        assert_eq!(a.view, view, "g {c}");
    }
    for (c, mode) in [
        ('d', Mode::Drafts { sel: 0 }),
        ('c', Mode::Runs { sel: 0 }),
        ('l', Mode::Filed { sel: 0 }),
    ] {
        let (mut a, _f) = app();
        keys(&mut a, &[Key::Char('g'), Key::Char(c)]);
        assert_eq!(a.mode, mode, "g {c}");
    }
    // The places locus's table had (h a t c l m) are mail's lists or nothing: no window moves.
    for c in ['h', 't', 'm', 'a', 'c', 'l'] {
        let (mut a, _f) = app();
        keys(&mut a, &[Key::Char('g'), Key::Char(c)]);
        assert!(a.opened.is_empty(), "g {c} asked for another window: {:?}", a.opened);
    }
    // From a thread too.
    let (mut a, _f) = app();
    let r = select(&mut a, "Lab partner for Thursday?");
    let (acc, t) = keryx_core::model::parse_thread_ref(&r).unwrap();
    a.open_thread(acc, t);
    keys(&mut a, &[Key::Char('g'), Key::Char('s')]);
    assert_eq!((a.mode.clone(), a.view.clone()), (Mode::List, View::Sent));
    // A: locus's Approve, the one way to another window.
    keys(&mut a, &[Key::Char('A')]);
    assert_eq!(a.opened.last().unwrap().to_string_lossy(), "home:approve");
}

#[test]
fn e_opens_the_threads_draft_or_says_how_to_write_one() {
    let (mut a, _f) = app();
    select(&mut a, "Lab partner for Thursday?");
    a.key(Key::Char('e'));
    assert!(a.message.as_ref().unwrap().0.contains("nothing to edit"));
    let dir = tempfile::tempdir().unwrap();
    a.drafts_dir = Some(dir.path().to_path_buf());
    a.key(Key::Char('R'));
    let req = a.editor_request.take().unwrap();
    std::fs::write(a.draft_path(&req.purpose).unwrap(), "To: x@example.com\n\nkept words\n").unwrap();
    a.key(Key::Char('e'));
    assert!(a.editor_request.take().unwrap().text.contains("kept words"));
}
