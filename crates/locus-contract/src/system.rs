//! System (live jobs, recent results) and the budget (plan E): System shows a summary of the
//! same `Usage` the Budget view draws in full.

use crate::common::{Job, Time};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct System {
    /// Running jobs, updated live.
    #[serde(default)]
    pub running: Vec<Job>,
    /// Finished jobs, newest first; failures stay visible with their cause.
    #[serde(default)]
    pub recent: Vec<Job>,
    pub done_today: u32,
    pub failed_today: u32,
}

/// Background use against the weekly cap (about 15%) and the 5-hour window (stop at 60%).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Usage {
    /// Percentages are of your weekly limit, as `/usage` reports it.
    pub week_pct: f32,
    pub cap_pct: f32,
    pub projected_pct: f32,
    pub window: Window,
    /// One entry per day this week, Monday first.
    #[serde(default)]
    pub days: Vec<DayUse>,
    /// The most expensive jobs this week.
    #[serde(default)]
    pub top_jobs: Vec<Cost>,
    #[serde(default)]
    pub by_project: Vec<Cost>,
}

/// The current 5-hour window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Window {
    pub you_pct: f32,
    pub locus_pct: f32,
    /// Background jobs stop while your own use is above this.
    pub stop_at_pct: f32,
    pub resets: Time,
    /// Jobs held back by the window right now.
    #[serde(default)]
    pub held: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DayUse {
    pub date: NaiveDate,
    /// Split by job type; the day's total is the sum.
    #[serde(default)]
    pub by_kind: Vec<Cost>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cost {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub pct: f32,
}

impl DayUse {
    pub fn total(&self) -> f32 {
        self.by_kind.iter().map(|c| c.pct).sum()
    }
}
