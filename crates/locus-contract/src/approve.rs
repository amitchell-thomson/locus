//! The approve list and the auto log (plan D).

use crate::common::Time;
use serde::{Deserialize, Serialize};

/// The one approve list, grouped by kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Approve {
    #[serde(default)]
    pub groups: Vec<ApproveGroup>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApproveGroup {
    pub kind: ApproveKind,
    /// The heading when the kind alone is too coarse: mail filing is split by what it would do
    /// ("archive", "file → Admin"), so `Y` approves one kind of action at a time (10 Nov).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default)]
    pub items: Vec<ApproveItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApproveKind {
    /// A date found in your mail (an interview booked or moved), to put on the calendar linked
    /// to its application (spec E6).
    Date,
    /// A change to your calendar waiting for you (a kairos changeset from Claude or a script):
    /// one approve place, here, not a second list in kairos (round 7c).
    Calendar,
    /// A mail Claude drafted, waiting for your yes to send (keryx; X2: one approve place).
    MailSend,
    /// Where Claude would file or archive a thread (a keryx cleanup proposal).
    MailFiling,
    /// A fact from mail waiting for your yes (heuris H2, D2): a conflict with what you said, or
    /// not from a sender of authority.
    Fact,
}

/// What deciding a mail item acts on in keryx.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MailDecision {
    /// A pending send (a keryx changeset id): yes sends it, after the hold.
    Send(String),
    /// A cleanup proposal id: yes applies it.
    Proposal(String),
}

/// A date from mail, and what accepting it does: a new event, or moving the one already there.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MailDate {
    /// The application it is for (a registry id).
    pub for_id: String,
    /// The event's title: `Millennium interview`.
    pub title: String,
    pub at: crate::Time,
    pub minutes: u32,
    /// The mail it was read from (`personal:11801`).
    pub thread: String,
    /// When the calendar already has this application's event at another time: (source, uid)
    /// of the event to move.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moves: Option<(String, String)>,
    /// When the mail could be about more than one of the firm's applications: all of them,
    /// `for_id` the one chosen so far (←/→ in Approve). Never guessed for you.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApproveItem {
    pub text: String,
    /// Where it would go: `careers/g-research`, `archive`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// Which app proposed it: `keryx`, `kairos`, `heuris`.
    pub from: String,
    /// Arrived since you last opened the approve list (round 1d: Home shows "2 new").
    #[serde(default)]
    pub new: bool,
    /// For a date from mail: the date and what accepting it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<MailDate>,
    /// For a calendar change: the kairos changeset id to approve or refuse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changeset: Option<String>,
    /// For a mail item: what yes or no acts on in keryx.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mail: Option<MailDecision>,
    /// For a proposed fact: its id in heuris's ledger, approved or rejected there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fact: Option<u64>,
    /// What the detail pane shows for it (10 Nov: "easier to review the individual actions").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<ApproveDetail>,
}

/// One approve item, laid out for reviewing: the action first, then whom and what it is about,
/// why it was proposed, and the thing itself (the start of the mail, the calendar change).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApproveDetail {
    /// The action in a word or two, shown as a badge: `ARCHIVE`, `FILE → Admin`, `CALENDAR`.
    pub action: String,
    /// The sender, or who it is with.
    #[serde(default)]
    pub who: String,
    #[serde(default)]
    pub subject: String,
    /// Account and date, or where it comes from.
    #[serde(default)]
    pub meta: String,
    /// Why it was proposed, as its proposer wrote it.
    #[serde(default)]
    pub reason: String,
    /// The thing itself, as plain text.
    #[serde(default)]
    pub body: String,
}

impl Approve {
    pub fn len(&self) -> usize {
        self.groups.iter().map(|g| g.items.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Everything done automatically, live, with undo where it can be undone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutoLog {
    /// Newest first.
    #[serde(default)]
    pub entries: Vec<AutoAction>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutoAction {
    pub at: Time,
    pub text: String,
    pub undo: bool,
    /// For a fact written from mail or the calendar: its id, which `u` retires (heuris H2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fact: Option<u64>,
}
