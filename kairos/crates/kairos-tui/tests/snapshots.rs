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
inbox = "Errands"
[paths]
vdir = "{root}/vdir"
data = "{root}/data"
cache = "{root}/cache"
[sync]
full_cmd = ""
[[keys]]
name = "Careers"
index = 3
colour = "peach"
[[keys]]
name = "Errands"
index = 8
colour = "lavender"
area = "Personal"
aliases = ["Personal"]
[[sources]]
id = "personal"
kind = "icloud"
calendar = "Personal"
overlay = "personal"
key = "Errands"
writable = true
[[sources]]
id = "oqts"
kind = "icloud"
calendar = "OQTS"
overlay = "personal"
key = "Careers"
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
    a.key(Key::Right);
    a.key(Key::Right);
    let s = render_to_string(&a, 100, 30);
    assert!(s.contains("⚑ Lab report"), "all-day deadline in the strip:\n{s}");
    assert!(s.contains("⚑ 17:00 B4 sheet"), "timed deadline marker:\n{s}");
    insta::assert_snapshot!("deadline_day", s);
}

#[test]
fn agenda_view() {
    let (mut a, _t) = app("2026-10-15T08:00:00+01:00");
    a.key(Key::Char('g'));
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
    a.key(Key::Char('3'));
    let c = a.day_clusters(a.cursor);
    assert!(c.iter().all(|c| c.primary.key.as_deref() != Some("Careers")));
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
    a.key(Key::Right);
    assert_eq!(a.cursor.to_string(), "2026-10-16");
    a.key(Key::Char(']'));
    assert_eq!(a.cursor.to_string(), "2026-10-23");
    a.key(Key::Char('t'));
    assert_eq!(a.cursor.to_string(), "2026-10-15");
    a.key(Key::Char('['));
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

#[test]
fn add_form_with_live_preview() {
    let (mut a, _t) = app("2026-10-19T08:00:00+01:00");
    a.key(Key::Char('a'));
    a.key(Key::Char('e'));
    for ch in "Squash".chars() {
        a.key(Key::Char(ch));
    }
    a.key(Key::Tab);
    for ch in "19:00".chars() {
        a.key(Key::Char(ch));
    }
    a.key(Key::Tab);
    for ch in "90m".chars() {
        a.key(Key::Char(ch));
    }
    insta::assert_snapshot!(render_to_string(&a, 100, 30));
}

#[test]
fn work_week_shows_five_wider_days() {
    let (mut a, _t) = app("2026-10-22T08:00:00+01:00");
    a.key(Key::Char('w'));
    let s = render_to_string(&a, 140, 34);
    assert!(s.contains("Fri 23") && !s.contains("Sat 24"), "{s}");
}

#[test]
fn local_patches_survive_a_stale_reload() {
    let (mut a, _t) = app("2026-10-22T08:00:00+01:00");
    let squash = a
        .index
        .instances
        .iter()
        .find(|i| i.title == "Squash" && !i.recurring)
        .cloned()
        .expect("fixture has a one-off Squash");
    let mut moved = squash.clone();
    moved.key = Some("OQTS".into());
    a.patches.push(kairos_tui::app::LocalPatch {
        uid: squash.uid.clone(),
        inst: Some(moved),
        made: chrono::Utc::now(),
        until: chrono::Utc::now() + chrono::Duration::seconds(90),
    });
    // A reload that still has the old copy (the local copy not caught up yet).
    let stale = a.index.clone();
    a.set_index(stale.clone());
    let key_of = |a: &App| {
        let v: Vec<_> = a.index.instances.iter().filter(|i| i.uid == squash.uid).collect();
        assert_eq!(v.len(), 1);
        v[0].key.clone()
    };
    assert_eq!(key_of(&a).as_deref(), Some("OQTS"));
    // kairosd's index, requested before the change was made, can't show it either…
    a.set_server_index(stale.clone(), chrono::Utc::now() - chrono::Duration::seconds(5));
    assert_eq!(key_of(&a).as_deref(), Some("OQTS"));
    // …one requested after it is the truth (here: the change didn't happen).
    a.set_server_index(stale, chrono::Utc::now() + chrono::Duration::seconds(1));
    assert_eq!(key_of(&a), squash.key);
    assert!(a.patches.is_empty());
}

#[test]
fn selected_timed_deadline_is_a_red_block() {
    use ratatui::{Terminal, backend::TestBackend};
    let (mut a, _t) = app("2026-10-21T08:00:00+01:00");
    a.key(Key::Tab);
    a.key(Key::Right);
    a.key(Key::Right);
    let red = kairos_tui::ui::selected_style(kairos_tui::ui::c("red")).bg.unwrap();
    // Background of the cell holding the timed deadline's ⚑, if drawn.
    let flag_bg = |a: &App| {
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|f| kairos_tui::ui::draw(f, a)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..30u16)
            .flat_map(|y| (0..100u16).map(move |x| (x, y)))
            .find(|&(x, y)| buf[(x, y)].symbol() == "⚑" && buf[(x + 2, y)].symbol() == "1")
            .map(|p| buf[p].bg)
    };
    let idx = a
        .day_clusters(a.cursor)
        .iter()
        .position(|c| c.primary.title.contains("B4 sheet"))
        .expect("B4 deadline on the day");
    a.sel = usize::from(idx == 0); // something else selected
    assert_ne!(flag_bg(&a), Some(red), "unselected deadline is a line, not a block");
    a.sel = idx;
    assert_eq!(flag_bg(&a), Some(red), "selected deadline is a red block");
    let s = render_to_string(&a, 100, 30);
    assert!(s.contains("⚑ 17:00 B4 sheet") && !s.contains("B4 sheet ━"), "{s}");
}

#[test]
fn deadline_over_a_selected_event_stays_unhighlighted() {
    use ratatui::{Terminal, backend::TestBackend};
    // An event 16:30–17:30 under the 17:00 B4 deadline (the 2026-10-12 bug:
    // selecting the event painted the deadline line blue too).
    let t = tempfile::tempdir().unwrap();
    let cfg = setup(t.path());
    let ics = fixture("deadline_and_block.ics");
    let head = &ics[..ics.find("BEGIN:VEVENT").unwrap()];
    fs::write(
        t.path().join("vdir/p1/under.ics"),
        format!(
            "{head}BEGIN:VEVENT\r\nUID:under@kairos.test\r\nDTSTAMP:20261001T000000Z\r\n\
             DTSTART;TZID=Europe/London:20261023T163000\r\nDTEND;TZID=Europe/London:20261023T173000\r\n\
             SUMMARY:Under\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        ),
    )
    .unwrap();
    let mut a = load(cfg, &t.path().join("config.toml"), now("2026-10-21T08:00:00+01:00")).unwrap();
    a.key(Key::Tab);
    a.key(Key::Right);
    a.key(Key::Right);
    a.sel = a
        .day_clusters(a.cursor)
        .iter()
        .position(|c| c.primary.title == "Under")
        .expect("Under on the day");
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    term.draw(|f| kairos_tui::ui::draw(f, &a)).unwrap();
    let buf = term.backend().buffer();
    let flag = (0..30u16)
        .flat_map(|y| (0..100u16).map(move |x| (x, y)))
        .find(|&(x, y)| buf[(x, y)].symbol() == "⚑" && buf[(x + 2, y)].symbol() == "1")
        .expect("timed deadline drawn");
    assert_ne!(
        buf[flag].bg,
        kairos_tui::ui::selected_style(kairos_tui::ui::c("lavender"))
            .bg
            .unwrap(),
        "deadline took the selection's highlight"
    );
    // The selected event itself is highlighted in its own colour (Personal: lavender).
    let under = (0..30u16)
        .flat_map(|y| (0..100u16).map(move |x| (x, y)))
        .find(|&(x, y)| buf[(x, y)].symbol() == "U" && buf[(x + 1, y)].symbol() == "n")
        .expect("event drawn");
    assert_eq!(
        Some(buf[under].bg),
        kairos_tui::ui::selected_style(kairos_tui::ui::c("lavender")).bg
    );
}

#[test]
fn selected_work_block_highlights_its_deadline() {
    use ratatui::{Terminal, backend::TestBackend};
    // Week view: the B4 block (Wed 21 09:00) and its ⚑ deadline (Fri 23 17:00) are both visible.
    let (mut a, _t) = app("2026-10-21T08:00:00+01:00");
    let flag_bg = |a: &App| {
        let mut term = Terminal::new(TestBackend::new(160, 40)).unwrap();
        term.draw(|f| kairos_tui::ui::draw(f, a)).unwrap();
        let buf = term.backend().buffer().clone();
        let flag = (0..40u16)
            .flat_map(|y| (0..160u16).map(move |x| (x, y)))
            .find(|&(x, y)| buf[(x, y)].symbol() == "⚑" && buf[(x + 2, y)].symbol() == "1")
            .expect("timed deadline drawn");
        buf[flag].bg
    };
    let red = kairos_tui::ui::linked_style(kairos_tui::ui::c("red")).bg.unwrap();
    let block = a
        .day_clusters(a.cursor)
        .iter()
        .position(|c| c.primary.meta.task_id.as_deref() == Some("task-b4"))
        .expect("B4 block on Wed");
    a.sel = usize::from(block == 0);
    assert_ne!(flag_bg(&a), red, "nothing linked selected");
    a.sel = block;
    assert_eq!(flag_bg(&a), red, "the block's deadline lights up");
}

#[test]
fn selected_deadline_highlights_its_work_blocks() {
    use ratatui::{Terminal, backend::TestBackend};
    // Week view: select the ⚑ B4 deadline (Fri 23); its block (Wed 21 09:00) turns blue.
    let (mut a, _t) = app("2026-10-21T08:00:00+01:00");
    let block_bg = |a: &App| {
        let mut term = Terminal::new(TestBackend::new(160, 40)).unwrap();
        term.draw(|f| kairos_tui::ui::draw(f, a)).unwrap();
        let buf = term.backend().buffer().clone();
        // The block: "┆󰄱 B4 sheet" (dashed edge: a scheduler slot; the box: work to do).
        let at = (0..40u16)
            .flat_map(|y| (0..158u16).map(move |x| (x, y)))
            .find(|&(x, y)| {
                buf[(x, y)].symbol() == "┆" && buf[(x + 1, y)].symbol() == "󰄱" && buf[(x + 3, y)].symbol() == "B"
            })
            .expect("B4 block drawn");
        buf[(at.0 + 1, at.1)].bg
    };
    let blue = kairos_tui::ui::linked_style(kairos_tui::ui::c("lavender")).bg.unwrap(); // linked: a tint of its own colour
    // Off Wednesday (where the block itself would be selected) and off the deadline.
    a.key(Key::Right);
    assert_ne!(block_bg(&a), blue);
    a.key(Key::Right);
    a.sel = a
        .day_clusters(a.cursor)
        .iter()
        .position(|c| c.primary.title.contains("B4 sheet"))
        .expect("B4 deadline on Fri");
    assert_eq!(block_bg(&a), blue, "the deadline's work block lights up");
}

#[test]
fn proposed_project_time_is_no_longer_drawn() {
    // Spec K6: project time is measured from your Claude sessions, or a work block you place;
    // kairos no longer proposes ◌ time, even if a proposal were still in its state.
    let (mut a, _t) = app("2026-10-21T08:00:00+01:00");
    a.key(Key::Tab); // day view
    a.proposals = vec![kairos_core::api::ProposalItem {
        id: "p1".into(),
        start: now("2026-10-21T14:00:00+01:00"),
        end: now("2026-10-21T16:00:00+01:00"),
        title: "Project time".into(),
        project_id: None,
    }];
    let s = render_to_string(&a, 100, 30);
    assert!(!s.contains("◌ Project time"), "{s}");
    assert!(a.day_clusters(a.cursor).iter().all(|c| c.primary.uid != "p1"));
}

fn project(
    id: &str,
    name: &str,
    status: kairos_core::project::ProjectStatus,
    week: u32,
) -> kairos_core::api::ProjectItem {
    let t = now("2026-10-01T09:00:00+01:00");
    kairos_core::api::ProjectItem {
        project: kairos_core::project::Project {
            id: id.into(),
            name: name.into(),
            status,
            note: String::new(),
            key: None,
            registry: None,
            created_at: t,
            updated_at: t,
        },
        week_min: week,
        month_min: week,
        unconfirmed_min: 0,
        last_worked: (week > 0).then(|| now("2026-10-20T14:00:00+01:00")),
        next_session: None,
    }
}

#[test]
fn projects_view_and_picking_one_for_proposed_time() {
    use kairos_core::project::ProjectStatus;
    use kairos_tui::app::{Mode, View};
    let (mut a, _t) = app("2026-10-21T08:00:00+01:00");
    a.projects = vec![
        project("p-k", "kairos", ProjectStatus::Active, 150),
        project("p-m", "email TUI", ProjectStatus::Active, 0),
        project("p-l", "Locus", ProjectStatus::Next, 0),
        project("p-o", "old site", ProjectStatus::Done, 0),
    ];
    a.projects[0].project.note = "next: phase 9 gateway".into();
    a.projects[1].unconfirmed_min = 60;
    let mut t = kairos_core::task::Task::new(
        "3YP interim report",
        "Engineering",
        120,
        kairos_core::model::Author::Alec,
        now("2026-10-01T09:00:00+01:00"),
    );
    t.project_id = Some("p-k".into());
    a.tasks.push(t);
    // R: the Projects view, grouped by status, with time spent.
    a.key(Key::Char('P'));
    assert_eq!(a.view, View::Projects);
    let s = render_to_string(&a, 130, 24);
    for want in [
        "Active — working on now · 2",
        "Next — should be working on · 1",
        "Done · 1",
        "kairos",
        "next: phase 9 gateway",
        "2h30 this week",
        "last Tue 20 Oct",
        "not worked on yet",
        "1h to check in",
        "3YP interim report",
    ] {
        assert!(s.contains(want), "{want:?} missing:\n{s}");
    }
    assert_eq!(a.key_hints(), "n plan time · a add · e edit · s status · d delete");
    // n: plan project time for the selected project by hand.
    a.key(Key::Char('n'));
    let Mode::Form(f) = &a.mode else { panic!("no form") };
    assert_eq!(
        (f.title(), f.get("Project"), f.get("Duration")),
        ("project time", "kairos", "2h")
    );
    let mut f = f.clone();
    f.fields[1].value = "thu 14:00".into();
    assert_eq!(a.form_preview(&f).unwrap(), "Thu 22 Oct 14:00–16:00 · kairos");
    a.key(Key::Esc);
    // a: the new-project form; e: edit the selected one.
    a.key(Key::Char('a'));
    let Mode::Form(f) = &a.mode else { panic!("no form") };
    assert_eq!(f.title(), "new project");
    assert_eq!(f.get("Status"), "active");
    assert_eq!(a.form_preview(f), Err("Name: what the project is called".into()));
    a.key(Key::Esc);
    a.key(Key::Char('e'));
    let Mode::Form(f) = &a.mode else { panic!("no form") };
    assert_eq!((f.title(), f.get("Name")), ("edit project", "kairos"));
    a.key(Key::Esc);
    // P again from Projects is a no-op; Esc goes back to the week.
    a.key(Key::Esc);
    assert_eq!(a.view, View::Week);
}

#[test]
fn squeezed_work_blocks_carry_a_warning() {
    let (mut a, _t) = app("2026-10-21T08:00:00+01:00");
    a.key(Key::Tab);
    a.squeezed = vec![(
        "task-b4".into(),
        now("2026-10-21T09:00:00+01:00"),
        vec!["into a short gap".into()],
    )];
    a.sel = a
        .day_clusters(a.cursor)
        .iter()
        .position(|c| c.primary.meta.task_id.as_deref() == Some("task-b4"))
        .unwrap();
    let s = render_to_string(&a, 160, 30);
    assert!(s.contains("┆󰄱 ⚠ B4 sheet"), "{s}");
    assert!(s.contains("⚠ squeezed into a short gap"), "{s}");
}

#[test]
fn tasks_view_lists_completed_tasks_and_edits_a_task() {
    use kairos_tui::app::Mode;
    let (mut a, _t) = app("2026-10-19T08:00:00+01:00");
    let mut t = Task::new(
        "Purchase furniture",
        "Personal",
        60,
        Author::Alec,
        now("2026-10-12T08:00:00Z"),
    );
    t.id = "t-1".into();
    t.due = Some(Due::At(now("2026-10-12T22:59:00Z")));
    let mut done = Task::new(
        "Populate calendar",
        "Personal",
        30,
        Author::Alec,
        now("2026-10-12T08:00:00Z"),
    );
    done.id = "t-0".into();
    done.status = kairos_core::task::TaskStatus::Done;
    done.remaining_min = 0;
    done.updated_at = now("2026-10-12T21:46:00Z");
    a.tasks = vec![t];
    a.done_tasks = vec![done];
    a.view = kairos_tui::app::View::Tasks;
    let s = render_to_string(&a, 110, 14);
    assert!(s.contains("Completed · 1"), "{s}");
    assert!(
        s.contains("󰄵 Populate calendar") && s.contains("finished Mon 12 Oct · 30m of 30m planned"),
        "{s}"
    );
    // e edits the task; clearing Due means no deadline.
    a.key(Key::Char('e'));
    let Mode::Form(f) = &a.mode else { panic!("no form") };
    assert_eq!(f.get("Due"), "2026-10-12 23:59");
    assert_eq!(f.get("Left"), "1h");
    for _ in 0..3 {
        a.key(Key::Tab); // Title → For → Left → Due
    }
    for _ in 0..16 {
        a.key(Key::Backspace);
    }
    let Mode::Form(f) = &a.mode else { panic!("no form") };
    assert_eq!(f.get("Due"), "");
    assert!(a.form_preview(f).unwrap().contains("no deadline"));
}

#[test]
fn work_block_edges_say_who_controls_them_and_boxes_say_their_state() {
    let (mut a, _t) = app("2026-10-21T08:00:00+01:00");
    a.key(Key::Tab);
    let s = render_to_string(&a, 100, 30);
    assert!(s.contains("┆󰄱 B4 sheet"), "kairos's slot: thin dashed, to do\n{s}");
    // Alec placed it (moved by hand / started now): heavy dashed, stays put.
    for i in a
        .index
        .instances
        .iter_mut()
        .filter(|i| i.meta.task_id.as_deref() == Some("task-b4"))
    {
        i.meta.pinned = true;
    }
    let s = render_to_string(&a, 100, 30);
    assert!(s.contains("╏󰄱 B4 sheet"), "{s}");
    // Once it's over it's history: solid; a waiting box until checked in…
    a.now = now("2026-10-21T11:00:00+01:00");
    let s = render_to_string(&a, 100, 30);
    assert!(s.contains("▌󰡖 B4 sheet"), "{s}");
    // …and ✓ once it is.
    for i in a
        .index
        .instances
        .iter_mut()
        .filter(|i| i.meta.kind == kairos_core::model::EventKind::TaskBlock)
    {
        i.title = "✓ B4 sheet".into();
    }
    let s = render_to_string(&a, 100, 30);
    assert!(s.contains("▌󰄵 B4 sheet") && !s.contains("󰄱 ✓"), "{s}");
}

#[test]
fn keys_do_what_the_hints_say_for_each_kind_of_item() {
    use kairos_tui::app::{Confirm, Mode, PromptKind, View};
    let (mut a, _t) = app("2026-10-21T08:00:00+01:00");
    // The B4 task is open (so its ⚑ can edit it).
    let mut t = Task::new("B4 sheet", "Personal", 240, Author::Claude, now("2026-10-12T08:00:00Z"));
    t.id = "task-b4".into();
    t.due = Some(Due::At(now("2026-10-23T16:00:00Z")));
    a.tasks = vec![t];
    let pick = |a: &mut App, pred: &dyn Fn(&kairos_core::model::Instance) -> bool| {
        a.sel = a.day_clusters(a.cursor).iter().position(|c| pred(&c.primary)).unwrap();
    };
    // Work block (Wed 21): hints, and d = skip.
    pick(&mut a, &|i| i.meta.kind == kairos_core::model::EventKind::TaskBlock);
    assert_eq!(a.key_hints(), "n start · m move · d skip · e edit");
    a.key(Key::Char('d'));
    assert!(matches!(a.mode, Mode::Confirm(Confirm::Skip)));
    a.key(Key::Char('n')); // cancel
    // Its ⚑ (Fri 23): e opens the task's form at Due; m moves the deadline; d removes
    // it with all the task's planned time (drops the task).
    a.key(Key::Right);
    a.key(Key::Right);
    pick(&mut a, &|i| i.title.contains("B4 sheet"));
    assert_eq!(a.key_hints(), "e edit · m move · d remove · n start");
    a.key(Key::Char('e'));
    let Mode::Form(f) = &a.mode else { panic!("no form") };
    assert_eq!(f.title(), "edit task");
    assert_eq!(f.fields[f.focus].label, "Due");
    a.key(Key::Esc);
    a.key(Key::Char('m'));
    assert!(matches!(&a.mode, Mode::Prompt(PromptKind::MoveDeadline { task_id }, _) if task_id == "task-b4"));
    a.key(Key::Esc);
    a.key(Key::Char('d'));
    assert!(matches!(&a.mode, Mode::Confirm(Confirm::DropTask(id)) if id == "task-b4"));
    a.key(Key::Char('n'));
    // a: event or task?
    a.key(Key::Char('a'));
    assert!(matches!(a.mode, Mode::AddChoice));
    a.key(Key::Char('t'));
    let Mode::Form(f) = &a.mode else { panic!("no form") };
    assert_eq!(f.title(), "new task");
    a.key(Key::Esc);
    // Tab: week → day → month → agenda → week; Shift-Tab back.
    a.view = View::Week;
    a.key(Key::Tab);
    assert_eq!(a.view, View::Day);
    a.key(Key::Tab);
    assert_eq!(a.view, View::Month);
    a.key(Key::Tab);
    assert_eq!(a.view, View::Agenda);
    a.key(Key::Tab);
    assert_eq!(a.view, View::Week);
    a.key(Key::BackTab);
    assert_eq!(a.view, View::Agenda);
    a.view = View::Week;
    a.key(Key::Char('g'));
    assert_eq!(a.view, View::Agenda);
}

#[test]
fn tasks_view_keys_d_drops_capital_d_finishes_o_reopens() {
    use kairos_tui::app::{Confirm, Mode, PromptKind, View};
    let (mut a, _t) = app("2026-10-19T08:00:00+01:00");
    let mut t = Task::new("Open one", "Personal", 60, Author::Alec, now("2026-10-12T08:00:00Z"));
    t.id = "t-1".into();
    let mut done = Task::new("Done one", "Personal", 30, Author::Alec, now("2026-10-12T08:00:00Z"));
    done.id = "t-0".into();
    done.status = kairos_core::task::TaskStatus::Done;
    a.tasks = vec![t];
    a.done_tasks = vec![done];
    a.view = View::Tasks;
    a.sel = 0;
    assert_eq!(
        a.key_hints(),
        "e edit · D done · W log work · + time · n start · d drop"
    );
    a.key(Key::Char('d'));
    assert!(matches!(&a.mode, Mode::Confirm(Confirm::DropTask(id)) if id == "t-1"));
    a.key(Key::Char('n'));
    a.key(Key::Down); // onto the finished one
    assert_eq!(a.sel, 1);
    assert_eq!(a.key_hints(), "o reopen");
    a.key(Key::Char('o'));
    assert!(matches!(&a.mode, Mode::Prompt(PromptKind::Reopen { id, .. }, _) if id == "t-0"));
}

#[test]
fn month_view_cycles_in_with_tab() {
    use kairos_tui::app::View;
    let (mut a, _t) = app("2026-10-21T08:00:00+01:00");
    a.key(Key::Tab);
    a.key(Key::Tab);
    assert_eq!(a.view, View::Month);
    let s = render_to_string(&a, 120, 34);
    assert!(s.contains("October 2026"), "{s}");
    assert!(
        s.contains("┆09:00 󰄱 B4 sh"),
        "work blocks keep their dashed mark and box:\n{s}"
    );
    insta::assert_snapshot!(s);
    // The arrows work as elsewhere; Tab goes on to the agenda, then back to the week.
    a.key(Key::Right);
    assert_eq!(a.cursor.to_string(), "2026-10-22");
    a.key(Key::Tab);
    assert_eq!(a.view, View::Agenda);
    a.key(Key::Tab);
    assert_eq!(a.view, View::Week);
}

#[test]
fn work_blocks_are_italic_and_say_where_their_task_stands() {
    use ratatui::{Terminal, backend::TestBackend, style::Modifier};
    let (mut a, _t) = app("2026-10-21T08:00:00+01:00");
    a.key(Key::Tab); // day view
    let mut t = Task::new("B4 sheet", "Personal", 240, Author::Claude, now("2026-10-12T08:00:00Z"));
    t.id = "task-b4".into();
    t.remaining_min = 150;
    a.tasks = vec![t];
    a.sel = a
        .day_clusters(a.cursor)
        .iter()
        .position(|c| c.primary.meta.task_id.as_deref() == Some("task-b4"))
        .unwrap();
    let s = render_to_string(&a, 130, 30);
    // A future block: where the task will stand when it starts.
    assert!(
        s.contains("task: 1h30/4h done by then · 1h left after · no deadline"),
        "{s}"
    );
    // With another of its blocks earlier (1 h), that counts too.
    let mut earlier = a
        .index
        .instances
        .iter()
        .find(|i| {
            i.meta.task_id.as_deref() == Some("task-b4") && i.meta.kind == kairos_core::model::EventKind::TaskBlock
        })
        .unwrap()
        .clone();
    earlier.uid = "earlier".into();
    earlier.span = kairos_core::model::Span::Timed {
        start: now("2026-10-20T19:00:00+01:00"),
        end: now("2026-10-20T20:00:00+01:00"),
    };
    a.index.instances.push(earlier);
    let s = render_to_string(&a, 130, 30);
    assert!(
        s.contains("task: 2h30/4h done by then · done after this · no deadline"),
        "{s}"
    );
    a.sel = usize::from(a.sel == 0); // something else selected, so the block isn't highlighted
    let mut term = Terminal::new(TestBackend::new(130, 30)).unwrap();
    term.draw(|f| kairos_tui::ui::draw(f, &a)).unwrap();
    let buf = term.backend().buffer().clone();
    let at = (0..30u16)
        .flat_map(|y| (0..129u16).map(move |x| (x, y)))
        .find(|&(x, y)| buf[(x, y)].symbol() == "┆" && buf[(x + 1, y)].symbol() == "󰄱")
        .unwrap();
    assert!(buf[(at.0 + 1, at.1)].modifier.contains(Modifier::ITALIC));
}

#[test]
fn off_grid_starts_get_a_minutes_tag_not_a_time_in_the_title() {
    let (mut a, _t) = app("2026-10-21T07:00:00+01:00");
    a.key(Key::Tab); // day view (half-hour rows)
    // Move the B4 block to 09:15 (between rows) and the lecture-free day stays.
    for i in a
        .index
        .instances
        .iter_mut()
        .filter(|i| i.meta.task_id.as_deref() == Some("task-b4"))
    {
        if let kairos_core::model::Span::Timed { start, end } = &mut i.span {
            *start += chrono::Duration::minutes(15);
            *end += chrono::Duration::minutes(15);
        }
    }
    let s = render_to_string(&a, 100, 40);
    let line = s.lines().find(|l| l.contains("B4 sheet") && l.contains('┆')).unwrap();
    assert!(
        line.contains("󰄱 B4 sheet") && line.trim_end().trim_end_matches('│').trim_end().ends_with(":15"),
        "{line}"
    );
    assert!(!s.contains("09:15 󰄱"), "no time in front of the title:\n{s}");
}

#[test]
fn past_blocks_wait_for_a_check_in_right_on_the_calendar() {
    use kairos_tui::app::{Mode, PromptKind};
    let (mut a, _t) = app("2026-10-21T11:00:00+01:00"); // after the 09:00–10:30 block
    a.key(Key::Tab);
    a.sel = a
        .day_clusters(a.cursor)
        .iter()
        .position(|c| c.primary.meta.task_id.as_deref() == Some("task-b4"))
        .unwrap();
    // Waiting box (not a tick yet), and the check-in keys as hints.
    let s = render_to_string(&a, 130, 30);
    assert!(
        s.contains(&format!("▌{} B4 sheet", kairos_core::build::WAITING_ICON)),
        "{s}"
    );
    assert_eq!(a.key_hints(), "y done · c partly · + longer · x missed · F finished");
    // c asks how long (prefilled with half the block).
    a.key(Key::Char('c'));
    assert!(matches!(&a.mode, Mode::Prompt(PromptKind::Partial { t }, b) if t.title == "B4 sheet" && b == "45m"));
    a.key(Key::Esc);
    // + took longer: a form for time spent, what it still needs, and from when.
    a.key(Key::Char('+'));
    let Mode::Form(f) = &a.mode else { panic!("no form") };
    assert_eq!(f.title(), "took longer");
    assert_eq!(f.get("Spent"), "2h30");
    assert_eq!(
        a.form_preview(f).unwrap(),
        format!("2h30 spent · {} more, planned as soon as there's time", f.get("Needs"))
    );
    let mut f = f.clone();
    f.fields[2].value = "sat".into();
    assert!(
        a.form_preview(&f).unwrap().ends_with("planned from Sat 24 Oct"),
        "{}",
        a.form_preview(&f).unwrap()
    );
    f.fields[1].value = "0".into();
    assert_eq!(a.form_preview(&f).unwrap(), "2h30 spent · finished");
    a.key(Key::Esc);
    // W logs work on its task.
    a.key(Key::Char('W'));
    assert!(matches!(&a.mode, Mode::Prompt(PromptKind::LogWork { task_id, .. }, _) if task_id == "task-b4"));
    a.key(Key::Esc);
    // The status bar nudges when something waits.
    a.checkins = vec![kairos_core::api::CheckinItem {
        block_uid: "x".into(),
        task_id: "task-b4".into(),
        title: "B4 sheet".into(),
        start: now("2026-10-21T09:00:00+01:00"),
        end: now("2026-10-21T10:30:00+01:00"),
        planned_min: 90,
        outcome: None,
        ask_remaining: false,
        remaining_min: 90,
        project: false,
    }];
    a.message = None;
    let s = render_to_string(&a, 130, 30);
    assert!(s.contains("1 to check in · C"), "{s}");
}

#[test]
fn a_title_cut_short_by_a_tag_ends_in_an_ellipsis() {
    // One row, "☐ Populate Calendar" beside ":15": the cut is marked.
    let lines = kairos_tui::ui::wrap_first_narrow("󰄱 Populate Calendar", 12, 16, 1);
    assert_eq!(lines.len(), 1);
    assert!(lines[0].ends_with('…'), "{lines:?}");
    // With a second row it wraps instead.
    let lines = kairos_tui::ui::wrap_first_narrow("󰄱 Populate Calendar", 12, 16, 2);
    assert_eq!(lines, ["󰄱 Populate", "Calendar"]);
}

#[test]
fn every_hour_is_the_same_height_and_the_day_scrolls() {
    // Spec K6: two rows an hour on any screen; a day that does not fit scrolls rather than
    // squashing, ↑/↓ past the first or last item moving it an hour.
    let (mut a, _t) = app("2026-10-21T08:00:00+01:00");
    a.key(Key::Tab); // day view
    let short = render_to_string(&a, 100, 24);
    let rows_per_hour = |s: &str| {
        let hours: Vec<usize> = s
            .lines()
            .enumerate()
            .filter(|(_, l)| l.strip_prefix('│').is_some_and(|r| r.get(2..5) == Some(":00")))
            .map(|(i, _)| i)
            .collect();
        hours.windows(2).map(|w| w[1] - w[0]).min().unwrap_or(0)
    };
    assert!(rows_per_hour(&short) >= 2, "{short}");
    let (h0, h1, e0, e1) = a.grid.get();
    assert!(
        h0 > e0 || h1 < e1,
        "a short window shows part of the day: {:?}",
        a.grid.get()
    );
    // Past the last item, ↓ scrolls an hour later.
    let n = a.day_clusters(a.cursor).len();
    for _ in 0..n + 1 {
        a.key(Key::Down);
    }
    render_to_string(&a, 100, 24);
    assert!(a.grid.get().0 > h0 || h1 == e1, "{:?} -> {:?}", (h0, h1), a.grid.get());
    // A tall screen shows more hours, not taller ones.
    let tall = render_to_string(&a, 100, 60);
    assert_eq!(rows_per_hour(&tall), rows_per_hour(&short), "{tall}");
}

#[test]
fn a_tasks_due_date_is_drawn_as_a_flag_on_its_day() {
    // Spec K5: the date is the task's attribute, not an event on any calendar.
    let (mut a, _t) = app("2026-10-19T08:00:00+01:00");
    let mut t = kairos_core::task::Task::new(
        "B15 sheet 3",
        "Studies",
        300,
        kairos_core::model::Author::Alec,
        now("2026-10-19T08:00:00+01:00"),
    );
    t.due = Some(kairos_core::task::Due::At(now("2026-10-23T12:00:00+01:00")));
    a.tasks.push(t.clone());
    let s = render_to_string(&a, 140, 34);
    assert!(s.contains("⚑ 12:00 B15 shee"), "{s}");
    let fri = chrono::NaiveDate::from_ymd_opt(2026, 10, 23).unwrap();
    let mark = a
        .day_clusters(fri)
        .into_iter()
        .find(|c| c.primary.title.contains("B15 sheet 3"))
        .unwrap();
    assert_eq!(mark.primary.meta.task_id.as_deref(), Some(t.id.as_str()));
}

#[test]
fn worked_time_is_a_faint_bar_not_an_event() {
    let (mut a, _t) = app("2026-10-21T18:00:00+01:00");
    a.key(Key::Tab); // day view
    let mut idx = a.index.clone();
    idx.instances.push(kairos_core::model::Instance {
        uid: "worked-locus-1".into(),
        recurrence_id: None,
        source: "worked".into(),
        overlay: kairos_core::config::Overlay::Personal,
        key: Some("Careers".into()),
        title: "locus".into(),
        location: None,
        description: None,
        span: kairos_core::model::Span::Timed {
            start: now("2026-10-21T14:00:00+01:00"),
            end: now("2026-10-21T16:00:00+01:00"),
        },
        recurring: false,
        transparent: true,
        tentative: false,
        meta: kairos_core::model::Meta {
            kind: kairos_core::model::EventKind::Worked,
            ..Default::default()
        },
        href: None,
        alarms: Vec::new(),
    });
    a.set_index(idx);
    assert!(
        a.day_clusters(a.cursor)
            .iter()
            .all(|c| c.primary.uid != "worked-locus-1"),
        "not selectable"
    );
    let s = render_to_string(&a, 100, 40);
    assert!(s.contains("worked · locus"), "{s}");
    assert!(s.matches('│').count() >= 3, "{s}");
}

#[test]
fn a_linked_item_says_where_it_opens_and_locus_can_open_a_day() {
    // Spec K8. Enter itself is not pressed here: it would bring the locus session forward.
    let (mut a, _t) = app("2026-10-19T08:00:00+01:00");
    let mut idx = a.index.clone();
    let monday = now("2026-10-19T00:00:00+01:00");
    let squash = idx
        .instances
        .iter_mut()
        .find(|i| i.title.contains("Squash") && i.span.bounds(chrono_tz::UTC).0 >= monday)
        .unwrap();
    squash.meta.locus = Some("millennium".into());
    let (uid, day) = (squash.uid.clone(), squash.start(a.tz).with_timezone(&a.tz).date_naive());
    a.set_index(idx);
    // Locus asks for that day and event: the day view opens with it selected.
    a.open_day(day, Some(&uid));
    assert_eq!(a.view, kairos_tui::app::View::Day);
    assert_eq!(a.selected().unwrap().primary.uid, uid);
    assert!(a.key_hints().starts_with("⏎ open in locus"), "{}", a.key_hints());
    let s = render_to_string(&a, 120, 30);
    assert!(s.contains("→ millennium"), "{s}");
    // v still shows the details.
    a.key(Key::Char('v'));
    assert!(a.expanded);
}

#[test]
fn adding_asks_what_it_is_for_and_l_links_afterwards() {
    // ADR-009 / spec E1: For, by type-ahead over the registry and Personal's streams, in place of
    // a calendar and a type; how it is planned follows from the estimate.
    use kairos_tui::app::Mode;
    let (mut a, _t) = app("2026-10-19T08:00:00+01:00");
    assert!(a.fors.iter().any(|o| o.id == "b15"), "the registry's items are offered");
    a.key(Key::Char('a'));
    a.key(Key::Char('t'));
    for c in "B15 sheet 4".chars() {
        a.key(Key::Char(c));
    }
    a.key(Key::Tab);
    for c in "b15".chars() {
        a.key(Key::Char(c));
    }
    let s = render_to_string(&a, 120, 34);
    assert!(s.contains("● B15 Control Systems"), "the matches as you type:\n{s}");
    let Mode::Form(f) = &a.mode else { panic!("no form") };
    let p = a.form_preview(f).unwrap();
    assert!(
        p.contains("5h in 2–3 h blocks") && p.contains("for B15 Control Systems"),
        "{p}"
    );
    assert!(!f.fields.iter().any(|x| x.label == "Type" || x.label == "Calendar"));
    // An errand needs nothing but its name.
    a.key(Key::Esc);
    a.key(Key::Char('a'));
    a.key(Key::Char('t'));
    for c in "laundry".chars() {
        a.key(Key::Char(c));
    }
    a.key(Key::Tab);
    for c in "err".chars() {
        a.key(Key::Char(c));
    }
    let Mode::Form(f) = &a.mode else { panic!("no form") };
    assert!(
        a.form_preview(f).unwrap().starts_with("20m in one go"),
        "{:?}",
        a.form_preview(f)
    );
    a.key(Key::Esc);
    // L on an event: the same For, for linking afterwards.
    a.key(Key::Esc);
    let n = a.day_clusters(a.cursor).len();
    assert!(n > 0);
    a.key(Key::Char('L'));
    let Mode::Form(f) = &a.mode else {
        panic!("L opens the link form")
    };
    assert_eq!(f.title(), "what is it for");
}

#[test]
fn shift_arrows_nudge_the_selected_item() {
    // Spec E3: Shift-↑/↓ 30 minutes, Shift-←/→ a day; weeks with [ ].
    let (mut a, _t) = app("2026-10-19T08:00:00+01:00");
    a.key(Key::Tab); // day view, Mon 19
    let sel = a
        .day_clusters(a.cursor)
        .into_iter()
        .position(|c| c.primary.title.contains("Squash"))
        .unwrap();
    a.sel = sel;
    let (i, start, day) = a.nudge_target(30).unwrap().unwrap();
    assert_eq!(start - i.span.bounds(a.tz).0, chrono::Duration::minutes(30));
    assert_eq!(day, a.cursor);
    let (_, start, day) = a.nudge_target(24 * 60).unwrap().unwrap();
    assert_eq!(day, a.cursor.succ_opt().unwrap());
    assert_eq!(start.with_timezone(&a.tz).format("%H:%M").to_string(), "19:00");
    // Without kairosd the key says so and changes nothing.
    a.key(Key::ShiftDown);
    assert!(a.message.as_ref().is_some_and(|m| m.0.contains("kairosd")));
    // [ ] move a week.
    let c = a.cursor;
    a.key(Key::Char(']'));
    assert_eq!(a.cursor, c + chrono::Duration::days(7));
}

#[test]
fn a_due_date_shows_at_its_time_or_at_the_top_without_one() {
    // Spec E5: a flag on the day at its time; with no time, at the top with the all-day items.
    let (mut a, _t) = app("2026-10-19T08:00:00+01:00");
    let mk = |title: &str, due: kairos_core::task::Due| {
        let mut t = kairos_core::task::Task::new(
            title,
            "Personal",
            60,
            kairos_core::model::Author::Alec,
            now("2026-10-19T08:00:00+01:00"),
        );
        t.due = Some(due);
        t
    };
    a.tasks.push(mk(
        "Apps close",
        kairos_core::task::Due::Date(chrono::NaiveDate::from_ymd_opt(2026, 10, 21).unwrap()),
    ));
    a.tasks.push(mk(
        "Report in",
        kairos_core::task::Due::At(now("2026-10-21T15:00:00+01:00")),
    ));
    a.key(Key::Right);
    a.key(Key::Right);
    a.key(Key::Tab); // Wed 21, day view
    let s = render_to_string(&a, 100, 40);
    let lines: Vec<&str> = s.lines().collect();
    let rule = lines
        .iter()
        .position(|l| l.starts_with("│────"))
        .unwrap_or_else(|| panic!("{s}"));
    let top = lines
        .iter()
        .position(|l| l.contains("Apps close"))
        .unwrap_or_else(|| panic!("{s}"));
    let timed = lines
        .iter()
        .position(|l| l.contains("Report in"))
        .unwrap_or_else(|| panic!("{s}"));
    assert!(top < rule, "no time: at the top\n{s}");
    assert!(timed > rule && lines[timed].contains("15:00"), "at its time\n{s}");
}
