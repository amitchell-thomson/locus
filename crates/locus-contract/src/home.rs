//! Home's own lines, and the work section (Home's rhythm grid and the area chart behind `w`).
//!
//! Home shows only what no other view shows (plan D). Its other sections are other screens'
//! contracts, summarised: projects come from each Project view, Explore from `Explore`, approve
//! from `Approve`, system from `System`.

use crate::common::Time;
use chrono::{NaiveDate, NaiveTime};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Home {
    /// The next calendar item, one line; none when the day is clear.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<Next>,
    pub mail: MailLine,
    /// The attended project: the current kairos work block's, or the one marked with `m`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attended: Option<String>,
    /// The weekly review is in its slot and has not been done.
    #[serde(default)]
    pub review_ready: bool,
    /// A chore that fits the gap you are in now (spec K9): named on the top line only when there
    /// is room left after everything else there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chore_now: Option<Chore>,
    /// Chores overdue or put off, for "waiting for you" (spec K9).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chores_waiting: Vec<Chore>,
}

/// A chore from kairos (a task of type chore): life admin for a gap.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Chore {
    pub title: String,
    pub minutes: u32,
    /// How many times its block was missed.
    #[serde(default)]
    pub put_off: u32,
    #[serde(default)]
    pub overdue: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Next {
    pub title: String,
    pub at: Time,
    /// The area key, for its colour.
    pub area: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MailLine {
    pub need_reply: u32,
    pub deadlines_proposed: u32,
}

/// When you worked: from transcripts, commits and kairos logs (backfilled since August).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Work {
    /// This week, Monday first, one entry per day so far.
    #[serde(default)]
    pub week: Vec<WorkDay>,
    /// Deep blocks this week that stayed on their project, out of those planned.
    pub blocks_kept: u32,
    pub blocks_planned: u32,
    pub streak_days: u32,
    /// The area chart: hours per project per day over the last eight weeks.
    pub chart: WorkChart,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkDay {
    pub date: NaiveDate,
    #[serde(default)]
    pub spans: Vec<WorkSpan>,
}

/// A stretch of work on one project. Intensity is how active it was (1 light to 4 deep), from
/// how much happened in it; the grid draws it as ░ ▒ ▓ █.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkSpan {
    pub start: NaiveTime,
    pub end: NaiveTime,
    pub project: String,
    pub intensity: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkChart {
    pub from: NaiveDate,
    #[serde(default)]
    pub series: Vec<WorkSeries>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkSeries {
    pub project: String,
    /// Hours per day from `from`, one entry per day.
    pub hours: Vec<f32>,
}

impl WorkSpan {
    pub fn hours(&self) -> f32 {
        let mins = (self.end - self.start).num_minutes();
        mins.max(0) as f32 / 60.0
    }
}

impl Work {
    /// Hours per project this week, largest first.
    pub fn totals(&self) -> Vec<(String, f32)> {
        let mut t: Vec<(String, f32)> = Vec::new();
        for s in self.week.iter().flat_map(|d| &d.spans) {
            match t.iter_mut().find(|(p, _)| *p == s.project) {
                Some((_, h)) => *h += s.hours(),
                None => t.push((s.project.clone(), s.hours())),
            }
        }
        t.sort_by(|a, b| b.1.total_cmp(&a.1));
        t
    }
}
