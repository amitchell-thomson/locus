//! How dates read in lists (SPEC §7): `14:03` today, `Tue` within the last
//! week, `14 Oct` this year, `14 Oct 2025` otherwise. Europe/London.

use chrono::{DateTime, Datelike, TimeZone};
use chrono_tz::Tz;

pub fn date_label(ts: i64, now: DateTime<Tz>) -> String {
    let Some(t) = now.timezone().timestamp_opt(ts, 0).single() else {
        return String::new();
    };
    let (d, today) = (t.date_naive(), now.date_naive());
    if d == today {
        t.format("%H:%M").to_string()
    } else if d < today && (today - d).num_days() < 7 {
        t.format("%a").to_string()
    } else if d.year() == today.year() {
        t.format("%-d %b").to_string()
    } else {
        t.format("%-d %b %Y").to_string()
    }
}

/// Full form for headers: `Tue 14 Oct 2026, 14:03`.
pub fn date_full(ts: i64, tz: Tz) -> String {
    tz.timestamp_opt(ts, 0)
        .single()
        .map(|t| t.format("%a %-d %b %Y, %H:%M").to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::Europe::London;

    #[test]
    fn labels() {
        let now = London.with_ymd_and_hms(2026, 10, 14, 15, 0, 0).unwrap();
        let at = |y, m, d, h, mi| London.with_ymd_and_hms(y, m, d, h, mi, 0).unwrap().timestamp();
        assert_eq!(date_label(at(2026, 10, 14, 14, 3), now), "14:03");
        assert_eq!(date_label(at(2026, 10, 14, 0, 5), now), "00:05");
        assert_eq!(date_label(at(2026, 10, 13, 23, 59), now), "Tue");
        assert_eq!(date_label(at(2026, 10, 8, 9, 0), now), "Thu");
        assert_eq!(date_label(at(2026, 10, 7, 9, 0), now), "7 Oct");
        assert_eq!(date_label(at(2026, 1, 2, 9, 0), now), "2 Jan");
        assert_eq!(date_label(at(2025, 12, 31, 9, 0), now), "31 Dec 2025");
        // Clock skew: a message "from tomorrow" isn't shown as a weekday.
        assert_eq!(date_label(at(2026, 10, 15, 9, 0), now), "15 Oct");
        assert_eq!(date_full(at(2026, 10, 14, 14, 3), London), "Wed 14 Oct 2026, 14:03");
    }
}
