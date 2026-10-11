#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Layout snapshots (text only) at fixed times over `fixtures/edge`.

use chrono::{DateTime, Utc};
use kairos_core::changeset::{Changeset, Item, Op, Risk, Status};
use kairos_core::config::Config;
use kairos_core::model::Author;
use kairos_core::task::{Due, Task};
use kairos_tui::app::{App, Key};
use kairos_tui::{load, render_to_string};
use std::fs;

fn fixture(name: &str) -> String {
    fs::read_to_string(format!("{}/../../fixtures/edge/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn now(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}

fn setup(dir: &std::path::Path) -> Config {
    for (d, name, files) in [
        (
            "p1",
            "Personal",
            vec![
                "dedup_personal.ics",
                "recur_override_moved.ics",
                "deadline_and_block.ics",
                "dst_weekly_sunday.ics",
                "allday.ics",
            ],
        ),
        ("o1", "OQTS", vec!["dedup_oqts.ics"]),
        ("f1", "Family", vec!["dedup_family.ics"]),
    ] {
        let p = dir.join("vdir").join(d);
        fs::create_dir_all(&p).unwrap();
        fs::write(p.join("displayname"), name).unwrap();
        for f in files {
            fs::write(p.join(f), fixture(f)).unwrap();
        }
    }
    let toml = format!(
        r#"
[paths]
vdir = "{root}/vdir"
data = "{root}/data"
cache = "{root}/cache"
[sync]
full_cmd = ""
[[keys]]
name = "Personal"
index = 1
colour = "lavender"
[[keys]]
name = "OQTS"
index = 6
colour = "mauve"
[[sources]]
id = "personal"
kind = "icloud"
calendar = "Personal"
overlay = "personal"
key = "Personal"
writable = true
[[sources]]
id = "oqts"
kind = "icloud"
calendar = "OQTS"
overlay = "personal"
key = "OQTS"
writable = true
[[sources]]
id = "family"
kind = "icloud"
calendar = "Family"
overlay = "family"
writable = true
"#,
        root = dir.display()
    );
    let path = dir.join("config.toml");
    fs::write(&path, toml).unwrap();
    Config::load(&path).unwrap()
}

fn app(at: &str) -> (App, tempfile::TempDir) {
    let t = tempfile::tempdir().unwrap();
    let cfg = setup(t.path());
    let a = load(cfg, &t.path().join("config.toml"), now(at)).unwrap();
    (a, t)
}

#[test]
fn week_wide() {
    let (a, _t) = app("2026-10-22T08:00:00+01:00");
    insta::assert_snapshot!(render_to_string(&a, 140, 34));
}

#[test]
fn week_80_cols_collapses_to_three_days() {
    let (a, _t) = app("2026-10-22T08:00:00+01:00");
    let s = render_to_string(&a, 80, 24);
    assert!(s.contains("Wed 21") && s.contains("Thu 22") && s.contains("Fri 23") && !s.contains("Mon 19"));
    insta::assert_snapshot!(s);
}

#[test]
fn day_view_with_deadline_marker_and_task_block() {
    let (mut a, _t) = app("2026-10-21T08:00:00+01:00");
    a.key(Key::Tab);
    insta::assert_snapshot!(render_to_string(&a, 100, 30));
    a.key(Key::Char('l'));
    a.key(Key::Char('l'));
    let s = render_to_string(&a, 100, 30);
    assert!(s.contains("⚑ Lab report"), "all-day deadline in the strip:\n{s}");
    assert!(s.contains("⚑ 17:00 B4 sheet"), "timed deadline marker:\n{s}");
    insta::assert_snapshot!("deadline_day", s);
}

#[test]
fn agenda_view() {
    let (mut a, _t) = app("2026-10-15T08:00:00+01:00");
    a.key(Key::Tab);
    a.key(Key::Tab);
    insta::assert_snapshot!(render_to_string(&a, 100, 40));
}

#[test]
fn dedup_cluster_expands() {
    let (mut a, _t) = app("2026-10-15T08:00:00+01:00");
    a.key(Key::Enter);
    let s = render_to_string(&a, 100, 30);
    assert!(s.contains("⧉ 3 copies"), "{s}");
    assert!(s.contains("family") && s.contains("personal") && s.contains("oqts"));
    insta::assert_snapshot!(s);
}

#[test]
fn toggles_hide_and_reveal() {
    let (mut a, _t) = app("2026-10-15T08:00:00+01:00");
    a.key(Key::Char('f'));
    let c = a.day_clusters(a.cursor);
    assert_eq!(
        c[0].primary.source, "personal",
        "hiding Family reveals the personal copy"
    );
    a.key(Key::Char('6'));
    let c = a.day_clusters(a.cursor);
    assert!(c.iter().all(|c| c.primary.key.as_deref() != Some("OQTS")));
    a.key(Key::Char('p'));
    assert!(a.day_clusters(a.cursor).is_empty());
}

#[test]
fn search_filters() {
    let (mut a, _t) = app("2026-10-19T08:00:00+01:00");
    a.key(Key::Char('/'));
    for ch in "squash".chars() {
        a.key(Key::Char(ch));
    }
    a.key(Key::Enter);
    let c = a.day_clusters(a.cursor);
    assert!(
        c.iter().all(|c| c.primary.title.to_lowercase().contains("squash")),
        "{c:?}"
    );
    assert!(!c.is_empty());
}

#[test]
fn navigation() {
    let (mut a, _t) = app("2026-10-15T08:00:00+01:00");
    a.key(Key::Char('l'));
    assert_eq!(a.cursor.to_string(), "2026-10-16");
    a.key(Key::Char('L'));
    assert_eq!(a.cursor.to_string(), "2026-10-23");
    a.key(Key::Char('t'));
    assert_eq!(a.cursor.to_string(), "2026-10-15");
    a.key(Key::Char('H'));
    assert_eq!(a.cursor.to_string(), "2026-10-08");
}

fn pending_move() -> Changeset {
    let before = fixture("dedup_personal.ics");
    let after = before
        .replace("20261019T190000", "20261020T183000")
        .replace("20261019T200000", "20261020T193000");
    Changeset {
        id: "cs-ghost".into(),
        author: Author::Claude,
        created_at: now("2026-10-18T10:00:00+01:00"),
        ops: vec![Op::UpdateEvent {
            item: Item {
                source: "personal".into(),
                href: "dedup_personal.ics".into(),
            },
            uid: "squash-mon@kairos.test".into(),
            before,
            after,
        }],
        risk: Risk::High,
        reasons: vec!["changes an event not created by kairos".into()],
        status: Status::Pending,
        summary: "Update \"Squash\" → Tue 20 Oct 18:30–19:30".into(),
        error: None,
        decided_at: None,
    }
}

#[test]
fn approvals_pane_and_ghost_blocks() {
    let (mut a, _t) = app("2026-10-19T08:00:00+01:00");
    a.pending = vec![pending_move()];
    let week = render_to_string(&a, 140, 34);
    assert!(week.contains("◌"), "ghost block for the pending change:\n{week}");
    a.view = kairos_tui::app::View::Approvals;
    let s = render_to_string(&a, 120, 24);
    assert!(s.contains("+ DTSTART;TZID=Europe/London:20261020T183000"), "{s}");
    insta::assert_snapshot!(s);
}

#[test]
fn tasks_view() {
    let (mut a, _t) = app("2026-10-19T08:00:00+01:00");
    let mut t = Task::new(
        "B4 problem sheet",
        "Personal",
        240,
        Author::Claude,
        now("2026-10-12T08:00:00Z"),
    );
    t.id = "t-1".into();
    t.remaining_min = 90;
    t.due = Some(Due::At(now("2026-10-23T16:00:00Z")));
    let mut t2 = Task::new("Careers essay", "OQTS", 120, Author::Alec, now("2026-10-12T08:00:00Z"));
    t2.id = "t-2".into();
    a.tasks = vec![t, t2];
    a.view = kairos_tui::app::View::Tasks;
    insta::assert_snapshot!(render_to_string(&a, 110, 12));
}

#[test]
fn help_popup() {
    let (mut a, _t) = app("2026-10-19T08:00:00+01:00");
    a.key(Key::Char('?'));
    insta::assert_snapshot!(render_to_string(&a, 100, 30));
}

#[test]
fn writes_without_daemon_explain_themselves() {
    let (mut a, _t) = app("2026-10-19T08:00:00+01:00");
    a.key(Key::Char('a'));
    for ch in "Coffee @ tomorrow 10:00".chars() {
        a.key(Key::Char(ch));
    }
    a.key(Key::Enter);
    let (m, err) = a.message.clone().unwrap();
    assert!(err && m.contains("kairosd"), "{m}");
}
