//! Types more than one screen shows: the taxonomy, the registry, sessions, chores, finds, jobs.

use chrono::{DateTime, FixedOffset, NaiveDate};
use serde::{Deserialize, Serialize};

pub type Time = DateTime<FixedOffset>;

/// Areas and their streams (plan B.3). Personal filters by stream; the other areas filter whole.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Taxonomy {
    pub areas: Vec<Area>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Area {
    /// `Studies`, `Projects`, `Careers`, `Personal`.
    pub id: String,
    /// The Mocha colour name kairos gives this area (`maroon` for Projects).
    pub colour: String,
    #[serde(default)]
    pub streams: Vec<String>,
    /// Whether filters split this area into its streams (only Personal, 17 Oct).
    #[serde(default)]
    pub filter_by_stream: bool,
}

/// The project registry (plan B.2: the shared root owns it). Every kind of work is a project:
/// code, the CV, a course, an opportunity (plan D, 19 Oct: "include other types of work").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    pub projects: Vec<Project>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    /// Stable id, e.g. `quast`.
    pub id: String,
    pub name: String,
    /// One of the taxonomy's areas.
    pub area: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<String>,
    pub kind: ProjectKind,
    /// tmux sessions that mean "working on this project" (`locus-dev` for locus).
    #[serde(default)]
    pub sessions: Vec<String>,
    /// Parts with their own repository, sized by where the work is (C.6: OQTS is oxdaq and the
    /// website).
    #[serde(default)]
    pub components: Vec<Component>,
    /// Dossier depth (C.6), inferred and overridable; none for work with no dossier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<Depth>,
    /// Shown on Home. A project nobody has touched for weeks is registered but not active.
    pub active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectKind {
    /// A repository with a dossier.
    Code,
    /// Registered, with change records, no dossier (the CV, C.12).
    Light,
    Course,
    /// An application with its own Project view and a prep plan (C.10).
    Opportunity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Depth {
    Field,
    Design,
    Outcome,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Component {
    pub name: String,
    pub repo: String,
}

impl Registry {
    /// The project a tmux session belongs to, if any.
    pub fn by_session(&self, session: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.sessions.iter().any(|s| s == session))
    }

    pub fn get(&self, id: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.id == id)
    }
}

/// A Claude Code session in a project (Slice 3 fills these from Claude Code hooks).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    /// What the session is called in the list: `main chat`, `sweep`.
    pub name: String,
    pub state: SessionState,
    /// When it entered this state; the screen shows the time since.
    pub since: Time,
    /// What it is doing, or what it is asking: `running pytest -k barrier`,
    /// `allow: cargo test --release?`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doing: Option<String>,
    /// What it is for and what it wants, in plain words for someone without the chat's
    /// context (round 1, 21 Oct): "wants to run the full battery on the server". Written from
    /// the transcript by the change-record job; the raw `doing` stays as the detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Started in the background (`claude -p`, or a session you left running).
    #[serde(default)]
    pub background: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// Sorted first: a session that needs you goes to the top, and that is all (screens v2).
    NeedsYou,
    Working,
    Failed,
    Done,
    Idle,
}

/// A short thing you tend to put off, offered in a gap (plan B.1). Owned by kairos.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Chore {
    pub title: String,
    pub minutes: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<NaiveDate>,
}

/// What kind of thing an item in New, Changes or Search is; each has its D.2 glyph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Recheck,
    Redecided,
    Decision,
    Answer,
    ScoutFind,
    Reading,
    Idea,
    Commits,
    Chat,
    Dossier,
    Ink,
    Mail,
    Calendar,
    AutoAction,
}

/// One unread thing in a Project view's New list, or one line of Changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Item {
    pub kind: ItemKind,
    pub text: String,
    pub at: Time,
    #[serde(default)]
    pub unread: bool,
}

/// A scout find or one of your ideas, as a card with its reasoning (C.7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Find {
    pub id: String,
    pub kind: FindKind,
    /// The project it bears on; none for general reading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub title: String,
    /// The card's body: what was found.
    pub summary: String,
    /// Why it matters to you.
    pub why: String,
    /// How it differs from what you have.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub differs: Option<String>,
    /// What it touches: a decision, a section.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub touches: Option<String>,
    /// What it costs to read, in minutes.
    pub minutes: u32,
    pub at: Time,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindKind {
    /// A method or idea that bears on a project (scout job 1).
    Method,
    /// What a course offers a project (job 2).
    Course,
    Reading,
    /// Patterns across your own repositories (job 4).
    Pattern,
    /// A new project idea (job 5): held back unless clearly the best.
    NewProject,
}

/// A background job, running or finished (plan E; locus core owns them).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Job {
    /// The job type: `dossier`, `scout`, `change record`, `ink readback`, `tablet push`, `answers`.
    pub kind: String,
    /// What it is working on: `quast`, `brief v1`.
    pub subject: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub started: Time,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<Time>,
    /// The step it is on while running (`step 4 of 6 · rendering plots`), or what it produced.
    pub detail: String,
    pub outcome: JobOutcome,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state", deny_unknown_fields)]
pub enum JobOutcome {
    Running,
    Done,
    /// Nothing fails silently: a failure carries its cause and what happens next.
    Failed {
        cause: String,
        next: String,
    },
}
