//! `kairos-core`: pure calendar logic. No network, no SQLite; file access is
//! limited to reading config files given an explicit path.

pub mod api;
pub mod build;
pub mod changeset;
pub mod colour;
pub mod config;
pub mod constraints;
pub mod dates;
pub mod dedup;
pub mod expand;
pub mod ics;
pub mod model;
pub mod normalize;
pub mod risk;
pub mod sched;
pub mod task;
pub mod time;
pub mod view;

pub use chrono_tz::Tz;
