//! Natural-ish date/time input: `tue 19:00`, `fri 17:00`, `2026-10-23`,
//! `tomorrow 9`, `23 oct 14:30`, `today 7pm`. **Ambiguity is an error.**
//!
//! Rules:
//! - `<weekday>` is the next such day within the coming week; if today *is*
//!   that weekday it's ambiguous (today or in a week?) → error. Use `today`
//!   or `next <weekday>` (= the first one strictly after today).
//! - Bare hours `8`–`23` are 24-hour clock; bare `1`–`7` are ambiguous
//!   (morning or afternoon?) → error. Use `3pm` or `15:00`.
//! - `23 oct` / `oct 23` without a year: the next such date on/after today.
//! - Slashed dates (`10/11`) are rejected: day/month order is ambiguous.

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Weekday};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    Date(NaiveDate),
    DateTime(NaiveDateTime),
}

impl When {
    pub fn date(&self) -> NaiveDate {
        match self {
            When::Date(d) => *d,
            When::DateTime(dt) => dt.date(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DateError {
    #[error("ambiguous: {0}")]
    Ambiguous(String),
    #[error("can't understand {0:?}")]
    Unparsed(String),
    #[error("invalid date/time: {0}")]
    Invalid(String),
}

fn weekday(s: &str) -> Option<Weekday> {
    let w = match s {
        "mon" | "monday" => Weekday::Mon,
        "tue" | "tues" | "tuesday" => Weekday::Tue,
        "wed" | "weds" | "wednesday" => Weekday::Wed,
        "thu" | "thur" | "thurs" | "thursday" => Weekday::Thu,
        "fri" | "friday" => Weekday::Fri,
        "sat" | "saturday" => Weekday::Sat,
        "sun" | "sunday" => Weekday::Sun,
        _ => return None,
    };
    Some(w)
}

fn month(s: &str) -> Option<u32> {
    let m = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let s = if s == "sept" { "sep" } else { s };
    m.iter()
        .position(|x| s.starts_with(x) && (s.len() == 3 || is_full_month(s)))
        .map(|i| i as u32 + 1)
}

fn is_full_month(s: &str) -> bool {
    [
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
    ]
    .contains(&s)
}

/// Parse a time token. `Ok(None)` = not a time token.
pub fn parse_time(tok: &str) -> Result<Option<NaiveTime>, DateError> {
    let t = tok.to_ascii_lowercase();
    if t == "noon" {
        return Ok(NaiveTime::from_hms_opt(12, 0, 0));
    }
    if t == "midnight" {
        return Err(DateError::Ambiguous(
            "'midnight' (start or end of the day?) — use 00:00 with an explicit date".into(),
        ));
    }
    let (body, ampm) = if let Some(b) = t.strip_suffix("am") {
        (b, Some(false))
    } else if let Some(b) = t.strip_suffix("pm") {
        (b, Some(true))
    } else {
        (t.as_str(), None)
    };
    let (h, m) = match body.split_once([':', '.']) {
        Some((h, m)) if m.len() == 2 => (h, m),
        Some(_) => return Ok(None),
        None => (body, "00"),
    };
    if h.is_empty() || !h.chars().all(|c| c.is_ascii_digit()) || !m.chars().all(|c| c.is_ascii_digit()) || h.len() > 2 {
        return Ok(None);
    }
    let (mut hh, mm): (u32, u32) = (
        h.parse().map_err(|_| DateError::Unparsed(tok.into()))?,
        m.parse().map_err(|_| DateError::Unparsed(tok.into()))?,
    );
    match ampm {
        Some(pm) => {
            if !(1..=12).contains(&hh) {
                return Err(DateError::Invalid(tok.into()));
            }
            hh %= 12;
            if pm {
                hh += 12;
            }
        }
        None => {
            let bare = !body.contains([':', '.']);
            if bare && (1..=7).contains(&hh) {
                return Err(DateError::Ambiguous(format!(
                    "{tok:?} (morning or afternoon?) — write {hh}am, {hh}pm or {:02}:00",
                    hh + 12
                )));
            }
        }
    }
    NaiveTime::from_hms_opt(hh, mm, 0)
        .map(Some)
        .ok_or_else(|| DateError::Invalid(tok.into()))
}

/// Parse `input` relative to `today`.
pub fn parse_when(input: &str, today: NaiveDate) -> Result<When, DateError> {
    let lower = input.trim().to_ascii_lowercase();
    let toks: Vec<&str> = lower.split_whitespace().collect();
    if toks.is_empty() {
        return Err(DateError::Unparsed(input.into()));
    }
    let mut date: Option<NaiveDate> = None;
    let mut time: Option<NaiveTime> = None;
    let mut i = 0;
    let set_date = |date: &mut Option<NaiveDate>, d: NaiveDate| -> Result<(), DateError> {
        if date.is_some() {
            return Err(DateError::Ambiguous(format!("two dates in {input:?}")));
        }
        *date = Some(d);
        Ok(())
    };
    while i < toks.len() {
        let t = toks[i];
        if t == "at" || t == "on" {
            i += 1;
            continue;
        }
        if t == "today" {
            set_date(&mut date, today)?;
        } else if t == "tomorrow" || t == "tmr" || t == "tmrw" {
            set_date(&mut date, today + Duration::days(1))?;
        } else if t == "next" {
            let w = toks
                .get(i + 1)
                .and_then(|s| weekday(s))
                .ok_or_else(|| DateError::Unparsed(input.into()))?;
            let mut d = today + Duration::days(1);
            while d.weekday() != w {
                d += Duration::days(1);
            }
            set_date(&mut date, d)?;
            i += 1;
        } else if let Some(w) = weekday(t) {
            if today.weekday() == w {
                return Err(DateError::Ambiguous(format!(
                    "{t:?} is today — say 'today' or 'next {t}'"
                )));
            }
            let mut d = today + Duration::days(1);
            while d.weekday() != w {
                d += Duration::days(1);
            }
            set_date(&mut date, d)?;
        } else if t.len() == 10 && t.as_bytes().get(4) == Some(&b'-') {
            let d = NaiveDate::parse_from_str(t, "%Y-%m-%d").map_err(|_| DateError::Invalid(t.into()))?;
            set_date(&mut date, d)?;
        } else if t.contains('/') {
            return Err(DateError::Ambiguous(format!(
                "{t:?}: day/month order is ambiguous — use 2026-10-23 or '23 oct'"
            )));
        } else if let Some(m) = month(t) {
            // "oct 23"
            let day = toks
                .get(i + 1)
                .map(|s| s.trim_end_matches(|c: char| c.is_alphabetic()))
                .and_then(|s| s.parse::<u32>().ok())
                .ok_or_else(|| DateError::Unparsed(input.into()))?;
            set_date(&mut date, next_date(today, m, day)?)?;
            i += 1;
        } else if let Some(m) = toks.get(i + 1).and_then(|s| month(s))
            && let Ok(day) = t.trim_end_matches(|c: char| c.is_alphabetic()).parse::<u32>()
        {
            // "23 oct" / "23rd oct"
            set_date(&mut date, next_date(today, m, day)?)?;
            i += 1;
        } else if let Some(tm) = parse_time(t)? {
            if time.is_some() {
                return Err(DateError::Ambiguous(format!("two times in {input:?}")));
            }
            time = Some(tm);
        } else {
            return Err(DateError::Unparsed(input.into()));
        }
        i += 1;
    }
    match (date, time) {
        (Some(d), Some(t)) => Ok(When::DateTime(d.and_time(t))),
        (Some(d), None) => Ok(When::Date(d)),
        (None, Some(_)) => Err(DateError::Ambiguous(format!(
            "{input:?} has a time but no day — add 'today', 'tomorrow', a weekday or a date"
        ))),
        (None, None) => Err(DateError::Unparsed(input.into())),
    }
}

fn next_date(today: NaiveDate, m: u32, day: u32) -> Result<NaiveDate, DateError> {
    let this = NaiveDate::from_ymd_opt(today.year(), m, day);
    match this {
        Some(d) if d >= today => Ok(d),
        _ => NaiveDate::from_ymd_opt(today.year() + 1, m, day).ok_or_else(|| DateError::Invalid(format!("{day}/{m}"))),
    }
}

/// Parse a duration: `90m`, `90`, `2h`, `1h30`, `1h30m`, `1.5h`.
pub fn parse_minutes(s: &str) -> Result<u32, DateError> {
    let t = s.trim().to_ascii_lowercase().replace(' ', "");
    let err = || DateError::Unparsed(s.to_string());
    if let Some(m) = t.strip_suffix("min").or_else(|| t.strip_suffix('m'))
        && !m.contains('h')
    {
        return m.parse().map_err(|_| err());
    }
    if let Ok(n) = t.parse::<u32>() {
        return Ok(n);
    }
    let t2 = t.strip_suffix('m').unwrap_or(&t);
    if let Some((h, rest)) = t2.split_once('h') {
        let hours: f64 = h.parse().map_err(|_| err())?;
        let mins: u32 = if rest.is_empty() {
            0
        } else {
            rest.parse().map_err(|_| err())?
        };
        if hours < 0.0 || mins >= 60 {
            return Err(err());
        }
        return Ok((hours * 60.0).round() as u32 + mins);
    }
    Err(err())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Sunday 11 Oct 2026.
    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 11).unwrap()
    }
    fn dt(s: &str) -> When {
        When::DateTime(NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap())
    }

    #[test]
    fn spec_examples() {
        assert_eq!(parse_when("tue 19:00", today()).unwrap(), dt("2026-10-13 19:00"));
        assert_eq!(parse_when("fri 17:00", today()).unwrap(), dt("2026-10-16 17:00"));
        assert_eq!(
            parse_when("2026-10-23", today()).unwrap(),
            When::Date(NaiveDate::from_ymd_opt(2026, 10, 23).unwrap())
        );
        assert_eq!(parse_when("tomorrow 9", today()).unwrap(), dt("2026-10-12 09:00"));
    }

    #[test]
    fn more_forms() {
        assert_eq!(parse_when("23 oct 14:30", today()).unwrap(), dt("2026-10-23 14:30"));
        assert_eq!(parse_when("oct 23 2pm", today()).unwrap(), dt("2026-10-23 14:00"));
        assert_eq!(
            parse_when("23rd october", today()).unwrap(),
            When::Date(NaiveDate::from_ymd_opt(2026, 10, 23).unwrap())
        );
        assert_eq!(
            parse_when("1 jan", today()).unwrap(),
            When::Date(NaiveDate::from_ymd_opt(2027, 1, 1).unwrap())
        );
        assert_eq!(parse_when("today 7pm", today()).unwrap(), dt("2026-10-11 19:00"));
        assert_eq!(
            parse_when("next sun", today()).unwrap(),
            When::Date(NaiveDate::from_ymd_opt(2026, 10, 18).unwrap())
        );
        assert_eq!(
            parse_when("Wednesday at 12:15", today()).unwrap(),
            dt("2026-10-14 12:15")
        );
        assert_eq!(parse_when("tomorrow noon", today()).unwrap(), dt("2026-10-12 12:00"));
        assert_eq!(parse_when("tomorrow 12am", today()).unwrap(), dt("2026-10-12 00:00"));
    }

    #[test]
    fn ambiguity_is_an_error() {
        // Today is Sunday.
        assert!(matches!(parse_when("sun 10:00", today()), Err(DateError::Ambiguous(_))));
        assert!(matches!(
            parse_when("tomorrow 3", today()),
            Err(DateError::Ambiguous(_))
        ));
        assert!(matches!(parse_when("10/11", today()), Err(DateError::Ambiguous(_))));
        assert!(matches!(parse_when("17:00", today()), Err(DateError::Ambiguous(_))));
        assert!(matches!(
            parse_when("today tomorrow", today()),
            Err(DateError::Ambiguous(_))
        ));
        assert!(matches!(
            parse_when("tomorrow 9 10", today()),
            Err(DateError::Ambiguous(_))
        ));
        assert!(matches!(
            parse_when("tomorrow midnight", today()),
            Err(DateError::Ambiguous(_))
        ));
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(parse_when("whenever", today()).is_err());
        assert!(parse_when("2026-02-30", today()).is_err());
        assert!(parse_when("tomorrow 25:00", today()).is_err());
        assert!(parse_when("", today()).is_err());
        assert!(parse_when("31 feb", today()).is_err());
    }

    #[test]
    fn minutes() {
        assert_eq!(parse_minutes("90m").unwrap(), 90);
        assert_eq!(parse_minutes("90").unwrap(), 90);
        assert_eq!(parse_minutes("2h").unwrap(), 120);
        assert_eq!(parse_minutes("1h30").unwrap(), 90);
        assert_eq!(parse_minutes("1h30m").unwrap(), 90);
        assert_eq!(parse_minutes("1.5h").unwrap(), 90);
        assert_eq!(parse_minutes("45min").unwrap(), 45);
        assert!(parse_minutes("abc").is_err());
    }
}
