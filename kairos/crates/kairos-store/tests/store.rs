#![allow(clippy::unwrap_used, clippy::expect_used)]

use chrono::{TimeZone, Utc};
use kairos_core::changeset::{Changeset, Risk, Status};
use kairos_core::config::Config;
use kairos_core::model::{Author, EventKind, Meta};
use kairos_core::task::{Task, TaskStatus};
use kairos_store::{cache, db, feeds, load, vdir};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::Path;

fn fixture(name: &str) -> String {
    fs::read_to_string(format!("{}/../../fixtures/edge/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// A config rooted in `root` with two iCloud calendars and one feed.
fn setup(root: &Path, feed_url: &str) -> Config {
    let toml = format!(
        r#"
timezone = "Europe/London"
[paths]
vdir = "{root}/vdir"
data = "{root}/data"
cache = "{root}/cache"
[[keys]]
name = "Personal"
index = 1
colour = "lavender"
[[keys]]
name = "OQTS"
index = 2
colour = "mauve"
[[sources]]
id = "personal"
kind = "icloud"
calendar = "Personal"
overlay = "personal"
key = "Personal"
writable = true
[[sources]]
id = "family"
kind = "icloud"
calendar = "Family"
overlay = "family"
writable = true
[[sources]]
id = "oqts-shared"
kind = "ics_readonly"
url = "{feed_url}"
overlay = "personal"
key = "OQTS"
"#,
        root = root.display()
    );
    let cfg = Config::from_toml(&toml).unwrap();
    assert_eq!(cfg.validate(), vec![]);
    // vdirsyncer-style opaque collection ids with displayname files.
    for (dir, name, file) in [
        ("AB12-personal", "Personal", "dedup_personal.ics"),
        ("CD34-family", "Family", "dedup_family.ics"),
    ] {
        let d = root.join("vdir").join(dir);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("displayname"), name).unwrap();
        // One event per file, like vdirsyncer.
        let cals = kairos_core::ics::parse(&fixture(file)).unwrap();
        for (i, ev) in cals[0].children_named("VEVENT").enumerate() {
            let mut cal = cals[0].clone();
            cal.children.retain(|c| c.name != "VEVENT");
            cal.children.push(ev.clone());
            fs::write(d.join(format!("item{i}.ics")), cal.to_ics()).unwrap();
        }
    }
    cfg
}

#[test]
fn loads_sources_by_displayname_and_reports_missing_feed() {
    let t = tempfile::tempdir().unwrap();
    let cfg = setup(t.path(), "http://127.0.0.1:9/never");
    let l = load::load_all(&cfg, &load::Sidecar::new()).unwrap();
    assert!(l.warnings.is_empty(), "{:?}", l.warnings);
    let ids: Vec<&str> = l.sources.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["personal", "family"]);
    assert_eq!(l.sources[0].series.len(), 5);
    assert_eq!(l.missing.len(), 1);
    assert!(l.missing[0].1.contains("not fetched"));
    // hrefs are the file names
    assert!(
        l.sources[0]
            .series
            .iter()
            .all(|s| s.href.as_deref().is_some_and(|h| h.starts_with("item")))
    );
}

#[test]
fn sidecar_metadata_is_applied() {
    let t = tempfile::tempdir().unwrap();
    let cfg = setup(t.path(), "http://127.0.0.1:9/never");
    let mut side = load::Sidecar::new();
    side.insert(
        ("personal".into(), "squash-mon@kairos.test".into()),
        Meta {
            kind: EventKind::TaskBlock,
            flex: true,
            created_by: Some(Author::Scheduler),
            ..Default::default()
        },
    );
    let l = load::load_all(&cfg, &side).unwrap();
    let s = l.sources[0]
        .series
        .iter()
        .find(|s| s.uid == "squash-mon@kairos.test")
        .unwrap();
    assert_eq!(s.master.as_ref().unwrap().meta.kind, EventKind::TaskBlock);
}

#[test]
fn cache_hit_then_invalidated_by_vdir_change() {
    let t = tempfile::tempdir().unwrap();
    let cfg = setup(t.path(), "http://127.0.0.1:9/never");
    let now = Utc.with_ymd_and_hms(2026, 10, 14, 9, 0, 0).unwrap();
    let (from, to) = cache::default_window(&cfg, now);
    let (idx, rebuilt) = cache::get(&cfg, None, now, from, to).unwrap();
    assert!(rebuilt);
    assert!(!idx.instances.is_empty());
    let (_, rebuilt) = cache::get(&cfg, None, now, from, to).unwrap();
    assert!(!rebuilt, "second read must hit the cache");
    // vdirsyncer adds an item (atomic rename → dir mtime changes).
    std::thread::sleep(std::time::Duration::from_millis(20));
    let dir = vdir::resolve(
        &cfg,
        cfg.source("personal").unwrap(),
        &vdir::collections(&cfg.vdir()).unwrap(),
    )
    .unwrap();
    vdir::write_item(&dir, "new.ics", &fixture("timezones.ics")).unwrap();
    let (idx2, rebuilt) = cache::get(&cfg, None, now, from, to).unwrap();
    assert!(rebuilt);
    assert!(idx2.instances.len() > idx.instances.len());
    // A window outside the cached one is computed but not cached.
    let far = (to + chrono::Duration::days(30), to + chrono::Duration::days(31));
    let (_, rebuilt) = cache::get(&cfg, None, now, far.0, far.1).unwrap();
    assert!(rebuilt);
}

#[test]
fn db_round_trips() {
    let d = db::Db::open_in_memory().unwrap();
    let now = Utc.with_ymd_and_hms(2026, 10, 12, 9, 0, 0).unwrap();
    let m = Meta {
        kind: EventKind::Deadline,
        created_by: Some(Author::Claude),
        ..Default::default()
    };
    d.meta_set("sport", "u1", &m).unwrap();
    assert_eq!(d.meta_get("sport", "u1").unwrap(), Some(m.clone()));
    assert_eq!(d.meta_all().unwrap().len(), 1);
    d.meta_delete("sport", "u1").unwrap();
    assert!(d.meta_get("sport", "u1").unwrap().is_none());

    let mut t = Task::new("B4 sheet", "Engineering", 240, Author::Claude, now);
    d.task_put(&t).unwrap();
    assert_eq!(d.task_get(&t.id).unwrap(), Some(t.clone()));
    assert_eq!(d.task_find("b4").unwrap().len(), 1);
    t.status = TaskStatus::Done;
    d.task_put(&t).unwrap();
    assert!(d.tasks(Some(TaskStatus::Open)).unwrap().is_empty());
    assert_eq!(d.tasks(None).unwrap().len(), 1);

    let c = Changeset {
        id: "cs-1".into(),
        author: Author::Claude,
        created_at: now,
        ops: vec![],
        risk: Risk::High,
        reasons: vec!["x".into()],
        status: Status::Pending,
        summary: "s".into(),
        error: None,
        decided_at: None,
    };
    d.cs_put(&c).unwrap();
    assert_eq!(d.cs_list(Some(Status::Pending), 10).unwrap().len(), 1);
    let mut c2 = c.clone();
    c2.status = Status::Applied;
    d.cs_put(&c2).unwrap();
    assert!(d.cs_list(Some(Status::Pending), 10).unwrap().is_empty());
    assert_eq!(d.cs_get("cs-1").unwrap().unwrap().status, Status::Applied);

    let ck = db::Checkin {
        block_uid: "b1".into(),
        task_id: t.id.clone(),
        day: now.date_naive(),
        planned_min: 60,
        outcome: None,
        actual_min: None,
        answered_at: None,
    };
    d.checkin_upsert_planned(&ck).unwrap();
    assert_eq!(d.checkins(true).unwrap().len(), 1);
    assert!(d.checkin_answer("b1", db::Outcome::Partial, 30, now).unwrap());
    assert!(d.checkins(true).unwrap().is_empty());
    assert_eq!(d.checkins(false).unwrap()[0].actual_min, Some(30));
}

/// Minimal HTTP server: first request 200 with ETag, then 304 if the ETag matches.
fn serve(body: String, requests: usize) -> (String, std::thread::JoinHandle<Vec<String>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/basic.ics", l.local_addr().unwrap());
    let h = std::thread::spawn(move || {
        let mut seen = Vec::new();
        for _ in 0..requests {
            let (mut s, _) = l.accept().unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut head = String::new();
            loop {
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                head.push_str(&line);
            }
            let cond = head.to_lowercase().contains("if-none-match: \"v1\"");
            let resp = if cond {
                "HTTP/1.1 304 Not Modified\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
            } else {
                format!(
                    "HTTP/1.1 200 OK\r\nETag: \"v1\"\r\nContent-Type: text/calendar\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
            };
            s.write_all(resp.as_bytes()).unwrap();
            seen.push(head);
        }
        seen
    });
    (url, h)
}

#[test]
fn feed_fetch_conditional_get() {
    let t = tempfile::tempdir().unwrap();
    let (url, h) = serve(fixture("dedup_oqts.ics"), 2);
    let cfg = setup(t.path(), &url);
    let src = cfg.source("oqts-shared").unwrap();
    let client = feeds::client().unwrap();
    let r1 = feeds::fetch(&cfg, src, &client).unwrap();
    assert!(matches!(r1, feeds::FetchOutcome::Updated { events: 5, .. }), "{r1:?}");
    let r2 = feeds::fetch(&cfg, src, &client).unwrap();
    assert_eq!(r2, feeds::FetchOutcome::NotModified);
    let heads = h.join().unwrap();
    assert!(heads[1].to_lowercase().contains("if-none-match"));
    let st = feeds::state(&cfg, "oqts-shared");
    assert_eq!(st.etag.as_deref(), Some("\"v1\""));
    let l = load::load_all(&cfg, &load::Sidecar::new()).unwrap();
    assert!(l.missing.is_empty());
    assert_eq!(l.sources.len(), 3);
}

#[test]
fn bad_feed_response_keeps_last_good_copy() {
    let t = tempfile::tempdir().unwrap();
    let (url, h) = serve(fixture("dedup_oqts.ics"), 1);
    let cfg = setup(t.path(), &url);
    let src = cfg.source("oqts-shared").unwrap().clone();
    let client = feeds::client().unwrap();
    feeds::fetch(&cfg, &src, &client).unwrap();
    h.join().unwrap();
    let (url2, h2) = serve("<html>not a calendar</html>".into(), 1);
    let mut src2 = src.clone();
    src2.url = Some(url2);
    // A fresh state so no If-None-Match is sent.
    fs::remove_file(t.path().join("data/feeds/oqts-shared.state.json")).unwrap();
    assert!(feeds::fetch(&cfg, &src2, &client).is_err());
    h2.join().unwrap();
    let kept = fs::read_to_string(vdir::feed_path(&cfg, "oqts-shared")).unwrap();
    assert!(kept.contains("BEGIN:VCALENDAR"));
    assert!(feeds::state(&cfg, "oqts-shared").last_error.is_some());
}
