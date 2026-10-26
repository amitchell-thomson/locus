#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Performance budgets (SPEC §8.2) on a synthetic 10k-message mirror.
//! Run in release: `cargo test --release -p keryx-cli --test perf -- --nocapture`
//! (scripts/ci.sh does). Ignored in debug builds, where the budgets don't apply.

use chrono::TimeZone;
use keryx_core::config::Config;
use keryx_core::model::{Address, Attachment, Location, Message};
use keryx_store::db::{Db, Raw};
use keryx_store::view::{self, Filter, View};
use std::time::Instant;

const WORDS: &[&str] = &[
    "meeting",
    "budget",
    "sheet",
    "lecture",
    "project",
    "deadline",
    "report",
    "society",
    "dinner",
    "lab",
    "thesis",
    "review",
    "draft",
    "invoice",
    "travel",
    "booking",
    "seminar",
    "tutorial",
    "exam",
    "notes",
    "results",
    "college",
    "treasurer",
    "minutes",
    "agenda",
    "careers",
    "internship",
    "application",
    "reference",
    "schedule",
];

fn synthetic(n: usize) -> (Config, Db) {
    let cfg = Config::load(&keryx_store::testkit::root().join("config/config.test.toml")).unwrap();
    let mut db = Db::open_in_memory().unwrap();
    let accounts = ["personal", "oxford", "oqts", "oqts-shared"];
    let mut seed: u64 = 42;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    db.transaction(|db| {
        for i in 0..n {
            let r = rnd();
            let account = accounts[(r % 4) as usize];
            let thread = format!("t{}", i / 3);
            let body: String = (0..150)
                .map(|k| WORDS[((r >> (k % 40)) as usize + k) % WORDS.len()])
                .collect::<Vec<_>>()
                .join(" ");
            let m = Message {
                account: account.into(),
                source_id: format!("{i}"),
                message_id: format!("m{i}@x"),
                thread_id: thread,
                from: Some(Address::new(
                    Some(&format!("Person {}", r % 300)),
                    &format!("p{}@example.com", r % 300),
                )),
                to: vec![Address::new(None, "newc7370@ox.ac.uk")],
                cc: vec![],
                bcc: vec![],
                reply_to: vec![],
                subject: format!(
                    "{} {} {}",
                    WORDS[(r % 30) as usize],
                    WORDS[((r >> 5) % 30) as usize],
                    i % 97
                ),
                date: 1_760_000_000 + i as i64 * 600,
                location: Location::Labels {
                    labels: vec![if i % 10 < 3 { "INBOX".into() } else { "Label_1".into() }],
                },
                in_inbox: i % 10 < 3,
                read: r % 3 != 0,
                flagged: r % 50 == 0,
                size: 2000,
                attachments: if r % 7 == 0 {
                    vec![Attachment {
                        id: 2,
                        name: "a.pdf".into(),
                        mime: "application/pdf".into(),
                        size: 1000,
                    }]
                } else {
                    vec![]
                },
                snippet: body.chars().take(140).collect(),
                body_text: body,
                hidden_text_removed: false,
                in_reply_to: vec![],
                references: vec![],
                list_headers: vec![],
            };
            db.upsert(&m, Raw::None, keryx_core::text::TEXT_VERSION)?;
        }
        Ok(())
    })
    .unwrap();
    (cfg, db)
}

fn best_of<T>(n: usize, mut f: impl FnMut() -> T) -> (std::time::Duration, T) {
    let mut best = std::time::Duration::MAX;
    let mut last = None;
    for _ in 0..n {
        let t = Instant::now();
        let v = f();
        best = best.min(t.elapsed());
        last = Some(v);
    }
    (best, last.unwrap())
}

#[test]
#[cfg_attr(debug_assertions, ignore = "budgets apply to release builds")]
fn budgets_10k() {
    let (cfg, db) = synthetic(10_000);
    let now = chrono_tz::Europe::London
        .with_ymd_and_hms(2026, 10, 14, 15, 0, 0)
        .unwrap();
    let f = Filter {
        accounts: vec![],
        limit: 50,
    };

    let (inbox, rows) = best_of(5, || view::list(&db, &cfg, &View::Inbox, &f).unwrap());
    assert_eq!(rows.len(), 50);
    let q = keryx_core::query::parse("budget from:person", now).unwrap();
    let (search, hits) = best_of(5, || view::search(&db, &cfg, &q, &f).unwrap());
    assert!(!hits.is_empty());

    let mut app = keryx_tui::app::App::new(cfg.clone(), synthetic(10_000).1, now);
    let (frame, _) = best_of(1, || {
        app.reload();
        keryx_tui::render_to_string(&mut app, 160, 50)
    });

    println!("10k messages: inbox {inbox:?}, search {search:?}, TUI first frame {frame:?}");
    assert!(inbox.as_millis() < 30, "inbox {inbox:?} ≥ 30 ms");
    assert!(search.as_millis() < 50, "search {search:?} ≥ 50 ms");
    assert!(frame.as_millis() < 150, "TUI first frame {frame:?} ≥ 150 ms");
}
