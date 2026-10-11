//! Request/response types shared by kairosd's HTTP API, the MCP server and
//! the CLI. Times in requests are strings parsed by [`parse_time`].

use crate::changeset::WriteResult;
use crate::dates::{self, When};
use crate::model::Instance;
use crate::sched::Shortfall;
use crate::task::{Due, Task};
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

/// A parsed time from a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeIn {
    Date(NaiveDate),
    At(DateTime<Utc>),
}

impl TimeIn {
    pub fn as_due(self) -> Due {
        match self {
            TimeIn::Date(d) => Due::Date(d),
            TimeIn::At(t) => Due::At(t),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct TimeParseError(pub String);

/// Accepts RFC 3339 (`2026-10-16T17:00:00+01:00`), local `2026-10-16T17:00`
/// / `2026-10-16 17:00`, a date `2026-10-16`, or natural input
/// (`fri 17:00`, `tomorrow 9`) relative to `now`. Ambiguity is an error.
pub fn parse_time(s: &str, tz: Tz, now: DateTime<Utc>) -> Result<TimeIn, TimeParseError> {
    let s = s.trim();
    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
        return Ok(TimeIn::At(t.with_timezone(&Utc)));
    }
    for f in [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%d %H:%M:%S",
    ] {
        if let Ok(n) = NaiveDateTime::parse_from_str(s, f) {
            return Ok(TimeIn::At(crate::time::resolve_local(n, tz)));
        }
    }
    let today = now.with_timezone(&tz).date_naive();
    match dates::parse_when(s, today) {
        Ok(When::Date(d)) => Ok(TimeIn::Date(d)),
        Ok(When::DateTime(n)) => Ok(TimeIn::At(crate::time::resolve_local(n, tz))),
        Err(e) => Err(TimeParseError(format!("{s:?}: {e}"))),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CreateEventReq {
    pub title: String,
    /// Start (see [`parse_time`]). A date means an all-day event.
    pub start: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_min: Option<u32>,
    /// Key name; default `Personal` (the inbox).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Alarm offsets in minutes before start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alarms: Option<Vec<i64>>,
    /// Make this a pinned work block for the given task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct UpdateEventReq {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_min: Option<u32>,
    /// `""` clears the location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// Family events only: whether it blocks the scheduler (sidecar flag; not an iCloud write).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocking: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyReq {
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CreateTaskReq {
    pub title: String,
    pub key: String,
    pub estimate_min: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub earliest_start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub splittable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_chunk_min: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_chunk_min: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_window: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// Create a ⚑ deadline event too (default true when `due` is set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create_deadline: Option<bool>,
    /// Run the scheduler afterwards (default true).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct UpdateTaskReq {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_min: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining_min: Option<u32>,
    /// `""` clears the deadline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub splittable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_chunk_min: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_chunk_min: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_window: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CompleteTaskReq {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_min: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeadlineReq {
    pub title: String,
    pub due: String,
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PlanReq {
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannedBlock {
    pub task_id: String,
    pub title: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PlanResp {
    pub dry_run: bool,
    pub created: Vec<PlannedBlock>,
    pub deleted: Vec<PlannedBlock>,
    pub kept: usize,
    pub moved: usize,
    pub shortfalls: Vec<Shortfall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changeset: Option<WriteResult>,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskResp {
    pub task: Task,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changesets: Vec<WriteResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<PlanResp>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckinReq {
    pub block_uid: String,
    /// `done` | `partial` | `missed`
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_min: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckinItem {
    pub block_uid: String,
    pub task_id: String,
    pub title: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub planned_min: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FreeSlot {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub minutes: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventsResp {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub events: Vec<Instance>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiError {
    pub error: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use chrono_tz::Europe::London;

    #[test]
    fn time_inputs() {
        let now = Utc.with_ymd_and_hms(2026, 10, 11, 12, 0, 0).unwrap();
        let fri5 = Utc.with_ymd_and_hms(2026, 10, 16, 16, 0, 0).unwrap();
        assert_eq!(
            parse_time("2026-10-16T17:00:00+01:00", London, now).unwrap(),
            TimeIn::At(fri5)
        );
        assert_eq!(parse_time("2026-10-16T17:00", London, now).unwrap(), TimeIn::At(fri5));
        assert_eq!(parse_time("2026-10-16 17:00", London, now).unwrap(), TimeIn::At(fri5));
        assert_eq!(parse_time("fri 17:00", London, now).unwrap(), TimeIn::At(fri5));
        assert_eq!(
            parse_time("2026-10-16", London, now).unwrap(),
            TimeIn::Date(NaiveDate::from_ymd_opt(2026, 10, 16).unwrap())
        );
        assert!(parse_time("sun 9", London, now).is_err(), "today is Sunday: ambiguous");
    }
}
