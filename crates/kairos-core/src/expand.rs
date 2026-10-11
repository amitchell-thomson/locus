//! Expand [`EventSeries`] into concrete [`Instance`]s within a window.
//!
//! Recurrence is evaluated in **wall-clock time of the master's zone**
//! (RFC 5545: "19:00 every Sunday" stays 19:00 local across DST), using the
//! `rrule` crate on naive times. Each wall-clock occurrence is then resolved
//! to an instant with [`crate::time::resolve_local`], so ambiguous and
//! non-existent local times follow one documented rule everywhere.
//!
//! Overrides (`RECURRENCE-ID`) replace the matching occurrence; overrides with
//! `RANGE=THISANDFUTURE` shift and restyle that and all later occurrences;
//! `STATUS:CANCELLED` overrides remove their occurrence; moved overrides
//! appear at their new time even if their original slot is outside the window.

use crate::config::Overlay;
use crate::model::{EventData, EventSeries, Instance, Span};
use crate::normalize::Warning;
use crate::time::{DtValue, resolve_local};
use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;

/// Where a series comes from (for tagging instances).
#[derive(Debug, Clone, Copy)]
pub struct SourceTag<'a> {
    pub id: &'a str,
    pub overlay: Overlay,
    pub key: Option<&'a str>,
}

/// Identity of an occurrence's original slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Slot {
    Date(NaiveDate),
    At(DateTime<Utc>),
}

impl Slot {
    pub fn id(&self) -> String {
        match self {
            Slot::Date(d) => d.format("%Y%m%d").to_string(),
            Slot::At(t) => t.format("%Y%m%dT%H%M%SZ").to_string(),
        }
    }
}

/// Hard cap on occurrences generated per series per call.
const MAX_OCCURRENCES: usize = 20_000;

/// Convert any DtValue to wall-clock time in `zone` (None = UTC).
fn naive_in(v: &DtValue, zone: Option<Tz>, display: Tz) -> NaiveDateTime {
    match (v, zone) {
        (DtValue::Date(d), _) => d.and_time(chrono::NaiveTime::MIN),
        (DtValue::Floating(n), _) => *n,
        (_, Some(z)) => v.instant(display).with_timezone(&z).naive_local(),
        (_, None) => v.instant(display).naive_utc(),
    }
}

fn span_of(start: &DtValue, end: &DtValue, display: Tz) -> Span {
    match (start, end) {
        (DtValue::Date(s), DtValue::Date(e)) => Span::AllDay {
            start: *s,
            end: (*e).max(s.succ_opt().unwrap_or(*s)),
        },
        (DtValue::Date(s), other) => {
            let e = other.naive().date();
            Span::AllDay {
                start: *s,
                end: e.max(s.succ_opt().unwrap_or(*s)),
            }
        }
        _ => {
            let s = start.instant(display);
            let e = end.instant(display);
            Span::Timed {
                start: s,
                end: e.max(s),
            }
        }
    }
}

fn slot_of(v: &DtValue, master_is_date: bool, display: Tz) -> Slot {
    if master_is_date {
        Slot::Date(v.naive().date())
    } else {
        match v {
            DtValue::Date(d) => Slot::Date(*d),
            _ => Slot::At(v.instant(display)),
        }
    }
}

fn instance(series: &EventSeries, tag: SourceTag<'_>, d: &EventData, span: Span, rid: Option<String>) -> Instance {
    Instance {
        uid: series.uid.clone(),
        recurrence_id: rid,
        source: tag.id.to_string(),
        overlay: tag.overlay,
        key: tag.key.map(String::from),
        title: d.summary.clone(),
        location: d.location.clone(),
        span,
        recurring: series.is_recurring(),
        transparent: d.transparent,
        tentative: d.tentative,
        meta: d.meta.clone(),
        href: series.href.clone(),
        alarms: d.alarms.clone(),
    }
}

fn in_window(span: &Span, from: DateTime<Utc>, to: DateTime<Utc>, display: Tz) -> bool {
    let (s, e) = span.bounds(display);
    if s == e {
        s >= from && s < to
    } else {
        s < to && e > from
    }
}

/// Rewrite UNTIL into wall-clock UTC-tagged form for naive expansion.
fn localise_rrule(rule: &str, zone: Option<Tz>, master_is_date: bool, display: Tz) -> String {
    rule.split(';')
        .map(|part| {
            let Some(v) = part.strip_prefix("UNTIL=").or_else(|| part.strip_prefix("until=")) else {
                return part.to_string();
            };
            let parsed = DtValue::parse(v, None, None);
            let naive = match parsed {
                Ok(DtValue::Date(d)) => d.and_hms_opt(23, 59, 59).unwrap_or(d.and_time(chrono::NaiveTime::MIN)),
                Ok(other) if master_is_date => other.naive().date().and_hms_opt(23, 59, 59).unwrap_or(other.naive()),
                Ok(other) => naive_in(&other, zone, display),
                Err(_) => return part.to_string(),
            };
            format!("UNTIL={}", naive.format("%Y%m%dT%H%M%SZ"))
        })
        .collect::<Vec<_>>()
        .join(";")
}

/// Wall-clock occurrences of the master's rules, `≤ until` (naive).
fn occurrences(
    m: &EventData,
    zone: Option<Tz>,
    display: Tz,
    until: NaiveDateTime,
    warnings: &mut Vec<Warning>,
    source: &str,
) -> Vec<NaiveDateTime> {
    let start = m.start.naive();
    let master_is_date = m.start.is_date();
    let mut out = Vec::new();
    if m.rrules.is_empty() {
        out.push(start);
    } else {
        let mut text = format!("DTSTART:{}", start.format("%Y%m%dT%H%M%SZ"));
        for r in &m.rrules {
            text.push_str("\nRRULE:");
            text.push_str(&localise_rrule(r, zone, master_is_date, display));
        }
        match text.parse::<rrule::RRuleSet>() {
            Ok(set) => {
                for dt in &set {
                    let n = dt.naive_utc();
                    if n > until || out.len() >= MAX_OCCURRENCES {
                        break;
                    }
                    out.push(n);
                }
            }
            Err(e) => {
                warnings.push(Warning {
                    source: source.to_string(),
                    uid: Some(m.uid.clone()),
                    message: format!("unsupported RRULE ({e}); showing first occurrence only"),
                });
                out.push(start);
            }
        }
    }
    for r in &m.rdates {
        let n = naive_in(r, zone, display);
        if n <= until {
            out.push(n);
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Expand one series into instances overlapping `[from, to)`.
pub fn expand(
    series: &EventSeries,
    tag: SourceTag<'_>,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    display: Tz,
    warnings: &mut Vec<Warning>,
) -> Vec<Instance> {
    let mut out = Vec::new();
    let Some(m) = &series.master else {
        // Orphan overrides (e.g. a single shared occurrence): show as-is.
        for o in series.overrides.iter().filter(|o| !o.cancelled) {
            let span = span_of(&o.start, &o.end, display);
            if in_window(&span, from, to, display) {
                let rid = o
                    .recurrence_id
                    .as_ref()
                    .map(|r| slot_of(r, o.start.is_date(), display).id());
                out.push(instance(series, tag, o, span, rid));
            }
        }
        return out;
    };
    if m.cancelled {
        return out;
    }
    if !series.is_recurring() {
        let span = span_of(&m.start, &m.end, display);
        if in_window(&span, from, to, display) {
            out.push(instance(series, tag, m, span, None));
        }
        return out;
    }

    let master_is_date = m.start.is_date();
    let zone = m.start.zone(display);
    let master_span = span_of(&m.start, &m.end, display);
    let exact_len = master_span.duration(display);
    let wall_len = m.end.naive() - m.start.naive();

    let exdates: Vec<Slot> = m.exdates.iter().map(|x| slot_of(x, master_is_date, display)).collect();
    let ex_dates_only: Vec<NaiveDate> = m
        .exdates
        .iter()
        .filter_map(|x| match x {
            DtValue::Date(d) => Some(*d),
            _ => None,
        })
        .collect();

    // Overrides keyed by slot.
    let mut exact: Vec<(Slot, &EventData)> = Vec::new();
    let mut future: Vec<(Slot, NaiveDateTime, &EventData)> = Vec::new();
    for o in &series.overrides {
        let Some(rid) = &o.recurrence_id else { continue };
        let slot = slot_of(rid, master_is_date, display);
        exact.push((slot, o));
        if o.this_and_future {
            future.push((slot, naive_in(rid, zone, display), o));
        }
    }
    future.sort_by_key(|f| f.0);

    // Generation window, in wall-clock terms of the master zone, widened by the
    // event length, any THISANDFUTURE shift, and a day for DST slack.
    let max_shift = future
        .iter()
        .map(|(_, rid_n, o)| (naive_in(&o.start, zone, display) - *rid_n).abs())
        .max()
        .unwrap_or_else(Duration::zero);
    let to_wall = match zone {
        Some(z) => to.with_timezone(&z).naive_local(),
        None => to.naive_utc(),
    } + max_shift
        + Duration::days(1);
    let from_wall = match zone {
        Some(z) => from.with_timezone(&z).naive_local(),
        None => from.naive_utc(),
    } - max_shift
        - exact_len.max(wall_len)
        - Duration::days(1);

    for occ in occurrences(m, zone, display, to_wall, warnings, &series.source) {
        if occ < from_wall {
            continue;
        }
        let orig = if master_is_date {
            DtValue::Date(occ.date())
        } else {
            m.start.with_naive(occ)
        };
        let slot = slot_of(&orig, master_is_date, display);
        if exdates.contains(&slot) || (!master_is_date && ex_dates_only.contains(&occ.date())) {
            continue;
        }
        if exact.iter().any(|(s, _)| *s == slot) {
            continue; // emitted below from the override itself
        }
        // Latest THISANDFUTURE override at or before this slot, if any.
        let fut = future.iter().rev().find(|(s, _, _)| *s <= slot);
        let (data, start_val, span) = match fut {
            Some((_, rid_n, o)) => {
                let shift = naive_in(&o.start, zone, display) - *rid_n;
                let len = o.end.naive() - o.start.naive();
                let s = orig.with_naive(occ + shift);
                let e = s.with_naive(s.naive() + len);
                (*o, s, span_of(&s, &e, display))
            }
            None => {
                let span = if master_is_date {
                    let s = occ.date();
                    let days = wall_len.num_days().max(1);
                    Span::AllDay {
                        start: s,
                        end: s + Duration::days(days),
                    }
                } else {
                    let s = match zone {
                        Some(z) => resolve_local(occ, z),
                        None => Utc.from_utc_datetime(&occ),
                    };
                    Span::Timed {
                        start: s,
                        end: s + exact_len,
                    }
                };
                (m, orig, span)
            }
        };
        let _ = start_val;
        if in_window(&span, from, to, display) {
            out.push(instance(series, tag, data, span, Some(slot.id())));
        }
    }

    for (slot, o) in &exact {
        if o.cancelled {
            continue;
        }
        let span = span_of(&o.start, &o.end, display);
        if in_window(&span, from, to, display) {
            out.push(instance(series, tag, o, span, Some(slot.id())));
        }
    }
    out.sort_by_key(|i| i.span.bounds(display).0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ics::parse;
    use crate::normalize::series_from_calendars;
    use chrono_tz::Europe::London;

    fn expand_str(body: &str, from: &str, to: &str) -> Vec<Instance> {
        let s = format!("BEGIN:VCALENDAR\nVERSION:2.0\n{body}\nEND:VCALENDAR\n");
        let cals = parse(&s).unwrap();
        let mut w = Vec::new();
        let series = series_from_calendars(&cals, "src", None, &mut w);
        let tag = SourceTag {
            id: "src",
            overlay: Overlay::Personal,
            key: Some("Personal"),
        };
        let f = |s: &str| {
            London
                .from_local_datetime(&NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap())
                .unwrap()
                .with_timezone(&Utc)
        };
        let mut out = Vec::new();
        for s in &series {
            out.extend(expand(s, tag, f(from), f(to), London, &mut w));
        }
        assert!(w.is_empty(), "warnings: {w:?}");
        out
    }

    fn local(i: &Instance) -> String {
        match i.span {
            Span::Timed { start, .. } => start.with_timezone(&London).format("%Y-%m-%d %H:%M").to_string(),
            Span::AllDay { start, .. } => start.to_string(),
        }
    }

    #[test]
    fn simple_weekly_with_count() {
        let v = expand_str(
            "BEGIN:VEVENT\nUID:w\nDTSTART;TZID=Europe/London:20261004T190000\nDTEND;TZID=Europe/London:20261004T200000\nRRULE:FREQ=WEEKLY;COUNT=3\nEND:VEVENT",
            "2026-10-01 00:00",
            "2026-12-01 00:00",
        );
        assert_eq!(
            v.iter().map(local).collect::<Vec<_>>(),
            ["2026-10-04 19:00", "2026-10-11 19:00", "2026-10-18 19:00"]
        );
        assert!(v.iter().all(|i| i.recurring));
    }

    #[test]
    fn until_in_utc_is_inclusive_in_local_terms() {
        // UNTIL 18:00Z on 18 Oct == 19:00 BST: the 18 Oct occurrence is included.
        let v = expand_str(
            "BEGIN:VEVENT\nUID:w\nDTSTART;TZID=Europe/London:20261004T190000\nDURATION:PT1H\nRRULE:FREQ=WEEKLY;UNTIL=20261018T180000Z\nEND:VEVENT",
            "2026-10-01 00:00",
            "2026-12-01 00:00",
        );
        assert_eq!(v.len(), 3);
    }

    #[test]
    fn window_clips_infinite_rule() {
        let v = expand_str(
            "BEGIN:VEVENT\nUID:d\nDTSTART;TZID=Europe/London:20200101T090000\nDURATION:PT30M\nRRULE:FREQ=DAILY\nEND:VEVENT",
            "2026-10-12 00:00",
            "2026-10-15 00:00",
        );
        assert_eq!(v.len(), 3);
        assert_eq!(local(&v[0]), "2026-10-12 09:00");
    }

    #[test]
    fn all_day_recurring() {
        let v = expand_str(
            "BEGIN:VEVENT\nUID:b\nDTSTART;VALUE=DATE:20250310\nDTEND;VALUE=DATE:20250311\nRRULE:FREQ=YEARLY\nEND:VEVENT",
            "2026-01-01 00:00",
            "2027-12-31 00:00",
        );
        assert_eq!(v.iter().map(local).collect::<Vec<_>>(), ["2026-03-10", "2027-03-10"]);
        assert_eq!(v[0].recurrence_id.as_deref(), Some("20260310"));
    }
}
