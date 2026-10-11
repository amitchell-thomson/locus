//! Parsed iCalendar components → [`EventSeries`] (grouped by UID).

use crate::ics::Component;
use crate::model::{EventData, EventSeries, Meta};
use crate::time::{DtValue, parse_duration};
use chrono::Duration;
use std::collections::BTreeMap;

/// A non-fatal problem found while normalising (never silently dropped).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub source: String,
    pub uid: Option<String>,
    pub message: String,
}

/// Normalise one VEVENT. `None` (with a warning) if it has no usable DTSTART.
pub fn event_data(c: &Component, source: &str, warnings: &mut Vec<Warning>) -> Option<EventData> {
    let uid = c.text("UID").unwrap_or_default();
    let warn = |w: &mut Vec<Warning>, m: String| {
        w.push(Warning {
            source: source.to_string(),
            uid: Some(uid.clone()).filter(|u| !u.is_empty()),
            message: m,
        })
    };
    let start = match c.prop("DTSTART").map(DtValue::from_prop) {
        Some(Ok(v)) => v,
        Some(Err(e)) => {
            warn(warnings, format!("unparseable DTSTART: {e}"));
            return None;
        }
        None => {
            warn(warnings, "VEVENT without DTSTART".into());
            return None;
        }
    };
    if let Some(t) = c.prop("DTSTART").and_then(|p| p.param("TZID"))
        && matches!(start, DtValue::Floating(_))
    {
        warn(warnings, format!("unknown TZID {t:?}; treating as local time"));
    }
    let end = compute_end(c, start, |m| warn(warnings, m));
    let parse_list = |name: &str, w: &mut Vec<Warning>| -> Vec<DtValue> {
        let mut out = Vec::new();
        for p in c.props_named(name) {
            for v in p.value.split(',') {
                match DtValue::parse(v, p.param("VALUE"), p.param("TZID")) {
                    Ok(d) => out.push(d),
                    Err(e) => warn(w, format!("bad {name}: {e}")),
                }
            }
        }
        out
    };
    let rdates = parse_list("RDATE", warnings);
    let exdates = parse_list("EXDATE", warnings);
    let (recurrence_id, this_and_future) = match c.prop("RECURRENCE-ID") {
        Some(p) => match DtValue::from_prop(p) {
            Ok(v) => (
                Some(v),
                p.param("RANGE")
                    .is_some_and(|r| r.eq_ignore_ascii_case("THISANDFUTURE")),
            ),
            Err(e) => {
                warn(warnings, format!("bad RECURRENCE-ID: {e}"));
                return None;
            }
        },
        None => (None, false),
    };
    let status = c.value("STATUS").unwrap_or("").trim().to_ascii_uppercase();
    let alarms = c
        .children_named("VALARM")
        .filter_map(|a| {
            let t = a.prop("TRIGGER")?;
            if t.param("VALUE").is_some_and(|v| v.eq_ignore_ascii_case("DATE-TIME")) {
                return None;
            }
            if t.param("RELATED").is_some_and(|r| r.eq_ignore_ascii_case("END")) {
                return None;
            }
            parse_duration(&t.value).ok().map(|d| -d.num_minutes())
        })
        .collect();
    Some(EventData {
        uid,
        summary: c.text("SUMMARY").unwrap_or_default(),
        location: c.text("LOCATION").filter(|l| !l.trim().is_empty()),
        description: c.text("DESCRIPTION").filter(|l| !l.trim().is_empty()),
        start,
        end,
        rrules: c.props_named("RRULE").map(|p| p.value.clone()).collect(),
        rdates,
        exdates,
        recurrence_id,
        this_and_future,
        cancelled: status == "CANCELLED",
        tentative: status == "TENTATIVE",
        transparent: c
            .value("TRANSP")
            .is_some_and(|t| t.trim().eq_ignore_ascii_case("TRANSPARENT")),
        sequence: c.value("SEQUENCE").and_then(|s| s.trim().parse().ok()).unwrap_or(0),
        meta: Meta::from_component(c),
        alarms,
    })
}

fn compute_end(c: &Component, start: DtValue, mut warn: impl FnMut(String)) -> DtValue {
    if let Some(p) = c.prop("DTEND") {
        match DtValue::from_prop(p) {
            Ok(e) => {
                // DTEND must be after DTSTART; all-day DTEND <= DTSTART is a common
                // bug in exporters — treat as one day.
                if start.is_date() {
                    let e = match e {
                        DtValue::Date(d) => d,
                        other => other.naive().date(),
                    };
                    let s = start.naive().date();
                    if e <= s {
                        return DtValue::Date(s.succ_opt().unwrap_or(s));
                    }
                    return DtValue::Date(e);
                }
                return e;
            }
            Err(err) => warn(format!("bad DTEND: {err}")),
        }
    }
    if let Some(d) = c.value("DURATION") {
        match parse_duration(d) {
            Ok(d) => {
                if start.is_date() {
                    let days = d.num_days().max(1);
                    return start.with_naive(start.naive() + Duration::days(days));
                }
                return start.with_naive(start.naive() + d);
            }
            Err(e) => warn(format!("{e}")),
        }
    }
    if start.is_date() {
        start.with_naive(start.naive() + Duration::days(1))
    } else {
        start
    }
}

/// All VEVENTs in a set of parsed VCALENDARs, grouped by UID.
/// `href` is the vdir file name when the source is one-item-per-file.
pub fn series_from_calendars(
    cals: &[Component],
    source: &str,
    href: Option<&str>,
    warnings: &mut Vec<Warning>,
) -> Vec<EventSeries> {
    let mut by_uid: BTreeMap<String, EventSeries> = BTreeMap::new();
    let mut anon = 0usize;
    for cal in cals {
        for ev in cal.children_named("VEVENT") {
            let Some(mut data) = event_data(ev, source, warnings) else {
                continue;
            };
            if data.uid.is_empty() {
                anon += 1;
                data.uid = format!("kairos-anon-{}-{anon}", href.unwrap_or(source));
                warnings.push(Warning {
                    source: source.to_string(),
                    uid: None,
                    message: format!("VEVENT without UID ({:?}); assigned {}", data.summary, data.uid),
                });
            }
            let s = by_uid.entry(data.uid.clone()).or_insert_with(|| EventSeries {
                uid: data.uid.clone(),
                source: source.to_string(),
                href: href.map(String::from),
                master: None,
                overrides: Vec::new(),
            });
            if data.recurrence_id.is_some() {
                s.overrides.push(data);
            } else if s.master.is_some() {
                warnings.push(Warning {
                    source: source.to_string(),
                    uid: Some(data.uid.clone()),
                    message: "duplicate master VEVENT for UID; keeping the higher SEQUENCE".into(),
                });
                if s.master.as_ref().is_some_and(|m| data.sequence > m.sequence) {
                    s.master = Some(data);
                }
            } else {
                s.master = Some(data);
            }
        }
    }
    by_uid.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ics::parse;

    fn one(body: &str) -> (Vec<EventSeries>, Vec<Warning>) {
        let s = format!("BEGIN:VCALENDAR\nVERSION:2.0\n{body}\nEND:VCALENDAR\n");
        let cals = parse(&s).unwrap();
        let mut w = Vec::new();
        let v = series_from_calendars(&cals, "src", Some("a.ics"), &mut w);
        (v, w)
    }

    #[test]
    fn all_day_default_end_is_next_day() {
        let (v, w) = one("BEGIN:VEVENT\nUID:1\nDTSTART;VALUE=DATE:20261023\nSUMMARY:x\nEND:VEVENT");
        assert!(w.is_empty());
        let m = v[0].master.as_ref().unwrap();
        assert_eq!(
            m.end,
            DtValue::Date(chrono::NaiveDate::from_ymd_opt(2026, 10, 24).unwrap())
        );
    }

    #[test]
    fn duration_property() {
        let (v, _) = one("BEGIN:VEVENT\nUID:1\nDTSTART:20261023T100000Z\nDURATION:PT90M\nEND:VEVENT");
        let m = v[0].master.as_ref().unwrap();
        assert_eq!(m.end.naive() - m.start.naive(), Duration::minutes(90));
    }

    #[test]
    fn overrides_grouped_with_master() {
        let (v, _) = one(
            "BEGIN:VEVENT\nUID:r\nDTSTART:20261023T100000Z\nRRULE:FREQ=DAILY;COUNT=3\nEND:VEVENT\nBEGIN:VEVENT\nUID:r\nRECURRENCE-ID;RANGE=THISANDFUTURE:20261024T100000Z\nDTSTART:20261024T110000Z\nEND:VEVENT",
        );
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].overrides.len(), 1);
        assert!(v[0].overrides[0].this_and_future);
        assert!(v[0].is_recurring());
    }

    #[test]
    fn missing_dtstart_warns() {
        let (v, w) = one("BEGIN:VEVENT\nUID:1\nSUMMARY:x\nEND:VEVENT");
        assert!(v.is_empty());
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn missing_uid_gets_synthetic() {
        let (v, w) = one("BEGIN:VEVENT\nDTSTART:20261023T100000Z\nEND:VEVENT");
        assert_eq!(v.len(), 1);
        assert!(v[0].uid.starts_with("kairos-anon-"));
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn alarms_parsed() {
        let (v, _) = one(
            "BEGIN:VEVENT\nUID:1\nDTSTART:20261023T100000Z\nBEGIN:VALARM\nTRIGGER:-P1D\nACTION:DISPLAY\nEND:VALARM\nBEGIN:VALARM\nTRIGGER:-PT2H\nACTION:DISPLAY\nEND:VALARM\nEND:VEVENT",
        );
        assert_eq!(v[0].master.as_ref().unwrap().alarms, vec![1440, 120]);
    }
}
