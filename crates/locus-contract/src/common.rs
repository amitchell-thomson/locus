//! Types more than one screen shows: the taxonomy, the registry, sessions, chores, finds, jobs.

use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};

pub type Time = DateTime<FixedOffset>;

/// Areas and their streams (plan B.3). Personal filters by stream; the other areas filter whole.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Taxonomy {
    pub areas: Vec<Area>,
    /// The kinds a registry entry can have (ADR-006), with what each gets.
    #[serde(default)]
    pub kinds: Vec<Named>,
    /// A task's one type (sheet, coursework, admin, chore); a due date is its attribute.
    #[serde(default)]
    pub task_types: Vec<Named>,
}

/// An id with a line saying what it is (a kind, a task type).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Named {
    pub id: String,
    pub about: String,
}

impl Taxonomy {
    pub fn area(&self, id: &str) -> Option<&Area> {
        self.areas.iter().find(|a| a.id == id)
    }

    /// Whether `name` is an area or a stream of one (`Studies`, `Errands`).
    pub fn resolves(&self, name: &str) -> bool {
        self.area_of(name).is_some()
    }

    /// The Mocha colour of an area or a stream (`Studies` blue, `Sport` green): the one place
    /// both apps take colours from (L3).
    pub fn colour_of(&self, name: &str) -> Option<&str> {
        let a = self.area_of(name)?;
        Some(a.stream_colours.get(name).map_or(a.colour.as_str(), String::as_str))
    }

    /// The area a name belongs to: an area is its own, a stream its parent's (`Sport` -> Personal).
    pub fn area_of(&self, name: &str) -> Option<&Area> {
        self.areas
            .iter()
            .find(|a| a.id == name || a.streams.iter().any(|s| s == name))
    }
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
    /// Each stream's Mocha colour (Sport `green`); a stream without one takes the area's.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub stream_colours: std::collections::BTreeMap<String, String>,
}

/// The registry (plan B.2: the shared root owns it): one entry for each thing you work on,
/// of three kinds (ADR-006).
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
    /// The course a repo does the work of (3YP for B3). One kind per thing; a thing that seems
    /// to be two kinds is two linked things (ADR-006).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub course: Option<String>,
    /// The firm an application is with: one company can have several applications (Brevan
    /// Howard 2026 and 2027), and the tablet's Careers folders are per company, so capture
    /// matches a folder to this (Slice 0, 0.12).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub company: Option<String>,
}

/// What a registry entry is (ADR-006). The kind decides which view shows it and whether
/// anything follows it; the area decides where Home lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectKind {
    /// A repository you work in: tmux sessions, Claude chats, change records, a branch graph.
    /// The only kind the Project window shows and follows, in any area: quast (Projects), 3YP
    /// (Studies), the CV (Careers).
    Repo,
    /// A course: notes, lectures, Canvas deadlines, topic tracks; no repository of its own.
    Course,
    /// A pursuit with stages owned by a company: an internship or job application. No
    /// repository or sessions; it has a stage and whose move it is, dates, mail, a prep plan.
    Application,
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
    /// More about it, a paragraph per entry: a commit's message, what a chat decided. Changes
    /// shows it beside the timeline (round 4).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detail: Vec<String>,
    /// Set on mechanical work that recurs (a nightly job's commit): items with the same name fold
    /// into one row on Changes, so they do not crowd out your own work (round 4: "they would
    /// crowd out actual interesting work by me").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routine: Option<String>,
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
