//! iCalendar date/time values and their resolution to instants.
//!
//! **Ambiguous / non-existent local times** follow RFC 5545 §3.3.5 exactly:
//! - a local time that occurs twice (clocks go back, e.g. 01:30 on
//!   25 Oct 2026 in Europe/London) means the **first** occurrence (BST, +01:00);
//! - a local time that does not occur (clocks go forward, e.g. 01:30 on
//!   28 Mar 2027) is interpreted with the UTC offset **before** the gap
//!   (GMT, +00:00), i.e. it lands at 02:30 BST.

use crate::ics::Property;
use chrono::{DateTime, Duration, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, Offset, TimeZone, Utc};
use chrono_tz::Tz;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DtValue {
    Date(NaiveDate),
    Utc(NaiveDateTime),
    Zoned(NaiveDateTime, Tz),
    /// No zone: interpreted in the display timezone.
    Floating(NaiveDateTime),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TimeError {
    #[error("bad date/time value {0:?}")]
    Bad(String),
    #[error("bad duration {0:?}")]
    Duration(String),
}

impl DtValue {
    pub fn is_date(&self) -> bool {
        matches!(self, DtValue::Date(_))
    }

    /// Parse a DTSTART/DTEND/EXDATE/RECURRENCE-ID-style property. Unknown TZIDs
    /// degrade to floating (display zone) rather than failing.
    pub fn from_prop(p: &Property) -> Result<DtValue, TimeError> {
        Self::parse(&p.value, p.param("VALUE"), p.param("TZID"))
    }

    pub fn parse(value: &str, value_type: Option<&str>, tzid: Option<&str>) -> Result<DtValue, TimeError> {
        let v = value.trim();
        if value_type.is_some_and(|t| t.eq_ignore_ascii_case("DATE")) || v.len() == 8 {
            return parse_date(v).map(DtValue::Date);
        }
        if let Some(stripped) = v.strip_suffix(['Z', 'z']) {
            return parse_naive(stripped).map(DtValue::Utc);
        }
        let n = parse_naive(v)?;
        match tzid.and_then(resolve_tzid) {
            Some(tz) => Ok(DtValue::Zoned(n, tz)),
            None => Ok(DtValue::Floating(n)),
        }
    }

    /// Wall-clock value in the value's own zone (dates at midnight).
    pub fn naive(&self) -> NaiveDateTime {
        match *self {
            DtValue::Date(d) => d.and_time(NaiveTime::MIN),
            DtValue::Utc(n) | DtValue::Zoned(n, _) | DtValue::Floating(n) => n,
        }
    }

    /// The zone wall-clock arithmetic happens in (`display` for floating/date).
    pub fn zone(&self, display: Tz) -> Option<Tz> {
        match *self {
            DtValue::Zoned(_, tz) => Some(tz),
            DtValue::Utc(_) => None,
            DtValue::Floating(_) | DtValue::Date(_) => Some(display),
        }
    }

    /// Absolute instant; dates resolve to local midnight in `display`.
    pub fn instant(&self, display: Tz) -> DateTime<Utc> {
        match *self {
            DtValue::Utc(n) => Utc.from_utc_datetime(&n),
            DtValue::Zoned(n, tz) => resolve_local(n, tz),
            DtValue::Floating(n) => resolve_local(n, display),
            DtValue::Date(d) => resolve_local(d.and_time(NaiveTime::MIN), display),
        }
    }

    /// Same kind of value, shifted to a new wall-clock time.
    pub fn with_naive(&self, n: NaiveDateTime) -> DtValue {
        match *self {
            DtValue::Date(_) => DtValue::Date(n.date()),
            DtValue::Utc(_) => DtValue::Utc(n),
            DtValue::Zoned(_, tz) => DtValue::Zoned(n, tz),
            DtValue::Floating(_) => DtValue::Floating(n),
        }
    }

    /// Render as a property value plus optional TZID / VALUE params.
    pub fn to_prop(&self, name: &str) -> Property {
        match *self {
            DtValue::Date(d) => Property::new(name, &d.format("%Y%m%d").to_string()).with_param("VALUE", "DATE"),
            DtValue::Utc(n) => Property::new(name, &n.format("%Y%m%dT%H%M%SZ").to_string()),
            DtValue::Zoned(n, tz) => {
                Property::new(name, &n.format("%Y%m%dT%H%M%S").to_string()).with_param("TZID", tz.name())
            }
            DtValue::Floating(n) => Property::new(name, &n.format("%Y%m%dT%H%M%S").to_string()),
        }
    }
}

/// RFC 5545 §3.3.5 resolution (see module docs).
pub fn resolve_local(n: NaiveDateTime, tz: Tz) -> DateTime<Utc> {
    match tz.from_local_datetime(&n) {
        LocalResult::Single(t) => t.with_timezone(&Utc),
        LocalResult::Ambiguous(first, _) => first.with_timezone(&Utc),
        LocalResult::None => {
            // Offset in force just before the gap.
            let before = n - Duration::hours(3);
            let off = match tz.from_local_datetime(&before) {
                LocalResult::Single(t) | LocalResult::Ambiguous(t, _) => t.offset().fix(),
                LocalResult::None => tz.offset_from_utc_datetime(&n).fix(),
            };
            Utc.from_utc_datetime(&(n - Duration::seconds(i64::from(off.local_minus_utc()))))
        }
    }
}

fn parse_date(v: &str) -> Result<NaiveDate, TimeError> {
    NaiveDate::parse_from_str(v.get(..8).unwrap_or(v), "%Y%m%d").map_err(|_| TimeError::Bad(v.to_string()))
}

fn parse_naive(v: &str) -> Result<NaiveDateTime, TimeError> {
    NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S")
        .or_else(|_| NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M"))
        .map_err(|_| TimeError::Bad(v.to_string()))
}

/// Map a TZID to an IANA zone, accepting common non-IANA spellings
/// (leading `/`, Outlook/Windows names, `GMT`).
pub fn resolve_tzid(tzid: &str) -> Option<Tz> {
    let t = tzid.trim().trim_matches('"');
    let t = t.trim_start_matches('/');
    if let Ok(tz) = t.parse::<Tz>() {
        return Some(tz);
    }
    // e.g. "/mozilla.org/20050126_1/Europe/London"
    for (j, _) in t.match_indices('/') {
        if let Ok(tz) = t[j + 1..].parse::<Tz>() {
            return Some(tz);
        }
    }
    let windows = [
        ("GMT Standard Time", "Europe/London"),
        ("Greenwich Standard Time", "Atlantic/Reykjavik"),
        ("W. Europe Standard Time", "Europe/Berlin"),
        ("Central Europe Standard Time", "Europe/Budapest"),
        ("Central European Standard Time", "Europe/Warsaw"),
        ("Romance Standard Time", "Europe/Paris"),
        ("E. Europe Standard Time", "Europe/Chisinau"),
        ("FLE Standard Time", "Europe/Kiev"),
        ("GTB Standard Time", "Europe/Bucharest"),
        ("Eastern Standard Time", "America/New_York"),
        ("Central Standard Time", "America/Chicago"),
        ("Mountain Standard Time", "America/Denver"),
        ("Pacific Standard Time", "America/Los_Angeles"),
        ("China Standard Time", "Asia/Shanghai"),
        ("Tokyo Standard Time", "Asia/Tokyo"),
        ("Singapore Standard Time", "Asia/Singapore"),
        ("India Standard Time", "Asia/Kolkata"),
        ("AUS Eastern Standard Time", "Australia/Sydney"),
        ("UTC", "UTC"),
        ("Coordinated Universal Time", "UTC"),
        ("GMT", "Europe/London"),
    ];
    windows
        .iter()
        .find(|(w, _)| w.eq_ignore_ascii_case(t))
        .and_then(|(_, iana)| iana.parse().ok())
}

/// Parse an RFC 5545 DURATION (`P1D`, `-PT15M`, `P1W`, `PT1H30M`).
pub fn parse_duration(s: &str) -> Result<Duration, TimeError> {
    let err = || TimeError::Duration(s.to_string());
    let mut t = s.trim();
    let mut sign = 1;
    if let Some(r) = t.strip_prefix('-') {
        sign = -1;
        t = r;
    } else if let Some(r) = t.strip_prefix('+') {
        t = r;
    }
    let t = t.strip_prefix('P').ok_or_else(err)?;
    let mut total = Duration::zero();
    let mut num = String::new();
    let mut in_time = false;
    let mut any = false;
    for c in t.chars() {
        match c {
            'T' => in_time = true,
            '0'..='9' => num.push(c),
            u => {
                let n: i64 = num.parse().map_err(|_| err())?;
                num.clear();
                any = true;
                total += match (u, in_time) {
                    ('W', false) => Duration::weeks(n),
                    ('D', false) => Duration::days(n),
                    ('H', true) => Duration::hours(n),
                    ('M', true) => Duration::minutes(n),
                    ('S', true) => Duration::seconds(n),
                    _ => return Err(err()),
                };
            }
        }
    }
    if !num.is_empty() || !any {
        return Err(err());
    }
    Ok(total * sign)
}

pub fn format_duration(d: Duration) -> String {
    let neg = d < Duration::zero();
    let mut secs = d.num_seconds().abs();
    let mut s = String::from(if neg { "-P" } else { "P" });
    let days = secs / 86_400;
    secs %= 86_400;
    if days > 0 {
        s.push_str(&format!("{days}D"));
    }
    if secs > 0 || days == 0 {
        s.push('T');
        let (h, m, sec) = (secs / 3600, (secs % 3600) / 60, secs % 60);
        if h > 0 {
            s.push_str(&format!("{h}H"));
        }
        if m > 0 {
            s.push_str(&format!("{m}M"));
        }
        if sec > 0 || (h == 0 && m == 0) {
            s.push_str(&format!("{sec}S"));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::Europe::London;

    fn n(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }

    #[test]
    fn ambiguous_time_takes_first_occurrence() {
        // 25 Oct 2026, clocks go back 02:00 BST -> 01:00 GMT; 01:30 happens twice.
        let t = resolve_local(n("2026-10-25 01:30"), London);
        assert_eq!(
            t,
            Utc.from_utc_datetime(&n("2026-10-25 00:30")),
            "first (BST) occurrence"
        );
    }

    #[test]
    fn nonexistent_time_uses_offset_before_gap() {
        // 28 Mar 2027, clocks go forward 01:00 GMT -> 02:00 BST; 01:30 never happens.
        let t = resolve_local(n("2027-03-28 01:30"), London);
        assert_eq!(t, Utc.from_utc_datetime(&n("2027-03-28 01:30")));
        assert_eq!(t.with_timezone(&London).format("%H:%M").to_string(), "02:30");
    }

    #[test]
    fn parse_variants() {
        assert_eq!(
            DtValue::parse("20261023", Some("DATE"), None).unwrap(),
            DtValue::Date(NaiveDate::from_ymd_opt(2026, 10, 23).unwrap())
        );
        assert_eq!(
            DtValue::parse("20261023T120000Z", None, None).unwrap(),
            DtValue::Utc(n("2026-10-23 12:00"))
        );
        assert_eq!(
            DtValue::parse("20261023T120000", None, Some("Europe/Vienna")).unwrap(),
            DtValue::Zoned(n("2026-10-23 12:00"), chrono_tz::Europe::Vienna)
        );
        assert_eq!(
            DtValue::parse("20261023T120000", None, None).unwrap(),
            DtValue::Floating(n("2026-10-23 12:00"))
        );
        assert_eq!(
            DtValue::parse("20261023T120000", None, Some("GMT Standard Time")).unwrap(),
            DtValue::Zoned(n("2026-10-23 12:00"), London)
        );
        assert!(DtValue::parse("2026-10-23", None, None).is_err());
    }

    #[test]
    fn tzid_variants() {
        assert_eq!(resolve_tzid("/Europe/London"), Some(London));
        assert_eq!(resolve_tzid("/mozilla.org/20050126_1/Europe/London"), Some(London));
        assert_eq!(resolve_tzid("Nowhere/Special"), None);
    }

    #[test]
    fn vienna_in_london_time() {
        let v = DtValue::Zoned(n("2026-10-23 12:00"), chrono_tz::Europe::Vienna);
        let local = v.instant(London).with_timezone(&London);
        assert_eq!(local.format("%H:%M").to_string(), "11:00");
    }

    #[test]
    fn floating_is_display_zone() {
        let v = DtValue::Floating(n("2026-07-01 09:00"));
        assert_eq!(
            v.instant(London).with_timezone(&London).format("%H:%M").to_string(),
            "09:00"
        );
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("PT1H30M").unwrap(), Duration::minutes(90));
        assert_eq!(parse_duration("-PT15M").unwrap(), Duration::minutes(-15));
        assert_eq!(parse_duration("P1D").unwrap(), Duration::days(1));
        assert_eq!(parse_duration("P2W").unwrap(), Duration::weeks(2));
        assert_eq!(parse_duration("P1DT2H").unwrap(), Duration::hours(26));
        assert!(parse_duration("PT").is_err());
        assert!(parse_duration("1H").is_err());
        for m in [-1440, -120, 0, 15, 90, 1500] {
            let d = Duration::minutes(m);
            assert_eq!(parse_duration(&format_duration(d)).unwrap(), d);
        }
    }

    #[test]
    fn to_prop_round_trip() {
        for v in [
            DtValue::Date(NaiveDate::from_ymd_opt(2026, 10, 23).unwrap()),
            DtValue::Utc(n("2026-10-23 12:00")),
            DtValue::Zoned(n("2026-10-23 12:00"), London),
            DtValue::Floating(n("2026-10-23 12:00")),
        ] {
            assert_eq!(DtValue::from_prop(&v.to_prop("DTSTART")).unwrap(), v);
        }
    }
}
