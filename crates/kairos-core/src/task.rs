//! Tasks (stored by kairosd in SQLite).

use crate::model::Author;
use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    #[default]
    Open,
    Done,
    Dropped,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskStatus::Open => "open",
            TaskStatus::Done => "done",
            TaskStatus::Dropped => "dropped",
        }
    }
    pub fn parse(s: &str) -> Option<TaskStatus> {
        match s {
            "open" => Some(TaskStatus::Open),
            "done" => Some(TaskStatus::Done),
            "dropped" => Some(TaskStatus::Dropped),
            _ => None,
        }
    }
}

/// Date-only or exact deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Due {
    Date(NaiveDate),
    At(DateTime<Utc>),
}

impl Due {
    /// The instant work must be finished by (date-only = end of that day, local).
    pub fn instant(&self, tz: Tz) -> DateTime<Utc> {
        match *self {
            Due::At(t) => t,
            Due::Date(d) => {
                let end = d.succ_opt().unwrap_or(d).and_time(NaiveTime::MIN);
                crate::time::resolve_local(end, tz)
            }
        }
    }

    pub fn describe(&self, tz: Tz) -> String {
        match *self {
            Due::Date(d) => d.format("%a %-d %b").to_string(),
            Due::At(t) => t.with_timezone(&tz).format("%a %-d %b %H:%M").to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub key: String,
    pub estimate_min: u32,
    pub remaining_min: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<Due>,
    /// UID of the linked deadline event, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_uid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub earliest_start: Option<DateTime<Utc>>,
    /// 1 (high) – 3 (low).
    pub priority: u8,
    pub splittable: bool,
    pub min_chunk_min: u32,
    pub max_chunk_min: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_window: Option<String>,
    pub status: TaskStatus,
    #[serde(default)]
    pub notes: String,
    pub created_by: Author,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Task {
    pub fn new(title: &str, key: &str, estimate_min: u32, created_by: Author, now: DateTime<Utc>) -> Task {
        Task {
            id: format!("t-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]),
            title: title.to_string(),
            key: key.to_string(),
            estimate_min,
            remaining_min: estimate_min,
            due: None,
            deadline_uid: None,
            earliest_start: None,
            priority: 2,
            splittable: true,
            min_chunk_min: 30,
            max_chunk_min: 120,
            preferred_window: None,
            status: TaskStatus::Open,
            notes: String::new(),
            created_by,
            created_at: now,
            updated_at: now,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn date_due_is_end_of_local_day() {
        let d = Due::Date(NaiveDate::from_ymd_opt(2026, 10, 23).unwrap());
        assert_eq!(
            d.instant(chrono_tz::Europe::London),
            Utc.with_ymd_and_hms(2026, 10, 23, 23, 0, 0).unwrap()
        );
    }

    #[test]
    fn json_round_trip() {
        let t = Task::new("B4 sheet", "Engineering", 240, Author::Claude, Utc::now());
        let s = serde_json::to_string(&t).unwrap();
        assert_eq!(serde_json::from_str::<Task>(&s).unwrap(), t);
    }
}
