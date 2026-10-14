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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::Europe::London;

    fn cfg() -> Config {
        Config::load(&std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config/config.example.toml"))
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
}
