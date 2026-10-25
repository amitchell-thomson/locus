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
        rest_after_min: 0,
        counts_for_load: true,
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
        unavailable: &[],
        reflow_after: None,
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
        if b.start < now + Duration::minutes(15) {
            return Err(format!("block at {} within 15 min of now", local(b.start)));
        }
        if b.end <= b.start {
            return Err("empty block".into());
        }
        for bz in busy {
            let rest = bz.end + Duration::minutes(i64::from(bz.rest_after_min));
            if bz.rest_after_min > 0 && b.start < rest && bz.end < b.end {
                return Err(format!(
                    "block {} in recovery after {}",
                    local(b.start),
                    local(bz.start)
                ));
            }
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
        let floor = i64::from(
            c.squeeze_block_minutes
                .min(10 * (task.remaining_min.div_ceil(10) as u16).min(u16::MAX / 20)),
        );
        if task.splittable && b.minutes() < floor.min(i64::from(task.min_chunk_min)) {
            return Err(format!(
                "{} min block for {} at {}",
                b.minutes(),
                task.id,
                local(b.start)
            ));
        }
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
        if total(p, &t.id) > i64::from(t.remaining_min.div_ceil(10) * 10) {
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
    // Due 23:00 today, 2 h lead → aim to finish by 21:00. Placed the same
    // morning, preferring the 09:00 deep-work window; no squeeze.
    let tasks = [task("today", 60, Some("2026-10-12 23:00"))];
    let p = run_plan("2026-10-12 08:00", &tasks, &[], &[]);
    assert!(p.shortfalls.is_empty() && p.squeezed.is_empty(), "{p:?}");
    assert_eq!(local(p.create[0].start), "Mon 12 09:00");
    // With lectures it still fits well before 21:00.
    let p = run_plan("2026-10-12 08:00", &tasks, &lectures(), &[]);
    assert!(p.shortfalls.is_empty() && p.squeezed.is_empty(), "{p:?}");
    assert_eq!(total(&p, "today"), 60);
    check_invariants(at("2026-10-12 08:00"), &tasks, &lectures(), &p).unwrap();
}

#[test]
fn lead_is_a_preference_not_a_wall() {
    // The 2026-10-12 case: tasks made at 13:43 for 20:00 and 23:59 the same
    // day. A 12 h lead (08:00 / 11:59) had already passed, which used to mean
    // "short" despite a free evening.
    let tasks = [
        task("populate", 30, Some("2026-10-12 20:00")),
        task("furniture", 60, Some("2026-10-12 23:59")),
    ];
    let busy = [busy("2026-10-12 14:00", "2026-10-12 16:00")];
    let p = run_plan("2026-10-12 13:43", &tasks, &busy, &[]);
    assert!(p.shortfalls.is_empty(), "{:?}", p.shortfalls);
    assert_eq!(total(&p, "populate"), 30);
    assert_eq!(total(&p, "furniture"), 60);
    // Both fit outside their 2 h leads (by 18:00 / 21:45): no warning.
    assert!(p.squeezed.is_empty(), "{:?}", p.squeezed);
    for b in &p.create {
        assert!(b.start >= at("2026-10-12 16:10"), "{}", local(b.start));
    }
    check_invariants(at("2026-10-12 13:43"), &tasks, &busy, &p).unwrap();
}

#[test]
fn lead_still_preferred_when_there_is_room() {
    // Due Wed 17:00: nothing lands in the last 2 h (from 15:00).
    let tasks = [task("t", 4 * 60, Some("2026-10-14 17:00"))];
    let p = run_plan("2026-10-12 08:00", &tasks, &lectures(), &[]);
    assert!(p.shortfalls.is_empty() && p.squeezed.is_empty(), "{p:?}");
    for b in &p.create {
        assert!(b.end <= at("2026-10-14 15:00"), "{}", local(b.end));
    }
}

#[test]
fn squeeze_into_the_lead_is_warned() {
    // Due 20:00; busy until 17:30, then dinner 18:45–19:30. Before the lead
    // (18:00) there's only 17:40–18:00, too short for an unsplittable hour, so
    // it lands in the last 2 h and the plan says so.
    let mut t = task("essay", 60, Some("2026-10-12 20:00"));
    t.title = "Essay".into();
    t.splittable = false;
    let tasks = [t];
    let busy = [busy("2026-10-12 08:00", "2026-10-12 17:30")];
    let p = run_plan("2026-10-12 08:00", &tasks, &busy, &[]);
    assert!(p.shortfalls.is_empty(), "{:?}", p.shortfalls);
    assert_eq!(total(&p, "essay"), 60);
    assert_eq!(p.squeezed.len(), 1, "{p:?}");
    let s = &p.squeezed[0];
    assert_eq!(s.task_id, "essay");
    assert!(s.squeezed_min > 0);
    assert!(
        s.message.starts_with("Essay: ") && s.message.contains("squeezed into the last 2 h before Mon 12 Oct 20:00"),
        "{}",
        s.message
    );
    check_invariants(at("2026-10-12 08:00"), &tasks, &busy, &p).unwrap();
    // The warning persists on a replan that keeps the block.
    let p2 = run_plan("2026-10-12 08:00", &tasks, &busy, &as_existing(&p));
    assert_eq!(p2.squeezed, p.squeezed);
}

#[test]
fn lead_fallback_never_displaces_a_lead_respecting_task() {
    // "a" (due Mon 18:00, 3 h) runs into its lead; "b" (due Mon 23:00, 1 h)
    // has the evening. The fallback for "a" doesn't push "b" into its own
    // lead, and only "a" is warned about.
    let busy = vec![self::busy("2026-10-12 08:00", "2026-10-12 13:30")];
    let tasks = [
        task("a", 3 * 60, Some("2026-10-12 18:00")),
        task("b", 60, Some("2026-10-12 23:00")),
    ];
    let p = run_plan("2026-10-12 08:00", &tasks, &busy, &[]);
    assert!(p.shortfalls.is_empty(), "{:?}", p.shortfalls);
    for b in p.create.iter().filter(|b| b.task_id == "b") {
        assert!(b.end <= at("2026-10-12 21:00"), "b at {}", local(b.start));
    }
    let warned: Vec<&str> = p.squeezed.iter().map(|s| s.task_id.as_str()).collect();
    assert_eq!(warned, ["a"]);
    check_invariants(at("2026-10-12 08:00"), &tasks, &busy, &p).unwrap();
}

#[test]
fn urgent_task_is_not_left_short_to_spare_another_a_squeeze() {
    // "a": 2 h unsplittable, due 18:00, free from 15:10 → only 15:15–17:15
    // (inside its lead) works. "b": 1 h, due 22:00. Lead-first would give b
    // 15:15 (earliest) and leave a short; the deadline-order fallback wins.
    let mut a = task("a", 120, Some("2026-10-12 18:00"));
    a.splittable = false;
    let tasks = [a, task("b", 60, Some("2026-10-12 22:00"))];
    let busy = vec![self::busy("2026-10-12 08:00", "2026-10-12 15:00")];
    let p = run_plan("2026-10-12 08:00", &tasks, &busy, &[]);
    assert!(p.shortfalls.is_empty(), "{:?}", p.shortfalls);
    assert_eq!(total(&p, "a"), 120);
    assert_eq!(total(&p, "b"), 60);
    assert!(p.squeezed.iter().any(|s| s.task_id == "a"));
    check_invariants(at("2026-10-12 08:00"), &tasks, &busy, &p).unwrap();
}

#[test]
fn replan_unsqueezes_when_room_opens_up() {
    // Squeezed into the lead because 08:00–17:30 was busy; once that event is
    // gone, a replan moves the block out of the lead.
    let mut t = task("essay", 60, Some("2026-10-12 20:00"));
    t.splittable = false;
    let tasks = [t];
    let busy = [busy("2026-10-12 08:00", "2026-10-12 17:30")];
    let p = run_plan("2026-10-12 08:00", &tasks, &busy, &[]);
    assert_eq!(p.squeezed.len(), 1);
    let p2 = run_plan("2026-10-12 08:00", &tasks, &[], &as_existing(&p));
    assert!(p2.squeezed.is_empty(), "{p2:?}");
    assert_eq!(p2.moved, 1);
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
    let tasks = [task("now", 30, Some("2026-10-12 08:10"))];
    let p = run_plan("2026-10-12 08:00", &tasks, &[], &[]);
    assert!(p.create.is_empty());
    let s = &p.shortfalls[0];
    assert!(
        s.message.contains("nothing can be planned before 08:20"),
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
            rest_after_min: 0,
            counts_for_load: true,
        },
        Busy {
            start: at("2026-10-12 12:00"),
            end: at("2026-10-12 12:30"),
            location: Some("Iffley Road".into()),
            rest_after_min: 0,
            counts_for_load: true,
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
fn starts_15_min_from_now_at_the_earliest() {
    let tasks = [task("t", 60, Some("2026-10-12 23:00"))];
    let p = run_plan("2026-10-12 10:05", &tasks, &[], &[]);
    // 10:05 + 15 = 10:20, already on a 10-min slot; deep work (to 12:30) suits it.
    assert_eq!(local(p.create[0].start), "Mon 12 10:20");
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
        rest_after_min: 0,
        counts_for_load: true,
    });
    let p2 = run_plan("2026-10-12 08:00", &tasks, &b, &existing);
    assert_eq!(p2.moved, 1, "{p2:#?}");
    assert_eq!(p2.delete.len(), 1);
    assert_eq!(p2.delete[0].uid, victim.uid);
    assert_eq!(total(&p2, "b4"), 360);
}

#[test]
fn freed_time_reflows_work_into_the_gap() {
    // Tue is taken all day, so the sheet goes on the other days. Then Tue's
    // event is deleted: a normal replan keeps everything, a reflow moves
    // work into Tue (Alec, 2026-10-14) — pinned blocks stay put.
    let tasks = [task("b4", 360, Some("2026-10-16 17:00"))];
    let mut b = lectures();
    b.push(busy("2026-10-13 08:00", "2026-10-13 22:00"));
    let p1 = run_plan("2026-10-12 08:00", &tasks, &b, &[]);
    let tue = |p: &Plan| {
        p.keep
            .iter()
            .chain(&p.create)
            .filter(|x| {
                x.start.with_timezone(&London).date_naive()
                    == at("2026-10-13 12:00").with_timezone(&London).date_naive()
            })
            .count()
    };
    assert_eq!(tue(&p1), 0);
    let mut existing = as_existing(&p1);
    existing[0].pinned = true;
    let stable = run_plan("2026-10-12 08:00", &tasks, &lectures(), &existing);
    assert!(stable.create.is_empty() && stable.delete.is_empty(), "{stable:?}");
    let c = constraints();
    let p = plan(&PlanInput {
        now: at("2026-10-12 08:00"),
        tz: London,
        constraints: &c,
        tasks: &tasks,
        busy: &lectures(),
        existing: &existing,
        unavailable: &[],
        reflow_after: Some(at("2026-10-12 10:00")),
    });
    assert!(tue(&p) >= 1, "work moves into the freed Tuesday: {p:#?}");
    assert!(
        p.keep.iter().any(|k| k.uid == existing[0].uid && k.pinned),
        "pinned stays"
    );
    assert_eq!(total(&p, "b4"), 360);
    assert!(p.shortfalls.is_empty());
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
        why: None,
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
    prop::collection::vec((0i64..5 * 24 * 4, 1i64..16, prop::bool::weighted(0.2)), 0..20).prop_map(|v| {
        v.into_iter()
            .map(|(q, len, sport)| {
                let s = at("2026-10-12 00:00") + Duration::minutes(q * 15);
                Busy {
                    start: s,
                    end: s + Duration::minutes(len * 15),
                    location: None,
                    rest_after_min: if sport { 60 } else { 0 },
                    counts_for_load: true,
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
            prop_assert_eq!(got + short, i64::from(t.remaining_min.div_ceil(10) * 10));
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

// ------------------------------------------------------------ 2026-10-12 scheme: types, tiers, spreading, comfort

/// A task of a type from the example constraints (`sheet`, `admin`, …).
fn typed(id: &str, kind: &str, mins: u32, due: Option<&str>) -> Task {
    let mut t = task(id, mins, due);
    t.apply_type(constraints().task_type(kind).unwrap());
    t
}

fn days_of(p: &Plan, task: &str) -> Vec<String> {
    let mut d: Vec<String> = p
        .create
        .iter()
        .chain(&p.keep)
        .filter(|b| b.task_id == task)
        .map(|b| b.start.with_timezone(&London).format("%a").to_string())
        .collect();
    d.dedup();
    d
}

#[test]
fn sheet_is_spread_over_days_in_two_to_three_hour_blocks() {
    // 5 h sheet due Fri 17:00, lectures every weekday morning and afternoon.
    let tasks = [typed("b4", "sheet", 300, Some("2026-10-16 17:00"))];
    let p = run_plan("2026-10-12 08:00", &tasks, &lectures(), &[]);
    assert!(p.shortfalls.is_empty() && p.squeezed.is_empty(), "{p:#?}");
    assert_eq!(total(&p, "b4"), 300);
    for b in &p.create {
        assert!(
            (120..=180).contains(&b.minutes()),
            "{} min at {}",
            b.minutes(),
            local(b.start)
        );
        assert!(
            b.why.as_deref().is_some_and(|w| w.starts_with("full-length block")),
            "{:?}",
            b.why
        );
    }
    // 5 h = two 2.5 h blocks, on different days.
    assert_eq!(
        p.create.len(),
        2,
        "{:?}",
        p.create.iter().map(|b| local(b.start)).collect::<Vec<_>>()
    );
    let d = days_of(&p, "b4");
    assert_eq!(d.len(), 2, "{d:?}");
    check_invariants(at("2026-10-12 08:00"), &tasks, &lectures(), &p).unwrap();
}

#[test]
fn busy_week_crams_into_the_one_free_day() {
    // Everything Mon–Thu is taken except Wednesday: the sheet all goes on Wednesday.
    let mut b = Vec::new();
    for d in [12, 13, 15] {
        b.push(busy(&format!("2026-10-{d} 08:00"), &format!("2026-10-{d} 22:00")));
    }
    let tasks = [typed("b4", "sheet", 300, Some("2026-10-16 08:00"))];
    let p = run_plan("2026-10-12 07:00", &tasks, &b, &[]);
    assert!(p.shortfalls.is_empty(), "{:?}", p.shortfalls);
    assert_eq!(days_of(&p, "b4"), ["Wed"]);
    assert!(
        p.create
            .iter()
            .all(|b| b.why.as_deref().is_some_and(|w| w.contains("only day with room")))
    );
    check_invariants(at("2026-10-12 07:00"), &tasks, &b, &p).unwrap();
}

#[test]
fn spreading_weighs_days_by_free_time() {
    // 6 h due Thu night; Monday has room for ~2 h, Tuesday and Wednesday are free.
    let b = vec![
        busy("2026-10-12 08:00", "2026-10-12 17:00"),
        busy("2026-10-15 08:00", "2026-10-15 22:00"),
    ];
    let tasks = [typed("cw", "coursework", 360, Some("2026-10-15 23:00"))];
    let p = run_plan("2026-10-12 07:00", &tasks, &b, &[]);
    assert!(p.shortfalls.is_empty() && p.squeezed.is_empty(), "{p:#?}");
    let tue_wed: i64 = p
        .create
        .iter()
        .filter(|x| ["Tue", "Wed"].contains(&x.start.with_timezone(&London).format("%a").to_string().as_str()))
        .map(Block::minutes)
        .sum();
    assert!(tue_wed >= 240, "most of it on the free days: {tue_wed} min");
}

#[test]
fn admin_is_one_block_and_fits_an_hour_gap() {
    // Lectures 09–10 and 11–12: the gap leaves 40 min after travel. A 30-min
    // errand fits it (admin's normal size: no warning); a 45-min one doesn't,
    // and goes elsewhere in one piece.
    let b = vec![
        busy("2026-10-12 09:00", "2026-10-12 10:00"),
        busy("2026-10-12 11:00", "2026-10-12 12:00"),
    ];
    let tasks = [typed("call", "admin", 30, Some("2026-10-12 18:00"))];
    let p = run_plan("2026-10-12 08:30", &tasks, &b, &[]);
    assert!(p.shortfalls.is_empty() && p.squeezed.is_empty(), "{p:#?}");
    assert_eq!(local(p.create[0].start), "Mon 12 10:10");
    let tasks = [typed("post", "admin", 45, Some("2026-10-12 18:00"))];
    let p = run_plan("2026-10-12 08:30", &tasks, &b, &[]);
    assert!(p.shortfalls.is_empty() && p.squeezed.is_empty(), "{p:#?}");
    assert_eq!(p.create.len(), 1);
    assert_eq!(total(&p, "post"), 50, "45 min rounds up to 10-min slots");
    assert!(
        p.create[0].start >= at("2026-10-12 12:10"),
        "{}",
        local(p.create[0].start)
    );
}

#[test]
fn shorter_blocks_only_when_full_ones_dont_fit() {
    // Before the deadline's 2 h lead (19:00) only a 90-min window
    // (14:40–16:10) and an 85-min one after a proper break (17:20–18:45)
    // exist: blocks of 60–90 min, no warning.
    let b = vec![
        busy("2026-10-12 07:00", "2026-10-12 12:00"),
        busy("2026-10-12 14:00", "2026-10-12 14:30"),
        busy("2026-10-12 16:20", "2026-10-12 16:50"),
    ];
    let tasks = [typed("cw", "coursework", 150, Some("2026-10-12 21:00"))];
    let p = run_plan("2026-10-12 06:00", &tasks, &b, &[]);
    assert!(p.shortfalls.is_empty() && p.squeezed.is_empty(), "{p:#?}");
    assert_eq!(total(&p, "cw"), 150);
    for x in &p.create {
        assert!((60..=90).contains(&x.minutes()), "{}", x.minutes());
        assert!(
            x.why.as_deref().is_some_and(|w| w.starts_with("shorter block")),
            "{:?}",
            x.why
        );
    }
}

#[test]
fn hour_gaps_between_lectures_are_a_warned_last_resort() {
    // Lectures 09–10, 11–12, 13–14 (+ meals) and nothing else free before the
    // deadline: 40-min blocks in the gaps, each warned about.
    let mut b = Vec::new();
    for (s, e) in [
        ("07:00", "09:00"),
        ("10:00", "11:00"),
        ("12:00", "12:45"),
        ("13:30", "14:30"),
        ("15:30", "23:00"),
    ] {
        b.push(busy(&format!("2026-10-12 {s}"), &format!("2026-10-12 {e}")));
    }
    let tasks = [typed("b4", "sheet", 80, Some("2026-10-12 23:00"))];
    let p = run_plan("2026-10-12 06:00", &tasks, &b, &[]);
    assert!(p.shortfalls.is_empty(), "{:?}", p.shortfalls);
    assert_eq!(total(&p, "b4"), 80);
    for x in &p.create {
        assert_eq!(x.minutes(), 40, "{}", local(x.start));
    }
    assert_eq!(p.squeezed.len(), 1);
    assert!(
        p.squeezed[0].message.contains("into a short gap"),
        "{}",
        p.squeezed[0].message
    );
    check_invariants(at("2026-10-12 06:00"), &tasks, &b, &p).unwrap();
}

#[test]
fn no_work_straight_after_sport() {
    // Sport 17:00–18:00 with an hour's recovery: nothing 18:00–19:00, even
    // as a squeeze; the work lands after dinner.
    let mut b = vec![busy("2026-10-12 07:00", "2026-10-12 17:00")];
    b.push(Busy {
        rest_after_min: 60,
        ..busy("2026-10-12 17:00", "2026-10-12 18:00")
    });
    let tasks = [typed("b4", "sheet", 60, Some("2026-10-12 23:00"))];
    let p = run_plan("2026-10-12 06:00", &tasks, &b, &[]);
    assert!(p.shortfalls.is_empty(), "{:?}", p.shortfalls);
    for x in &p.create {
        assert!(x.start >= at("2026-10-12 19:00"), "{}", local(x.start));
    }
}

#[test]
fn three_hours_back_to_back_needs_a_break() {
    // Lectures 09:00–12:00: a full block can't follow straight on (10-min
    // buffer isn't a break); it starts after a 30-min gap at the earliest.
    let b = vec![busy("2026-10-12 09:00", "2026-10-12 12:00")];
    let mut t = typed("b4", "sheet", 120, Some("2026-10-12 18:00"));
    t.priority = 3; // no deep-work pull
    let p = run_plan("2026-10-12 08:00", &[t], &b, &[]);
    assert!(p.shortfalls.is_empty() && p.squeezed.is_empty(), "{p:#?}");
    assert!(
        p.create[0].start >= at("2026-10-12 12:30"),
        "{}",
        local(p.create[0].start)
    );
}

#[test]
fn long_day_caps_events_plus_work() {
    // 9 h of events on Monday: at most 1 h of work there (10 h day load);
    // the rest of a 4 h sheet goes to Tuesday.
    let b = vec![
        busy("2026-10-12 08:30", "2026-10-12 12:30"),
        busy("2026-10-12 13:30", "2026-10-12 18:30"),
    ];
    let tasks = [typed("b4", "sheet", 240, Some("2026-10-13 23:00"))];
    let p = run_plan("2026-10-12 07:00", &tasks, &b, &[]);
    assert!(p.shortfalls.is_empty() && p.squeezed.is_empty(), "{p:#?}");
    let monday: i64 = p
        .create
        .iter()
        .filter(|x| x.start.with_timezone(&London).day() == 12)
        .map(Block::minutes)
        .sum();
    assert!(monday <= 60, "{monday} min on the 9 h day");
}

#[test]
fn new_event_moves_only_the_displaced_work() {
    // Work planned on Tuesday afternoon; a social lands on it. It moves
    // (earlier or later, wherever is best) and nothing else does.
    let b0 = vec![
        busy("2026-10-12 07:00", "2026-10-12 22:00"),
        busy("2026-10-13 07:00", "2026-10-13 12:00"),
        busy("2026-10-14 07:00", "2026-10-14 12:00"),
    ];
    let mut post = typed("post", "admin", 30, Some("2026-10-15 12:00"));
    post.earliest_start = Some(at("2026-10-14 08:00")); // a different day from the sheet
    let tasks = [typed("b4", "sheet", 150, Some("2026-10-15 12:00")), post];
    let p1 = run_plan("2026-10-12 06:00", &tasks, &b0, &[]);
    assert!(p1.shortfalls.is_empty() && p1.squeezed.is_empty(), "{p1:#?}");
    let existing = as_existing(&p1);
    let victim = existing.iter().find(|b| b.task_id == "b4").unwrap().clone();
    let mut b1 = b0.clone();
    b1.push(Busy {
        start: victim.start,
        end: victim.end,
        location: None,
        rest_after_min: 0,
        counts_for_load: true,
    });
    let p2 = run_plan("2026-10-12 06:00", &tasks, &b1, &existing);
    assert_eq!(p2.moved, 1, "{p2:#?}");
    assert_eq!(p2.delete[0].uid, victim.uid);
    assert_eq!(total(&p2, "b4"), 150);
    assert!(p2.shortfalls.is_empty() && p2.squeezed.is_empty(), "{p2:#?}");
    check_invariants(at("2026-10-12 06:00"), &tasks, &b1, &p2).unwrap();
}

#[test]
fn social_and_sport_dont_count_toward_the_day_load() {
    // 9 h of social on Monday wouldn't leave room under the 10 h load if it
    // counted; it doesn't, so a 2 h block still fits around it that day.
    let mut b = vec![
        Busy {
            counts_for_load: false,
            ..busy("2026-10-12 08:30", "2026-10-12 12:30")
        },
        Busy {
            counts_for_load: false,
            ..busy("2026-10-12 13:30", "2026-10-12 18:30")
        },
    ];
    let tasks = [typed("b4", "sheet", 120, Some("2026-10-12 23:59"))];
    let p = run_plan("2026-10-12 07:00", &tasks, &b, &[]);
    assert!(p.shortfalls.is_empty() && p.squeezed.is_empty(), "{p:#?}");
    // Counting (a lecture day), only 1 h fits under the 10 h load.
    for x in &mut b {
        x.counts_for_load = true;
    }
    let p = run_plan("2026-10-12 07:00", &tasks, &b, &[]);
    assert!(!p.shortfalls.is_empty() || !p.squeezed.is_empty(), "{p:#?}");
}

#[test]
fn weekends_start_at_ten() {
    // Friday night: the sheet's blocks start ≥ 10:00.
    let c = constraints();
    let tasks = [typed("b4", "sheet", 300, Some("2026-10-19 12:00"))];
    let input = PlanInput {
        now: at("2026-10-16 22:00"),
        tz: London,
        constraints: &c,
        tasks: &tasks,
        busy: &[],
        existing: &[],
        unavailable: &[],
        reflow_after: None,
    };
    let p = plan(&input);
    assert!(p.shortfalls.is_empty(), "{:?}", p.shortfalls);
    let weekend = |t: DateTime<Utc>| {
        matches!(
            t.with_timezone(&London).weekday(),
            chrono::Weekday::Sat | chrono::Weekday::Sun
        )
    };
    let blocks: Vec<&Block> = p.create.iter().filter(|b| weekend(b.start)).collect();
    assert!(!blocks.is_empty());
    for b in blocks {
        assert!(b.start.with_timezone(&London).hour() >= 10, "{}", local(b.start));
    }
    assert_eq!(local(p.create[0].start), "Sat 17 10:00");
}

#[test]
fn a_block_longer_than_whats_left_is_replaced() {
    // A 60-min block was planned; a check-in left only 30 min: the replan
    // swaps it for a 30-min one instead of keeping the hour.
    let t = typed("furniture", "admin", 60, None);
    let p1 = run_plan("2026-10-12 08:00", std::slice::from_ref(&t), &[], &[]);
    assert_eq!(total(&p1, "furniture"), 60);
    let mut t2 = t.clone();
    t2.remaining_min = 30;
    let p2 = run_plan("2026-10-12 08:00", &[t2.clone()], &[], &as_existing(&p1));
    assert_eq!(total(&p2, "furniture"), 30, "{p2:#?}");
    check_invariants(at("2026-10-12 08:00"), &[t2], &[], &p2).unwrap();
}

#[test]
fn a_one_block_task_goes_on_the_earliest_day_with_room() {
    // A 30-min errand with no deadline: today if there's room, not the
    // emptiest day in the next fortnight.
    let tasks = [typed("furniture", "admin", 30, None)];
    let p = run_plan("2026-10-13 08:00", &tasks, &lectures(), &[]);
    assert!(p.shortfalls.is_empty(), "{:?}", p.shortfalls);
    assert!(
        local(p.create[0].start).starts_with("Tue 13"),
        "{}",
        local(p.create[0].start)
    );
}

#[test]
fn a_long_free_evening_beats_a_short_gap_for_the_second_block() {
    // Alec's real week (2026-10-13): a 6 h Tute Sheet due Fri 21:00, planned
    // at Tue 01:40. The spreading order is Wed, Thu, Tue; Thursday's longest
    // stretch is 2 h 10 (futsal recovery, B14, lunch). The old code went Wed
    // 3 h → Thu 2 h 10 → a 50-min leftover in Tuesday's free evening, called
    // "squeezed into a short gap".
    let sport = |a: &str, b: &str| Busy {
        rest_after_min: 60,
        counts_for_load: false,
        ..busy(a, b)
    };
    let b = vec![
        busy("2026-10-13 09:30", "2026-10-13 12:30"), // Quantum Research
        busy("2026-10-13 13:00", "2026-10-13 16:00"),
        sport("2026-10-13 15:00", "2026-10-13 17:00"), // football
        sport("2026-10-13 16:00", "2026-10-13 17:00"), // futsal
        busy("2026-10-13 18:00", "2026-10-13 18:30"),  // (a pinned block)
        busy("2026-10-14 09:30", "2026-10-14 12:30"),
        busy("2026-10-14 14:00", "2026-10-14 15:00"), // B3
        sport("2026-10-15 07:00", "2026-10-15 09:00"),
        busy("2026-10-15 09:00", "2026-10-15 10:00"), // B14
        busy("2026-10-15 13:30", "2026-10-15 16:30"),
        busy("2026-10-15 17:30", "2026-10-15 21:00"),
        busy("2026-10-16 09:00", "2026-10-16 10:00"),
        busy("2026-10-16 11:00", "2026-10-16 13:00"),
        busy("2026-10-16 13:30", "2026-10-16 16:30"),
        busy("2026-10-16 17:30", "2026-10-16 23:59"),
    ];
    let tasks = [typed("tute", "coursework", 360, Some("2026-10-16 21:00"))];
    let p = run_plan("2026-10-13 01:40", &tasks, &b, &[]);
    let blocks: Vec<(String, i64)> = p.create.iter().map(|x| (local(x.start), x.minutes())).collect();
    assert!(
        p.create.iter().all(|x| x.minutes() >= 60),
        "no leftovers under an hour: {blocks:?}"
    );
    assert!(
        p.create
            .iter()
            .any(|x| local(x.start).starts_with("Tue 13 19:30") && x.minutes() >= 120),
        "Tuesday evening gets a real block: {blocks:?}"
    );
    assert!(p.squeezed.is_empty(), "{:?}", p.squeezed);
    assert_eq!(total(&p, "tute"), 360);
}

#[test]
fn a_chore_takes_the_smallest_gap_it_fits_not_the_long_ones() {
    // Spec K9: chores are life admin for a gap; the long free stretches stay for work. The
    // morning is long and free, the afternoon has a short gap after lunch: the chore takes that.
    let c = constraints();
    let mut chore = task("laundry", 20, Some("2026-10-12 22:00"));
    chore.apply_type(c.task_type("chore").unwrap());
    let day = vec![
        busy("2026-10-12 10:30", "2026-10-12 12:45"),
        busy("2026-10-12 14:20", "2026-10-12 18:45"),
    ];
    let p = run_plan("2026-10-12 08:00", &[chore], &day, &[]);
    let b = p.create.iter().find(|b| b.task_id == "laundry").unwrap();
    assert_eq!(b.minutes(), 20);
    // The gap between the busy times either side, lunch (a meal in the profile) counted.
    let mut around = day.clone();
    around.push(busy("2026-10-12 12:45", "2026-10-12 13:30"));
    let before = around
        .iter()
        .filter(|x| x.end <= b.start)
        .map(|x| x.end)
        .max()
        .unwrap_or(b.start);
    let after = around
        .iter()
        .filter(|x| x.start >= b.end)
        .map(|x| x.start)
        .min()
        .unwrap_or(b.end);
    assert!(
        (after - before).num_minutes() < 90,
        "{} in a {} min gap",
        local(b.start),
        (after - before).num_minutes()
    );
}
