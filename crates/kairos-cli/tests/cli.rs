#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn fixture(name: &str) -> String {
    fs::read_to_string(format!("{}/../../fixtures/edge/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn setup(dir: &Path) {
    for (d, name, files) in [
        (
            "p1",
            "Personal",
            vec![
                "dedup_personal.ics",
                "recur_override_moved.ics",
                "deadline_and_block.ics",
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
    fs::write(
        dir.join("config.toml"),
        format!(
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
        ),
    )
    .unwrap();
}

fn kairos(dir: &Path, now: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kairos"))
        .args(["--config", dir.join("config.toml").to_str().unwrap()])
        .args(args)
        .env("KAIROS_NOW", now)
        .env("NO_COLOR", "1")
        .env_remove("KAIROS_TOKEN")
        .output()
        .unwrap()
}

fn out(o: &Output) -> String {
    assert!(o.status.success(), "stderr: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).into_owned()
}

#[test]
fn next_tmux_segment() {
    let t = tempfile::tempdir().unwrap();
    setup(t.path());
    let s = out(&kairos(t.path(), "2026-10-15T17:30:00+01:00", &["next", "--tmux"]));
    assert_eq!(
        s.trim_end(),
        "#[bg=#313244,fg=#585b70]\u{e0b3}#[bg=#313244,fg=#cdd6f4] \u{f00f0} OQTS speaker event 19:05 (in 1h35) #[nobold]"
    );
    // Deadline within 24 h wins, in red.
    let s = out(&kairos(t.path(), "2026-10-23T09:00:00+01:00", &["next", "--tmux"]));
    assert!(s.contains("fg=#f38ba8,bold] ⚑ B4 sheet 17:00 "), "{s}");
    // Nothing coming up → empty output (the segment disappears).
    let s = out(&kairos(t.path(), "2027-06-01T09:00:00+01:00", &["next", "--tmux"]));
    assert_eq!(s, "");
}

#[test]
fn ls_json_schema() {
    let t = tempfile::tempdir().unwrap();
    setup(t.path());
    let s = out(&kairos(
        t.path(),
        "2026-10-15T08:00:00+01:00",
        &["ls", "--from", "2026-10-15", "--to", "2026-10-17", "--json"],
    ));
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert_eq!(v["schema"], 1);
    assert_eq!(v["timezone"], "Europe/London");
    let evs = v["events"].as_array().unwrap();
    let speaker = evs
        .iter()
        .find(|e| e["title"].as_str().unwrap().eq_ignore_ascii_case("OQTS speaker event"))
        .unwrap();
    assert_eq!(speaker["overlay"], "family");
    assert_eq!(speaker["duplicates"].as_array().unwrap().len(), 2);
    for field in [
        "uid",
        "recurrence_id",
        "start",
        "end",
        "all_day",
        "date",
        "key",
        "source",
        "kind",
        "flex",
        "colour",
        "duplicates",
    ] {
        assert!(speaker.get(field).is_some(), "missing {field}");
    }
    let lecture = evs.iter().find(|e| e["title"] == "A1 Mathematics Lecture").unwrap();
    assert_eq!(lecture["start"], "2026-10-16T10:00:00+01:00");
    assert_eq!(lecture["key"], "Personal");
}

#[test]
fn today_compact_and_week() {
    let t = tempfile::tempdir().unwrap();
    setup(t.path());
    let s = out(&kairos(t.path(), "2026-10-23T08:00:00+01:00", &["today", "--compact"]));
    assert!(s.starts_with("Fri 23 Oct · "), "{s}");
    assert!(s.contains("⚑ B4 sheet Fri 17:00") || s.contains("⚑ Lab report"), "{s}");
    let s = out(&kairos(t.path(), "2026-10-19T08:00:00+01:00", &["week"]));
    assert!(s.contains("Mon 19 Oct · today"));
    assert!(s.contains("Thu 22 Oct"));
    // The moved lecture and the OQTS copy at the new slot are one real event: merged (⧉).
    assert!(
        s.lines()
            .any(|l| l.contains("14:00–15:00") && l.contains("B4 Lecture") && l.contains("⧉")),
        "{s}"
    );
    assert!(
        s.contains("┆ ☐ B4 sheet"),
        "scheduler slots are marked ┆ and boxed:\n{s}"
    );
}

#[test]
fn ambiguous_dates_are_errors() {
    let t = tempfile::tempdir().unwrap();
    setup(t.path());
    // 2026-10-19 is a Monday: "mon" is ambiguous; "tomorrow 3" is am/pm-ambiguous.
    for when in [&["mon", "10:00"][..], &["tomorrow", "3"][..], &["10/11"][..]] {
        let mut args = vec!["add", "Coffee"];
        args.extend_from_slice(when);
        let o = kairos(t.path(), "2026-10-19T08:00:00+01:00", &args);
        assert!(!o.status.success());
        assert!(
            String::from_utf8_lossy(&o.stderr).contains("ambiguous"),
            "{}",
            String::from_utf8_lossy(&o.stderr)
        );
    }
}

#[test]
fn writes_without_daemon_fail_clearly() {
    let t = tempfile::tempdir().unwrap();
    setup(t.path());
    let o = kairos(
        t.path(),
        "2026-10-19T08:00:00+01:00",
        &["add", "Coffee", "tomorrow", "10:00"],
    );
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("daemon.url"));
}

#[test]
fn doctor_reports_missing_sources() {
    let t = tempfile::tempdir().unwrap();
    setup(t.path());
    fs::remove_dir_all(t.path().join("vdir/o1")).unwrap();
    let o = kairos(t.path(), "2026-10-19T08:00:00+01:00", &["doctor"]);
    assert!(!o.status.success());
    let s = String::from_utf8_lossy(&o.stdout);
    assert!(
        s.contains("oqts") && s.contains("no vdir collection named \"OQTS\""),
        "{s}"
    );
}

#[test]
fn next_within_stays_empty_until_something_is_close() {
    let t = tempfile::tempdir().unwrap();
    setup(t.path());
    let now = "2026-10-19T06:00:00+01:00";
    let far = out(&kairos(t.path(), now, &["next", "--tmux", "--within", "30m"]));
    assert_eq!(far.trim(), "", "nothing within 30 min");
    let all = out(&kairos(t.path(), now, &["next", "--tmux"]));
    assert!(!all.trim().is_empty());
}
