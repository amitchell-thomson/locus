//! Natural-ish date/time input (`tue 19:00`, `fri 17:00`, `tomorrow 9`; ambiguity is an error).
//! This was a copy of `eco-dates`, the parser keryx shares; since L6 (one parser, not two)
//! kairos uses that crate, and this module only keeps the path `kairos_core::dates` callers use.
//! The rules and their tests live in `crates/eco-dates`.

pub use eco_dates::*;
