//! The symbols, chosen by Alec from Nerd Font Material Design glyphs (locus plan v4, D.2,
//! 19 Oct 2026). One source for every glyph: a screen names `glyph::NEEDS_YOU`, never a code
//! point, so a change of mind is one edit here.
//!
//! All are in plane 15 (U+F0000 and up), each one terminal cell wide in a Nerd Font. BMP
//! private-use glyphs (fa, cod, oct) were ruled out because they are dropped from chat output,
//! which is where the choice was made. Done and failed match kairos's task boxes at his request:
//! `DONE` is the same code point as kairos's `DONE_ICON`. The plan table gives failed as
//! U+F0158; an earlier note said U+F0157, and the plan is the final word.

// Session and project state.
pub const ATTENDED: &str = "\u{f01a4}";
pub const UNATTENDED: &str = "\u{f01a3}";
pub const UNREAD: &str = "\u{f0178}";
pub const WORKING: &str = "\u{f04e6}";
pub const NEEDS_YOU: &str = "\u{f0e47}";
pub const IDLE: &str = "\u{f0904}";
pub const DONE: &str = "\u{f0135}";
pub const FAILED: &str = "\u{f0158}";

// Decision status.
pub const RECHECK: &str = "\u{f1276}";
pub const HOLDING: &str = "\u{f033e}";
pub const REDECIDED: &str = "\u{f00fb}";
pub const REVIEW: &str = "\u{f0349}";

// Kinds of thing.
pub const SCOUT_FIND: &str = "\u{f0b4e}";
pub const READING: &str = "\u{f1004}";
pub const INK: &str = "\u{f0d13}";
pub const CALENDAR: &str = "\u{f00ed}";
pub const IDEA: &str = "\u{f0335}";
pub const ANSWER: &str = "\u{f173e}";
pub const MAIL: &str = "\u{f01ee}";
pub const COMMITS: &str = "\u{f062c}";
pub const CHAT: &str = "\u{f028c}";
pub const APPROVE: &str = "\u{f0d39}";
pub const DOSSIER: &str = "\u{f0219}";
pub const AUTO_ACTION: &str = "\u{f06a9}";
pub const CHORES: &str = "\u{f0756}";
pub const PROJECT: &str = "\u{f14df}";
pub const BUDGET: &str = "\u{f04c5}";

/// Every glyph with its name, for the keys screen and for tests.
pub const ALL: &[(&str, &str)] = &[
    ("attended", ATTENDED),
    ("unattended", UNATTENDED),
    ("unread", UNREAD),
    ("working", WORKING),
    ("needs you", NEEDS_YOU),
    ("idle", IDLE),
    ("done", DONE),
    ("failed", FAILED),
    ("decision: re-check", RECHECK),
    ("decision: holding", HOLDING),
    ("decision: replaced", REDECIDED),
    ("decision: under review", REVIEW),
    ("scout find", SCOUT_FIND),
    ("reading", READING),
    ("ink", INK),
    ("calendar", CALENDAR),
    ("idea", IDEA),
    ("answer", ANSWER),
    ("mail", MAIL),
    ("commits", COMMITS),
    ("chat", CHAT),
    ("approve list", APPROVE),
    ("dossier", DOSSIER),
    ("automatic action", AUTO_ACTION),
    ("chores", CHORES),
    ("project", PROJECT),
    ("budget", BUDGET),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_glyph_is_one_plane_15_char_and_distinct() {
        let mut seen = std::collections::HashSet::new();
        for (name, g) in ALL {
            let chars: Vec<char> = g.chars().collect();
            assert_eq!(chars.len(), 1, "{name}");
            assert!(u32::from(chars[0]) >= 0xF0000, "{name} is not plane 15");
            assert!(seen.insert(*g), "{name} duplicates another glyph");
        }
        assert_eq!(ALL.len(), 26);
    }
}
