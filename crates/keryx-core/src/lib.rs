//! `keryx-core`: pure email logic. No network, no SQLite; file access is
//! limited to reading config files given an explicit path.

pub mod applescript;
pub mod attach;
pub mod automated;
pub mod build;
pub mod changeset;
pub mod colour;
pub mod compose;
pub mod config;
pub mod display;
pub mod invite;
pub mod location;
pub mod mime;
pub mod model;
pub mod outgoing;
pub mod query;
pub mod reply;
pub mod risk;
pub mod text;
pub mod thread;
pub mod unsubscribe;
pub mod when;

pub use chrono_tz::Tz;
