//! The Project view and the three screens it opens: Decisions, Answered, Changes.

use crate::common::{Item, ItemKind, Session, Time};
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
    /// An application's own record: its stages, its timeline from mail, the calendar and what
    /// you sent, and the questions you were asked (round 5). Only an application has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application: Option<Application>,
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
    /// What was decided and why: the choice first, then its reasons, one per item. The ADR's
    /// Decision section (round 2y: the entry's sections follow the ADR template the repos use).
    pub decision: Vec<DecisionPoint>,
    /// The options rejected, each "the option: why it lost" ("keeps me in the loop on other
    /// options so I can reopen questions on why another method might not be better now"). The
    /// ADR's Alternatives section.
    #[serde(default)]
    pub alternatives: Vec<DecisionPoint>,
    /// What has to stay true for it to hold. The ADR's "What would make this wrong".
    #[serde(default)]
    pub assumes: Vec<DecisionPoint>,
    /// What it makes easier and harder, what it gives up, and what rests on it. The ADR's
    /// Consequences.
    #[serde(default)]
    pub consequences: Vec<DecisionPoint>,
    /// The situation that forced the call, with the findings it rested on. The ADR's Context.
    #[serde(default)]
    pub context: Vec<DecisionPoint>,
    /// Your words, each with where you said them; quoted on screen whenever there are any.
    #[serde(default)]
    pub you: Vec<Quote>,
    /// Why it is worth re-checking, when it is: what changed, in a line, and the numbers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recheck: Option<DecisionPoint>,
    /// The decisions it builds on, by id. The screen also lists the reverse: what builds on it.
    #[serde(default)]
    pub rests_on: Vec<String>,
    /// The branch the decision was made on, while it is not yet on the trunk (round 2y: "I
    /// often work on branches and want those decisions to be viewable live as well").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The chat where it was decided, if one is known (19 Oct: a link to the chat).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat: Option<String>,
    /// Decided, replaced or flagged for re-checking since you last looked (round 2l: the
    /// Project view's decisions section marks it, as New marks an unread item).
    #[serde(default)]
    pub new: bool,
}

/// One item in a decision's section: a short label, read at a glance and joined into the
/// section's folded line, and a short detail shown when it opens (round 2za: "each section is
/// still too long and hard to parse ... my preference of being shorter so that I actually read
/// it").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionPoint {
    /// Two to five words.
    pub label: String,
    /// One short line, or empty when the label says it all.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
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
    /// When you last opened Changes: entries after it are new, and a line marks the place
    /// (round 4: "a you were here last would be good too").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<Time>,
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

/// What the Application screen shows beyond where you are (round 5, 23 Oct: "a timeline view
/// built from the emails/ kairos/ submissions etc ... would need to look different than the git
/// branches as it is a different concept").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Application {
    /// The programme, one line: `Quantitative Research Internship, London, summer 2027`.
    pub role: String,
    /// The stages in order, past and to come.
    pub stages: Vec<Stage>,
    /// Everything dated, oldest first: mail, interviews, assessments, what you sent.
    #[serde(default)]
    pub events: Vec<AppEvent>,
    /// The questions you were asked, by round, pinned down with you after the interview
    /// (round 5: "definitely capture the questions I was asked ... my rough notes during the
    /// interview are not reliable enough, so it should be something I interact with").
    #[serde(default)]
    pub questions: Vec<AskedQuestion>,
    /// The next dated thing, if one is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<NextDate>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stage {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<NaiveDate>,
    pub state: StageState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageState {
    /// Where the application itself began, applied or invited: a mark of its own on the track,
    /// not a step passed (round 5e).
    Start,
    Done,
    Current,
    Upcoming,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppEvent {
    pub at: Time,
    pub kind: AppEventKind,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detail: Vec<String>,
    /// The calendar event this is (spec K8): a kairos event uid. An interview on the timeline
    /// is that event, and `o` opens it in kairos.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppEventKind {
    Mail,
    Interview,
    Assessment,
    Submission,
    Call,
    Outcome,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskedQuestion {
    /// The round it was asked in: `final round · trader`.
    pub round: String,
    pub text: String,
    /// The track it bears on, so it can feed your learning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<String>,
    /// `draft` from your notes until you have pinned it down; then `confirmed`.
    pub state: QuestionState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionState {
    Draft,
    Confirmed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NextDate {
    pub at: Time,
    pub what: String,
}
