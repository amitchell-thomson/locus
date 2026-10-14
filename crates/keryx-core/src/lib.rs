//! `keryx-core`: pure email logic. No network, no SQLite; file access is
//! limited to reading config files given an explicit path.

pub mod build;
pub mod colour;
pub mod config;
pub mod mime;
pub mod model;
pub mod text;

pub use chrono_tz::Tz;
