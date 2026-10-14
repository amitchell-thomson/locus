//! Request and response types of keryxd's HTTP API (shared by the daemon,
//! the TUI's client and the CLI).

use crate::state::{Action, Proposal, Review};
use keryx_core::changeset::Changeset;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Intent {
    Archive,
    Unarchive,
    File {
        key: String,
    },
    Flag {
        on: bool,
    },
    Read {
        on: bool,
    },
    Snooze {
        until: i64,
    },
    /// Wake now: back to the inbox, latest unread.
    Unsnooze,
}

impl Intent {
    pub fn verb(&self) -> String {
        match self {
            Intent::Archive => "archive".into(),
            Intent::Unarchive => "move back to the inbox".into(),
            Intent::File { key } => format!("file into {key}"),
            Intent::Flag { on: true } => "flag".into(),
            Intent::Flag { on: false } => "unflag".into(),
            Intent::Read { on: true } => "mark read".into(),
            Intent::Read { on: false } => "mark unread".into(),
            Intent::Snooze { .. } => "snooze".into(),
            Intent::Unsnooze => "wake".into(),
        }
    }
    /// Takes the thread out of the inbox (clears needs-reply).
    pub fn leaves_inbox(&self) -> bool {
        matches!(self, Intent::Archive | Intent::File { .. } | Intent::Snooze { .. })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriageReq {
    #[serde(flatten)]
    pub intent: Intent,
    pub refs: Vec<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(skip)]
    pub review_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TriageOutcome {
    /// Changes that were applied (low risk / Alec's own).
    pub applied: Option<Changeset>,
    /// Threads turned into proposals for Alec's review.
    pub review_id: Option<String>,
    pub proposed: usize,
    /// Threads refused (forbidden) or with nothing to do, with why.
    pub skipped: Vec<(String, String)>,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposeItem {
    pub r#ref: String,
    #[serde(flatten)]
    pub action: Action,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposeReq {
    #[serde(default)]
    pub review_id: Option<String>,
    pub title: String,
    pub items: Vec<ProposeItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposeOutcome {
    pub review_id: String,
    pub added: usize,
    pub rejected: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewItem {
    pub proposal: Proposal,
    pub r#ref: String,
    pub group: String,
    pub subject: String,
    pub who: String,
    pub date: i64,
    pub stale: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewView {
    pub review: Review,
    pub items: Vec<ReviewItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum Decision {
    Accept,
    Decline,
    Change(Action),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AcceptGroup {
    pub accepted: usize,
    pub skipped_stale: usize,
    pub changesets: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UndoReview {
    pub undone: usize,
    pub refused: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Feedback {
    pub account: String,
    pub from: String,
    pub subject: String,
    pub proposed: String,
    pub decision: String,
    pub instead: Option<String>,
}
