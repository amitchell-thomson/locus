//! What each locus screen shows.
//!
//! Plan D: "once a screen settles, what it shows becomes a typed contract in the shared root,
//! and the backend is built to fill it." These types are that contract. The TUI reads them from
//! fixture files during Slice 1 and from the HTTP service once a screen is wired; JSON Schema is
//! exported from them for the Python side (Slice 1 spec, D3). The first cut per screen lands in
//! step 1.3 and is frozen in 1.6.

use serde::{Deserialize, Serialize};

/// The fixture set's manifest: which snapshot this is and when it was taken.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Short name of the set, e.g. `default` or `empty`.
    pub name: String,
    /// The moment the snapshot pretends it is (ISO 8601, Europe/London).
    pub now: String,
    pub description: String,
}

/// The project registry (plan B.2: the shared root owns areas, streams and the repository
/// registry). Slice 1 needs it for the Project view to follow tmux; step 1.3 fills it out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registry {
    pub projects: Vec<Project>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    /// Stable id, e.g. `quast`.
    pub id: String,
    pub name: String,
    /// One of the four areas (plan B.3): Studies, Projects, Careers, Personal.
    pub area: String,
    /// tmux sessions that mean "working on this project" (`locus-dev` for locus).
    pub sessions: Vec<String>,
}

impl Registry {
    /// The project a tmux session belongs to, if any.
    pub fn by_session(&self, session: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.sessions.iter().any(|s| s == session))
    }
}
