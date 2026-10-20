//! The Project view and the three screens it opens: Decisions, Answered, Changes.

use crate::common::{Chore, Item, ItemKind, Session, Time};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// The Project view (plan D): one screen in every state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectView {
    pub project: String,
    /// A short stage label in the header: `barrier control`, `final round 1 Oct`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    /// Where you are, in at most three slowly changing lines: the state, where it falls short,
    /// the next step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub where_you_are: Option<WhereYouAre>,
    /// Everything new, newest first; read items stay in Changes.
    #[serde(default)]
    pub new: Vec<Item>,
    /// Only this project's sessions.
    #[serde(default)]
    pub sessions: Vec<Session>,
    /// The chores that fit the gap you are in.
    #[serde(default)]
    pub chores: Vec<Chore>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph: Option<BranchGraph>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WhereYouAre {
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub short: Option<String>,
    pub next: String,
}

/// The branch graph (plan D.1): each branch a lane, collapsed to its fork, its merge and its
/// milestones, each stage labelled from its change records (by hand in Slice 1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BranchGraph {
    pub trunk: String,
    /// The first day drawn.
    pub from: NaiveDate,
    /// Points on the trunk itself.
    #[serde(default)]
    pub milestones: Vec<Milestone>,
    /// Oldest first.
    #[serde(default)]
    pub lanes: Vec<Lane>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Milestone {
    pub date: NaiveDate,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lane {
    /// The branch name as git has it.
    pub branch: String,
    /// What this stage was, in words (`SMC replaces the flow`).
    pub stage: String,
    pub forked: NaiveDate,
    /// None while the branch is open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged: Option<NaiveDate>,
    /// The last commit's date, for an open branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tip: Option<NaiveDate>,
    pub commits: u32,
    /// What selecting the lane shows: what happened in it.
    #[serde(default)]
    pub happened: Vec<String>,
}

/// The decisions in a project's decision log (C.6), with their live status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decisions {
    pub project: String,
    /// The log's path in the repository, where Enter would open the full entry.
    pub log: String,
    #[serde(default)]
    pub items: Vec<Decision>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    /// The id the repository uses: `ADR-006`, `§8bn`.
    pub id: String,
    pub title: String,
    pub date: NaiveDate,
    pub status: DecisionStatus,
    /// What was chosen, and the reason.
    pub chose: String,
    /// The options rejected, each with why.
    #[serde(default)]
    pub over: Vec<String>,
    /// Your words, with where you said them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub you: Option<Quote>,
    #[serde(default)]
    pub evidence: Vec<String>,
    /// Why it is worth re-checking, when it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recheck: Option<String>,
    /// What rests on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub affects: Option<String>,
    /// The chat where it was decided, if one is known (19 Oct: a link to the chat).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state", deny_unknown_fields)]
pub enum DecisionStatus {
    Holding,
    Recheck,
    /// Replaced by a later decision.
    Redecided {
        by: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Quote {
    pub text: String,
    pub source: String,
}

/// Every answer for the project, from the ask line or a circled `?` (plan D, Ask and Answered).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Answered {
    pub project: String,
    #[serde(default)]
    pub items: Vec<Answer>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Answer {
    /// The question, short.
    pub title: String,
    pub asked: AskedFrom,
    /// The passage it is linked to: `dossier p4`, `brief v1 §2`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passage: Option<String>,
    pub at: Time,
    /// The answer, a paragraph per entry.
    pub body: Vec<String>,
    /// Links the linking job found, each with its reason (C.4).
    #[serde(default)]
    pub links: Vec<String>,
    /// The read-only Claude thread behind it; `f` continues it, `c` opens it as a session.
    pub thread: String,
    #[serde(default)]
    pub unread: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AskedFrom {
    /// Typed on the ask line.
    Ask,
    /// A circled `?` on paper.
    Ink,
}

/// The full timeline behind New.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Changes {
    pub project: String,
    pub since: NaiveDate,
    /// Newest first.
    #[serde(default)]
    pub entries: Vec<Item>,
}

impl ProjectView {
    pub fn unread(&self) -> usize {
        self.new.iter().filter(|i| i.unread).count()
    }

    /// The most recent thing to happen here, for "a project with nothing new".
    pub fn last_new(&self) -> Option<Time> {
        self.new.iter().map(|i| i.at).max()
    }

    pub fn has_recheck(&self) -> bool {
        self.new.iter().any(|i| i.unread && i.kind == ItemKind::Recheck)
    }
}
