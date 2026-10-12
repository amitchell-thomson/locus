//! Deterministic task scheduler (SPEC §7).
//!
//! Model: time from `now + min_lead` (rounded up to a slot) to the horizon is
//! cut into `slot_minutes` slots. A slot is *usable* if it lies inside the
//! day's profile window, outside sleep, meals and no-schedule periods, and
//! clear of busy events plus buffers. Blocks also get a `buffer_minutes` gap
//! after them, chunks of the same task are ≥ 60 min apart, and each day's
//! task-block total stays ≤ `max_focus_hours_per_day`.
//!
//! `deadline_lead_hours` (default 2) is a preference, not a wall: every task
//! is first placed to finish that long before its deadline; only what doesn't
//! fit there falls back to the time up to the deadline itself (after all tasks
//! have had their lead-respecting turn), and the plan carries a [`Squeeze`]
//! warning for it. Nothing is placed after a deadline.
//!
//! Algorithm:
//! 1. **Stable pass** — keep every existing future block that is still
//!    feasible and still needed (pinned blocks always stay), then place what's
//!    missing, tasks in earliest-deadline-first order (slack, then priority,
//!    as tiebreaks).
//! 2. If anything is short, alternatives are tried — deadline-order fallback
//!    (see `deadline_lead_hours` below) and a **fresh pass** from scratch
//!    (keeping only pinned blocks) — and used only if they strictly reduce the
//!    total shortfall (then squeezed minutes). So blocks move only when that's
//!    required.
//! 3. Each chunk goes on the earliest day with room; within that day the
//!    start with the best preferred-window fit wins (earliest on ties).
//!
//! Infeasibility is reported per task, never silently overbooked.

use crate::constraints::{Constraints, Hm, Profile};
use crate::task::{Due, Task, TaskStatus};
use chrono::{DateTime, Datelike, Duration, NaiveDate, Timelike, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Minimum gap between two chunks of the same task.
pub const SAME_TASK_GAP_MIN: i64 = 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Busy {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    /// UID of an existing block (None for a new placement).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    pub task_id: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    #[serde(default)]
    pub pinned: bool,
}

impl Block {
    pub fn minutes(&self) -> i64 {
        (self.end - self.start).num_minutes()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shortfall {
    pub task_id: String,
    pub title: String,
    pub short_min: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<Due>,
    pub message: String,
    pub options: Vec<String>,
}

/// A task with work planned inside its deadline lead (the last
/// `deadline_lead_hours` before it's due): it fits, but only just.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Squeeze {
    pub task_id: String,
    pub title: String,
    pub due: Due,
    /// Minutes of work planned inside the lead.
    pub squeezed_min: u32,
    pub lead_hours: u16,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Plan {
    /// Existing blocks that stay where they are.
    pub keep: Vec<Block>,
    /// New blocks to create.
    pub create: Vec<Block>,
    /// Existing blocks to delete.
    pub delete: Vec<Block>,
    pub shortfalls: Vec<Shortfall>,
    /// Tasks whose (non-pinned) blocks run into their deadline lead.
    #[serde(default)]
    pub squeezed: Vec<Squeeze>,
    /// Existing blocks whose time changed (deleted and not replaced 1:1).
    pub moved: usize,
}

pub struct PlanInput<'a> {
    pub now: DateTime<Utc>,
    pub tz: Tz,
    pub constraints: &'a Constraints,
    pub tasks: &'a [Task],
    /// Fixed (blocking) events.
    pub busy: &'a [Busy],
    /// Existing task blocks (future and past).
    pub existing: &'a [Block],
}

struct Grid<'a> {
    t0: DateTime<Utc>,
    slot: i64,
    n: usize,
    usable: Vec<bool>,
    /// Local date of each slot.
    date: Vec<NaiveDate>,
    tz: Tz,
    c: &'a Constraints,
}

fn in_window(m: u16, start: Hm, end: Hm) -> bool {
    if start <= end {
        m >= start.minutes() && m < end.minutes()
    } else {
        m >= start.minutes() || m < end.minutes()
    }
}

impl<'a> Grid<'a> {
    fn build(input: &PlanInput<'a>, horizon_end: DateTime<Utc>) -> Grid<'a> {
        let c = input.constraints;
        let slot = i64::from(c.slot_minutes.max(1));
        let earliest = input.now + Duration::minutes(i64::from(c.min_lead_minutes));
        // Round up to a slot boundary (in UTC minutes; London offsets are whole hours).
        let mins = earliest.timestamp() / 60;
        let rounded = (mins + slot - 1) / slot * slot;
        let t0 = DateTime::from_timestamp(rounded * 60, 0).unwrap_or(earliest);
        let n = ((horizon_end - t0).num_minutes() / slot).max(0) as usize;
        let mut usable = vec![false; n];
        let mut date = Vec::with_capacity(n);
        for (i, u) in usable.iter_mut().enumerate() {
            let s = t0 + Duration::minutes(i as i64 * slot);
            let local = s.with_timezone(&input.tz);
            let d = local.date_naive();
            date.push(d);
            let m = (local.hour() * 60 + local.minute()) as u16;
            let Some(p) = c.profile_for(d) else { continue };
            *u = slot_allowed(p, m, slot as u16, local.weekday());
        }
        let mut g = Grid {
            t0,
            slot,
            n,
            usable,
            date,
            tz: input.tz,
            c,
        };
        // Busy events + buffers. Location-change buffer applies before an event
        // whose location differs from the previous event's that day.
        let mut busy: Vec<&Busy> = input.busy.iter().collect();
        busy.sort_by_key(|b| b.start);
        let mut prev_loc: Option<(&str, NaiveDate)> = None;
        for b in busy {
            let d = b.start.with_timezone(&input.tz).date_naive();
            let p = c.profile_for(d);
            let base = i64::from(p.map_or(10, |p| p.buffer_minutes));
            let loc_buf = i64::from(p.map_or(30, |p| p.buffer_location_change_minutes));
            let before = match (prev_loc, b.location.as_deref()) {
                (Some((pl, pd)), Some(l)) if pd == d && !pl.eq_ignore_ascii_case(l) => loc_buf,
                _ => base,
            };
            g.block_out(b.start - Duration::minutes(before), b.end + Duration::minutes(base));
            if let Some(l) = b.location.as_deref() {
                prev_loc = Some((l, d));
            }
        }
        g
    }

    fn idx(&self, t: DateTime<Utc>) -> i64 {
        (t - self.t0).num_minutes().div_euclid(self.slot)
    }

    fn idx_ceil(&self, t: DateTime<Utc>) -> i64 {
        let m = (t - self.t0).num_minutes();
        (m + self.slot - 1).div_euclid(self.slot)
    }

    fn at(&self, i: usize) -> DateTime<Utc> {
        self.t0 + Duration::minutes(i as i64 * self.slot)
    }

    /// Mark `[a, b)` unusable (partial slots count).
    fn block_out(&mut self, a: DateTime<Utc>, b: DateTime<Utc>) {
        let i0 = self.idx(a).max(0);
        let i1 = self.idx_ceil(b).min(self.n as i64);
        for i in i0..i1 {
            self.usable[i as usize] = false;
        }
    }
}

fn slot_allowed(p: &Profile, m: u16, slot: u16, wd: chrono::Weekday) -> bool {
    let end = m + slot; // slot must fit entirely
    let inside = |s: Hm, e: Hm| m >= s.minutes() && end <= e.minutes();
    if !inside(p.day_window.start, p.day_window.end) {
        return false;
    }
    let hits = |s: Hm, e: Hm| {
        // overlap of [m, end) with window [s, e) (wrapping allowed)
        (0..slot).any(|k| in_window(m + k, s, e))
    };
    if hits(p.sleep.start, p.sleep.end) {
        return false;
    }
    if p.meals.iter().any(|w| hits(w.start, w.end)) {
        return false;
    }
    if p.no_schedule
        .iter()
        .any(|ns| (ns.days.is_empty() || ns.days.contains(&wd)) && hits(ns.start, ns.end))
    {
        return false;
    }
    true
}

struct Need<'t> {
    task: &'t Task,
    need_slots: usize,
    /// Exclusive latest slot index (deadline minus lead), clipped to grid.
    deadline_idx: usize,
    /// Exclusive latest slot index (the deadline itself), clipped to grid.
    hard_idx: usize,
    earliest_idx: usize,
}

fn ordered_needs<'t>(input: &PlanInput<'_>, tasks: &'t [Task], g: &Grid<'_>, horizon_idx: usize) -> Vec<Need<'t>> {
    let mut v: Vec<Need<'t>> = tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Open && t.remaining_min > 0)
        .map(|t| {
            let due = t.due.map(|d| d.instant(input.tz));
            let lead = due.map(|due| {
                let p = g.c.profile_for(due.with_timezone(&g.tz).date_naive());
                due - Duration::hours(i64::from(p.map_or(12, |p| p.deadline_lead_hours)))
            });
            let clip = |x: DateTime<Utc>| g.idx(x).clamp(0, horizon_idx as i64) as usize;
            let deadline_idx = lead.map_or(horizon_idx, clip);
            let hard_idx = due.map_or(horizon_idx, clip);
            let earliest_idx = t
                .earliest_start
                .map_or(0, |e| g.idx_ceil(e).clamp(0, g.n as i64) as usize);
            let slot = g.slot as u32;
            Need {
                task: t,
                need_slots: t.remaining_min.div_ceil(slot) as usize,
                deadline_idx,
                hard_idx,
                earliest_idx,
            }
        })
        .collect();
    // EDF, then least slack, then priority, then id (determinism).
    v.sort_by(|a, b| {
        let sa = a.deadline_idx as i64 - a.need_slots as i64;
        let sb = b.deadline_idx as i64 - b.need_slots as i64;
        let da = a.task.due.map(|d| d.instant(input.tz));
        let db = b.task.due.map(|d| d.instant(input.tz));
        let key = |d: Option<DateTime<Utc>>| d.map_or((1, DateTime::<Utc>::MAX_UTC), |x| (0, x));
        key(da)
            .cmp(&key(db))
            .then(sa.cmp(&sb))
            .then(a.task.priority.cmp(&b.task.priority))
            .then(a.task.id.cmp(&b.task.id))
    });
    v
}

/// Score of a placement for a task: higher is better.
fn score(g: &Grid<'_>, t: &Task, i: usize, len: usize) -> i64 {
    let local = g.at(i).with_timezone(&g.tz);
    let m = (local.hour() * 60 + local.minute()) as u16;
    let e = m + (len as u16) * g.slot as u16;
    let overlap = |s: Hm, en: Hm| -> i64 {
        let a = m.max(s.minutes());
        let b = e.min(en.minutes());
        i64::from(b.saturating_sub(a))
    };
    let mut sc = 0;
    if let Some(w) = t.preferred_window.as_deref().and_then(|w| g.c.window(w)) {
        sc += 4 * overlap(w.start, w.end);
    }
    if let Some(p) = g.c.profile_for(local.date_naive()) {
        // Deep-work windows suit high-priority tasks.
        let weight = match t.priority {
            1 => 2,
            2 => 1,
            _ => 0,
        };
        sc += weight * p.deep_work.iter().map(|w| overlap(w.start, w.end)).sum::<i64>();
    }
    sc
}

/// `buffer_minutes` of slot `i`'s day, in whole slots.
fn buf_slots(g: &Grid<'_>, i: usize) -> usize {
    let buf = g.c.profile_for(g.date[i]).map_or(10, |p| i64::from(p.buffer_minutes));
    ((buf + g.slot - 1) / g.slot) as usize
}

struct State {
    usable: Vec<bool>,
    /// Slots holding a task block. Kept apart from `usable` so the buffer
    /// between two blocks holds on both sides, whichever was placed first
    /// (otherwise a replan that keeps blocks in another order would move one).
    taken: Vec<bool>,
    focus: BTreeMap<NaiveDate, i64>,
    /// Per task: slot ranges already allocated.
    chunks: BTreeMap<String, Vec<(usize, usize)>>,
}

impl State {
    fn can_place(&self, g: &Grid<'_>, task: &str, i: usize, len: usize) -> bool {
        if i + len > g.n || !self.usable[i..i + len].iter().all(|&u| u) {
            return false;
        }
        let buf = buf_slots(g, i);
        if self.taken[i.saturating_sub(buf)..(i + len + buf).min(g.n)]
            .iter()
            .any(|&t| t)
        {
            return false;
        }
        let gap = (SAME_TASK_GAP_MIN / g.slot) as usize;
        if let Some(cs) = self.chunks.get(task)
            && cs.iter().any(|&(a, b)| i < b + gap && a < i + len + gap)
        {
            return false;
        }
        // Focus cap per local day.
        let mut per_day: BTreeMap<NaiveDate, i64> = BTreeMap::new();
        for k in i..i + len {
            *per_day.entry(g.date[k]).or_default() += g.slot;
        }
        per_day.iter().all(|(d, add)| {
            let cap = g.c.profile_for(*d).map_or(0.0, |p| p.max_focus_hours_per_day * 60.0) as i64;
            self.focus.get(d).copied().unwrap_or(0) + add <= cap
        })
    }

    fn place(&mut self, g: &Grid<'_>, task: &str, i: usize, len: usize) {
        for k in i..(i + len).min(g.n) {
            self.usable[k] = false;
            self.taken[k] = true;
        }
        for k in i..i + len {
            *self.focus.entry(g.date[k]).or_default() += g.slot;
        }
        self.chunks.entry(task.to_string()).or_default().push((i, i + len));
    }
}

/// A chunk for `n` ending by slot `limit` (exclusive).
fn find_chunk(g: &Grid<'_>, st: &State, n: &Need<'_>, remaining: usize, limit: usize) -> Option<(usize, usize)> {
    let t = n.task;
    let slot = g.slot as u32;
    let min_len = if t.splittable {
        (t.min_chunk_min.div_ceil(slot) as usize).max(1).min(remaining)
    } else {
        remaining
    };
    let max_len = if t.splittable {
        (t.max_chunk_min / slot).max(min_len as u32) as usize
    } else {
        remaining
    }
    .min(remaining);
    let hi = limit.min(g.n);
    let mut i = n.earliest_idx;
    while i < hi {
        if !st.can_place(g, &t.id, i, min_len) || i + min_len > hi {
            i += 1;
            continue;
        }
        // Earliest feasible day found: choose the best start on that day.
        let day = g.date[i];
        let mut best: Option<(i64, usize, usize)> = None; // (score, start, len)
        let mut j = i;
        while j < hi && g.date[j] == day {
            if j + min_len <= hi && st.can_place(g, &t.id, j, min_len) {
                let mut len = min_len;
                while len < max_len && j + len < hi && st.can_place(g, &t.id, j, len + 1) {
                    len += 1;
                }
                let sc = score(g, t, j, len) * 1000 + len as i64; // prefer fit, then longer chunks
                if best.is_none_or(|(bs, _, _)| sc > bs) {
                    best = Some((sc, j, len));
                }
            }
            j += 1;
        }
        return best.map(|(_, s, l)| (s, l));
    }
    None
}

/// `lead_first`: every task's lead-respecting turn before any fallback
/// (fewest squeezes); otherwise each task falls back right after its own turn,
/// in deadline order (urgent work first, which can avoid a shortfall).
fn run(input: &PlanInput<'_>, g: &Grid<'_>, horizon_idx: usize, keep_existing: bool, lead_first: bool) -> (Plan, u32) {
    let mut st = State {
        usable: g.usable.clone(),
        taken: vec![false; g.n],
        focus: BTreeMap::new(),
        chunks: BTreeMap::new(),
    };
    let mut plan = Plan::default();
    let needs = ordered_needs(input, input.tasks, g, horizon_idx);
    let mut remaining: BTreeMap<&str, usize> = needs.iter().map(|n| (n.task.id.as_str(), n.need_slots)).collect();

    // Existing future blocks, by task then start.
    let mut existing: Vec<&Block> = input.existing.iter().filter(|b| b.end > g.t0).collect();
    existing.sort_by(|a, b| (a.task_id.as_str(), a.start).cmp(&(b.task_id.as_str(), b.start)));
    // Blocks that already started (but aren't checked in) count as in progress.
    for b in input.existing.iter().filter(|b| b.start < g.t0 && b.end > input.now) {
        if let Some(r) = remaining.get_mut(b.task_id.as_str()) {
            let used = (b.end.min(g.t0) - b.start).num_minutes().max(0) / g.slot;
            *r = r.saturating_sub(used as usize);
        }
        plan.keep.push(b.clone());
    }
    existing.retain(|b| b.start >= g.t0);

    // Pinned first (always kept), then others (in deadline order of their task).
    let order: BTreeMap<&str, usize> = needs.iter().enumerate().map(|(k, n)| (n.task.id.as_str(), k)).collect();
    existing.sort_by_key(|b| {
        (
            !b.pinned,
            order.get(b.task_id.as_str()).copied().unwrap_or(usize::MAX),
            b.start,
        )
    });
    for b in existing {
        let i = g.idx(b.start).max(0) as usize;
        let len = (b.minutes() / g.slot).max(0) as usize;
        let need = remaining.get(b.task_id.as_str()).copied().unwrap_or(0);
        let deadline_ok = needs
            .iter()
            .find(|n| n.task.id == b.task_id)
            .is_some_and(|n| i + len <= n.hard_idx);
        let aligned = (b.start - g.t0).num_minutes() % g.slot == 0;
        if b.pinned {
            for k in i..(i + len).min(g.n) {
                st.usable[k] = false;
                st.taken[k] = true;
                *st.focus.entry(g.date[k]).or_default() += g.slot;
            }
            st.chunks.entry(b.task_id.clone()).or_default().push((i, i + len));
            if let Some(r) = remaining.get_mut(b.task_id.as_str()) {
                *r = r.saturating_sub(len);
            }
            plan.keep.push(b.clone());
        } else if keep_existing && need > 0 && len > 0 && aligned && deadline_ok && st.can_place(g, &b.task_id, i, len)
        {
            st.place(g, &b.task_id, i, len);
            if let Some(r) = remaining.get_mut(b.task_id.as_str()) {
                *r = r.saturating_sub(len);
            }
            plan.keep.push(b.clone());
        } else {
            plan.delete.push(b.clone());
        }
    }

    // Each task gets a turn honouring its deadline lead (soft) and one that
    // may run up to the deadline itself; `lead_first` decides the order.
    let turns: Vec<(usize, bool)> = if lead_first {
        [true, false]
            .into_iter()
            .flat_map(|soft| (0..needs.len()).map(move |k| (k, soft)))
            .collect()
    } else {
        (0..needs.len()).flat_map(|k| [(k, true), (k, false)]).collect()
    };
    let mut total_short = 0u32;
    for (k, soft) in turns {
        let n = &needs[k];
        let limit = if soft { n.deadline_idx } else { n.hard_idx };
        let Some(rem) = remaining.get_mut(n.task.id.as_str()) else {
            continue;
        };
        while *rem > 0 {
            match find_chunk(g, &st, n, *rem, limit) {
                Some((i, len)) => {
                    st.place(g, &n.task.id, i, len);
                    plan.create.push(Block {
                        uid: None,
                        task_id: n.task.id.clone(),
                        start: g.at(i),
                        end: g.at(i + len),
                        pinned: false,
                    });
                    *rem -= len;
                }
                None => break,
            }
        }
    }
    for n in &needs {
        let rem = remaining.get(n.task.id.as_str()).copied().unwrap_or(0);
        if rem > 0 {
            let short = (rem as u32) * g.slot as u32;
            total_short += short;
            plan.shortfalls.push(shortfall(n.task, short, g.t0, input.tz));
        }
    }
    // A deleted block re-created at the same time for the same task isn't a move.
    let mut moved = 0;
    for d in &plan.delete {
        if let Some(pos) = plan
            .create
            .iter()
            .position(|c| c.uid.is_none() && c.task_id == d.task_id && c.start == d.start && c.end == d.end)
        {
            plan.create[pos].uid = d.uid.clone();
            plan.keep.push(plan.create.remove(pos));
        } else {
            moved += 1;
        }
    }
    plan.delete
        .retain(|d| !plan.keep.iter().any(|k| k.uid.is_some() && k.uid == d.uid));
    plan.moved = moved;
    plan.keep.sort_by_key(|b| b.start);
    plan.create.sort_by_key(|b| b.start);
    plan.delete.sort_by_key(|b| b.start);
    plan.squeezed = squeezes(input, g, &needs, &plan);
    (plan, total_short)
}

/// Warn about every task with future, non-pinned work inside its deadline lead.
fn squeezes(input: &PlanInput<'_>, g: &Grid<'_>, needs: &[Need<'_>], plan: &Plan) -> Vec<Squeeze> {
    let mut out = Vec::new();
    for n in needs {
        let Some(due) = n.task.due else { continue };
        let lead_from = g.at(n.deadline_idx);
        let mins: i64 = plan
            .keep
            .iter()
            .chain(&plan.create)
            .filter(|b| b.task_id == n.task.id && !b.pinned && b.end > input.now && b.end > lead_from)
            .map(|b| (b.end - b.start.max(lead_from)).num_minutes().max(0))
            .sum();
        if mins == 0 {
            continue;
        }
        let instant = due.instant(input.tz);
        let lead_hours =
            g.c.profile_for(instant.with_timezone(&input.tz).date_naive())
                .map_or(2, |p| p.deadline_lead_hours);
        out.push(Squeeze {
            task_id: n.task.id.clone(),
            title: n.task.title.clone(),
            due,
            squeezed_min: mins as u32,
            lead_hours,
            message: format!(
                "{}: {} squeezed into the last {lead_hours} h before {} (no room earlier)",
                n.task.title,
                fmt_hours(mins as u32),
                due.describe(input.tz)
            ),
        });
    }
    out
}

fn fmt_hours(min: u32) -> String {
    if min.is_multiple_of(60) {
        format!("{} h", min / 60)
    } else if min < 60 {
        format!("{min} min")
    } else {
        format!("{:.1} h", f64::from(min) / 60.0)
    }
}

/// `first` is the earliest time the scheduler may place anything.
fn shortfall(t: &Task, short: u32, first: DateTime<Utc>, tz: Tz) -> Shortfall {
    let mut when = t.due.map(|d| format!(" before {}", d.describe(tz))).unwrap_or_default();
    let too_soon = t.due.is_some_and(|d| d.instant(tz) <= first);
    let mut options = Vec::new();
    if too_soon {
        // No amount of free time helps: the deadline is inside the planning margin.
        when.push_str(&format!(
            " (nothing can be planned before {})",
            first.with_timezone(&tz).format("%H:%M")
        ));
        options.push("put a block in yourself (the scheduler keeps a margin before now)".to_string());
    } else {
        options.push("extend the day window or reduce buffers in constraints.toml".to_string());
    }
    options.extend([
        format!("cut the estimate (currently {})", fmt_hours(t.remaining_min)),
        "lower the priority of (or drop) another task due earlier".to_string(),
    ]);
    if t.due.is_some() {
        options.push("move the deadline".into());
    }
    if !t.splittable {
        options.push("allow splitting into chunks".into());
    }
    Shortfall {
        task_id: t.id.clone(),
        title: t.title.clone(),
        short_min: short,
        due: t.due,
        message: format!("{}: {} short{}", t.title, fmt_hours(short), when),
        options,
    }
}

/// Plan blocks for all open tasks.
pub fn plan(input: &PlanInput<'_>) -> Plan {
    let c = input.constraints;
    let latest_due = input
        .tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Open)
        .filter_map(|t| t.due.map(|d| d.instant(input.tz)))
        .max();
    let horizon = input.now + Duration::days(i64::from(c.horizon_days));
    let horizon_end = latest_due.map_or(horizon, |d| d.max(horizon));
    let g = Grid::build(input, horizon_end);
    let horizon_idx = g.idx(horizon).clamp(0, g.n as i64) as usize;
    let horizon_idx = horizon_idx.max(latest_due.map(|d| g.idx(d).clamp(0, g.n as i64) as usize).unwrap_or(0));
    let (stable, short) = run(input, &g, horizon_idx, true, true);
    if short == 0 {
        return stable;
    }
    // Short: try deadline-order fallback and/or a fresh start; a candidate
    // wins only if strictly better on (shortfall, squeezed minutes), so the
    // stable plan stays on ties.
    let squeeze = |p: &Plan| p.squeezed.iter().map(|s| s.squeezed_min).sum::<u32>();
    let mut best_key = (short, squeeze(&stable));
    let mut best = stable;
    for (keep, lead_first) in [(true, false), (false, true), (false, false)] {
        let (p, short) = run(input, &g, horizon_idx, keep, lead_first);
        let key = (short, squeeze(&p));
        if key < best_key {
            best_key = key;
            best = p;
        }
    }
    best
}

#[cfg(test)]
mod tests;
