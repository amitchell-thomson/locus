//! The Tasks page (spec E7): every task kairos holds, grouped by what it is for (ADR-009), with
//! when it is due, how much is left and when it is next planned. Home shows it as one line.

use crate::common::Time;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Tasks {
    #[serde(default)]
    pub tasks: Vec<TaskItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskItem {
    /// kairos's task id (`t-…`), so the page can act on it through kairosd.
    pub id: String,
    pub title: String,
    /// What it is for: a registry id, or none for a Personal stream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub for_id: Option<String>,
    /// How that is shown: `B15 Control Systems`, `Errands`.
    pub for_label: String,
    /// The area, for its colour.
    pub area: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<Time>,
    /// The due date has no time (it shows with the day's all-day items).
    #[serde(default)]
    pub due_is_date: bool,
    /// Planned before this event (an interview): its title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    pub left_min: u32,
    pub estimate_min: u32,
    /// When it is next planned on the calendar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_block: Option<Time>,
    /// How many of its blocks you checked in as missed.
    #[serde(default)]
    pub put_off: u32,
    /// When you finished it (done this week).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done: Option<Time>,
}
