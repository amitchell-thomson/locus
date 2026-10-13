//! Building and editing `.ics` items that kairos writes.

use crate::ics::{self, Component, Property};
use crate::model::{EventKind, Meta};
use crate::time::{DtValue, format_duration};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use chrono_tz::Tz;

pub const PRODID: &str = "-//kairos//kairos 0.1//EN";
pub const DEADLINE_PREFIX: &str = "⚑ ";

/// When a new event happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewTime {
    Timed {
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    },
    /// `end` exclusive.
    AllDay {
        start: NaiveDate,
        end: NaiveDate,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewEvent {
    pub uid: String,
    pub title: String,
    pub time: NewTime,
    pub location: Option<String>,
    pub description: Option<String>,
    pub meta: Meta,
    /// Minutes before start.
    pub alarms: Vec<i64>,
    pub transparent: bool,
    /// RRULE value (e.g. `FREQ=WEEKLY;UNTIL=20261205T235959Z`).
    pub rrule: Option<String>,
}

/// The bare task title of a work block ("☐ B4 sheet" / "✓ B4 sheet" → "B4 sheet").
pub fn bare_block_title(t: &str) -> &str {
    t.trim_start_matches("☐ ")
        .trim_start_matches("✓ ")
        .trim_start_matches(['☐', '✓'])
        .trim()
}

/// Terminal boxes (Nerd Font checkboxes: same size, centred) for work blocks;
/// the stored titles keep ☐ / ✓ so the phone (no Nerd Font) shows a box too.
pub const TODO_ICON: &str = "\u{f0131}"; // nf-md-checkbox_blank_outline
pub const DONE_ICON: &str = "\u{f0135}"; // nf-md-checkbox_marked

/// A work block's title as the TUI/CLI draw it: "󰄱 B4 sheet" / "󰄵 B4 sheet".
pub fn block_label(stored: &str) -> String {
    let icon = if stored.trim_start().starts_with('✓') {
        DONE_ICON
    } else {
        TODO_ICON
    };
    format!("{icon} {}", bare_block_title(stored))
}

/// A work block still to do: "☐ B4 sheet" (the box shows on the phone too).
pub fn todo_block_title(t: &str) -> String {
    format!("☐ {}", bare_block_title(t))
}

/// A work block done (checked in): "✓ B4 sheet".
pub fn done_block_title(t: &str) -> String {
    format!("✓ {}", bare_block_title(t))
}

pub fn new_uid() -> String {
    format!("{}@kairos", uuid::Uuid::new_v4())
}

/// Static VTIMEZONE for Europe/London (the only zone kairos writes in; other
/// display zones are written as UTC).
fn london_vtimezone() -> Component {
    let mut tz = Component::new("VTIMEZONE");
    tz.push(Property::new("TZID", "Europe/London"));
    let mut d = Component::new("DAYLIGHT");
    for (k, v) in [
        ("TZOFFSETFROM", "+0000"),
        ("TZOFFSETTO", "+0100"),
        ("TZNAME", "BST"),
        ("DTSTART", "19810329T010000"),
        ("RRULE", "FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU"),
    ] {
        d.push(Property::new(k, v));
    }
    let mut s = Component::new("STANDARD");
    for (k, v) in [
        ("TZOFFSETFROM", "+0100"),
        ("TZOFFSETTO", "+0000"),
        ("TZNAME", "GMT"),
        ("DTSTART", "19961027T020000"),
        ("RRULE", "FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU"),
    ] {
        s.push(Property::new(k, v));
    }
    tz.children = vec![d, s];
    tz
}

fn dt_value(t: DateTime<Utc>, tz: Tz) -> DtValue {
    if tz == chrono_tz::Europe::London {
        DtValue::Zoned(t.with_timezone(&tz).naive_local(), tz)
    } else {
        DtValue::Utc(t.naive_utc())
    }
}

fn stamp(now: DateTime<Utc>) -> Property {
    Property::new("DTSTAMP", &now.format("%Y%m%dT%H%M%SZ").to_string())
}

fn alarm(minutes_before: i64, title: &str) -> Component {
    let mut a = Component::new("VALARM");
    a.push(Property::new("ACTION", "DISPLAY"));
    a.push(Property::text("DESCRIPTION", title));
    a.push(Property::new(
        "TRIGGER",
        &format_duration(-Duration::minutes(minutes_before)),
    ));
    a
}

/// Set DTSTART/DTEND on a VEVENT.
pub fn set_time(ev: &mut Component, time: NewTime, tz: Tz) {
    ev.remove("DURATION");
    match time {
        NewTime::Timed { start, end } => {
            ev.set(dt_value(start, tz).to_prop("DTSTART"));
            ev.set(dt_value(end.max(start), tz).to_prop("DTEND"));
        }
        NewTime::AllDay { start, end } => {
            ev.set(DtValue::Date(start).to_prop("DTSTART"));
            ev.set(DtValue::Date(end.max(start.succ_opt().unwrap_or(start))).to_prop("DTEND"));
        }
    }
}

/// A whole `.ics` file for a new event.
pub fn event_file(e: &NewEvent, tz: Tz, now: DateTime<Utc>) -> String {
    let mut cal = Component::new("VCALENDAR");
    cal.push(Property::new("VERSION", "2.0"));
    cal.push(Property::new("PRODID", PRODID));
    cal.push(Property::new("CALSCALE", "GREGORIAN"));
    if tz == chrono_tz::Europe::London && matches!(e.time, NewTime::Timed { .. }) {
        cal.children.push(london_vtimezone());
    }
    let mut ev = Component::new("VEVENT");
    ev.push(Property::text("UID", &e.uid));
    ev.push(stamp(now));
    ev.push(Property::new("CREATED", &now.format("%Y%m%dT%H%M%SZ").to_string()));
    ev.push(Property::new("SEQUENCE", "0"));
    set_time(&mut ev, e.time, tz);
    if let Some(r) = &e.rrule {
        ev.push(Property::new("RRULE", r));
    }
    ev.push(Property::text("SUMMARY", &e.title));
    if let Some(l) = &e.location {
        ev.push(Property::text("LOCATION", l));
    }
    if let Some(d) = &e.description {
        ev.push(Property::text("DESCRIPTION", d));
    }
    if e.transparent {
        ev.push(Property::new("TRANSP", "TRANSPARENT"));
    }
    e.meta.apply_to(&mut ev);
    for m in &e.alarms {
        ev.children.push(alarm(*m, &e.title));
    }
    cal.children.push(ev);
    cal.to_ics()
}

/// A deadline: `⚑ title`, transparent, all-day (date) or zero-duration (timed)
/// with alarms.
pub fn deadline(uid: &str, title: &str, due: crate::task::Due, meta: Meta, alarms: &[i64]) -> NewEvent {
    let title = if title.starts_with(DEADLINE_PREFIX.trim()) {
        title.to_string()
    } else {
        format!("{DEADLINE_PREFIX}{title}")
    };
    let (time, alarms) = match due {
        crate::task::Due::Date(d) => (
            NewTime::AllDay {
                start: d,
                end: d.succ_opt().unwrap_or(d),
            },
            Vec::new(),
        ),
        crate::task::Due::At(t) => (NewTime::Timed { start: t, end: t }, alarms.to_vec()),
    };
    NewEvent {
        uid: uid.to_string(),
        title,
        time,
        location: None,
        description: None,
        meta: Meta {
            kind: EventKind::Deadline,
            ..meta
        },
        alarms,
        transparent: true,
        rrule: None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EditError {
    #[error("not valid iCalendar: {0}")]
    Parse(#[from] ics::IcsError),
    #[error("no VEVENT with UID {0}")]
    NoEvent(String),
}

/// What to change on an existing event (master VEVENT only).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Edit {
    pub title: Option<String>,
    pub time: Option<NewTime>,
    pub location: Option<Option<String>>,
    pub description: Option<Option<String>>,
    pub meta: Option<Meta>,
}

/// Apply an edit to a stored `.ics` file, bumping SEQUENCE and DTSTAMP and
/// keeping everything else (X-APPLE-*, alarms, attendees) intact.
pub fn edit_file(text: &str, uid: &str, edit: &Edit, tz: Tz, now: DateTime<Utc>) -> Result<String, EditError> {
    let mut cals = ics::parse(text)?;
    let mut found = false;
    for cal in &mut cals {
        let needs_tz = matches!(edit.time, Some(NewTime::Timed { .. }))
            && tz == chrono_tz::Europe::London
            && !cal
                .children_named("VTIMEZONE")
                .any(|t| t.value("TZID") == Some("Europe/London"));
        if needs_tz {
            cal.children.insert(0, london_vtimezone());
        }
        for ev in cal.children.iter_mut().filter(|c| c.name == "VEVENT") {
            if ev.text("UID").as_deref() != Some(uid) || ev.prop("RECURRENCE-ID").is_some() {
                continue;
            }
            found = true;
            if let Some(t) = &edit.title {
                ev.set(Property::text("SUMMARY", t));
            }
            if let Some(t) = edit.time {
                set_time(ev, t, tz);
            }
            match &edit.location {
                Some(Some(l)) => ev.set(Property::text("LOCATION", l)),
                Some(None) => ev.remove("LOCATION"),
                None => {}
            }
            match &edit.description {
                Some(Some(d)) => ev.set(Property::text("DESCRIPTION", d)),
                Some(None) => ev.remove("DESCRIPTION"),
                None => {}
            }
            if let Some(m) = &edit.meta {
                m.apply_to(ev);
            }
            let seq: i64 = ev.value("SEQUENCE").and_then(|s| s.trim().parse().ok()).unwrap_or(0);
            ev.set(Property::new("SEQUENCE", &(seq + 1).to_string()));
            ev.set(stamp(now));
            ev.set(Property::new(
                "LAST-MODIFIED",
                &now.format("%Y%m%dT%H%M%SZ").to_string(),
            ));
        }
    }
    if !found {
        return Err(EditError::NoEvent(uid.to_string()));
    }
    Ok(cals.iter().map(Component::to_ics).collect())
}

/// Apply `meta` to every VEVENT (master + overrides) of a file — used when
/// moving between calendars or sharing to Family.
pub fn set_meta_file(text: &str, meta: &Meta) -> Result<String, EditError> {
    let mut cals = ics::parse(text)?;
    for cal in &mut cals {
        for ev in cal.children.iter_mut().filter(|c| c.name == "VEVENT") {
            meta.apply_to(ev);
        }
    }
    Ok(cals.iter().map(Component::to_ics).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::normalize::series_from_calendars;
    use chrono::TimeZone;
    use chrono_tz::Europe::London;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 12, 9, 0, 0).unwrap()
    }

    fn reparse(s: &str) -> crate::model::EventSeries {
        let cals = ics::parse(s).unwrap();
        let mut w = Vec::new();
        let mut v = series_from_calendars(&cals, "x", None, &mut w);
        assert!(w.is_empty(), "{w:?}");
        v.remove(0)
    }

    #[test]
    fn timed_event_round_trip() {
        let e = NewEvent {
            uid: "u1@kairos".into(),
            title: "Coffee, with Sam".into(),
            time: NewTime::Timed {
                start: London
                    .with_ymd_and_hms(2026, 10, 25, 10, 0, 0)
                    .unwrap()
                    .with_timezone(&Utc),
                end: London
                    .with_ymd_and_hms(2026, 10, 25, 11, 0, 0)
                    .unwrap()
                    .with_timezone(&Utc),
            },
            location: Some("Missing Bean".into()),
            description: None,
            rrule: None,
            meta: Meta {
                created_by: Some(crate::model::Author::Claude),
                ..Default::default()
            },
            alarms: vec![15],
            transparent: false,
        };
        let s = event_file(&e, London, now());
        assert!(s.contains("DTSTART;TZID=Europe/London:20261025T100000"));
        assert!(s.contains("BEGIN:VTIMEZONE"));
        let m = reparse(&s).master.unwrap();
        assert_eq!(m.summary, "Coffee, with Sam");
        assert_eq!(m.alarms, vec![15]);
        assert_eq!(m.meta.created_by, Some(crate::model::Author::Claude));
    }

    #[test]
    fn timed_deadline_is_zero_duration_with_alarms() {
        let t = London
            .with_ymd_and_hms(2026, 10, 16, 17, 0, 0)
            .unwrap()
            .with_timezone(&Utc);
        let d = deadline(
            "d@kairos",
            "B4 sheet",
            crate::task::Due::At(t),
            Meta::default(),
            &[1440, 120],
        );
        let m = reparse(&event_file(&d, London, now())).master.unwrap();
        assert_eq!(m.summary, "⚑ B4 sheet");
        assert_eq!(m.start, m.end);
        assert_eq!(m.alarms, vec![1440, 120]);
        assert!(m.transparent);
        assert_eq!(m.meta.kind, EventKind::Deadline);
    }

    #[test]
    fn date_deadline_is_all_day() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 23).unwrap();
        let d = deadline(
            "d@kairos",
            "⚑ Lab",
            crate::task::Due::Date(day),
            Meta::default(),
            &[1440],
        );
        let m = reparse(&event_file(&d, London, now())).master.unwrap();
        assert_eq!(m.summary, "⚑ Lab", "no double flag");
        assert_eq!(m.start, DtValue::Date(day));
        assert_eq!(m.end, DtValue::Date(day.succ_opt().unwrap()));
    }

    #[test]
    fn edit_keeps_unknown_properties_and_bumps_sequence() {
        let src = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:a\r\nSEQUENCE:3\r\nDTSTART:20261020T100000Z\r\nDTEND:20261020T110000Z\r\nSUMMARY:Old\r\nX-APPLE-TRAVEL-ADVISORY-BEHAVIOR:AUTOMATIC\r\nBEGIN:VALARM\r\nTRIGGER:-PT10M\r\nACTION:DISPLAY\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let edit = Edit {
            title: Some("New".into()),
            ..Default::default()
        };
        let out = edit_file(src, "a", &edit, London, now()).unwrap();
        assert!(out.contains("X-APPLE-TRAVEL-ADVISORY-BEHAVIOR:AUTOMATIC"));
        assert!(out.contains("SEQUENCE:4"));
        let m = reparse(&out).master.unwrap();
        assert_eq!(m.summary, "New");
        assert_eq!(m.alarms, vec![10]);
        assert!(matches!(
            edit_file(src, "zzz", &edit, London, now()),
            Err(EditError::NoEvent(_))
        ));
    }
}
