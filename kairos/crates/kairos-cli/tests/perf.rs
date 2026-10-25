#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Performance budgets (SPEC §8) on a generated 5 000-event calendar:
//! `kairos next --tmux` < 20 ms, `kairos today` < 50 ms, TUI cold start < 150 ms.
//! Budgets are asserted in release builds (`scripts/ci.sh` runs this with
//! `--release`); debug builds only print the timings.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

const NOW: &str = "2026-10-14T12:00:00+01:00";

fn generate(dir: &Path) {
    let mut seed: u64 = 0x5eed;
    let mut rnd = move |n: u64| {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 33) % n
    };
    let cals = ["Personal", "Engineering", "Sport"];
    for (k, name) in cals.iter().enumerate() {
        let d = dir.join("vdir").join(format!("c{k}"));
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("displayname"), name).unwrap();
    }
    for n in 0..5000u64 {
        let cal = rnd(3);
        let offset_days = rnd(365) as i64 - 182;
        let date = chrono::NaiveDate::from_ymd_opt(2026, 10, 14).unwrap() + chrono::Duration::days(offset_days);
        let hour = 7 + rnd(14);
        let min = [0, 15, 30, 45][rnd(4) as usize];
        let dur = [30, 60, 90, 120][rnd(4) as usize];
        let start = date.and_hms_opt(hour as u32, min, 0).unwrap();
        let end = start + chrono::Duration::minutes(dur);
        let mut s = String::new();
        let _ = write!(
            s,
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//perf//EN\r\nBEGIN:VEVENT\r\nUID:perf-{n}\r\nDTSTAMP:20260101T000000Z\r\nDTSTART;TZID=Europe/London:{}\r\nDTEND;TZID=Europe/London:{}\r\nSUMMARY:Event number {n}\r\nLOCATION:Room {}\r\n",
            start.format("%Y%m%dT%H%M%S"),
            end.format("%Y%m%dT%H%M%S"),
            rnd(40)
        );
        if n % 17 == 0 {
            s.push_str("RRULE:FREQ=WEEKLY\r\n");
        }
        s.push_str("END:VEVENT\r\nEND:VCALENDAR\r\n");
        fs::write(
            dir.join("vdir").join(format!("c{cal}")).join(format!("perf-{n}.ics")),
            s,
        )
        .unwrap();
    }
    let mut toml = format!(
        "[paths]\nvdir = \"{0}/vdir\"\ndata = \"{0}/data\"\ncache = \"{0}/cache\"\n[sync]\nfull_cmd = \"\"\n",
        dir.display()
    );
    for (k, name) in cals.iter().enumerate() {
        let _ = write!(
            toml,
            "[[keys]]\nname = \"{name}\"\nindex = {}\ncolour = \"blue\"\n[[sources]]\nid = \"s{k}\"\nkind = \"icloud\"\ncalendar = \"{name}\"\noverlay = \"personal\"\nkey = \"{name}\"\nwritable = true\n",
            k + 1
        );
    }
    fs::write(dir.join("config.toml"), toml).unwrap();
}

fn median_ms(dir: &Path, args: &[&str], runs: usize) -> f64 {
    let mut v: Vec<Duration> = (0..runs)
        .map(|_| {
            let t = Instant::now();
            let out = Command::new(env!("CARGO_BIN_EXE_kairos"))
                .args(["--config", dir.join("config.toml").to_str().unwrap()])
                .args(args)
                .env("KAIROS_NOW", NOW)
                .env("NO_COLOR", "1")
                .output()
                .unwrap();
            let e = t.elapsed();
            assert!(
                out.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            e
        })
        .collect();
    v.sort();
    v[runs / 2].as_secs_f64() * 1000.0
}

#[test]
fn budgets_on_5k_events() {
    let t = tempfile::tempdir().unwrap();
    generate(t.path());
    // Build the cache (what the launchd/systemd sync job does after vdirsyncer).
    let t0 = Instant::now();
    let out = Command::new(env!("CARGO_BIN_EXE_kairos"))
        .args([
            "--config",
            t.path().join("config.toml").to_str().unwrap(),
            "sync",
            "--local",
        ])
        .env("KAIROS_NOW", NOW)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let build = t0.elapsed().as_secs_f64() * 1000.0;

    let next = median_ms(t.path(), &["next", "--tmux"], 15);
    let today = median_ms(t.path(), &["today"], 15);

    // TUI cold start: load config + cache, compute the view, render the first frame.
    let mut tui = Vec::new();
    for _ in 0..7 {
        let t1 = Instant::now();
        let cfg = kairos_core::config::Config::load(&t.path().join("config.toml")).unwrap();
        let now = chrono::DateTime::parse_from_rfc3339(NOW)
            .unwrap()
            .with_timezone(&chrono::Utc);
        let app = kairos_tui::load(cfg, &t.path().join("config.toml"), now).unwrap();
        let frame = kairos_tui::render_to_string(&app, 160, 48);
        assert!(frame.contains("week"));
        tui.push(t1.elapsed());
    }
    tui.sort();
    let tui = tui[3].as_secs_f64() * 1000.0;

    eprintln!(
        "5k events: cache build {build:.1} ms · next --tmux {next:.1} ms · today {today:.1} ms · TUI first frame {tui:.1} ms"
    );
    if !cfg!(debug_assertions) {
        assert!(next < 20.0, "kairos next --tmux took {next:.1} ms (budget 20)");
        assert!(today < 50.0, "kairos today took {today:.1} ms (budget 50)");
        assert!(tui < 150.0, "TUI cold start took {tui:.1} ms (budget 150)");
    }
}
