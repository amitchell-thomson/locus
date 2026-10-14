//! Deterministic task scheduler (SPEC §7).
//!
//! Model: time from `now + min_lead` (rounded up to a slot) to the horizon is
//! cut into `slot_minutes` slots. A slot is *usable* if it lies inside the
//! day's profile window, outside sleep, meals and no-schedule periods, clear
//! of busy events plus buffers (`buffer_minutes` either side, more before a
//! location change) and of the rest after a recovery event (sport). Blocks
//! keep `buffer_minutes` from each other, chunks of the same task are ≥ 60 min
//! apart, and each day's work stays ≤ `max_focus_hours_per_day`.
//!
//! **Tiers.** Work is placed in the best tier it fits, a lower tier only for
//! what's left over:
//! 1. *Ideal*: blocks of the task type's ideal length (e.g. 2–3 h), finishing
//!    `deadline_lead_hours` (2 h) before the deadline, and *comfortable*: no
//!    more than `max_continuous_minutes` of back-to-back commitments without a
//!    `break_minutes` break, and ≤ `max_day_load_hours` of events + work a day.
//! 2. *Shorter*: blocks down to the type's `min_block` (e.g. 60 min), same rules.
//! 3. *Squeeze*: blocks down to `squeeze_block_minutes` (a 1 h gap between
//!    lectures), up to the deadline itself — first within the comfort rules,
//!    then past them. Every squeezed block is reported as a [`Squeeze`]
//!    warning.
//!
//! What still doesn't fit is a [`Shortfall`] — never silently overbooked.
//!
//! **Spreading.** A task's hours are shared across the days before its
//! deadline in proportion to each day's free time: a sheet due Friday becomes
//! a block on two or three days; with one free day it all goes there (cram).
//! Each chunk goes on the day furthest below its share (earliest on ties);
//! within the day, the best preferred-window / deep-work fit wins.
//!
//! **Stability.** Existing future blocks that are still feasible and needed
//! stay (pinned ones always). If the result is short or squeezed, alternatives
//! — deadline-order fallback and a fresh pass from scratch — are tried, and
//! used only if strictly better on (shortfall, squeezed minutes). So blocks
//! move only when an event displaced them or a squeeze can be undone — or
//! when time was freed (`reflow_after`): then every unpinned block from
//! that time on is planned afresh, so work moves into the gap (Alec,
//! 2026-10-14), unless that would be worse on (shortfall, squeezed minutes).
//!
//! **Proposals.** [`propose`] fills the free gaps left after task work with
//! proposed project time (comfort rules on, same daily cap).

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
    /// No work for this long after it ends (recovery after sport).
    #[serde(default)]
    pub rest_after_min: u16,
    /// Counts toward the day's load (`max_day_load_hours`); not Social / Sport.
    #[serde(default = "yes")]
    pub counts_for_load: bool,
}

fn yes() -> bool {
    true
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
    /// Why this placement (new blocks only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
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

/// A task with squeezed work: blocks in a short gap, in the last
/// `deadline_lead_hours` before the deadline, or past the comfort limits.
/// It fits, but only just.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Squeeze {
    pub task_id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<Due>,
    pub squeezed_min: u32,
    /// The squeezed blocks.
    pub blocks: Vec<SqueezedBlock>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SqueezedBlock {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub reasons: Vec<String>,
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
    /// Times Alec skipped (deleted a work block there): no work, but not an
    /// event either (no buffers, no load, not "back-to-back").
    pub unavailable: &'a [(DateTime<Utc>, DateTime<Utc>)],
    /// Time was freed (an event deleted, shortened or moved away): blocks
    /// from this time on re-flow into the best layout (pinned ones stay).
    pub reflow_after: Option<DateTime<Utc>>,
}

struct Grid<'a> {
    t0: DateTime<Utc>,
    slot: i64,
    n: usize,
    usable: Vec<bool>,
    /// Slots covered by a busy event itself (no buffers): continuity.
    busy_occ: Vec<bool>,
    /// Busy minutes per local day (whole day, also before `t0`): day load.
    busy_load: BTreeMap<NaiveDate, i64>,
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
            busy_occ: vec![false; n],
            busy_load: BTreeMap::new(),
            date,
            tz: input.tz,
            c,
        };
        for (a, b) in input.unavailable {
            g.block_out(*a, *b);
        }
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
            let after = base.max(i64::from(b.rest_after_min));
            g.block_out(b.start - Duration::minutes(before), b.end + Duration::minutes(after));
            let i0 = g.idx(b.start).max(0);
            let i1 = g.idx_ceil(b.end).min(g.n as i64);
            for i in i0..i1 {
                g.busy_occ[i as usize] = true;
            }
            if b.counts_for_load {
                *g.busy_load.entry(d).or_default() += (b.end - b.start).num_minutes().max(0);
            }
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

    fn slots(&self, minutes: i64) -> usize {
        ((minutes + self.slot - 1) / self.slot).max(0) as usize
    }

    /// Mark `[a, b)` unusable (partial slots count).
    fn block_out(&mut self, a: DateTime<Utc>, b: DateTime<Utc>) {
        let i0 = self.idx(a).max(0);
        let i1 = self.idx_ceil(b).min(self.n as i64);
        for i in i0..i1 {
            self.usable[i as usize] = false;
        }
    }

    fn profile(&self, i: usize) -> Option<&Profile> {
        self.c.profile_for(self.date[i])
    }
}

fn slot_allowed(p: &Profile, m: u16, slot: u16, wd: chrono::Weekday) -> bool {
    let end = m + slot; // slot must fit entirely
    let inside = |s: Hm, e: Hm| m >= s.minutes() && end <= e.minutes();
    let weekend = matches!(wd, chrono::Weekday::Sat | chrono::Weekday::Sun);
    let day_start = match p.weekend_start {
        Some(w) if weekend => w.max(p.day_window.start),
        _ => p.day_window.start,
    };
    if !inside(day_start, p.day_window.end) {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Tier {
    Ideal,
    Shorter,
    /// Short gaps / the deadline lead, still within the comfort rules…
    Squeeze,
    /// …and only then past them.
    Overload,
}

const TIERS: [Tier; 4] = [Tier::Ideal, Tier::Shorter, Tier::Squeeze, Tier::Overload];

struct Need<'t> {
    task: &'t Task,
    need_slots: usize,
    /// Exclusive latest slot index (deadline minus lead), clipped to grid.
    deadline_idx: usize,
    /// Exclusive latest slot index (the deadline itself), clipped to grid.
    hard_idx: usize,
    earliest_idx: usize,
    ideal_min: usize,
    ideal_max: usize,
    short_min: usize,
    squeeze_min: usize,
}

impl Need<'_> {
    /// (shortest block, latest end, comfort rules on) for a tier.
    fn tier(&self, t: Tier, remaining: usize) -> (usize, usize, bool) {
        let (min, limit, comfort) = match t {
            Tier::Ideal => (self.ideal_min, self.deadline_idx, true),
            Tier::Shorter => (self.short_min, self.deadline_idx, true),
            Tier::Squeeze => (self.squeeze_min, self.hard_idx, true),
            Tier::Overload => (self.squeeze_min, self.hard_idx, false),
        };
        let min = if self.task.splittable { min } else { remaining };
        (min.clamp(1, remaining.max(1)), limit, comfort)
    }

    /// Target block length: the remainder split evenly into ideal-sized blocks.
    fn target_len(&self, remaining: usize) -> usize {
        if !self.task.splittable {
            return remaining;
        }
        let blocks = remaining.div_ceil(self.ideal_max.max(1)).max(1);
        remaining.div_ceil(blocks)
    }
}

fn ordered_needs<'t>(input: &PlanInput<'_>, tasks: &'t [Task], g: &Grid<'_>, horizon_idx: usize) -> Vec<Need<'t>> {
    let mut v: Vec<Need<'t>> = tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Open && t.remaining_min > 0)
        .map(|t| {
            let due = t.due.map(|d| d.instant(input.tz));
            let lead = due.map(|due| {
                let p = g.c.profile_for(due.with_timezone(&g.tz).date_naive());
                due - Duration::hours(i64::from(p.map_or(2, |p| p.deadline_lead_hours)))
            });
            let clip = |x: DateTime<Utc>| g.idx(x).clamp(0, horizon_idx as i64) as usize;
            let earliest_idx = t
                .earliest_start
                .map_or(0, |e| g.idx_ceil(e).clamp(0, g.n as i64) as usize);
            let short_min = g.slots(i64::from(t.min_chunk_min)).max(1);
            Need {
                task: t,
                need_slots: g.slots(i64::from(t.remaining_min)),
                deadline_idx: lead.map_or(horizon_idx, clip),
                hard_idx: due.map_or(horizon_idx, clip),
                earliest_idx,
                ideal_min: g.slots(i64::from(t.ideal_min())).max(short_min),
                ideal_max: g.slots(i64::from(t.max_chunk_min)).max(1),
                short_min,
                squeeze_min: g.slots(i64::from(g.c.squeeze_block_minutes)).clamp(1, short_min),
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
    g.slots(g.profile(i).map_or(10, |p| i64::from(p.buffer_minutes)))
}

#[derive(Clone)]
struct State {
    usable: Vec<bool>,
    /// Slots holding a task block. Kept apart from `usable` so the buffer
    /// between two blocks holds on both sides, whichever was placed first
    /// (otherwise a replan that keeps blocks in another order would move one).
    taken: Vec<bool>,
    /// Busy events + task blocks (continuity).
    occ: Vec<bool>,
    focus: BTreeMap<NaiveDate, i64>,
    /// Per task: slot ranges already allocated.
    chunks: BTreeMap<String, Vec<(usize, usize)>>,
}

impl State {
    fn new(g: &Grid<'_>) -> State {
        State {
            usable: g.usable.clone(),
            taken: vec![false; g.n],
            occ: g.busy_occ.clone(),
            focus: BTreeMap::new(),
            chunks: BTreeMap::new(),
        }
    }

    /// Length (slots) of the back-to-back run `[i, i+len)` would sit in:
    /// occupied slots joined by gaps shorter than the break.
    fn run_len(&self, g: &Grid<'_>, i: usize, len: usize) -> usize {
        let brk = g.slots(g.profile(i).map_or(30, |p| i64::from(p.break_minutes)));
        let mut a = i;
        while let Some(k) = (a.saturating_sub(brk)..a).find(|&k| self.occ[k]) {
            let mut k = k;
            while k > 0 && self.occ[k - 1] {
                k -= 1;
            }
            a = k;
        }
        let mut b = (i + len).min(g.n);
        while let Some(k) = (b..(b + brk).min(g.n)).rev().find(|&k| self.occ[k]) {
            let mut k = k + 1;
            while k < g.n && self.occ[k] {
                k += 1;
            }
            b = k;
        }
        b - a
    }

    fn comfortable(&self, g: &Grid<'_>, i: usize, len: usize, extra_load: i64) -> bool {
        let Some(p) = g.profile(i) else { return false };
        let max_run = g.slots(i64::from(p.max_continuous_minutes));
        if self.run_len(g, i, len) > max_run {
            return false;
        }
        let d = g.date[i];
        let load = g.busy_load.get(&d).copied().unwrap_or(0) + self.focus.get(&d).copied().unwrap_or(0) + extra_load;
        load <= (p.max_day_load_hours * 60.0) as i64
    }

    fn can_place(&self, g: &Grid<'_>, task: &str, i: usize, len: usize, comfort: bool) -> bool {
        if len == 0 || i + len > g.n || !self.usable[i..i + len].iter().all(|&u| u) {
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
        let capped = per_day.iter().all(|(d, add)| {
            let cap = g.c.profile_for(*d).map_or(0.0, |p| p.max_focus_hours_per_day * 60.0) as i64;
            self.focus.get(d).copied().unwrap_or(0) + add <= cap
        });
        capped && (!comfort || self.comfortable(g, i, len, len as i64 * g.slot))
    }

    /// Grow the task's chunk `[a, b)` to end at `b + add` (absorbing a crumb).
    fn can_extend(&self, g: &Grid<'_>, task: &str, b: usize, add: usize) -> bool {
        let e = b + add;
        if e > g.n || !self.usable[b..e].iter().all(|&u| u) {
            return false;
        }
        let buf = buf_slots(g, b);
        if self.taken[b..(e + buf).min(g.n)].iter().any(|&t| t) || g.date[b] != g.date[e - 1] {
            return false;
        }
        let gap = (SAME_TASK_GAP_MIN / g.slot) as usize;
        if let Some(cs) = self.chunks.get(task)
            && cs.iter().any(|&(x, _)| x > b && x < e + gap)
        {
            return false;
        }
        let d = g.date[b];
        let cap = g.c.profile_for(d).map_or(0.0, |p| p.max_focus_hours_per_day * 60.0) as i64;
        self.focus.get(&d).copied().unwrap_or(0) + add as i64 * g.slot <= cap
    }

    fn extend(&mut self, g: &Grid<'_>, task: &str, b: usize, add: usize) {
        for k in b..b + add {
            self.usable[k] = false;
            self.taken[k] = true;
            self.occ[k] = true;
            *self.focus.entry(g.date[k]).or_default() += g.slot;
        }
        if let Some(c) = self
            .chunks
            .get_mut(task)
            .and_then(|cs| cs.iter_mut().find(|c| c.1 == b))
        {
            c.1 = b + add;
        }
    }

    fn place(&mut self, g: &Grid<'_>, task: &str, i: usize, len: usize) {
        for k in i..(i + len).min(g.n) {
            self.usable[k] = false;
            self.taken[k] = true;
            self.occ[k] = true;
            *self.focus.entry(g.date[k]).or_default() += g.slot;
        }
        self.chunks.entry(task.to_string()).or_default().push((i, i + len));
    }
}

/// The next chunk for `n` in `tier`: (start, len, why).
fn find_chunk(g: &Grid<'_>, st: &State, n: &Need<'_>, remaining: usize, tier: Tier) -> Option<(usize, usize, String)> {
    let t = n.task;
    let (min_len, limit, comfort) = n.tier(tier, remaining);
    let target = n.target_len(remaining).max(min_len);
    let lo = n.earliest_idx;
    let hi = limit.min(g.n);
    if lo >= hi {
        return None;
    }
    // Free time per day, and this task's work already there.
    let mut days: BTreeMap<NaiveDate, (i64, i64)> = BTreeMap::new(); // date → (free slots, placed slots)
    for k in lo..hi {
        let e = days.entry(g.date[k]).or_default();
        if st.usable[k] && !st.taken[k] {
            e.0 += 1;
        }
    }
    for &(a, b) in st.chunks.get(&t.id).map(Vec::as_slice).unwrap_or(&[]) {
        if let Some(e) = days.get_mut(&g.date[a]) {
            e.1 += (b - a) as i64;
        }
    }
    let total_free: i64 = days.values().map(|v| v.0).sum();
    if total_free == 0 {
        return None;
    }
    let total = remaining as i64 + days.values().map(|v| v.1).sum::<i64>();
    let with_room = days.values().filter(|v| v.0 >= min_len as i64).count();
    // Furthest below its share first (placed/total vs free/total_free), earliest on ties.
    let mut order: Vec<(i64, NaiveDate)> = days
        .iter()
        .filter(|(_, v)| v.0 > 0)
        .map(|(d, v)| (v.1 * total_free - total * v.0, *d))
        .collect();
    order.sort();
    // Work that's one block anyway (a 30-min errand) isn't spread: the
    // earliest day with room.
    if total <= target as i64 {
        order.sort_by_key(|o| o.1);
    }
    let first_day = order.iter().map(|o| o.1).min();
    // The first day (in spreading order) that fits a full-size block wins;
    // if none does, the day that fits the longest one (not merely the first
    // that fits any) — so a long free evening beats a short morning gap.
    let mut partial: Option<(usize, usize, usize, NaiveDate)> = None; // (fits, start, len, day)
    for (_, day) in order {
        let mut best: Option<(usize, i64, usize, usize)> = None; // (fits, score, start, len)
        for j in lo..hi {
            if g.date[j] != day {
                continue;
            }
            if !st.can_place(g, &t.id, j, min_len, comfort) || j + min_len > hi {
                continue;
            }
            let mut len = min_len;
            while len < target && j + len < hi && st.can_place(g, &t.id, j, len + 1, comfort) {
                len += 1;
            }
            let fits = len;
            // Don't leave a crumb (shorter than any block): absorb it if
            // there's room, else leave a remainder that can stand alone.
            let rest = remaining - len.min(remaining);
            let crumb = n.squeeze_min.min(min_len);
            if rest > 0 && rest < crumb && j + len + rest <= hi && st.can_place(g, &t.id, j, len + rest, comfort) {
                len += rest;
            } else if rest > 0 && rest < min_len && len >= 2 * min_len - rest {
                len -= min_len - rest;
            } else if rest > 0 && rest < n.short_min && len >= n.short_min + (n.short_min - rest) {
                // Leave at least a proper (if short) block, not a squeeze.
                len -= n.short_min - rest;
            } else if rest > 0 && rest < crumb && len >= crumb + (crumb - rest) {
                len -= crumb - rest;
            }
            let sc = score(g, t, j, len);
            // Longest (up to target) first, then fit, then earliest.
            if best.is_none_or(|(bf, bs, _, _)| (fits, sc) > (bf, bs)) {
                best = Some((fits, sc, j, len));
            }
        }
        let Some((fits, _, s, l)) = best else { continue };
        if fits < target {
            if partial.is_none_or(|(pf, ..)| fits > pf) {
                partial = Some((fits, s, l, day));
            }
            continue;
        }
        let where_ = if with_room <= 1 {
            "the only day with room"
        } else if Some(day) == first_day {
            "earliest day with room"
        } else {
            "spreads the work across the days before it's due"
        };
        return Some((s, l, why(tier, where_)));
    }
    let (_, s, l, day) = partial?;
    let where_ = if with_room <= 1 {
        "the only day with room"
    } else if Some(day) == first_day {
        "earliest day with room"
    } else {
        "the day with the most room for it"
    };
    Some((s, l, why(tier, where_)))
}

/// The "why" of a placement: its tier and how its day was chosen.
fn why(tier: Tier, where_: &str) -> String {
    let what = match tier {
        Tier::Ideal => "full-length block",
        Tier::Shorter => "shorter block (no room for a full one)",
        Tier::Squeeze => "squeezed in (no room otherwise)",
        Tier::Overload => "squeezed in past the comfort limits (no room otherwise)",
    };
    format!("{what}; {where_}")
}

/// `lead_first`: every task's best-tier turn before anyone drops a tier
/// (fewest squeezes); otherwise each task goes down the tiers right after its
/// own turn, in deadline order (urgent work first, which can avoid a shortfall).
fn run(input: &PlanInput<'_>, g: &Grid<'_>, horizon_idx: usize, keep_existing: bool, lead_first: bool) -> (Plan, u32) {
    let mut st = State::new(g);
    let mut plan = Plan::default();
    let needs = ordered_needs(input, input.tasks, g, horizon_idx);
    let mut remaining: BTreeMap<&str, usize> = needs.iter().map(|n| (n.task.id.as_str(), n.need_slots)).collect();

    // Existing future blocks, by task then start.
    let mut existing: Vec<&Block> = input.existing.iter().filter(|b| b.end > g.t0).collect();
    existing.sort_by(|a, b| (a.task_id.as_str(), a.start).cmp(&(b.task_id.as_str(), b.start)));
    // Blocks already under way (or starting inside the planning margin) count
    // in full: that work is happening.
    for b in input.existing.iter().filter(|b| b.start < g.t0 && b.end > input.now) {
        if let Some(r) = remaining.get_mut(b.task_id.as_str()) {
            let used = g.slots(b.minutes().max(0));
            *r = r.saturating_sub(used);
        }
        let i1 = g.idx_ceil(b.end).clamp(0, g.n as i64) as usize;
        for k in 0..i1 {
            st.taken[k] = true;
            st.usable[k] = false;
            st.occ[k] = true;
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
                st.occ[k] = true;
                *st.focus.entry(g.date[k]).or_default() += g.slot;
            }
            st.chunks.entry(b.task_id.clone()).or_default().push((i, i + len));
            if let Some(r) = remaining.get_mut(b.task_id.as_str()) {
                *r = r.saturating_sub(len);
            }
            plan.keep.push(b.clone());
        } else if keep_existing
            && need > 0
            && len > 0
            // Not longer than what's left (the task shrank: replace it).
            && len <= need
            && aligned
            && deadline_ok
            && st.can_place(g, &b.task_id, i, len, false)
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

    let turns: Vec<(usize, Tier)> = if lead_first {
        TIERS
            .into_iter()
            .flat_map(|t| (0..needs.len()).map(move |k| (k, t)))
            .collect()
    } else {
        (0..needs.len()).flat_map(|k| TIERS.map(|t| (k, t))).collect()
    };
    for (k, tier) in turns {
        let n = &needs[k];
        let Some(rem) = remaining.get_mut(n.task.id.as_str()) else {
            continue;
        };
        while *rem > 0 {
            // A crumb (shorter than any block) is added onto one of the task's
            // new blocks, never a block of its own; if it can't be, it's short.
            if *rem < n.squeeze_min && (*rem as i64) * g.slot < i64::from(n.task.remaining_min) {
                let limit = n.tier(tier, *rem).1;
                let grow = plan.create.iter_mut().filter(|b| b.task_id == n.task.id).find(|b| {
                    let e = g.idx(b.end).max(0) as usize;
                    e + *rem <= limit && st.can_extend(g, &n.task.id, e, *rem)
                });
                if let Some(b) = grow {
                    let e = g.idx(b.end).max(0) as usize;
                    st.extend(g, &n.task.id, e, *rem);
                    b.end = g.at(e + *rem);
                    *rem = 0;
                }
                break;
            }
            match find_chunk(g, &st, n, *rem, tier) {
                Some((i, len, why)) => {
                    st.place(g, &n.task.id, i, len);
                    plan.create.push(Block {
                        uid: None,
                        task_id: n.task.id.clone(),
                        start: g.at(i),
                        end: g.at(i + len),
                        pinned: false,
                        why: Some(why),
                    });
                    *rem = rem.saturating_sub(len);
                }
                None => break,
            }
        }
    }
    let mut total_short = 0u32;
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
            let mut b = plan.create.remove(pos);
            b.uid = d.uid.clone();
            b.why = None;
            plan.keep.push(b);
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

/// A state holding `blocks` (and the grid's busy events).
fn state_with(g: &Grid<'_>, blocks: &[&Block]) -> State {
    let mut st = State::new(g);
    for b in blocks {
        let i = g.idx(b.start).clamp(0, g.n as i64) as usize;
        let j = g.idx_ceil(b.end).clamp(0, g.n as i64) as usize;
        if j > i {
            st.place(g, &b.task_id, i, j - i);
        }
    }
    st
}

/// Every task's squeezed future blocks (not pinned): in a short gap, in the
/// deadline lead, or past the comfort limits.
fn squeezes(input: &PlanInput<'_>, g: &Grid<'_>, needs: &[Need<'_>], plan: &Plan) -> Vec<Squeeze> {
    let all: Vec<&Block> = plan.keep.iter().chain(&plan.create).collect();
    let st = state_with(g, &all);
    let mut out = Vec::new();
    for n in needs {
        let t = n.task;
        let lead_from = (n.deadline_idx < n.hard_idx).then(|| g.at(n.deadline_idx));
        let lead_hours = t.due.map_or(2, |d| {
            g.c.profile_for(d.instant(input.tz).with_timezone(&input.tz).date_naive())
                .map_or(2, |p| p.deadline_lead_hours)
        });
        let mut blocks = Vec::new();
        let mut mins = 0i64;
        for b in all.iter().filter(|b| b.task_id == t.id && !b.pinned && b.start >= g.t0) {
            let i = g.idx(b.start).clamp(0, g.n as i64) as usize;
            let len = (b.minutes() / g.slot).max(0) as usize;
            if len == 0 || i + len > g.n {
                continue;
            }
            let mut reasons = Vec::new();
            let mut counted = 0;
            if let (Some(lf), Some(due)) = (lead_from, t.due)
                && b.end > lf
            {
                reasons.push(format!(
                    "into the last {lead_hours} h before {}",
                    due.describe(input.tz)
                ));
                counted = (b.end - b.start.max(lf)).num_minutes();
            }
            if t.splittable && len < n.short_min && (len as u32) * (g.slot as u32) < t.remaining_min {
                reasons.push("into a short gap".to_string());
                counted = b.minutes();
            }
            let run_ok = g
                .profile(i)
                .is_some_and(|p| st.run_len(g, i, len) <= g.slots(i64::from(p.max_continuous_minutes)));
            if !run_ok {
                reasons.push("back-to-back with other commitments".to_string());
                counted = b.minutes();
            } else if !st.comfortable(g, i, len, 0) {
                reasons.push("onto an overloaded day".to_string());
                counted = b.minutes();
            }
            if !reasons.is_empty() {
                mins += counted.max(0);
                blocks.push(SqueezedBlock {
                    start: b.start,
                    end: b.end,
                    reasons,
                });
            }
        }
        if blocks.is_empty() {
            continue;
        }
        let mut reasons: Vec<String> = blocks.iter().flat_map(|b| b.reasons.clone()).collect();
        reasons.dedup();
        let mut seen = std::collections::BTreeSet::new();
        reasons.retain(|r| seen.insert(r.clone()));
        out.push(Squeeze {
            task_id: t.id.clone(),
            title: t.title.clone(),
            due: t.due,
            squeezed_min: mins as u32,
            message: format!(
                "{}: {} squeezed {} (no room otherwise)",
                t.title,
                fmt_hours(mins as u32),
                reasons.join(", ")
            ),
            blocks,
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
        format!("{} h {:02} m", min / 60, min % 60)
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
    let squeeze = |p: &Plan| p.squeezed.iter().map(|s| s.squeezed_min).sum::<u32>();
    let (stable, short) = run(input, &g, horizon_idx, true, true);
    let mut best_key = (short, squeeze(&stable));
    if let Some(t) = input.reflow_after {
        // Blocks before `t` hold (as if pinned); the rest is planned afresh.
        let held: Vec<Block> = input
            .existing
            .iter()
            .cloned()
            .map(|mut b| {
                b.pinned |= b.start < t;
                b
            })
            .collect();
        let inp = PlanInput {
            existing: &held,
            reflow_after: None,
            ..*input
        };
        let (mut fresh, short) = run(&inp, &g, horizon_idx, false, true);
        let key = (short, squeeze(&fresh));
        if key <= best_key {
            for k in fresh.keep.iter_mut() {
                if let Some(o) = input.existing.iter().find(|o| o.uid.is_some() && o.uid == k.uid) {
                    k.pinned = o.pinned;
                }
            }
            return fresh;
        }
    }
    if best_key == (0, 0) {
        return stable;
    }
    // Short or squeezed: try deadline-order fallback and/or a fresh start; a
    // candidate wins only if strictly better on (shortfall, squeezed minutes),
    // so the stable plan stays on ties.
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

/// A proposed stretch of project time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposal {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

/// Proposed project time: from `projects.earliest` (09:00) on each of the next `projects.days_ahead` days,
/// up to `projects.max_per_day` sessions (the longest comfortable gaps, up to
/// `max_minutes`, at least `min_minutes`; earliest on ties) in the time
/// `plan`'s task work leaves free, within the daily work cap.
/// `avoid`: spans Alec dropped — never proposed again. `taken_days`: days
/// that already have an accepted session (no more proposals there).
pub fn propose(
    input: &PlanInput<'_>,
    plan: &Plan,
    avoid: &[(DateTime<Utc>, DateTime<Utc>)],
    taken_days: &[NaiveDate],
) -> Vec<Proposal> {
    let pc = &input.constraints.projects;
    if !pc.enabled {
        return Vec::new();
    }
    let horizon = input.now + Duration::days(i64::from(pc.days_ahead));
    let mut g = Grid::build(input, horizon);
    for (a, b) in avoid {
        g.block_out(*a, *b);
    }
    // Not before `projects.earliest` (09:00) local time.
    for i in 0..g.n {
        let l = g.at(i).with_timezone(&g.tz);
        if (l.hour() * 60 + l.minute()) < u32::from(pc.earliest.minutes()) {
            g.usable[i] = false;
        }
    }
    let blocks: Vec<&Block> = plan.keep.iter().chain(&plan.create).collect();
    let mut st = state_with(&g, &blocks);
    let min = g.slots(i64::from(pc.min_minutes)).max(1);
    let max = g.slots(i64::from(pc.max_minutes)).max(min);
    let id = "\u{0}project";
    let mut out = Vec::new();
    let mut days: Vec<NaiveDate> = g.date.clone();
    days.dedup();
    for day in days {
        if taken_days.contains(&day) {
            continue;
        }
        for _ in 0..pc.max_per_day {
            // The day's longest comfortable gap (capped), earliest on ties.
            let mut best: Option<(usize, usize)> = None; // (len, start)
            for i in (0..g.n).filter(|&i| g.date[i] == day) {
                if !st.can_place(&g, id, i, min, true) {
                    continue;
                }
                let mut len = min;
                while len < max && g.date[(i + len).min(g.n - 1)] == day && st.can_place(&g, id, i, len + 1, true) {
                    len += 1;
                }
                if best.is_none_or(|(bl, _)| len > bl) {
                    best = Some((len, i));
                }
            }
            let Some((len, i)) = best else { break };
            st.place(&g, id, i, len);
            out.push(Proposal {
                start: g.at(i),
                end: g.at(i + len),
            });
        }
    }
    out.sort_by_key(|p| p.start);
    out
}

#[cfg(test)]
mod tests;
