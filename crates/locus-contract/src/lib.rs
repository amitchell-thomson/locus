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
