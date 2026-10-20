//! Search (C.2) and the weekly review (plan D).

use crate::common::{Find, ItemKind, Time};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// One search and its results, grouped by kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Search {
    pub query: String,
    /// `everything`, or a project id (from a Project view; Tab widens).
    pub scope: String,
    #[serde(default)]
    pub hits: Vec<Hit>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hit {
    pub source: HitSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Time>,
    /// Where Enter jumps, said in words (19 Oct): `nvim projects/quast/jot.tex:41`.
    pub jump: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HitSource {
    Decisions,
    Answers,
    Explore,
    Notes,
    Library,
    Chats,
    Dossiers,
}

/// The weekly review: five steps in order of value, each skippable; it remembers where you
/// stopped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Review {
    pub from: NaiveDate,
    pub to: NaiveDate,
    /// Steps 1 to 5 and where each stands.
    pub steps: Vec<StepState>,
    pub week: ReviewWeek,
    #[serde(default)]
    pub rechecks: Vec<Recheck>,
    pub explore: ReviewExplore,
    pub locus: ReviewLocus,
    /// Deep blocks proposed for next week, per project.
    #[serde(default)]
    pub next_week: Vec<Blocks>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepState {
    Done,
    Current,
    Skipped,
    Todo,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewWeek {
    pub blocks_kept: u32,
    pub blocks_planned: u32,
    pub budget_pct: f32,
    #[serde(default)]
    pub dossiers_rebuilt: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recheck {
    pub project: String,
    pub decision: String,
    pub title: String,
    pub agreed: NaiveDate,
    pub flagged: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewExplore {
    /// Finds still in the queue; graded here in turn.
    pub left: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub best_idea: Option<Find>,
    /// Reading untouched for three weeks.
    #[serde(default)]
    pub untouched: Vec<String>,
}

/// Step 4: locus itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewLocus {
    /// Screens or features not opened this week, from the use log.
    #[serde(default)]
    pub unused: Vec<String>,
    /// Strike reasons this week, with what they suggest.
    #[serde(default)]
    pub strikes: Vec<String>,
    /// The orphan check (C.13): captured with no area attached.
    #[serde(default)]
    pub orphans: Vec<Orphan>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Orphan {
    pub kind: ItemKind,
    pub text: String,
    pub at: Time,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Blocks {
    pub project: String,
    pub blocks: u32,
}
