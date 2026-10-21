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
    /// Everything new, newest first; read items stay in Changes. Never a decision: a new or
    /// re-checked decision is marked `new` in the decision log, so it shows once (round 2l).
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
    /// The questions that matter most, from the dossier's front page (plan C.6): the bigger
    /// picture behind next (round 2d: "I want it to always remind me of what is next to
    /// address bigger picture than the next in the where you are section").
    #[serde(default)]
    pub questions: Vec<String>,
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
    pub kind: DueKind,
    pub at: NaiveDate,
}

/// What kind of thing is due. A sheet is its own kind because it is planned differently: a
/// weekly problem sheet is about five hours in two or three blocks (kairos's `sheet`), where
/// coursework needs its own estimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DueKind {
    Sheet,
    Coursework,
    Report,
    Exam,
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
    /// What was done on the branch, in plain sentences (round 2b: "more detail about what was
    /// done on each branch, in clear natural language, not this weird note form").
    #[serde(default)]
    pub summary: String,
    /// The branch's change records, oldest first: what Enter on the lane opens (round 2h). Its
    /// commits until change records exist (Slice 2).
    #[serde(default)]
    pub records: Vec<Record>,
}

/// The decisions in a project's decision log (C.6), with their live status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub date: NaiveDate,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decisions {
    pub project: String,
    /// The log's path in the repository, where Enter would open the full entry.
    pub log: String,
    #[serde(default)]
    pub items: Vec<Decision>,
}

/// One entry in a repo's decision log, with everything needed to understand it on its own
/// (round 2q: "I want more detail so that I can properly understand everything about the
/// decision"). Text never names another decision by its number; links go through `rests_on`
/// and `Redecided::by`, and the screen names the other decision by its title.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    /// The id the repository uses: `ADR-006`, `§8bn`. For links only, never shown.
    pub id: String,
    pub title: String,
    pub date: NaiveDate,
    pub status: DecisionStatus,
    /// Two lines at most, in plain words: what was chosen and the one biggest reason. The entry
    /// leads with it (round 2w: the entry was "a little too dense, and time consuming to read
    /// such that I might skip over it").
    pub brief: String,
    /// What needed deciding, and why it came up then.
    pub question: String,
    /// What was chosen, plainly.
    pub chose: String,
    /// Why: the reasons, one per item, each short (round 2w: bullets, not a paragraph).
    pub why: Vec<String>,
    /// The options rejected, each with why it lost ("keeps me in the loop on other options so
    /// I can reopen questions on why another method might not be better now").
    #[serde(default)]
    pub over: Vec<String>,
    /// What has to stay true for it to hold.
    #[serde(default)]
    pub assumes: Vec<String>,
    /// What it gives up: the trade-offs taken knowingly.
    #[serde(default)]
    pub costs: Vec<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
    /// Your words, each with where you said them; quoted on screen whenever there are any.
    #[serde(default)]
    pub you: Vec<Quote>,
    /// Why it is worth re-checking, when it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recheck: Option<String>,
    /// The decisions it builds on, by id. The screen also lists the reverse: what builds on it.
    #[serde(default)]
    pub rests_on: Vec<String>,
    /// What else rests on it: code, steps, other projects.
    #[serde(default)]
    pub affects: Vec<String>,
    /// The chat where it was decided, if one is known (19 Oct: a link to the chat).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat: Option<String>,
    /// Decided, replaced or flagged for re-checking since you last looked (round 2l: the
    /// Project view's decisions section marks it, as New marks an unread item).
    #[serde(default)]
    pub new: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state", deny_unknown_fields)]
pub enum DecisionStatus {
    Holding,
    Recheck,
    /// Reopened by you and being looked at again, in a chat with it as context (round 2q).
    Review,
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
    /// The decision it was asked about, by id, when asked from the Decisions page; the entry
    /// shows it under "asked" (round 2v).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
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
