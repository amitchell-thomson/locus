//! Normalised event model.

use crate::config::Overlay;
use crate::ics::Component;
use crate::time::DtValue;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    #[default]
    Event,
    Deadline,
    TaskBlock,
}

impl EventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::Event => "event",
            EventKind::Deadline => "deadline",
            EventKind::TaskBlock => "task_block",
        }
    }
    pub fn parse(s: &str) -> Option<EventKind> {
        match s.trim().to_ascii_lowercase().as_str() {
            "event" => Some(EventKind::Event),
            "deadline" => Some(EventKind::Deadline),
            "task_block" | "task-block" => Some(EventKind::TaskBlock),
            _ => None,
        }
    }
}

/// Who created a change / an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Author {
    Alec,
    Claude,
    Scheduler,
}

impl Author {
    pub fn as_str(self) -> &'static str {
        match self {
            Author::Alec => "alec",
            Author::Claude => "claude",
            Author::Scheduler => "scheduler",
        }
    }
    pub fn parse(s: &str) -> Option<Author> {
        match s.trim().to_ascii_lowercase().as_str() {
            "alec" => Some(Author::Alec),
            "claude" => Some(Author::Claude),
            "scheduler" => Some(Author::Scheduler),
            _ => None,
        }
    }
}

/// kairos metadata. Authoritative copy lives in the sidecar (SQLite);
/// mirrored as `X-KAIROS-*` on events kairos writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Meta {
    /// `None` = not created through kairos (phone, Calendar.app, imports).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<Author>,
    #[serde(default)]
    pub kind: EventKind,
    #[serde(default)]
    pub flex: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// For events shared to Family: the key they came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adopted_from: Option<String>,
    /// Family events: opt in to blocking the scheduler.
    #[serde(default)]
    pub blocking: bool,
    /// A block Alec placed explicitly; the scheduler never moves it.
    #[serde(default)]
    pub pinned: bool,
}

impl Meta {
    pub const PREFIX: &'static str = "X-KAIROS-";

    pub fn from_component(c: &Component) -> Meta {
        let get = |n: &str| {
            c.value(&format!("X-KAIROS-{n}"))
                .map(str::trim)
                .filter(|s| !s.is_empty())
        };
        let truthy = |n: &str| get(n).is_some_and(|v| v.eq_ignore_ascii_case("true") || v == "1");
        Meta {
            created_by: get("CREATED-BY").and_then(Author::parse),
            kind: get("KIND").and_then(EventKind::parse).unwrap_or_default(),
            flex: truthy("FLEX"),
            task_id: get("TASK-ID").map(String::from),
            origin_key: get("ORIGIN-KEY").map(String::from),
            adopted_from: get("ADOPTED-FROM").map(String::from),
            blocking: truthy("BLOCKING"),
            pinned: truthy("PINNED"),
        }
    }

    /// Write `X-KAIROS-*` properties onto a VEVENT (replacing old ones).
    pub fn apply_to(&self, c: &mut Component) {
        c.props.retain(|p| !p.name.starts_with(Self::PREFIX));
        use crate::ics::Property as P;
        if let Some(a) = self.created_by {
            c.push(P::new("X-KAIROS-CREATED-BY", a.as_str()));
        }
        if self.kind != EventKind::Event {
            c.push(P::new("X-KAIROS-KIND", self.kind.as_str()));
        }
        if self.flex {
            c.push(P::new("X-KAIROS-FLEX", "true"));
        }
        if let Some(t) = &self.task_id {
            c.push(P::text("X-KAIROS-TASK-ID", t));
        }
        if let Some(k) = &self.origin_key {
            c.push(P::text("X-KAIROS-ORIGIN-KEY", k));
        }
        if let Some(a) = &self.adopted_from {
            c.push(P::text("X-KAIROS-ADOPTED-FROM", a));
        }
        if self.blocking {
            c.push(P::new("X-KAIROS-BLOCKING", "true"));
        }
        if self.pinned {
            c.push(P::new("X-KAIROS-PINNED", "true"));
        }
    }

    /// Prefer non-default sidecar values over the mirrored properties.
    pub fn merged_with(&self, sidecar: &Meta) -> Meta {
        Meta {
            created_by: sidecar.created_by.or(self.created_by),
            kind: if sidecar.kind != EventKind::Event {
                sidecar.kind
            } else {
                self.kind
            },
            flex: sidecar.flex || self.flex,
            task_id: sidecar.task_id.clone().or_else(|| self.task_id.clone()),
            origin_key: sidecar.origin_key.clone().or_else(|| self.origin_key.clone()),
            adopted_from: sidecar.adopted_from.clone().or_else(|| self.adopted_from.clone()),
            blocking: sidecar.blocking || self.blocking,
            pinned: sidecar.pinned || self.pinned,
        }
    }
}

/// When an instance happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Span {
    Timed {
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    },
    /// `end` is exclusive (a single-day event on 23 Oct has end = 24 Oct).
    AllDay {
        start: NaiveDate,
        end: NaiveDate,
    },
}

impl Span {
    pub fn is_all_day(&self) -> bool {
        matches!(self, Span::AllDay { .. })
    }

    /// Instants in `tz` (all-day spans cover local midnights).
    pub fn bounds(&self, tz: Tz) -> (DateTime<Utc>, DateTime<Utc>) {
        match *self {
            Span::Timed { start, end } => (start, end),
            Span::AllDay { start, end } => (
                crate::time::resolve_local(start.and_time(chrono::NaiveTime::MIN), tz),
                crate::time::resolve_local(end.and_time(chrono::NaiveTime::MIN), tz),
            ),
        }
    }

    /// Local dates this span touches (every covered day; an event ending at
    /// exactly midnight does not touch the next day).
    pub fn local_dates(&self, tz: Tz) -> Vec<NaiveDate> {
        match *self {
            Span::AllDay { start, end } => {
                let mut v = Vec::new();
                let mut d = start;
                while d < end.max(start.succ_opt().unwrap_or(start)) {
                    v.push(d);
                    match d.succ_opt() {
                        Some(n) => d = n,
                        None => break,
                    }
                }
                v
            }
            Span::Timed { start, end } => {
                let s = start.with_timezone(&tz).date_naive();
                let last = if end > start {
                    (end - Duration::seconds(1)).with_timezone(&tz).date_naive()
                } else {
                    s
                };
                let mut v = Vec::new();
                let mut d = s;
                while d <= last {
                    v.push(d);
                    match d.succ_opt() {
                        Some(n) => d = n,
                        None => break,
                    }
                }
                v
            }
        }
    }

    pub fn overlaps(&self, other: &Span, tz: Tz) -> bool {
        let (a0, a1) = self.bounds(tz);
        let (b0, b1) = other.bounds(tz);
        a0 < b1 && b0 < a1
    }

    pub fn duration(&self, tz: Tz) -> Duration {
        let (a, b) = self.bounds(tz);
        b - a
    }
}

/// One concrete occurrence, ready to display.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Instance {
    pub uid: String,
    /// Original start of this occurrence for recurring events (RECURRENCE-ID).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurrence_id: Option<String>,
    pub source: String,
    pub overlay: Overlay,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    pub span: Span,
    pub recurring: bool,
    /// TRANSP:TRANSPARENT (doesn't block time).
    pub transparent: bool,
    pub tentative: bool,
    pub meta: Meta,
    /// vdir file name (for writes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    /// Alarm offsets in minutes before start.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alarms: Vec<i64>,
}

impl Instance {
    pub fn kind(&self) -> EventKind {
        self.meta.kind
    }

    /// Stable identity of the occurrence: `uid` + recurrence id.
    pub fn instance_id(&self) -> String {
        match &self.recurrence_id {
            Some(r) => format!("{}#{}", self.uid, r),
            None => self.uid.clone(),
        }
    }

    /// The key used for colouring (Family events keep their origin key).
    pub fn colour_key(&self) -> Option<&str> {
        self.meta.origin_key.as_deref().or(self.key.as_deref())
    }

    pub fn start(&self, tz: Tz) -> DateTime<Utc> {
        self.span.bounds(tz).0
    }
    pub fn end(&self, tz: Tz) -> DateTime<Utc> {
        self.span.bounds(tz).1
    }
}

/// Pre-expansion data for one VEVENT component.
#[derive(Debug, Clone, PartialEq)]
pub struct EventData {
    pub uid: String,
    pub summary: String,
    pub location: Option<String>,
    pub start: DtValue,
    /// Exclusive end, already computed from DTEND / DURATION / defaults.
    pub end: DtValue,
    pub rrules: Vec<String>,
    pub rdates: Vec<DtValue>,
    pub exdates: Vec<DtValue>,
    pub recurrence_id: Option<DtValue>,
    pub this_and_future: bool,
    pub cancelled: bool,
    pub tentative: bool,
    pub transparent: bool,
    pub sequence: i64,
    pub meta: Meta,
    pub alarms: Vec<i64>,
}

/// A UID's master and overrides within one source.
#[derive(Debug, Clone, PartialEq)]
pub struct EventSeries {
    pub uid: String,
    pub source: String,
    pub href: Option<String>,
    pub master: Option<EventData>,
    pub overrides: Vec<EventData>,
}

impl EventSeries {
    pub fn is_recurring(&self) -> bool {
        self.master
            .as_ref()
            .is_some_and(|m| !m.rrules.is_empty() || !m.rdates.is_empty())
            || !self.overrides.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use chrono_tz::Europe::London;

    #[test]
    fn meta_round_trip_through_properties() {
        let m = Meta {
            created_by: Some(Author::Scheduler),
            kind: EventKind::TaskBlock,
            flex: true,
            task_id: Some("t-1".into()),
            origin_key: Some("Sport".into()),
            adopted_from: None,
            blocking: false,
            pinned: true,
        };
        let mut c = Component::new("VEVENT");
        m.apply_to(&mut c);
        assert_eq!(Meta::from_component(&c), m);
    }

    #[test]
    fn sidecar_wins() {
        let ics = Meta {
            kind: EventKind::Event,
            ..Default::default()
        };
        let side = Meta {
            kind: EventKind::Deadline,
            created_by: Some(Author::Claude),
            ..Default::default()
        };
        assert_eq!(ics.merged_with(&side).kind, EventKind::Deadline);
    }

    fn d(y: i32, m: u32, dd: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, dd).unwrap()
    }

    #[test]
    fn all_day_dates_exclusive_end() {
        let s = Span::AllDay {
            start: d(2026, 10, 23),
            end: d(2026, 10, 24),
        };
        assert_eq!(s.local_dates(London), vec![d(2026, 10, 23)]);
        let multi = Span::AllDay {
            start: d(2026, 10, 23),
            end: d(2026, 10, 26),
        };
        assert_eq!(
            multi.local_dates(London),
            vec![d(2026, 10, 23), d(2026, 10, 24), d(2026, 10, 25)]
        );
    }

    #[test]
    fn timed_ending_at_midnight_does_not_touch_next_day() {
        let s = Span::Timed {
            start: London
                .with_ymd_and_hms(2026, 10, 23, 22, 0, 0)
                .unwrap()
                .with_timezone(&Utc),
            end: London
                .with_ymd_and_hms(2026, 10, 24, 0, 0, 0)
                .unwrap()
                .with_timezone(&Utc),
        };
        assert_eq!(s.local_dates(London), vec![d(2026, 10, 23)]);
    }

    #[test]
    fn all_day_bounds_are_local_midnight_bst() {
        let s = Span::AllDay {
            start: d(2026, 7, 1),
            end: d(2026, 7, 2),
        };
        let (a, b) = s.bounds(London);
        assert_eq!(a, Utc.with_ymd_and_hms(2026, 6, 30, 23, 0, 0).unwrap());
        assert_eq!(b - a, Duration::hours(24));
    }
}
