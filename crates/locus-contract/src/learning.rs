//! The learning system (C.11): goals, topic tracks and practice sheets. Not live yet; slice L
//! builds it.

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
