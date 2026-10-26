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
        /// The registry item the thread is for (`b14`), linked when filed (X7).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item: Option<String>,
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
            Intent::File { key, item: Some(i) } => format!("file into {key} for {i}"),
            Intent::File { key, .. } => format!("file into {key}"),
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

/// A message Alec (or a script) asks keryxd to send.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SendReq {
    pub account: String,
    /// Replying in this thread (`account:thread`): threading headers come from it.
    #[serde(default)]
    pub thread: Option<String>,
    pub to: Vec<keryx_core::model::Address>,
    #[serde(default)]
    pub cc: Vec<keryx_core::model::Address>,
    #[serde(default)]
    pub bcc: Vec<keryx_core::model::Address>,
    pub subject: String,
    pub body: String,
    /// Local file paths (Alec only; Claude's drafts can't attach).
    #[serde(default)]
    pub attachments: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DraftReplyReq {
    pub r#ref: String,
    pub body: String,
    #[serde(default)]
    pub reply_all: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DraftEmailReq {
    pub account: String,
    pub to: Vec<keryx_core::model::Address>,
    #[serde(default)]
    pub cc: Vec<keryx_core::model::Address>,
    pub subject: String,
    pub body: String,
}

/// What `L` (unsubscribe) does for a thread.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UnsubscribeOutcome {
    /// One-click done (and the thread archived).
    Done {
        changeset: Changeset,
    },
    /// Send this email (through the normal confirm) to unsubscribe.
    Mailto {
        account: String,
        to: String,
        subject: String,
        body: String,
    },
    /// Only a web page: keryx never visits it.
    Link {
        url: String,
    },
    None,
}

/// `t`: a kairos task from a thread (Alec, or a script).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct KairosTaskReq {
    pub r#ref: String,
    pub title: String,
    /// kairos calendar key (default: the thread's key's `kairos_key`, else Personal).
    #[serde(default)]
    pub key: Option<String>,
    /// Task type: sheet | coursework | admin | …
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub estimate_min: Option<u32>,
    /// Raw due string; kairos parses it ("fri 17:00").
    #[serde(default)]
    pub due: Option<String>,
    /// Accepting this suggestion.
    #[serde(default)]
    pub suggestion: Option<String>,
}

/// `i`: a kairos event from an invite.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct KairosEventReq {
    pub r#ref: String,
    pub title: String,
    pub start: String,
    #[serde(default)]
    pub end: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub suggestion: Option<String>,
}

/// What keryx did in kairos.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KairosOutcome {
    pub link: crate::state::Link,
    /// One line for the status bar ("task created · 3 blocks planned").
    pub summary: String,
    /// kairos's own words worth showing (shortfalls, squeezes, pending).
    #[serde(default)]
    pub notes: Vec<String>,
}

/// `w`: free slots between two times.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FreeReq {
    pub from: String,
    pub to: String,
    pub duration_min: u32,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FreeOut {
    /// (start, end) Unix seconds.
    pub slots: Vec<(i64, i64)>,
    /// One `Tue 15 Oct 14:00–15:00` line per slot.
    pub text: String,
}

/// A linked task's live state, for the thread view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskStatus {
    pub id: String,
    pub title: String,
    /// open | done | dropped
    pub status: String,
    #[serde(default)]
    pub due: Option<String>,
    #[serde(default)]
    pub remaining_min: Option<u32>,
}

/// Claude's `suggest_task`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SuggestTaskReq {
    pub r#ref: String,
    pub title: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub estimate_min: Option<u32>,
    #[serde(default)]
    pub due: Option<String>,
    pub reason: String,
}

/// Claude's `suggest_event`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SuggestEventReq {
    pub r#ref: String,
    pub title: String,
    pub start: String,
    #[serde(default)]
    pub end: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    pub reason: String,
}
