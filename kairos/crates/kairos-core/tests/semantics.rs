//! Semantic tests over `fixtures/edge/*.ics` (SPEC §9 Phase 2 required tests).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Europe::London;
use kairos_core::config::{Overlay, SourceKind};
use kairos_core::dedup::Cluster;
use kairos_core::ics;
use kairos_core::model::{EventKind, Instance, Span};
use kairos_core::normalize::series_from_calendars;
use kairos_core::view::{Filter, LoadedSource, instances, view};

fn fixture(name: &str) -> String {
    let p = format!("{}/../../fixtures/edge/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{p}: {e}"))
}

fn load(name: &str, id: &str, kind: SourceKind, overlay: Overlay, key: Option<&str>) -> LoadedSource {
    let cals = ics::parse(&fixture(name)).unwrap();
    let mut w = Vec::new();
    let series = series_from_calendars(&cals, id, Some(name), &mut w);
    assert!(w.is_empty(), "{name}: {w:?}");
    LoadedSource {
        id: id.into(),
        kind,
        overlay,
        key: key.map(String::from),
        series,
    }
}

fn personal(name: &str) -> LoadedSource {
    load(
        name,
        "personal",
        SourceKind::Icloud,
        Overlay::Personal,
        Some("Personal"),
    )
}

fn at(s: &str) -> DateTime<Utc> {
    let n = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap();
    London.from_local_datetime(&n).earliest().unwrap().with_timezone(&Utc)
}

fn expand_all(src: &[LoadedSource], from: &str, to: &str) -> Vec<Instance> {
    let mut w = Vec::new();
    let v = instances(src, at(from), at(to), London, &mut w);
    assert!(w.is_empty(), "{w:?}");
    let mut v = v;
    v.sort_by_key(|i| i.start(London));
    v
}

fn local(i: &Instance) -> String {
    match i.span {
        Span::Timed { start, end } => format!(
            "{}–{}",
            start.with_timezone(&London).format("%a %Y-%m-%d %H:%M"),
            end.with_timezone(&London).format("%H:%M")
        ),
        Span::AllDay { start, end } => format!("{start}..{end}"),
    }
}

fn d(y: i32, m: u32, dd: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, dd).unwrap()
}

// ---------------------------------------------------------------- DST

#[test]
fn weekly_sunday_1900_stays_local_across_autumn_change() {
    let src = [personal("dst_weekly_sunday.ics")];
    let v = expand_all(&src, "2026-10-17 00:00", "2026-11-02 00:00");
    let got: Vec<String> = v.iter().map(local).collect();
    assert_eq!(
        got,
        [
            "Sun 2026-10-18 19:00–20:00",
            "Sun 2026-10-25 19:00–20:00",
            "Sun 2026-11-01 19:00–20:00",
        ]
    );
    // And the UTC instants differ by the offset change.
    assert_eq!(
        v[0].start(London),
        Utc.with_ymd_and_hms(2026, 10, 18, 18, 0, 0).unwrap()
    );
    assert_eq!(
        v[1].start(London),
        Utc.with_ymd_and_hms(2026, 10, 25, 19, 0, 0).unwrap()
    );
}

#[test]
fn weekly_sunday_1900_stays_local_across_spring_change() {
    let src = [personal("dst_weekly_sunday.ics")];
    let v = expand_all(&src, "2027-03-20 00:00", "2027-04-05 00:00");
    let got: Vec<String> = v.iter().map(local).collect();
    assert_eq!(
        got,
        [
            "Sun 2027-03-21 19:00–20:00",
            "Sun 2027-03-28 19:00–20:00",
            "Sun 2027-04-04 19:00–20:00",
        ]
    );
    assert_eq!(v[1].start(London), Utc.with_ymd_and_hms(2027, 3, 28, 18, 0, 0).unwrap());
}

#[test]
fn ambiguous_0130_on_25_oct_2026_is_first_occurrence() {
    // Documented rule (RFC 5545 §3.3.5): first occurrence, i.e. 01:30 BST = 00:30Z.
    let src = [personal("dst_ambiguous.ics")];
    let v = expand_all(&src, "2026-10-24 00:00", "2026-10-26 00:00");
    assert_eq!(v.len(), 1);
    assert_eq!(
        v[0].start(London),
        Utc.with_ymd_and_hms(2026, 10, 25, 0, 30, 0).unwrap()
    );
    // Ends 02:30 GMT: 2 real hours, because 01:00–02:00 happens twice.
    assert_eq!(v[0].end(London), Utc.with_ymd_and_hms(2026, 10, 25, 2, 30, 0).unwrap());
}

#[test]
fn nonexistent_0130_on_28_mar_2027_uses_pre_gap_offset() {
    let src = [personal("dst_ambiguous.ics")];
    let v = expand_all(&src, "2027-03-27 00:00", "2027-03-29 00:00");
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].start(London), Utc.with_ymd_and_hms(2027, 3, 28, 1, 30, 0).unwrap());
    assert_eq!(local(&v[0]), "Sun 2027-03-28 02:30–03:30");
}

// ---------------------------------------------------------------- all-day

#[test]
fn all_day_end_is_exclusive_and_multi_day_covers_each_day() {
    let src = [personal("allday.ics")];
    let v = expand_all(&src, "2026-10-20 00:00", "2026-11-02 00:00");
    let by_title = |t: &str| v.iter().find(|i| i.title == t).unwrap();
    let single = by_title("Collections");
    assert_eq!(
        single.span,
        Span::AllDay {
            start: d(2026, 10, 23),
            end: d(2026, 10, 24)
        }
    );
    assert_eq!(single.span.local_dates(London), vec![d(2026, 10, 23)]);
    assert!(single.transparent);
    let multi = by_title("Lake District trip");
    assert_eq!(
        multi.span.local_dates(London),
        vec![d(2026, 10, 23), d(2026, 10, 24), d(2026, 10, 25)]
    );
    // 25 Oct is the 25-hour DST day; still exactly one calendar day.
    let noend = by_title("Mum's birthday");
    assert_eq!(
        noend.span,
        Span::AllDay {
            start: d(2026, 10, 30),
            end: d(2026, 10, 31)
        }
    );
}

#[test]
fn all_day_not_shown_on_next_day_in_window_query() {
    let src = [personal("allday.ics")];
    let v = expand_all(&src, "2026-10-24 00:00", "2026-10-25 00:00");
    let titles: Vec<&str> = v.iter().map(|i| i.title.as_str()).collect();
    assert_eq!(
        titles,
        ["Lake District trip"],
        "single-day 23 Oct event must not leak into 24 Oct"
    );
}

#[test]
fn all_day_in_non_utc_zone_has_no_off_by_one() {
    // A UTC-midnight boundary is 01:00 BST; a 23 Oct all-day event must not
    // touch 22 Oct when queried in London time.
    let src = [personal("allday.ics")];
    let v = expand_all(&src, "2026-10-22 00:00", "2026-10-23 00:00");
    assert!(v.is_empty(), "{:?}", v.iter().map(|i| &i.title).collect::<Vec<_>>());
}

// ---------------------------------------------------------------- recurrence

#[test]
fn count_and_exdate() {
    let src = [personal("recur_count_until_exdate.ics")];
    let v = expand_all(&src, "2026-10-01 00:00", "2026-12-01 00:00");
    let runs: Vec<String> = v.iter().filter(|i| i.title == "Morning run").map(local).collect();
    assert_eq!(
        runs,
        [
            "Mon 2026-10-12 09:00–10:00",
            "Tue 2026-10-13 09:00–10:00",
            "Thu 2026-10-15 09:00–10:00",
            "Fri 2026-10-16 09:00–10:00",
        ],
        "COUNT=5 with the 14th excluded leaves 4"
    );
}

#[test]
fn until_crossing_dst() {
    // UNTIL=20261103T140000Z is 14:00 GMT, i.e. the 3 Nov 14:00 local occurrence is included.
    let src = [personal("recur_count_until_exdate.ics")];
    let v = expand_all(&src, "2026-10-01 00:00", "2026-12-01 00:00");
    let labs: Vec<String> = v.iter().filter(|i| i.title == "P1 Lab").map(local).collect();
    assert_eq!(
        labs,
        [
            "Tue 2026-10-13 14:00–16:00",
            "Tue 2026-10-20 14:00–16:00",
            "Tue 2026-10-27 14:00–16:00",
            "Tue 2026-11-03 14:00–16:00",
        ]
    );
}

#[test]
fn moved_override_replaces_original_slot() {
    let src = [personal("recur_override_moved.ics")];
    let v = expand_all(&src, "2026-10-12 00:00", "2026-11-10 00:00");
    let got: Vec<String> = v.iter().map(|i| format!("{} {}", local(i), i.title)).collect();
    assert_eq!(
        got,
        [
            "Tue 2026-10-13 10:00–11:00 B4 Lecture",
            "Thu 2026-10-22 14:00–15:00 B4 Lecture (moved)",
            "Tue 2026-10-27 10:00–11:00 B4 Lecture",
            "Tue 2026-11-03 10:00–11:00 B4 Lecture",
        ]
    );
    let moved = &v[1];
    assert_eq!(moved.location.as_deref(), Some("LR2"));
    assert_eq!(moved.recurrence_id.as_deref(), Some("20261020T090000Z"));
}

#[test]
fn moved_override_visible_when_original_slot_outside_window() {
    let src = [personal("recur_override_moved.ics")];
    let v = expand_all(&src, "2026-10-21 00:00", "2026-10-23 00:00");
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].title, "B4 Lecture (moved)");
    // ...and querying only the original slot's day shows nothing.
    let v = expand_all(&src, "2026-10-20 00:00", "2026-10-21 00:00");
    assert!(v.is_empty());
}

#[test]
fn cancelled_instance_removed() {
    let src = [personal("recur_cancelled.ics")];
    let v = expand_all(&src, "2026-10-01 00:00", "2026-11-01 00:00");
    let got: Vec<String> = v.iter().map(local).collect();
    assert_eq!(got, ["Mon 2026-10-12 18:00–19:30", "Mon 2026-10-26 18:00–19:30"]);
}

#[test]
fn thisandfuture_shifts_later_instances() {
    let src = [personal("recur_thisandfuture.ics")];
    let v = expand_all(&src, "2026-10-01 00:00", "2026-12-01 00:00");
    let got: Vec<String> = v
        .iter()
        .filter(|i| i.uid == "thisandfuture@kairos.test")
        .map(|i| format!("{} {}", local(i), i.title))
        .collect();
    assert_eq!(
        got,
        [
            "Mon 2026-10-12 09:00–10:00 Tutorial",
            "Mon 2026-10-19 09:00–10:00 Tutorial",
            "Mon 2026-10-26 10:00–11:30 Tutorial (new slot)",
            "Mon 2026-11-02 10:00–11:30 Tutorial (new slot)",
            "Mon 2026-11-09 10:00–11:30 Tutorial (new slot)",
            "Mon 2026-11-16 10:00–11:30 Tutorial (new slot)",
        ]
    );
}

#[test]
fn icloud_style_split_series() {
    // iCloud implements "this and future" by ending the old series (UNTIL)
    // and starting a new UID. Both halves must show, without overlap.
    let src = [personal("recur_thisandfuture.ics")];
    let v = expand_all(&src, "2026-10-01 00:00", "2026-12-01 00:00");
    let got: Vec<String> = v.iter().filter(|i| i.title == "Rowing erg").map(local).collect();
    assert_eq!(
        got,
        [
            "Tue 2026-10-13 17:00–18:00",
            "Tue 2026-10-20 17:00–18:00",
            "Thu 2026-10-29 07:00–08:00",
            "Thu 2026-11-05 07:00–08:00",
            "Thu 2026-11-12 07:00–08:00",
        ]
    );
}

// ---------------------------------------------------------------- time zones

#[test]
fn floating_vienna_utc_outlook_in_london_time() {
    let src = [personal("timezones.ics")];
    let v = expand_all(&src, "2026-10-21 00:00", "2026-10-22 00:00");
    let got: Vec<String> = v.iter().map(|i| format!("{} {}", local(i), i.title)).collect();
    assert_eq!(
        got,
        [
            "Wed 2026-10-21 09:00–10:00 Floating coffee",
            "Wed 2026-10-21 11:00–12:00 Call with Vienna",
            "Wed 2026-10-21 16:00–17:00 UTC webinar",
            "Wed 2026-10-21 17:00–18:00 Outlook-zoned meeting",
        ]
    );
}

// ---------------------------------------------------------------- dedup

fn dedup_sources() -> Vec<LoadedSource> {
    vec![
        load(
            "dedup_personal.ics",
            "personal",
            SourceKind::Icloud,
            Overlay::Personal,
            Some("Personal"),
        ),
        load(
            "dedup_oqts.ics",
            "oqts-shared",
            SourceKind::IcsReadonly,
            Overlay::Personal,
            Some("OQTS"),
        ),
        load("dedup_family.ics", "family", SourceKind::Icloud, Overlay::Family, None),
    ]
}

fn dedup_view(filter: &Filter) -> Vec<Cluster> {
    let mut w = Vec::new();
    let v = view(
        &dedup_sources(),
        filter,
        at("2026-10-12 00:00"),
        at("2026-10-26 00:00"),
        London,
        &mut w,
    );
    assert!(w.is_empty());
    v
}

fn cluster_of<'a>(v: &'a [Cluster], uid: &str) -> &'a Cluster {
    v.iter().find(|c| c.all().any(|i| i.uid == uid)).unwrap()
}

#[test]
fn must_merge_same_uid_and_fuzzy_title() {
    let v = dedup_view(&Filter::default());
    // Same UID in personal + OQTS feed, plus a Family copy 5 min later with a
    // differently-cased title: one cluster, Family wins.
    let c = cluster_of(&v, "same-uid@kairos.test");
    assert_eq!(c.others.len(), 2, "{c:#?}");
    assert_eq!(c.primary.overlay, Overlay::Family);
    // Fuzzy: "OQTS committee meeting!" vs "OQTS Committee Meeting" 10 min apart.
    let c = cluster_of(&v, "personal-copy-committee@kairos.test");
    assert_eq!(c.others.len(), 1);
    assert_eq!(c.primary.source, "personal", "iCloud personal beats OQTS import");
}

#[test]
fn hiding_family_reveals_personal_copy() {
    let f = Filter {
        family: false,
        ..Filter::default()
    };
    let v = dedup_view(&f);
    let c = cluster_of(&v, "same-uid@kairos.test");
    assert_eq!(c.primary.source, "personal");
    assert_eq!(c.others.len(), 1);
}

#[test]
fn must_not_merge_different_lectures_same_time() {
    let v = dedup_view(&Filter::default());
    let a = cluster_of(&v, "lecture-a@kairos.test");
    let b = cluster_of(&v, "lecture-b@kairos.test");
    assert!(!a.is_merged() && !b.is_merged());
}

#[test]
fn must_not_merge_squash_on_different_days() {
    let v = dedup_view(&Filter::default());
    assert!(!cluster_of(&v, "squash-mon@kairos.test").is_merged());
    assert!(!cluster_of(&v, "squash-wed@kairos.test").is_merged());
}

#[test]
fn must_not_merge_moved_instance_with_original_slot() {
    // dedup_personal has "B4 Lecture" at the original Tue slot; dedup_oqts has
    // it at the moved Thu slot. Different days → separate.
    let v = dedup_view(&Filter::default());
    assert!(!cluster_of(&v, "original-slot@kairos.test").is_merged());
    assert!(!cluster_of(&v, "moved-copy@kairos.test").is_merged());
    // Same check against the real recurring fixture: the moved override and a
    // copy at the original slot never merge.
    let src = [
        personal("recur_override_moved.ics"),
        load(
            "dedup_personal.ics",
            "other",
            SourceKind::Icloud,
            Overlay::Personal,
            Some("Personal"),
        ),
    ];
    let mut w = Vec::new();
    let v = view(
        &src,
        &Filter::default(),
        at("2026-10-19 00:00"),
        at("2026-10-24 00:00"),
        London,
        &mut w,
    );
    let moved = cluster_of(&v, "lecture-moved@kairos.test");
    assert!(moved.all().all(|i| i.uid != "original-slot@kairos.test"));
}

#[test]
fn dedup_never_drops_instances() {
    let mut w = Vec::new();
    let all = instances(
        &dedup_sources(),
        at("2026-10-12 00:00"),
        at("2026-10-26 00:00"),
        London,
        &mut w,
    );
    let v = dedup_view(&Filter::default());
    let n: usize = v.iter().map(|c| 1 + c.others.len()).sum();
    assert_eq!(n, all.len());
}

#[test]
fn key_toggle_hides_only_that_key() {
    let mut f = Filter::default();
    f.toggle_key("OQTS");
    let v = dedup_view(&f);
    assert!(v.iter().all(|c| c.all().all(|i| i.key.as_deref() != Some("OQTS"))));
    assert!(v.iter().any(|c| c.primary.uid == "squash-mon@kairos.test"));
}

// ---------------------------------------------------------------- kairos kinds

#[test]
fn deadlines_and_task_blocks() {
    let src = [personal("deadline_and_block.ics")];
    let v = expand_all(&src, "2026-10-19 00:00", "2026-10-26 00:00");
    let dl = v.iter().find(|i| i.uid == "deadline-timed@kairos.test").unwrap();
    assert_eq!(dl.kind(), EventKind::Deadline);
    assert_eq!(dl.start(London), dl.end(London), "timed deadline is zero-duration");
    assert_eq!(dl.alarms, vec![1440, 120]);
    assert_eq!(dl.meta.task_id.as_deref(), Some("task-b4"));
    let all_day = v.iter().find(|i| i.uid == "deadline-allday@kairos.test").unwrap();
    assert!(all_day.span.is_all_day());
    let block = v.iter().find(|i| i.uid == "block-1@kairos.test").unwrap();
    assert_eq!(block.kind(), EventKind::TaskBlock);
    assert!(block.meta.flex);
}

#[test]
fn zero_duration_deadline_is_in_window_at_its_time() {
    let src = [personal("deadline_and_block.ics")];
    let v = expand_all(&src, "2026-10-23 17:00", "2026-10-23 17:01");
    assert!(v.iter().any(|i| i.uid == "deadline-timed@kairos.test"));
}

// ---------------------------------------------------------------- losslessness

#[test]
fn every_fixture_round_trips_losslessly() {
    let dirs = ["edge", "golden"];
    let mut n = 0;
    for dir in dirs {
        let p = format!("{}/../../fixtures/{dir}", env!("CARGO_MANIFEST_DIR"));
        let Ok(rd) = std::fs::read_dir(&p) else { continue };
        for e in rd {
            let path = e.unwrap().path();
            if path.extension().is_none_or(|x| x != "ics") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let a = ics::parse(&text).unwrap();
            let out: String = a.iter().map(|c| c.to_ics()).collect();
            let b = ics::parse(&out).unwrap();
            assert_eq!(a, b, "{}", path.display());
            // Logical-line equality with the source (modulo folding/line ends).
            assert_eq!(ics::unfold(&text), ics::unfold(&out), "{}", path.display());
            n += 1;
        }
    }
    assert!(n >= 12, "expected fixtures, found {n}");
}
