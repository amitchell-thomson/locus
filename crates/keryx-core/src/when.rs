//! Times typed by people or Claude: natural input via the shared
//! `eco-dates` parser ("fri 17:00", "tomorrow 9", "23 oct"; ambiguity is an
//! error), ISO 8601, or RFC 3339. A bare date means the configured morning.

use crate::config::{Config, parse_hhmm};
use chrono::{DateTime, NaiveDateTime, NaiveTime, TimeZone};
use chrono_tz::Tz;

/// A time in the future, as Unix seconds.
pub fn parse_future(cfg: &Config, now: DateTime<Tz>, input: &str) -> Result<i64, String> {
    let input = input.trim();
    let tz = cfg.tz();
    let t = if let Ok(t) = DateTime::parse_from_rfc3339(input) {
        t.timestamp()
    } else {
        let naive = match NaiveDateTime::parse_from_str(input, "%Y-%m-%dT%H:%M") {
            Ok(n) => n,
            Err(_) => {
                let morning = parse_hhmm(&cfg.snooze.morning)
                    .unwrap_or_else(|| NaiveTime::from_hms_opt(8, 0, 0).unwrap_or_default());
                match eco_dates::parse_when(input, now.with_timezone(&tz).date_naive()).map_err(|e| e.to_string())? {
                    eco_dates::When::DateTime(dt) => dt,
                    eco_dates::When::Date(d) => d.and_time(morning),
                }
            }
        };
        tz.from_local_datetime(&naive)
            .earliest()
            .ok_or("that time doesn't exist (clocks change)")?
            .timestamp()
    };
    if t <= now.timestamp() {
        return Err("that's in the past".into());
    }
    Ok(t)
}

/// A time for kairos, in the exact form keryx sends and the label the form
/// shows, so what Alec sees is what kairos gets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Canon {
    /// `2026-10-23` (all day) or `2026-10-23T17:00` (local wall time).
    pub iso: String,
    /// `Fri 23 Oct` or `Fri 23 Oct 17:00`.
    pub label: String,
    /// Start of the day / the moment (Unix seconds).
    pub ts: i64,
}

/// Natural input (shared `eco-dates` parser, the one kairos uses), ISO
/// 8601 or RFC 3339 → [`Canon`] in the display zone.
pub fn canonical(cfg: &Config, now: DateTime<Tz>, input: &str) -> Result<Canon, String> {
    let input = input.trim();
    let tz = cfg.tz();
    let w = if let Ok(t) = DateTime::parse_from_rfc3339(input) {
        eco_dates::When::DateTime(t.with_timezone(&tz).naive_local())
    } else if let Ok(n) = NaiveDateTime::parse_from_str(input, "%Y-%m-%dT%H:%M") {
        eco_dates::When::DateTime(n)
    } else if let Ok(d) = chrono::NaiveDate::parse_from_str(input, "%Y-%m-%d") {
        eco_dates::When::Date(d)
    } else {
        eco_dates::parse_when(input, now.with_timezone(&tz).date_naive()).map_err(|e| e.to_string())?
    };
    let local = |n: NaiveDateTime| {
        tz.from_local_datetime(&n)
            .earliest()
            .map(|d| d.timestamp())
            .ok_or_else(|| "that time doesn't exist (clocks change)".to_string())
    };
    Ok(match w {
        eco_dates::When::Date(d) => Canon {
            iso: d.format("%Y-%m-%d").to_string(),
            label: d.format("%a %-d %b").to_string(),
            ts: local(d.and_time(NaiveTime::MIN))?,
        },
        eco_dates::When::DateTime(n) => Canon {
            iso: n.format("%Y-%m-%dT%H:%M").to_string(),
            label: n.format("%a %-d %b %H:%M").to_string(),
            ts: local(n)?,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::Europe::London;

    fn cfg() -> Config {
        Config::load(&std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config/config.test.toml"))
            .unwrap()
    }

    #[test]
    fn forms() {
        let now = London.with_ymd_and_hms(2026, 10, 14, 15, 0, 0).unwrap();
        let at = |d, h| London.with_ymd_and_hms(2026, 10, d, h, 0, 0).unwrap().timestamp();
        let c = cfg();
        assert_eq!(parse_future(&c, now, "fri 17:00"), Ok(at(16, 17)));
        assert_eq!(parse_future(&c, now, "tomorrow"), Ok(at(15, 8)), "bare date = morning");
        assert_eq!(parse_future(&c, now, "2026-10-20T09:00"), Ok(at(20, 9)));
        assert_eq!(parse_future(&c, now, "2026-10-20T09:00:00+01:00"), Ok(at(20, 9)));
        assert!(parse_future(&c, now, "wed").unwrap_err().contains("ambiguous"));
        assert_eq!(
            parse_future(&c, now, "2026-10-01T09:00"),
            Err("that's in the past".into())
        );
    }

    #[test]
    fn canonical_forms_are_what_kairos_gets() {
        let now = London.with_ymd_and_hms(2026, 10, 14, 15, 0, 0).unwrap();
        let c = cfg();
        let x = canonical(&c, now, "fri 17:00").unwrap();
        assert_eq!(
            (x.iso.as_str(), x.label.as_str()),
            ("2026-10-16T17:00", "Fri 16 Oct 17:00")
        );
        let x = canonical(&c, now, "23 oct").unwrap();
        assert_eq!((x.iso.as_str(), x.label.as_str()), ("2026-10-23", "Fri 23 Oct"));
        // An invite in UTC shows and goes as London wall time.
        let x = canonical(&c, now, "2026-10-20T13:00:00Z").unwrap();
        assert_eq!(x.iso, "2026-10-20T14:00");
        assert_eq!(
            canonical(&c, now, "2026-10-20T09:30").unwrap().label,
            "Tue 20 Oct 09:30"
        );
        assert!(canonical(&c, now, "wed").unwrap_err().contains("ambiguous"));
    }
}
