//! "Reply with times I'm free" (SPEC §9): kairos's free slots → lines like
//! `Tue 15 Oct 14:00–15:00`, spread across days (one per day first, then a
//! second per day, …), in the display zone.

use chrono::{DateTime, TimeZone};
use chrono_tz::Tz;

/// A free window (Unix seconds).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub start: i64,
    pub end: i64,
}

fn local(ts: i64, tz: Tz) -> Option<DateTime<Tz>> {
    tz.timestamp_opt(ts, 0).single()
}

/// At most `n` slots of at least `duration_min`, spread across days, in time order.
pub fn pick(slots: &[Slot], duration_min: u32, n: usize, tz: Tz) -> Vec<Slot> {
    let long: Vec<Slot> = slots
        .iter()
        .copied()
        .filter(|s| s.end - s.start >= i64::from(duration_min) * 60)
        .collect();
    // Group by local day, keeping each day's slots in order.
    let mut days: Vec<(chrono::NaiveDate, Vec<Slot>)> = Vec::new();
    let mut sorted = long;
    sorted.sort_by_key(|s| s.start);
    for s in sorted {
        let Some(d) = local(s.start, tz).map(|d| d.date_naive()) else {
            continue;
        };
        match days.last_mut() {
            Some((day, v)) if *day == d => v.push(s),
            _ => days.push((d, vec![s])),
        }
    }
    let mut out = Vec::new();
    let mut round = 0;
    while out.len() < n {
        let mut any = false;
        for (_, v) in &days {
            if let Some(s) = v.get(round) {
                any = true;
                if out.len() < n {
                    out.push(*s);
                }
            }
        }
        if !any {
            break;
        }
        round += 1;
    }
    out.sort_by_key(|s| s.start);
    out
}

/// `Tue 15 Oct 14:00–15:00` (the end's date is shown too if it differs).
pub fn line(s: Slot, tz: Tz) -> String {
    let (Some(a), Some(b)) = (local(s.start, tz), local(s.end, tz)) else {
        return String::new();
    };
    if a.date_naive() == b.date_naive() {
        format!("{}–{}", a.format("%a %-d %b %H:%M"), b.format("%H:%M"))
    } else {
        format!("{}–{}", a.format("%a %-d %b %H:%M"), b.format("%a %-d %b %H:%M"))
    }
}

/// The block inserted into a reply: one slot per line.
pub fn text(slots: &[Slot], tz: Tz) -> String {
    slots.iter().map(|s| line(*s, tz)).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const TZ: Tz = chrono_tz::Europe::London;

    fn at(d: u32, h: u32, m: u32) -> i64 {
        TZ.with_ymd_and_hms(2026, 10, d, h, m, 0).unwrap().timestamp()
    }

    fn slot(d: u32, h1: u32, m1: u32, h2: u32, m2: u32) -> Slot {
        Slot {
            start: at(d, h1, m1),
            end: at(d, h2, m2),
        }
    }

    #[test]
    fn formats_in_london_time() {
        assert_eq!(line(slot(20, 14, 0, 15, 0), TZ), "Tue 20 Oct 14:00–15:00");
        assert_eq!(line(slot(21, 10, 0, 11, 30), TZ), "Wed 21 Oct 10:00–11:30");
        // After the clocks go back (25 Oct): still London wall time.
        assert_eq!(line(slot(27, 9, 0, 10, 0), TZ), "Tue 27 Oct 09:00–10:00");
    }

    #[test]
    fn short_slots_are_dropped() {
        let v = pick(&[slot(20, 9, 0, 9, 30), slot(20, 14, 0, 15, 0)], 60, 5, TZ);
        assert_eq!(v, vec![slot(20, 14, 0, 15, 0)]);
    }

    #[test]
    fn spread_across_days_not_one_morning() {
        let slots = [
            slot(20, 9, 0, 10, 0),
            slot(20, 10, 0, 11, 0),
            slot(20, 11, 0, 12, 0),
            slot(21, 14, 0, 15, 0),
            slot(22, 16, 0, 17, 0),
        ];
        let v = pick(&slots, 60, 3, TZ);
        assert_eq!(
            v,
            vec![slot(20, 9, 0, 10, 0), slot(21, 14, 0, 15, 0), slot(22, 16, 0, 17, 0)]
        );
        // More than there are days: second slots come next, still in time order.
        let v = pick(&slots, 60, 4, TZ);
        assert_eq!(v.len(), 4);
        assert_eq!(v[1], slot(20, 10, 0, 11, 0));
        assert_eq!(text(&v[..2], TZ), "Tue 20 Oct 09:00–10:00\nTue 20 Oct 10:00–11:00");
    }

    #[test]
    fn fewer_slots_than_asked() {
        assert_eq!(pick(&[slot(20, 9, 0, 10, 0)], 30, 5, TZ).len(), 1);
        assert!(pick(&[], 30, 5, TZ).is_empty());
    }
}
