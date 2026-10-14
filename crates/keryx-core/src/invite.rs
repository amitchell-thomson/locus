//! Calendar invites in mail (`text/calendar`, iTIP REQUEST / CANCEL),
//! shown as a card and turned into kairos events (SPEC §7). Times are
//! converted to the display zone (Europe/London).

use crate::model::Address;
use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone};
use chrono_tz::Tz;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Request,
    Cancel,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum When {
    Timed(DateTime<Tz>),
    AllDay(NaiveDate),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invite {
    pub method: Method,
    pub uid: String,
    pub summary: String,
    pub start: When,
    pub end: Option<When>,
    pub location: Option<String>,
    pub organizer: Option<Address>,
    pub cancelled: bool,
    /// Set when the event's time zone wasn't recognised (assumed `display`).
    pub tz_note: Option<String>,
}

/// Outlook's Windows zone names that show up in TZID, mapped to IANA.
const WINDOWS_ZONES: &[(&str, &str)] = &[
    ("GMT Standard Time", "Europe/London"),
    ("Greenwich Standard Time", "Etc/GMT"),
    ("W. Europe Standard Time", "Europe/Berlin"),
    ("Romance Standard Time", "Europe/Paris"),
    ("Central Europe Standard Time", "Europe/Budapest"),
    ("Central European Standard Time", "Europe/Warsaw"),
    ("E. Europe Standard Time", "Europe/Bucharest"),
    ("FLE Standard Time", "Europe/Kiev"),
    ("Eastern Standard Time", "America/New_York"),
    ("Central Standard Time", "America/Chicago"),
    ("Mountain Standard Time", "America/Denver"),
    ("Pacific Standard Time", "America/Los_Angeles"),
    ("China Standard Time", "Asia/Shanghai"),
    ("Tokyo Standard Time", "Asia/Tokyo"),
    ("India Standard Time", "Asia/Kolkata"),
    ("UTC", "UTC"),
    ("Coordinated Universal Time", "UTC"),
];

fn zone(tzid: &str) -> Option<Tz> {
    let t = tzid.trim().trim_matches('"');
    t.parse::<Tz>()
        .ok()
        .or_else(|| {
            WINDOWS_ZONES
                .iter()
                .find(|(w, _)| *w == t)
                .and_then(|(_, i)| i.parse().ok())
        })
        // Some senders use "/mozilla.org/20070129_1/Europe/London" style ids.
        .or_else(|| {
            let parts: Vec<&str> = t.rsplitn(3, '/').collect();
            (parts.len() >= 2)
                .then(|| format!("{}/{}", parts[1], parts[0]))
                .and_then(|s| s.parse().ok())
        })
}

/// Unfold content lines (RFC 5545 §3.1).
fn unfold(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in s.split('\n') {
        let line = line.trim_end_matches('\r');
        if let Some(cont) = line.strip_prefix(' ').or_else(|| line.strip_prefix('\t'))
            && let Some(last) = out.last_mut()
        {
            last.push_str(cont);
            continue;
        }
        out.push(line.to_string());
    }
    out
}

fn unescape(v: &str) -> String {
    let mut out = String::new();
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// A content line: (name, parameters, value).
type Prop = (String, Vec<(String, String)>, String);

/// `NAME;PARAM=x;PARAM2=y:value` → (name, params, value).
fn split_prop(line: &str) -> Option<Prop> {
    // The value starts at the first ':' outside quotes.
    let mut in_q = false;
    let idx = line.char_indices().find(|&(_, c)| {
        if c == '"' {
            in_q = !in_q;
        }
        c == ':' && !in_q
    })?;
    let (head, value) = (&line[..idx.0], &line[idx.0 + 1..]);
    let mut parts = head.split(';');
    let name = parts.next()?.to_ascii_uppercase();
    let params = parts
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (k.to_ascii_uppercase(), v.trim_matches('"').to_string()))
        .collect();
    Some((name, params, value.to_string()))
}

fn when(params: &[(String, String)], value: &str, display: Tz, note: &mut Option<String>) -> Option<When> {
    let v = value.trim();
    let is_date = params
        .iter()
        .any(|(k, v)| k == "VALUE" && v.eq_ignore_ascii_case("DATE"))
        || v.len() == 8;
    if is_date {
        return NaiveDate::parse_from_str(v, "%Y%m%d").ok().map(When::AllDay);
    }
    if let Some(utc) = v.strip_suffix('Z') {
        let naive = NaiveDateTime::parse_from_str(utc, "%Y%m%dT%H%M%S").ok()?;
        return Some(When::Timed(
            chrono::Utc.from_utc_datetime(&naive).with_timezone(&display),
        ));
    }
    let naive = NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S").ok()?;
    let tz = match params.iter().find(|(k, _)| k == "TZID") {
        Some((_, id)) => zone(id).unwrap_or_else(|| {
            *note = Some(format!("unknown time zone {id:?}; times assume {display}"));
            display
        }),
        None => display, // floating time
    };
    // Ambiguous local times take the first occurrence; non-existent ones the
    // pre-gap offset (as kairos, RFC 5545 §3.3.5).
    let local = tz.from_local_datetime(&naive).earliest().or_else(|| {
        tz.from_local_datetime(&(naive - chrono::Duration::hours(1)))
            .earliest()
            .map(|d| d + chrono::Duration::hours(1))
    })?;
    Some(When::Timed(local.with_timezone(&display)))
}

/// The first VEVENT of a `text/calendar` part.
pub fn parse(ics: &str, display: Tz) -> Option<Invite> {
    let lines = unfold(ics);
    let mut method = Method::Other;
    let mut in_event = false;
    let mut nested = 0;
    let mut inv = Invite {
        method: Method::Other,
        uid: String::new(),
        summary: String::new(),
        start: When::AllDay(NaiveDate::MIN),
        end: None,
        location: None,
        organizer: None,
        cancelled: false,
        tz_note: None,
    };
    let mut have_start = false;
    for line in &lines {
        let Some((name, params, value)) = split_prop(line) else {
            continue;
        };
        match (name.as_str(), in_event) {
            ("METHOD", false) => {
                method = match value.trim().to_ascii_uppercase().as_str() {
                    "REQUEST" => Method::Request,
                    "CANCEL" => Method::Cancel,
                    _ => Method::Other,
                }
            }
            ("BEGIN", _) if value.trim().eq_ignore_ascii_case("VEVENT") && !have_start => in_event = true,
            ("BEGIN", true) => nested += 1,
            ("END", true) if nested > 0 => nested -= 1,
            ("END", true) if value.trim().eq_ignore_ascii_case("VEVENT") => in_event = false,
            (_, true) if nested == 0 => match name.as_str() {
                "UID" => inv.uid = value.trim().to_string(),
                "SUMMARY" => inv.summary = crate::text::sanitize(&unescape(&value)),
                "LOCATION" => {
                    let l = crate::text::sanitize(&unescape(&value));
                    inv.location = (!l.trim().is_empty()).then_some(l);
                }
                "STATUS" => inv.cancelled = value.trim().eq_ignore_ascii_case("CANCELLED"),
                "DTSTART" => {
                    if let Some(w) = when(&params, &value, display, &mut inv.tz_note) {
                        inv.start = w;
                        have_start = true;
                    }
                }
                "DTEND" => inv.end = when(&params, &value, display, &mut inv.tz_note),
                "ORGANIZER" => {
                    let email = value.trim();
                    let email = email
                        .strip_prefix("mailto:")
                        .or_else(|| email.strip_prefix("MAILTO:"))
                        .unwrap_or(email);
                    let cn = params.iter().find(|(k, _)| k == "CN").map(|(_, v)| v.as_str());
                    inv.organizer = Some(Address::new(cn, email));
                }
                _ => {}
            },
            _ => {}
        }
    }
    if !have_start {
        return None;
    }
    inv.method = method;
    if method == Method::Cancel {
        inv.cancelled = true;
    }
    Some(inv)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::Europe::London;

    fn fixture(name: &str) -> Invite {
        let path = format!("{}/../../fixtures/eml/{name}", env!("CARGO_MANIFEST_DIR"));
        let p = crate::mime::parse(&std::fs::read(path).unwrap()).unwrap();
        parse(&p.calendars[0], London).unwrap()
    }

    fn london(s: &str) -> When {
        When::Timed(
            London
                .from_local_datetime(&NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap())
                .unwrap(),
        )
    }

    #[test]
    fn request_in_vienna_time_shows_in_london() {
        let i = fixture("11-invite-request.eml");
        assert_eq!(i.method, Method::Request);
        assert!(!i.cancelled);
        assert_eq!(i.summary, "Project meeting");
        assert_eq!(i.location.as_deref(), Some("Thom Building, Room 2"));
        // 15:00 Vienna (CEST) = 14:00 London (BST) on 22 Oct 2026.
        assert_eq!(i.start, london("2026-10-22 14:00"));
        assert_eq!(i.end, Some(london("2026-10-22 15:00")));
        assert_eq!(i.organizer.unwrap().display(), "Prof Smith <smith@eng.example.ac.uk>");
        assert_eq!(i.uid, "meeting-11@eng.example.ac.uk");
        assert!(i.tz_note.is_none());
    }

    #[test]
    fn cancel() {
        let i = fixture("12-invite-cancel.eml");
        assert_eq!(i.method, Method::Cancel);
        assert!(i.cancelled);
    }

    #[test]
    fn utc_all_day_windows_zones_and_dst() {
        let ics = |dtstart: &str| {
            format!(
                "BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\nUID:u\r\nSUMMARY:S\r\n{dtstart}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
            )
        };
        // After the clocks go back (25 Oct 2026): 14:00Z = 14:00 GMT.
        assert_eq!(
            parse(&ics("DTSTART:20261026T140000Z"), London).unwrap().start,
            london("2026-10-26 14:00")
        );
        assert_eq!(
            parse(&ics("DTSTART:20261021T140000Z"), London).unwrap().start,
            london("2026-10-21 15:00")
        );
        assert_eq!(
            parse(&ics("DTSTART;VALUE=DATE:20261023"), London).unwrap().start,
            When::AllDay(NaiveDate::from_ymd_opt(2026, 10, 23).unwrap())
        );
        assert_eq!(
            parse(&ics("DTSTART;TZID=\"Eastern Standard Time\":20261021T090000"), London)
                .unwrap()
                .start,
            london("2026-10-21 14:00")
        );
        let odd = parse(&ics("DTSTART;TZID=Mars/Olympus:20261021T090000"), London).unwrap();
        assert_eq!(odd.start, london("2026-10-21 09:00"));
        assert!(odd.tz_note.unwrap().contains("Mars/Olympus"));
        // Ambiguous 01:30 on 25 Oct 2026 in London: the first (BST) occurrence.
        let amb = parse(&ics("DTSTART;TZID=Europe/London:20261025T013000"), London).unwrap();
        let When::Timed(t) = amb.start else { panic!() };
        assert_eq!(t.to_rfc3339(), "2026-10-25T01:30:00+01:00");
    }

    #[test]
    fn folded_lines_and_escapes() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nSUMMARY:A very long\r\n  title\\, folded\r\nDTSTART:20261021T140000Z\r\nLOCATION:Room 1\\nFloor 2\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let i = parse(ics, London).unwrap();
        assert_eq!(i.summary, "A very long title, folded");
        assert_eq!(i.location.as_deref(), Some("Room 1\nFloor 2"));
        assert_eq!(i.method, Method::Other);
    }

    #[test]
    fn nested_alarm_doesnt_override_event_fields() {
        let ics = "BEGIN:VEVENT\nSUMMARY:Real\nDTSTART:20261021T140000Z\nBEGIN:VALARM\nSUMMARY:Alarm\nEND:VALARM\nEND:VEVENT\n";
        assert_eq!(parse(ics, London).unwrap().summary, "Real");
    }

    #[test]
    fn no_event_is_none() {
        assert!(parse("BEGIN:VCALENDAR\nEND:VCALENDAR\n", London).is_none());
    }
}
