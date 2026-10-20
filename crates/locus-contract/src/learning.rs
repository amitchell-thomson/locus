//! The learning system (C.11) and interview prep, its first mode (C.10).
//!
//! Topic tracks are general and outlive any one application; a company plan draws on them,
//! weighted by that interview's format.

use crate::common::Time;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Learning {
    #[serde(default)]
    pub goals: Vec<Goal>,
    #[serde(default)]
    pub tracks: Vec<Track>,
    #[serde(default)]
    pub sheets: Vec<Sheet>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Goal {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<NaiveDate>,
    pub state: GoalState,
    /// The syllabus, shown before it starts (C.11): tracks in order, with time.
    #[serde(default)]
    pub syllabus: Vec<SyllabusItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalState {
    /// Syllabus built, waiting for you to agree it.
    Proposed,
    Active,
    Done,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyllabusItem {
    pub track: String,
    /// What this part covers.
    pub what: String,
    pub hours: f32,
    /// Where the material comes from: `your library`, `B1 ML course`.
    pub from: String,
}

/// A topic track: a confidence bar and its history (C.10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Track {
    pub id: String,
    pub name: String,
    /// 0 to 1; moved by marked answers and logged sessions.
    pub confidence: f32,
    /// Confidence over time, oldest first.
    #[serde(default)]
    pub history: Vec<Point>,
    #[serde(default)]
    pub sessions: Vec<PracticeSession>,
    /// Error-log entries not yet passed on retry.
    #[serde(default)]
    pub open_misses: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Point {
    pub date: NaiveDate,
    pub confidence: f32,
}

/// One line per practice session keeps the bars honest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PracticeSession {
    pub date: NaiveDate,
    pub problems: u32,
    pub right: u32,
    pub minutes: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The sheet it came from, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheet: Option<String>,
}

/// A practice sheet for the tablet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sheet {
    pub id: String,
    pub title: String,
    pub tracks: Vec<String>,
    pub sent: NaiveDate,
    pub state: SheetState,
    /// Questions and how many were marked right, once marked.
    pub questions: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SheetState {
    /// Proposed, not yet sent.
    Draft,
    OnTablet,
    /// Answered in ink, waiting for the readback.
    Answered,
    Marked,
}

/// An opportunity's prep plan (C.10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prep {
    /// The opportunity's project id.
    pub opportunity: String,
    pub role: String,
    /// The round being prepared for: `final round`.
    pub round: String,
    pub date: NaiveDate,
    /// The time you said you would spend, and by when.
    pub budget_hours: f32,
    pub used_hours: f32,
    /// Each part of the interview: what it covers and how it is run.
    #[serde(default)]
    pub format: Vec<RoundPart>,
    /// The tracks this plan draws on, weakest first, with the time given to each.
    #[serde(default)]
    pub topics: Vec<PrepTopic>,
    /// Practice sessions for this plan, newest first.
    #[serde(default)]
    pub log: Vec<PrepSession>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<PlannedSession>,
    /// How it ended, once it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoundPart {
    pub name: String,
    pub covers: String,
    pub how: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrepTopic {
    pub track: String,
    /// Minutes a day this plan gives it.
    pub minutes_a_day: u32,
    /// Where its material comes from: `Green Book ch. 4-5`, `your B1 ML notes`.
    pub from: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrepSession {
    pub date: NaiveDate,
    pub track: String,
    pub problems: u32,
    pub right: u32,
    pub minutes: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedSession {
    pub at: Time,
    pub track: String,
    pub minutes: u32,
    /// Already a kairos block.
    pub in_kairos: bool,
}
