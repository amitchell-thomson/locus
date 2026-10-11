//! Changesets: every write kairos makes, with full pre- and post-images so
//! it can be audited and undone.

use crate::model::Author;
use crate::task::Task;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    Low,
    High,
}

impl Risk {
    pub fn as_str(self) -> &'static str {
        match self {
            Risk::Low => "low",
            Risk::High => "high",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    Applied,
    Rejected,
    Undone,
    Failed,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::Applied => "applied",
            Status::Rejected => "rejected",
            Status::Undone => "undone",
            Status::Failed => "failed",
        }
    }
    pub fn parse(s: &str) -> Option<Status> {
        Some(match s {
            "pending" => Status::Pending,
            "applied" => Status::Applied,
            "rejected" => Status::Rejected,
            "undone" => Status::Undone,
            "failed" => Status::Failed,
            _ => return None,
        })
    }
}

/// One file in a vdir collection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub source: String,
    pub href: String,
}

/// An operation with full images. Event images are whole `.ics` file
/// contents (a VCALENDAR with the master and any overrides).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    CreateEvent {
        item: Item,
        uid: String,
        after: String,
    },
    UpdateEvent {
        item: Item,
        uid: String,
        before: String,
        after: String,
    },
    DeleteEvent {
        item: Item,
        uid: String,
        before: String,
    },
    /// Move between calendars (re-key, share to family): delete + create,
    /// keeping the UID.
    MoveEvent {
        from: Item,
        to: Item,
        uid: String,
        before: String,
        after: String,
        share_to_family: bool,
    },
    CreateTask {
        after: Box<Task>,
    },
    UpdateTask {
        before: Box<Task>,
        after: Box<Task>,
    },
}

impl Op {
    pub fn describe(&self) -> String {
        match self {
            Op::CreateEvent { item, .. } => format!("create event in {}", item.source),
            Op::UpdateEvent { item, .. } => format!("update event in {}", item.source),
            Op::DeleteEvent { item, .. } => format!("delete event from {}", item.source),
            Op::MoveEvent {
                from,
                to,
                share_to_family,
                ..
            } => {
                if *share_to_family {
                    format!("share event from {} to Family", from.source)
                } else {
                    format!("move event {} → {}", from.source, to.source)
                }
            }
            Op::CreateTask { after } => format!("create task {:?}", after.title),
            Op::UpdateTask { after, .. } => format!("update task {:?}", after.title),
        }
    }

    /// Items written by this op (for post-write sync).
    pub fn items(&self) -> Vec<&Item> {
        match self {
            Op::CreateEvent { item, .. } | Op::UpdateEvent { item, .. } | Op::DeleteEvent { item, .. } => vec![item],
            Op::MoveEvent { from, to, .. } => vec![from, to],
            _ => vec![],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Changeset {
    pub id: String,
    pub author: Author,
    pub created_at: DateTime<Utc>,
    pub ops: Vec<Op>,
    /// Computed by [`crate::risk::assess`], never supplied by the caller.
    pub risk: Risk,
    /// Why the policy said high (empty for low).
    #[serde(default)]
    pub reasons: Vec<String>,
    pub status: Status,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<DateTime<Utc>>,
}

/// What write tools return (SPEC §8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WriteResult {
    pub changeset_id: String,
    pub risk: Risk,
    pub status: Status,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<String>,
}

impl From<&Changeset> for WriteResult {
    fn from(c: &Changeset) -> Self {
        WriteResult {
            changeset_id: c.id.clone(),
            risk: c.risk,
            status: c.status,
            summary: c.summary.clone(),
            reasons: c.reasons.clone(),
        }
    }
}

pub fn new_id() -> String {
    format!("cs-{}", &uuid::Uuid::new_v4().simple().to_string()[..10])
}
