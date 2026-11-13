//! What Home and the projects view show of each registry entry: its sessions, what is due, and
//! (for an application or the CV) where it stands.

use crate::common::Session;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// One registry entry as Home and the projects view draw it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectView {
    pub project: String,
    /// A short stage label in the header: `barrier control`, `final round 1 Oct`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    /// Only this project's sessions.
    #[serde(default)]
    pub sessions: Vec<Session>,
    /// What is due: sheets, coursework, reports, exams, each with its date (for courses, and
    /// repos such as 3YP that have deadlines).
    #[serde(default)]
    pub due: Vec<Due>,
    /// Where an application stands, and whose move it is (round 1d: "applications should say
    /// the status of the application. eg. waiting for them, x days, open assessment stage,
    /// interview 4, final round, offer made, rejected").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Due {
    pub title: String,
    pub at: NaiveDate,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Status {
    /// `applied`, `online assessment`, `interview 2 of 4`, `final round`, `offer made`,
    /// `offer accepted`, `rejected`; for the CV, its last release.
    pub stage: String,
    /// When it reached this stage.
    pub since: NaiveDate,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<Turn>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Turn {
    /// Waiting on them.
    Them,
    /// Something for you to do.
    You,
}
