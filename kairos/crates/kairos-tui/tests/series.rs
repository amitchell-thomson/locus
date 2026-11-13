#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Cleanup C1 (13 Nov 2026): one occurrence of a series is never moved, stretched, deleted or
//! marked "not going" from the TUI, because kairos can only rewrite the whole series.

use chrono::{DateTime, Utc};
use kairos_tui::app::{Key, SERIES_ONLY};
use kairos_tui::load;
use std::fs;

#[test]
fn one_occurrence_of_a_series_is_refused_not_rewritten() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path();
    let p = dir.join("vdir").join("p1");
    fs::create_dir_all(&p).unwrap();
    fs::write(p.join("displayname"), "Personal").unwrap();
    let ics = fs::read_to_string(format!(
        "{}/../../fixtures/edge/dst_weekly_sunday.ics",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    fs::write(p.join("dst_weekly_sunday.ics"), ics).unwrap();
    let toml = format!(
        "inbox = \"Errands\"\n[paths]\nvdir = \"{r}/vdir\"\ndata = \"{r}/data\"\ncache = \"{r}/cache\"\n[sync]\nfull_cmd = \"\"\n\
         [[keys]]\nname = \"Errands\"\nindex = 8\ncolour = \"lavender\"\narea = \"Personal\"\n\
         [[sources]]\nid = \"personal\"\nkind = \"icloud\"\ncalendar = \"Personal\"\noverlay = \"personal\"\nkey = \"Errands\"\nwritable = true\n",
        r = dir.display()
    );
    fs::write(dir.join("config.toml"), toml).unwrap();
    let cfg = kairos_core::config::Config::load(&dir.join("config.toml")).unwrap();
    let at: DateTime<Utc> = DateTime::parse_from_rfc3339("2026-10-18T08:00:00+01:00")
        .unwrap()
        .with_timezone(&Utc);
    let mut a = load(cfg, &dir.join("config.toml"), at).unwrap();
    a.key(Key::Tab); // day view, Sun 18
    let sel = a
        .day_clusters(a.cursor)
        .into_iter()
        .position(|c| c.primary.title.contains("Squash"))
        .expect("the Sunday squash occurrence");
    a.sel = sel;
    assert_eq!(a.nudge_target(30).unwrap_err(), SERIES_ONLY);
    for k in [
        Key::ShiftDown,
        Key::AltDown,
        Key::Char('x'),
        Key::Char('m'),
        Key::Char('B'),
    ] {
        a.message = None;
        a.key(k);
        let m = a.message.as_ref().map(|m| m.0.clone()).unwrap_or_default();
        assert_eq!(m, SERIES_ONLY, "{k:?}");
    }
}
