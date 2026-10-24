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

/// The task kind that makes a task a project.
pub const PROJECT_KIND: &str = "project";

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
    /// Shortest block outside a squeeze.
    pub min_chunk_min: u32,
    /// Preferred block length range (`ideal_chunk_min` 0 = `min_chunk_min`).
    pub max_chunk_min: u32,
    #[serde(default)]
    pub ideal_chunk_min: u32,
    /// Task type (`constraints.toml` `task_types`), e.g. `sheet`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_window: Option<String>,
    pub status: TaskStatus,
    #[serde(default)]
    pub notes: String,
    /// The project it belongs to (its checked-in time counts there too).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// Projects only (`kind = "project"`): active / next / paused while open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<crate::project::ProjectStatus>,
    /// Projects only: the locus registry entry it is (spec K7, `taxonomy/registry.toml`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
    /// The locus item it belongs to (spec K8): a course for a sheet, an application for its
    /// paperwork, a repo. A registry id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locus: Option<String>,
    pub created_by: Author,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Task {
    /// Apply a task type's block sizes (and default estimate if `estimate_min` is 0).
    pub fn apply_type(&mut self, ty: &crate::constraints::TaskType) {
        self.kind = Some(ty.name.clone());
        self.ideal_chunk_min = ty.ideal_min_minutes;
        self.max_chunk_min = ty.ideal_max_minutes;
        self.min_chunk_min = ty.min_block_minutes;
        self.splittable = ty.splittable;
        if self.estimate_min == 0
            && let Some(e) = ty.estimate_minutes
        {
            self.estimate_min = e;
            self.remaining_min = e;
        }
    }

    /// A project: open-ended work with no deadline or estimate. Its time on
    /// the calendar is sessions (events linked by `Meta.project_id`), placed
    /// by hand or accepted from proposals, never by the scheduler.
    pub fn is_project(&self) -> bool {
        self.kind.as_deref() == Some(PROJECT_KIND)
    }

    /// Preferred shortest block (never above the longest).
    pub fn ideal_min(&self) -> u32 {
        if self.ideal_chunk_min == 0 {
            self.min_chunk_min
        } else {
            self.ideal_chunk_min
                .max(self.min_chunk_min)
                .min(self.max_chunk_min.max(self.min_chunk_min))
        }
    }

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
            ideal_chunk_min: 0,
            kind: None,
            preferred_window: None,
            status: TaskStatus::Open,
            notes: String::new(),
            project_id: None,
            stage: None,
            registry: None,
            locus: None,
            created_by,
            created_at: now,
            updated_at: now,
        }
    }
}

/// A task's due date drawn on its day (spec K5: the date is the task's attribute, not an
/// event). The ⚑ mark the TUI and CLI show, built here and never written to a calendar:
/// open tasks with a due date in `[from, to)`, as instances of kind deadline that carry the task
/// id, so every key that acts on a ⚑ acts on the task.
pub fn due_marks(tasks: &[Task], from: DateTime<Utc>, to: DateTime<Utc>, tz: Tz) -> Vec<crate::model::Instance> {
    use crate::model::{EventKind, Instance, Meta, Span};
    tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Open && t.stage.is_none())
        .filter_map(|t| {
            let due = t.due?;
            let at = due.instant(tz);
            if at < from || at >= to {
                return None;
            }
            let span = match due {
                Due::Date(d) => Span::AllDay {
                    start: d,
                    end: d.succ_opt().unwrap_or(d),
                },
                Due::At(t) => Span::Timed { start: t, end: t },
            };
            Some(Instance {
                uid: format!("due-{}", t.id),
                recurrence_id: None,
                source: "tasks".into(),
                overlay: crate::config::Overlay::Personal,
                key: Some(t.key.clone()),
                title: format!("⚑ {}", t.title),
                location: None,
                description: None,
                span,
                recurring: false,
                transparent: true,
                tentative: false,
                meta: Meta {
                    kind: EventKind::Deadline,
                    task_id: Some(t.id.clone()),
                    created_by: Some(t.created_by),
                    ..Meta::default()
                },
                href: None,
                alarms: Vec::new(),
            })
        })
        .collect()
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
