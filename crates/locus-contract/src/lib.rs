//! What each locus screen shows.
//!
//! Plan D: "once a screen settles, what it shows becomes a typed contract in the shared root,
//! and the backend is built to fill it." These types are that contract. The TUI reads them from
//! fixture files during Slice 1 and from the HTTP service once a screen is wired; JSON Schema is
//! exported from them for the Python side (Slice 1 spec, D3). The first cut per screen landed in
//! step 1.3 and is frozen in 1.6.
//!
//! Every struct refuses unknown fields: a backend that renames a field fails loudly at the
//! boundary rather than leaving a section silently empty (CLAUDE.md §3).

pub mod common;
pub mod explore;
pub mod fors;
pub mod home;
pub mod learning;
pub mod link;
pub mod project;
pub mod review;
pub mod system;
pub mod tasks;
pub mod taxonomy;
pub mod uselog;

pub use common::*;
pub use explore::*;
pub use home::*;
pub use learning::*;
pub use project::*;
pub use review::*;
pub use system::*;
pub use tasks::*;

use serde::{Deserialize, Serialize};

/// The fixture set's manifest: which snapshot this is and when it was taken.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Short name of the set, e.g. `default` or `empty`.
    pub name: String,
    /// The moment the snapshot pretends it is (Europe/London).
    pub now: Time,
    pub description: String,
}

/// Which contract types each screen shows, for CONTRACTS.md and the freeze test (step 1.6).
/// Screen names are the TUI's (`locus-tui --dump <name>`).
pub const SCREENS: &[(&str, &[&str])] = &[
    (
        "home",
        &["Home", "Work", "ProjectView", "Explore", "Approve", "System", "Usage"],
    ),
    ("project", &["ProjectView", "BranchGraph"]),
    ("system", &["System", "Usage"]),
    ("budget", &["Usage"]),
    ("explore", &["Explore"]),
    ("approve", &["Approve"]),
    ("autolog", &["AutoLog"]),
    ("ink", &["InkConfirm"]),
    ("decisions", &["Decisions"]),
    ("answered", &["Answered"]),
    ("changes", &["Changes"]),
    ("search", &["Search"]),
    ("work", &["Work"]),
    ("prep", &["Prep", "Learning"]),
    ("learning", &["Learning"]),
    ("review", &["Review"]),
];
