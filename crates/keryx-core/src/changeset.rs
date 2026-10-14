//! Changesets (SPEC §4): every write keryx makes, with full pre- and
//! post-images so it can be audited, checked before applying, and undone.

use crate::model::Location;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Author {
    Alec,
    Claude,
    /// Non-interactive CLI, scripts, and requests without an author header.
    Script,
    /// keryxd itself (snooze wake-ups).
    System,
}

impl Author {
    pub fn as_str(self) -> &'static str {
        match self {
            Author::Alec => "alec",
            Author::Claude => "claude",
            Author::Script => "script",
            Author::System => "system",
        }
    }
    /// Header values; anything unknown (or missing) is `Script` — stricter
    /// than kairos, whose default is Alec. `system` can't be claimed.
    pub fn from_header(s: Option<&str>) -> Author {
        match s.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
            Some("alec") => Author::Alec,
            Some("claude") => Author::Claude,
            _ => Author::Script,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    Low,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    /// A send waiting out its hold (Phase 6).
    Holding,
    Applied,
    Sent,
    Rejected,
    Cancelled,
    Undone,
    Failed,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::Holding => "holding",
            Status::Applied => "applied",
            Status::Sent => "sent",
            Status::Rejected => "rejected",
            Status::Cancelled => "cancelled",
            Status::Undone => "undone",
            Status::Failed => "failed",
        }
    }
    pub fn parse(s: &str) -> Option<Status> {
        Some(match s {
            "pending" => Status::Pending,
            "holding" => Status::Holding,
            "applied" => Status::Applied,
            "sent" => Status::Sent,
            "rejected" => Status::Rejected,
            "cancelled" => Status::Cancelled,
            "undone" => Status::Undone,
            "failed" => Status::Failed,
            _ => return None,
        })
    }
}

/// One message as an op sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MsgRef {
    pub account: String,
    pub thread_id: String,
    pub source_id: String,
    /// The Message-ID header: how a message is re-found if Mail renumbers it.
    pub message_id: String,
}

/// A message's state at the source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MsgState {
    pub location: Location,
    pub read: bool,
    pub flagged: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// Location, read and flag changes of one message (archive, file,
    /// un-archive, flag, read are all this op).
    State {
        msg: MsgRef,
        before: MsgState,
        after: MsgState,
    },
    /// keryx's own snooze record for a thread (the archive is a State op).
    Snooze {
        account: String,
        thread_id: String,
        until: i64,
        /// The snooze it replaces, if any (for undo).
        previous: Option<i64>,
    },
    Unsnooze {
        account: String,
        thread_id: String,
        until: i64,
    },
}

impl Op {
    /// The inverse op (undo), when there is one.
    pub fn inverse(&self) -> Op {
        match self {
            Op::State { msg, before, after } => Op::State {
                msg: msg.clone(),
                before: after.clone(),
                after: before.clone(),
            },
            Op::Snooze {
                account,
                thread_id,
                until,
                previous,
            } => match previous {
                Some(p) => Op::Snooze {
                    account: account.clone(),
                    thread_id: thread_id.clone(),
                    until: *p,
                    previous: Some(*until),
                },
                None => Op::Unsnooze {
                    account: account.clone(),
                    thread_id: thread_id.clone(),
                    until: *until,
                },
            },
            Op::Unsnooze {
                account,
                thread_id,
                until,
            } => Op::Snooze {
                account: account.clone(),
                thread_id: thread_id.clone(),
                until: *until,
                previous: None,
            },
        }
    }

    pub fn thread(&self) -> (&str, &str) {
        match self {
            Op::State { msg, .. } => (&msg.account, &msg.thread_id),
            Op::Snooze { account, thread_id, .. } | Op::Unsnooze { account, thread_id, .. } => (account, thread_id),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Changeset {
    pub id: String,
    pub author: Author,
    pub interactive: bool,
    pub created_at: i64,
    pub ops: Vec<Op>,
    pub risk: Risk,
    pub reasons: Vec<String>,
    pub status: Status,
    pub summary: String,
    /// Cleanup review this decision belongs to, if any.
    #[serde(default)]
    pub review_id: Option<String>,
    /// Set when the changeset is itself an undo.
    #[serde(default)]
    pub undoes: Option<String>,
    /// Why it failed, if it did.
    #[serde(default)]
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn author_header_defaults_to_script() {
        assert_eq!(Author::from_header(Some("alec")), Author::Alec);
        assert_eq!(Author::from_header(Some(" Claude ")), Author::Claude);
        assert_eq!(
            Author::from_header(Some("system")),
            Author::Script,
            "system can't be claimed"
        );
        assert_eq!(Author::from_header(None), Author::Script);
        assert_eq!(Author::from_header(Some("root")), Author::Script);
    }

    #[test]
    fn inverses_round_trip() {
        let m = MsgRef {
            account: "a".into(),
            thread_id: "t".into(),
            source_id: "1".into(),
            message_id: "x".into(),
        };
        let s = |inbox: bool| MsgState {
            location: Location::Labels {
                labels: if inbox { vec!["INBOX".into()] } else { vec![] },
            },
            read: true,
            flagged: false,
        };
        let op = Op::State {
            msg: m,
            before: s(true),
            after: s(false),
        };
        assert_eq!(op.inverse().inverse(), op);
        let sn = Op::Snooze {
            account: "a".into(),
            thread_id: "t".into(),
            until: 5,
            previous: None,
        };
        assert!(matches!(sn.inverse(), Op::Unsnooze { until: 5, .. }));
        let re = Op::Snooze {
            account: "a".into(),
            thread_id: "t".into(),
            until: 9,
            previous: Some(5),
        };
        assert!(matches!(
            re.inverse(),
            Op::Snooze {
                until: 5,
                previous: Some(9),
                ..
            }
        ));
    }

    #[test]
    fn status_strings() {
        for s in [
            Status::Pending,
            Status::Holding,
            Status::Applied,
            Status::Sent,
            Status::Rejected,
            Status::Cancelled,
            Status::Undone,
            Status::Failed,
        ] {
            assert_eq!(Status::parse(s.as_str()), Some(s));
        }
    }
}
