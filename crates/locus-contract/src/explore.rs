//! Explore (C.7), the approve list and the auto log (plan D), and ink confirm (C.5).

use crate::common::{Find, Time};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Explore {
    /// The scout's finds waiting for a grade; at most `queue_cap`, and when full the scout waits.
    #[serde(default)]
    pub queue: Vec<Find>,
    pub queue_cap: u32,
    #[serde(default)]
    pub ideas: Vec<Idea>,
    /// What is on the tablet in `/Reading`.
    #[serde(default)]
    pub reading: Vec<Reading>,
    pub reading_cap: u32,
    /// Strike reasons so far, for the weekly review and for tuning.
    #[serde(default)]
    pub strikes: Vec<Strike>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Idea {
    pub title: String,
    pub from: IdeaFrom,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub state: IdeaState,
    pub at: Time,
    /// A new project idea that is the fortnight's best (C.7, job 5).
    #[serde(default)]
    pub best_of_fortnight: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdeaFrom {
    You,
    Jot,
    Scout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdeaState {
    Open,
    /// Handed to the scout; it comes back as a find.
    Handed,
    Parked,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reading {
    pub title: String,
    pub state: ReadingState,
    /// When it went to the tablet.
    pub since: NaiveDate,
    /// Pages read, when the tablet says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadingState {
    Untouched,
    InProgress,
    Done,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Strike {
    pub reason: StrikeReason,
    pub count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrikeReason {
    Irrelevant,
    KnewIt,
    WrongLevel,
    TooLong,
}

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
    #[serde(default)]
    pub items: Vec<ApproveItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApproveKind {
    /// A jot page item and where it would be filed (C.8).
    Filing,
    Mail,
    Deadline,
    /// Marks the readback could not place; Enter opens ink confirm.
    Ink,
    /// "You approved this kind N times: do it automatically?"
    StopAsking,
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
    /// Which app proposed it: `keryx`, `jot`, `ink readback`.
    pub from: String,
    /// Arrived since you last opened the approve list (round 1d: Home shows "2 new").
    #[serde(default)]
    pub new: bool,
    /// For an ink item: the ink-confirm fixture it opens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opens: Option<String>,
    /// For a date from mail: the date and what accepting it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<MailDate>,
    /// For a calendar change: the kairos changeset id to approve or refuse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changeset: Option<String>,
    /// For a mail item: what yes or no acts on in keryx.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mail: Option<MailDecision>,
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
    /// For a kind moved to automatic by "stop asking": since when.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub automatic_since: Option<NaiveDate>,
}

/// A mark the readback could not place: its crop beside the passages it might point at (C.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InkConfirm {
    pub document: String,
    pub page: u32,
    /// The crop of the mark and its surroundings, a PNG beside the fixture.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// What the readback made of the note, short.
    pub note: String,
    /// Two or three candidates; you press the number, or `x` for "a general note".
    #[serde(default)]
    pub candidates: Vec<Passage>,
    /// Further marks waiting after this one.
    #[serde(default)]
    pub remaining: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Passage {
    /// `§2 ¶3`, `D9`.
    pub at: String,
    pub text: String,
}
