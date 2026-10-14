//! `keryx-store`: the mirror database, source readers (Mail.app, Gmail) and
//! HTTP clients.

pub mod db;
pub mod gmail;
pub mod ingest;
pub mod keychain;
pub mod mailapp;
#[cfg(any(test, feature = "testkit"))]
pub mod testkit;
pub mod view;
