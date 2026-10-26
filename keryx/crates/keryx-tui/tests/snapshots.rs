#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Layout snapshots (text only) over the fixture mirror at a fixed time.

use chrono::TimeZone;
use keryx_store::testkit::fixture_db;
use keryx_tui::app::{App, Key, Mode};
use keryx_tui::render_to_string;

fn app() -> App {
    let (cfg, db) = fixture_db();
    let now = chrono_tz::Europe::London
        .with_ymd_and_hms(2026, 10, 14, 15, 0, 0)
        .unwrap();
    // Fresh syncs, so no stale warnings unless a test asks for one.
    for a in &cfg.accounts {
        let state = db
            .sync_state(&a.id)
            .unwrap()
            .map(|(s, _)| s)
            .unwrap_or_else(|| "{}".into());
        db.set_sync_state(&a.id, &state, now.timestamp()).unwrap();
    }
    let mut a = App::new(cfg, db, now);
    a.dry_open = true;
    a
}

fn keys(a: &mut App, ks: &[Key]) {
    for k in ks {
        a.key(*k);
    }
}

fn type_str(a: &mut App, s: &str) {
    for c in s.chars() {
        a.key(Key::Char(c));
    }
}

fn select(a: &mut App, subject: &str) {
    a.sel = a
        .rows
        .iter()
        .position(|r| r.subject == subject)
        .unwrap_or_else(|| panic!("{subject} not in list"));
}

#[test]
fn inbox_80() {
    insta::assert_snapshot!(render_to_string(&mut app(), 80, 24));
}

#[test]
fn inbox_120_with_reading_pane() {
    insta::assert_snapshot!(render_to_string(&mut app(), 120, 30));
}

#[test]
fn inbox_200() {
    insta::assert_snapshot!(render_to_string(&mut app(), 200, 30));
}

#[test]
fn thread_with_folded_quotes() {
    let mut a = app();
    select(&mut a, "Re: Group project plan");
    keys(&mut a, &[Key::Enter]);
    assert_eq!(a.mode, Mode::Thread);
    insta::assert_snapshot!(render_to_string(&mut a, 80, 24));
    keys(&mut a, &[Key::Char('z')]);
    let shown = render_to_string(&mut a, 80, 24);
    assert!(shown.contains("> Let's split the report"), "{shown}");
    assert!(
        shown.contains("Let's split the report into three parts."),
        "earlier message expanded"
    );
}

#[test]
fn thread_in_split_view() {
    let mut a = app();
    select(&mut a, "B4 problem sheet 3");
    keys(&mut a, &[Key::Enter]);
    insta::assert_snapshot!(render_to_string(&mut a, 140, 26));
}

#[test]
fn hidden_text_warning() {
    let mut a = app();
    select(&mut a, "Your account needs attention");
    keys(&mut a, &[Key::Enter]);
    let s = render_to_string(&mut a, 80, 16);
    assert!(s.contains("⚠ hidden text was removed"), "{s}");
    assert!(!s.contains("ignore previous"));
    insta::assert_snapshot!(s);
}

#[test]
fn invite_card() {
    let mut a = app();
    // The invite and its cancellation are one thread; the request is unread,
    // so both messages are expanded.
    select(&mut a, "Cancelled: Project meeting");
    keys(&mut a, &[Key::Enter]);
    let s = render_to_string(&mut a, 100, 30);
    assert!(
        s.contains("Project meeting · Thu 22 Oct, 14:00–15:00 · Thom Building, Room 2 · from Prof Smith"),
        "{s}"
    );
    assert!(s.contains("Cancelled: Project meeting · Thu 22 Oct, 14:00"), "{s}");
    insta::assert_snapshot!(s);
}

#[test]
fn attachment_opens_from_partial_emlx() {
    let mut a = app();
    select(&mut a, "B4 problem sheet 3");
    keys(&mut a, &[Key::Enter, Key::Char('o')]);
    let p = a.opened.last().expect("opened");
    assert!(p.ends_with("sheet3.pdf"));
    assert!(std::fs::read(p).unwrap().starts_with(b"%PDF"));
}

#[test]
fn attachment_from_full_emlx() {
    let mut a = app();
    select(&mut a, "Society minutes");
    keys(&mut a, &[Key::Enter, Key::Char('o')]);
    let p = a.opened.last().expect("opened");
    assert!(std::fs::read(p).unwrap().starts_with(b"PK"));
}

#[test]
fn arrows_move_over_attachments_and_enter_opens_the_highlighted_one() {
    let mut a = app();
    select(&mut a, "Society minutes");
    keys(&mut a, &[Key::Enter]);
    let open = a.open.as_mut().unwrap();
    let mut second = open.msgs[0].attachments[0].clone();
    second.name = format!("copy of {}", second.name);
    open.msgs[0].attachments.push(second);
    keys(&mut a, &[Key::Char('o')]);
    assert_eq!(a.mode, keryx_tui::app::Mode::Attachments);
    let before = a.opened.len();
    keys(&mut a, &[Key::Down]);
    assert_eq!(a.att_sel, 1);
    let s = render_to_string(&mut a, 90, 20);
    assert!(s.contains("copy of"), "{s}");
    keys(&mut a, &[Key::Up, Key::Enter]);
    assert_eq!(a.opened.len(), before + 1, "⏎ opens the highlighted one");
    assert_eq!(a.mode, keryx_tui::app::Mode::Attachments, "and the list stays");
    keys(&mut a, &[Key::Esc]);
    assert_eq!(a.mode, keryx_tui::app::Mode::Thread);
}

#[test]
fn live_search() {
    let mut a = app();
    keys(&mut a, &[Key::Char('/')]);
    type_str(&mut a, "from:jane");
    insta::assert_snapshot!(render_to_string(&mut a, 80, 12));
    // An incomplete query keeps results and shows the problem.
    type_str(&mut a, " \"unclosed");
    let s = render_to_string(&mut a, 80, 14);
    assert!(s.contains("unclosed quote"), "{s}");
    keys(&mut a, &[Key::Esc]);
    assert!(a.search.is_none());
}

#[test]
fn unknown_operator_is_explained() {
    let mut a = app();
    keys(&mut a, &[Key::Char('/')]);
    type_str(&mut a, "form:jane");
    let s = render_to_string(&mut a, 120, 8);
    assert!(s.contains("unknown search operator"), "{s}");
}

#[test]
fn goto_and_help_popups() {
    let mut a = app();
    keys(&mut a, &[Key::Char('g')]);
    insta::assert_snapshot!("goto", render_to_string(&mut a, 80, 20));
    keys(&mut a, &[Key::Char('1')]);
    assert_eq!(a.view, keryx_store::view::View::Key("Admin".into()));
    let admin = render_to_string(&mut a, 80, 10);
    assert!(
        admin.contains("Your appointment is confirmed") && !admin.lines().skip(1).any(|l| l.contains("admin")),
        "no redundant key tag: {admin}"
    );
    assert!(
        admin.contains("Your receipt"),
        "Gmail label filed under the key: {admin}"
    );
    keys(&mut a, &[Key::Char('?')]);
    insta::assert_snapshot!("help", render_to_string(&mut a, 80, 20));
}

#[test]
fn account_filter_cycles() {
    let mut a = app();
    keys(&mut a, &[Key::Tab]);
    assert_eq!(a.account_label(), "Personal");
    assert!(a.rows.iter().all(|r| r.account == "personal"));
    let s = render_to_string(&mut a, 80, 10);
    assert!(s.contains("all · personal · oxford"), "every inbox is a tab: {s}");
    keys(&mut a, &[Key::BackTab]);
    assert_eq!(a.account_label(), "All accounts");
}

#[test]
fn stale_sync_is_the_only_status() {
    let mut a = app();
    let now = a.now.timestamp();
    let state = a.db.sync_state("personal").unwrap().unwrap().0;
    a.db.set_sync_state("personal", &state, now - 14 * 60).unwrap();
    a.reload();
    let s = render_to_string(&mut a, 120, 6);
    assert!(s.contains("personal last synced 14 min ago"), "{s}");
}

#[test]
fn empty_views_say_so() {
    let mut a = app();
    keys(&mut a, &[Key::Char('/')]);
    type_str(&mut a, "\"no such words\"");
    keys(&mut a, &[Key::Enter]);
    assert!(render_to_string(&mut a, 60, 8).contains("nothing matches"));
}

#[test]
fn new_since_you_last_looked_is_a_rule_and_a_count() {
    let mut a = app();
    let since = a.rows[2].date;
    a.seen_since = Some(since);
    assert_eq!(a.new_count(), 2);
    let s = render_to_string(&mut a, 80, 14);
    let lines: Vec<&str> = s.lines().collect();
    let rule = lines.iter().position(|l| l.contains("new since")).expect("a rule");
    let older = lines
        .iter()
        .position(|l| l.contains(&a.rows[2].subject))
        .unwrap_or_else(|| panic!("{} not in {s}", a.rows[2].subject));
    assert!(rule < older, "the rule sits above what you had seen: {s}");
    assert!(s.contains("2 new"), "{s}");
    // A key after half an hour away moves the marker to when you last pressed one.
    a.last_key = a.now.timestamp() - 31 * 60;
    keys(&mut a, &[Key::Down]);
    assert_eq!(a.seen_since, Some(a.now.timestamp() - 31 * 60));
}

#[test]
fn the_inbox_opens_on_what_needs_a_reply_then_a_gap_then_the_days() {
    let mut a = app();
    let lab = a
        .rows
        .iter()
        .find(|r| r.subject == "Lab partner for Thursday?")
        .unwrap()
        .clone();
    a.db.set_tag(&keryx_store::state::Tag {
        account: lab.account.clone(),
        thread_id: lab.thread_id.clone(),
        kind: "needs_reply".into(),
        reason: "asks about Thursday".into(),
        author: "alec".into(),
        created_at: 0,
    })
    .unwrap();
    a.reload();
    a.sel = 0;
    let s = render_to_string(&mut a, 80, 30);
    let lines: Vec<&str> = s.lines().collect();
    assert!(lines[1].contains("needs reply"), "{s}");
    assert!(lines[3].contains("Lab partner for Thursday?"), "{s}");
    assert!(lines[2].contains('▌'), "the peach bar: {s}");
    assert_eq!(lines[4].trim_matches(|c| c == '│' || c == ' '), "", "a gap: {s}");
    assert!(lines[5].contains("today"), "{s}");
    assert!(s.contains('▎'), "unread is a blue bar: {s}");
}

#[test]
fn the_unread_bar_runs_down_both_lines_and_goes_once_you_read() {
    let mut a = app();
    a.sel = a.rows.iter().position(|r| r.unread).unwrap();
    let subject = a.rows[a.sel].subject.clone();
    a.width = 80;
    let s = render_to_string(&mut a, 80, 30);
    let lines: Vec<&str> = s.lines().collect();
    let at = lines.iter().position(|l| l.contains(subject.as_str())).unwrap();
    assert!(
        lines[at].contains('▎') && lines[at - 1].contains('▎'),
        "both lines: {s}"
    );
    keys(&mut a, &[Key::Enter, Key::Esc]);
    let s = render_to_string(&mut a, 80, 30);
    let lines: Vec<&str> = s.lines().collect();
    let at = lines.iter().position(|l| l.contains(subject.as_str())).unwrap();
    assert!(
        !lines[at].contains('▎') && !lines[at - 1].contains('▎'),
        "read now: {s}"
    );
}

#[test]
fn option_tab_adds_the_next_inbox_and_tab_goes_back_to_one() {
    let mut a = app();
    keys(&mut a, &[Key::Tab]);
    assert_eq!(a.account_label(), "Personal");
    keys(&mut a, &[Key::AltTab]);
    assert_eq!(a.account_label(), "Personal + Oxford");
    let accounts: std::collections::BTreeSet<&str> = a.rows.iter().map(|r| r.account.as_str()).collect();
    assert_eq!(accounts, ["oxford", "personal"].into_iter().collect(), "no OQTS");
    keys(&mut a, &[Key::AltTab, Key::AltTab]);
    assert_eq!(a.account_label(), "Personal + Oxford + OQTS + OQTS shared");
    keys(&mut a, &[Key::AltTab]);
    assert_eq!(a.shown_accounts().len(), 1, "every inbox in: back to one");
    keys(&mut a, &[Key::Tab]);
    assert!(a.also.is_empty());
}

#[test]
fn g_s_is_sent() {
    let mut a = app();
    keys(&mut a, &[Key::Char('g'), Key::Char('s')]);
    assert_eq!(a.view, keryx_store::view::View::Sent);
}
