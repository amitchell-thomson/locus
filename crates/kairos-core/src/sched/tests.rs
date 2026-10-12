use super::*;
use crate::model::Author;
use chrono::{NaiveDateTime, TimeZone};
use chrono_tz::Europe::London;
use proptest::prelude::*;

const CONSTRAINTS: &str = include_str!("../../../../config/constraints.example.toml");

fn constraints() -> Constraints {
    Constraints::from_toml(CONSTRAINTS).unwrap()
}

fn at(s: &str) -> DateTime<Utc> {
    let n = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap();
    London.from_local_datetime(&n).earliest().unwrap().with_timezone(&Utc)
}

fn task(id: &str, mins: u32, due: Option<&str>) -> Task {
    let mut t = Task::new(id, "Engineering", mins, Author::Claude, at("2026-10-01 00:00"));
    t.id = id.into();
    t.due = due.map(|d| Due::At(at(d)));
    t
}

fn busy(a: &str, b: &str) -> Busy {
    Busy {
        start: at(a),
        end: at(b),
        location: None,
    }
}

fn local(t: DateTime<Utc>) -> String {
    t.with_timezone(&London).format("%a %d %H:%M").to_string()
}

fn total(plan: &Plan, task: &str) -> i64 {
    plan.keep
        .iter()
        .chain(&plan.create)
        .filter(|b| b.task_id == task)
        .map(Block::minutes)
        .sum()
}

/// Lectures Mon–Fri 09:00–11:00 and 14:00–16:00 for the week of 12 Oct.
fn lectures() -> Vec<Busy> {
    let mut v = Vec::new();
    for d in 12..=16 {
        v.push(busy(&format!("2026-10-{d} 09:00"), &format!("2026-10-{d} 11:00")));
        v.push(busy(&format!("2026-10-{d} 14:00"), &format!("2026-10-{d} 16:00")));
    }
    v
}

fn run_plan(now: &str, tasks: &[Task], busy: &[Busy], existing: &[Block]) -> Plan {
    let c = constraints();
    plan(&PlanInput {
        now: at(now),
        tz: London,
        constraints: &c,
        tasks,
        busy,
        existing,
    })
}

/// Check every invariant the scheduler promises.
fn check_invariants(now: DateTime<Utc>, tasks: &[Task], busy: &[Busy], p: &Plan) -> Result<(), String> {
    let c = constraints();
    let mut blocks: Vec<&Block> = p
        .create
        .iter()
        .chain(p.keep.iter().filter(|b| !b.pinned && b.start >= now))
        .collect();
    blocks.sort_by_key(|b| b.start);
    let mut per_day: BTreeMap<NaiveDate, i64> = BTreeMap::new();
    for b in &blocks {
        if b.start < now + Duration::minutes(30) {
            return Err(format!("block at {} within 30 min of now", local(b.start)));
        }
        if b.end <= b.start {
            return Err("empty block".into());
        }
        for bz in busy {
            if b.start < bz.end && bz.start < b.end {
                return Err(format!(
                    "block {}–{} overlaps busy {}",
                    local(b.start),
                    local(b.end),
                    local(bz.start)
                ));
            }
        }
        let mut t = b.start;
        while t < b.end {
            let l = t.with_timezone(&London);
            let p = c.profile_for(l.date_naive()).unwrap();
            let m = (l.hour() * 60 + l.minute()) as u16;
            if !slot_allowed(p, m, c.slot_minutes, l.weekday()) {
                return Err(format!("block slot {} violates profile {}", local(t), p.name));
            }
            *per_day.entry(l.date_naive()).or_default() += i64::from(c.slot_minutes);
            t += Duration::minutes(i64::from(c.slot_minutes));
        }
        let task = tasks.iter().find(|t| t.id == b.task_id).ok_or("unknown task")?;
        if let Some(d) = task.due
            && b.end > d.instant(London)
        {
            return Err(format!(
                "block for {} ends {} after its deadline",
                task.id,
                local(b.end)
            ));
        }
    }
    for w in blocks.windows(2) {
        if w[1].start < w[0].end {
            return Err("blocks overlap each other".into());
        }
    }
    for (d, mins) in per_day {
        let cap = (c.profile_for(d).unwrap().max_focus_hours_per_day * 60.0) as i64;
        if mins > cap {
            return Err(format!("{d}: {mins} min > focus cap {cap}"));
        }
    }
    for t in tasks {
        if total(p, &t.id) > i64::from(t.remaining_min.div_ceil(15) * 15) {
            return Err(format!("{} over-allocated", t.id));
        }
    }
    Ok(())
}

#[test]
fn b4_sheet_fits_before_deadline() {
    let tasks = [task("b4", 240, Some("2026-10-16 17:00"))];
    let p = run_plan("2026-10-12 08:00", &tasks, &lectures(), &[]);
    assert!(p.shortfalls.is_empty(), "{:?}", p.shortfalls);
    assert_eq!(total(&p, "b4"), 240);
    check_invariants(at("2026-10-12 08:00"), &tasks, &lectures(), &p).unwrap();
    for b in &p.create {
        assert!(b.minutes() >= 30 && b.minutes() <= 120, "chunk {}", b.minutes());
    }
    // Earliest feasible day is Monday.
    assert!(
        local(p.create[0].start).starts_with("Mon 12"),
        "{}",
        local(p.create[0].start)
    );
}

#[test]
fn infeasible_reports_shortfall_never_overbooks() {
    let tasks = [task("big", 40 * 60, Some("2026-10-14 17:00"))];
    let p = run_plan("2026-10-12 08:00", &tasks, &lectures(), &[]);
    assert_eq!(p.shortfalls.len(), 1);
    let s = &p.shortfalls[0];
    assert!(s.message.starts_with("big: "), "{}", s.message);
    assert!(s.message.contains("short before Wed 14 Oct 17:00"), "{}", s.message);
    assert_eq!(i64::from(s.short_min) + total(&p, "big"), 40 * 60);
    assert!(s.options.iter().any(|o| o.contains("estimate")));
    check_invariants(at("2026-10-12 08:00"), &tasks, &lectures(), &p).unwrap();
}

#[test]
fn deadline_today() {
    // Due 23:00 today, 12 h lead → aim to finish by 11:00. Without lectures
    // that fits in the morning, preferring the 09:00 deep-work window.
    let tasks = [task("today", 60, Some("2026-10-12 23:00"))];
    let p = run_plan("2026-10-12 08:00", &tasks, &[], &[]);
    assert!(p.shortfalls.is_empty(), "{:?}", p.shortfalls);
    assert_eq!(local(p.create[0].start), "Mon 12 09:00");
    assert!(p.create[0].end <= at("2026-10-12 11:00"));
    // With lectures the morning is gone (08:30–08:50 is too short after the
    // buffer), so it falls back past the lead to the first free afternoon slot.
    let p = run_plan("2026-10-12 08:00", &tasks, &lectures(), &[]);
    assert!(p.shortfalls.is_empty(), "{:?}", p.shortfalls);
    assert_eq!(total(&p, "today"), 60);
    assert_eq!(local(p.create[0].start), "Mon 12 11:15");
    check_invariants(at("2026-10-12 08:00"), &tasks, &lectures(), &p).unwrap();
}

#[test]
fn lead_is_a_preference_not_a_wall() {
    // The 2026-10-12 case: tasks made at 13:43 for 20:00 and 23:59 the same
    // day. The 12 h lead (08:00 / 11:59) had already passed, which used to
    // mean "short" despite a free evening.
    let tasks = [
        task("populate", 30, Some("2026-10-12 20:00")),
        task("furniture", 60, Some("2026-10-12 23:59")),
    ];
    let busy = [busy("2026-10-12 14:00", "2026-10-12 16:00")];
    let p = run_plan("2026-10-12 13:43", &tasks, &busy, &[]);
    assert!(p.shortfalls.is_empty(), "{:?}", p.shortfalls);
    assert_eq!(total(&p, "populate"), 30);
    assert_eq!(total(&p, "furniture"), 60);
    for b in &p.create {
        assert!(b.start >= at("2026-10-12 16:10"), "{}", local(b.start));
    }
    check_invariants(at("2026-10-12 13:43"), &tasks, &busy, &p).unwrap();
}

#[test]
fn lead_still_preferred_when_there_is_room() {
    // Due Wed 17:00: Mon and Tue are free, so nothing lands in the last 12 h.
    let tasks = [task("t", 4 * 60, Some("2026-10-14 17:00"))];
    let p = run_plan("2026-10-12 08:00", &tasks, &lectures(), &[]);
    assert!(p.shortfalls.is_empty(), "{:?}", p.shortfalls);
    for b in &p.create {
        assert!(b.end <= at("2026-10-14 05:00"), "{}", local(b.end));
    }
}

#[test]
fn lead_fallback_never_displaces_a_lead_respecting_task() {
    // "a" (due Tue 17:00) only fits in Tue's last 12 h; "b" (due Tue 23:00)
    // fits Mon. Fallback for "a" happens after everyone's lead pass, so "b"
    // keeps its lead-respecting place.
    // Monday is free only 08:30–12:30 (four hours); everything else Mon is busy.
    let busy = vec![self::busy("2026-10-12 12:30", "2026-10-12 22:00")];
    let tasks = [
        task("a", 6 * 60, Some("2026-10-13 17:00")),
        task("b", 2 * 60, Some("2026-10-13 23:00")),
    ];
    let p = run_plan("2026-10-12 08:00", &tasks, &busy, &[]);
    assert!(p.shortfalls.is_empty(), "{:?}", p.shortfalls);
    for b in p.create.iter().filter(|b| b.task_id == "b") {
        assert!(b.end <= at("2026-10-13 11:00"), "b at {}", local(b.start));
    }
    check_invariants(at("2026-10-12 08:00"), &tasks, &busy, &p).unwrap();
}

#[test]
fn nothing_after_the_deadline() {
    // Only 30 min free before 12:00 (08:30–09:00 + lead ignored); the rest is short.
    let tasks = [task("late", 60, Some("2026-10-12 12:00"))];
    let busy = [busy("2026-10-12 09:10", "2026-10-12 23:00")];
    let p = run_plan("2026-10-12 08:00", &tasks, &busy, &[]);
    assert_eq!(total(&p, "late"), 30);
    assert_eq!(p.shortfalls[0].short_min, 30);
    assert!(p.create.iter().all(|b| b.end <= at("2026-10-12 12:00")));
}

#[test]
fn deadline_inside_planning_margin_says_so() {
    let tasks = [task("now", 30, Some("2026-10-12 08:20"))];
    let p = run_plan("2026-10-12 08:00", &tasks, &[], &[]);
    assert!(p.create.is_empty());
    let s = &p.shortfalls[0];
    assert!(
        s.message.contains("nothing can be planned before 08:30"),
        "{}",
        s.message
    );
    assert!(s.options[0].contains("block in yourself"), "{:?}", s.options);
}

#[test]
fn overloaded_week_prioritises_earliest_deadline() {
    let tasks = [
        task("later", 30 * 60, Some("2026-10-17 17:00")),
        task("sooner", 6 * 60, Some("2026-10-14 17:00")),
    ];
    let p = run_plan("2026-10-12 08:00", &tasks, &lectures(), &[]);
    assert_eq!(total(&p, "sooner"), 6 * 60, "{:?}", p.shortfalls);
    assert!(p.shortfalls.iter().any(|s| s.task_id == "later"));
    assert!(!p.shortfalls.iter().any(|s| s.task_id == "sooner"));
    check_invariants(at("2026-10-12 08:00"), &tasks, &lectures(), &p).unwrap();
}

#[test]
fn respects_lectures_buffers_and_meals() {
    let tasks = [task("t", 8 * 60, Some("2026-10-17 17:00"))];
    let b = lectures();
    let p = run_plan("2026-10-12 08:00", &tasks, &b, &[]);
    check_invariants(at("2026-10-12 08:00"), &tasks, &b, &p).unwrap();
    for blk in &p.create {
        for bz in &b {
            // 10-minute buffer around lectures.
            assert!(blk.end <= bz.start - Duration::minutes(10) || blk.start >= bz.end + Duration::minutes(10));
        }
    }
}

#[test]
fn location_change_gets_30_min_buffer() {
    let b = vec![
        Busy {
            start: at("2026-10-12 09:00"),
            end: at("2026-10-12 10:00"),
            location: Some("Thom Building".into()),
        },
        Busy {
            start: at("2026-10-12 12:00"),
            end: at("2026-10-12 12:30"),
            location: Some("Iffley Road".into()),
        },
    ];
    let mut t = task("t", 60, Some("2026-10-13 12:00"));
    t.splittable = false;
    let p = run_plan("2026-10-12 08:00", &[t], &b, &[]);
    let blk = &p.create[0];
    // 10:10–11:10 would end within 30 min of the 12:00 event at a new location; 10:10–11:10 ends 11:10 < 11:30 → ok.
    assert!(
        blk.end <= at("2026-10-12 11:30") || blk.start >= at("2026-10-12 12:40"),
        "{}",
        local(blk.start)
    );
}

#[test]
fn saturday_evening_no_schedule() {
    let tasks = [task("t", 20 * 60, Some("2026-10-20 12:00"))];
    let p = run_plan("2026-10-17 08:00", &tasks, &[], &[]);
    for b in &p.create {
        let l = b.start.with_timezone(&London);
        if l.weekday() == chrono::Weekday::Sat {
            assert!(
                b.end.with_timezone(&London).hour() < 18
                    || (b.end.with_timezone(&London).hour() == 18 && b.end.with_timezone(&London).minute() == 0)
            );
        }
    }
    check_invariants(at("2026-10-17 08:00"), &tasks, &[], &p).unwrap();
}

#[test]
fn dst_week_stays_in_local_day_window() {
    // Clocks go back Sun 25 Oct 2026 (still term).
    let tasks = [task("t", 12 * 60, Some("2026-10-28 17:00"))];
    let p = run_plan("2026-10-24 07:00", &tasks, &[], &[]);
    check_invariants(at("2026-10-24 07:00"), &tasks, &[], &p).unwrap();
    let on_sunday: Vec<String> = p
        .create
        .iter()
        .filter(|b| b.start.with_timezone(&London).day() == 25)
        .map(|b| local(b.start))
        .collect();
    assert!(!on_sunday.is_empty());
    assert!(on_sunday.iter().all(|s| s.as_str() >= "Sun 25 08:30"), "{on_sunday:?}");
}

#[test]
fn preferred_window_is_used() {
    let mut t = task("t", 60, Some("2026-10-16 17:00"));
    t.preferred_window = Some("evening".into());
    t.priority = 3;
    let p = run_plan("2026-10-12 08:00", &[t], &[], &[]);
    let h = p.create[0].start.with_timezone(&London).hour();
    assert!(h >= 17, "got {}", local(p.create[0].start));
}

#[test]
fn never_within_30_min() {
    let tasks = [task("t", 60, Some("2026-10-13 23:00"))];
    let p = run_plan("2026-10-12 10:05", &tasks, &[], &[]);
    assert!(p.create[0].start >= at("2026-10-12 10:35"));
}

fn as_existing(p: &Plan) -> Vec<Block> {
    p.create
        .iter()
        .enumerate()
        .map(|(i, b)| Block {
            uid: Some(format!("blk-{i}")),
            ..b.clone()
        })
        .collect()
}

#[test]
fn stability_unrelated_task_moves_nothing() {
    let t1 = task("b4", 240, Some("2026-10-16 17:00"));
    let p1 = run_plan("2026-10-12 08:00", std::slice::from_ref(&t1), &lectures(), &[]);
    let existing = as_existing(&p1);
    let t2 = task("essay", 180, Some("2026-10-23 17:00"));
    let p2 = run_plan("2026-10-12 08:00", &[t1, t2], &lectures(), &existing);
    assert_eq!(p2.moved, 0);
    assert!(p2.delete.is_empty());
    assert_eq!(p2.keep.len(), existing.len());
    assert_eq!(total(&p2, "essay"), 180);
}

#[test]
fn replan_with_no_changes_is_identity() {
    let tasks = [task("b4", 240, Some("2026-10-16 17:00")), task("e", 120, None)];
    let p1 = run_plan("2026-10-12 08:00", &tasks, &lectures(), &[]);
    let existing = as_existing(&p1);
    let p2 = run_plan("2026-10-12 08:00", &tasks, &lectures(), &existing);
    assert!(p2.create.is_empty() && p2.delete.is_empty(), "{p2:?}");
    assert_eq!(p2.moved, 0);
}

#[test]
fn collision_moves_only_the_affected_block() {
    let tasks = [task("b4", 360, Some("2026-10-16 17:00"))];
    let p1 = run_plan("2026-10-12 08:00", &tasks, &lectures(), &[]);
    let existing = as_existing(&p1);
    assert!(existing.len() >= 3);
    // A new fixed event lands on the second block.
    let victim = &existing[1];
    let mut b = lectures();
    b.push(Busy {
        start: victim.start,
        end: victim.end,
        location: None,
    });
    let p2 = run_plan("2026-10-12 08:00", &tasks, &b, &existing);
    assert_eq!(p2.moved, 1, "{p2:#?}");
    assert_eq!(p2.delete.len(), 1);
    assert_eq!(p2.delete[0].uid, victim.uid);
    assert_eq!(total(&p2, "b4"), 360);
}

#[test]
fn pinned_block_is_never_moved() {
    let tasks = [task("b4", 120, Some("2026-10-16 17:00"))];
    let pinned = Block {
        uid: Some("pin".into()),
        task_id: "b4".into(),
        start: at("2026-10-13 20:00"),
        end: at("2026-10-13 21:00"),
        pinned: true,
    };
    let p = run_plan("2026-10-12 08:00", &tasks, &lectures(), std::slice::from_ref(&pinned));
    assert!(p.keep.contains(&pinned));
    assert_eq!(total(&p, "b4"), 120);
}

#[test]
fn done_tasks_lose_their_blocks() {
    let mut t = task("b4", 120, Some("2026-10-16 17:00"));
    let p1 = run_plan("2026-10-12 08:00", std::slice::from_ref(&t), &[], &[]);
    let existing = as_existing(&p1);
    t.status = TaskStatus::Done;
    let p2 = run_plan("2026-10-12 08:00", &[t], &[], &existing);
    assert_eq!(p2.delete.len(), existing.len());
    assert!(p2.create.is_empty());
}

#[test]
fn deterministic() {
    let tasks = [
        task("a", 300, Some("2026-10-16 17:00")),
        task("b", 300, Some("2026-10-16 17:00")),
        task("c", 90, None),
    ];
    let p1 = run_plan("2026-10-12 08:00", &tasks, &lectures(), &[]);
    let p2 = run_plan("2026-10-12 08:00", &tasks, &lectures(), &[]);
    assert_eq!(p1, p2);
}

fn arb_busy() -> impl Strategy<Value = Vec<Busy>> {
    prop::collection::vec((0i64..5 * 24 * 4, 1i64..16), 0..20).prop_map(|v| {
        v.into_iter()
            .map(|(q, len)| {
                let s = at("2026-10-12 00:00") + Duration::minutes(q * 15);
                Busy {
                    start: s,
                    end: s + Duration::minutes(len * 15),
                    location: None,
                }
            })
            .collect()
    })
}

fn arb_tasks() -> impl Strategy<Value = Vec<Task>> {
    prop::collection::vec(
        (1u32..40, prop::option::of(1i64..7 * 24), 1u8..4, any::<bool>(), 1u32..5),
        1..6,
    )
    .prop_map(|v| {
        v.into_iter()
            .enumerate()
            .map(|(i, (q, due_h, prio, split, minc))| {
                let mut t = task(&format!("t{i}"), q * 15, None);
                t.due = due_h.map(|h| Due::At(at("2026-10-12 08:00") + Duration::hours(h)));
                t.priority = prio;
                t.splittable = split;
                t.min_chunk_min = minc * 15;
                t.max_chunk_min = minc * 15 + 60;
                t
            })
            .collect()
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]
    #[test]
    fn invariants_hold(busy in arb_busy(), tasks in arb_tasks()) {
        let p = run_plan("2026-10-12 08:00", &tasks, &busy, &[]);
        if let Err(e) = check_invariants(at("2026-10-12 08:00"), &tasks, &busy, &p) {
            prop_assert!(false, "{}", e);
        }
        // Every task is either fully allocated or reports the exact shortfall.
        for t in &tasks {
            let got = total(&p, &t.id);
            let short: i64 = p.shortfalls.iter().filter(|s| s.task_id == t.id).map(|s| i64::from(s.short_min)).sum();
            prop_assert_eq!(got + short, i64::from(t.remaining_min.div_ceil(15) * 15));
        }
    }

    #[test]
    fn replan_is_stable(busy in arb_busy(), tasks in arb_tasks()) {
        let p1 = run_plan("2026-10-12 08:00", &tasks, &busy, &[]);
        let existing = as_existing(&p1);
        let p2 = run_plan("2026-10-12 08:00", &tasks, &busy, &existing);
        prop_assert_eq!(p2.moved, 0);
        prop_assert!(p2.create.is_empty());
    }
}
