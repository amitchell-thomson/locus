//! The locus window (plan B.4, D): Home, the root, with its detail screens stacked on top. A
//! project lives in its own tmux session, which Enter on its row switches to (cleanup C3: the
//! Project window and its screens went 13 Nov). Live data is laid over the fixtures, which
//! remain as the fallback tests and `--dump` draw.

pub mod app;
pub mod calendar;
pub mod chat;
pub mod dump;
pub mod fixtures;
pub mod live;
pub mod mail;
pub mod maildates;
pub mod markdown;
pub mod screen;
pub mod ui;
pub mod uselog;
