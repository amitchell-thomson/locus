//! `keryx-store`: the mirror database, source readers (Mail.app, Gmail) and
//! HTTP clients.

pub mod api;
pub mod db;
pub mod gmail;
pub mod ingest;
pub mod keychain;
pub mod mailapp;
pub mod state;
#[cfg(any(test, feature = "testkit"))]
pub mod testkit;
pub mod view;
