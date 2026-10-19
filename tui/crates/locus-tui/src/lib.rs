//! The locus window (plan B.4, D): Home and Project, each a root, with their detail screens
//! stacked on top. Slice 1 runs every screen on fixture data; nothing an action key does leaves
//! the process except tmux (spec, "Rules for the prototype").

pub mod app;
pub mod dump;
pub mod fixtures;
pub mod screen;
pub mod ui;
pub mod uselog;
