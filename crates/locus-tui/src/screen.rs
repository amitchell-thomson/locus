//! Every screen the plan names, and the sections each one focuses by letter.
//!
//! The list is Slice 1 spec step 1.4's table. A section's letter is shown in its corner, which
//! is why Home's footer no longer lists them (plan D, 19 Oct: "do not need letter it says it in
//! each pane"). No letter may be a key of the shared table (`locus_theme::keys`, tested): Home's
//! system is `m` (it was `y`), System's recent `h` (was `e`) and usage `b` (was `u`), 13 Nov.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScreenId {
    Home,
    System,
    Budget,
    Approve,
    AutoLog,
    /// One approve item, whole: `⏎` on Approve (10 Nov).
    ApproveItem,
    WorkChart,
    Learning,
    Tasks,
    /// Every project you build, by where it stands (`p` on Home, 10 Nov).
    Projects,
    /// The Home chat's conversations (files-and-chat M2), on `i` from anywhere.
    Chat,
}

/// A part of a screen that a letter focuses; Enter on it opens `detail`, if it has one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Section {
    pub letter: char,
    pub title: &'static str,
    pub detail: Option<ScreenId>,
}

const fn s(letter: char, title: &'static str, detail: Option<ScreenId>) -> Section {
    Section { letter, title, detail }
}

impl ScreenId {
    pub const ALL: [ScreenId; 11] = [
        ScreenId::Home,
        ScreenId::System,
        ScreenId::Budget,
        ScreenId::Approve,
        ScreenId::AutoLog,
        ScreenId::ApproveItem,
        ScreenId::WorkChart,
        ScreenId::Learning,
        ScreenId::Tasks,
        ScreenId::Projects,
        ScreenId::Chat,
    ];

    /// The name used on the command line and in the use log.
    pub fn name(self) -> &'static str {
        match self {
            ScreenId::Home => "home",
            ScreenId::System => "system",
            ScreenId::Budget => "budget",
            ScreenId::Approve => "approve",
            ScreenId::AutoLog => "autolog",
            ScreenId::ApproveItem => "approve-item",
            ScreenId::WorkChart => "work",
            ScreenId::Learning => "learning",
            ScreenId::Tasks => "tasks",
            ScreenId::Projects => "projects",
            ScreenId::Chat => "chat",
        }
    }

    pub fn from_name(name: &str) -> Option<ScreenId> {
        ScreenId::ALL.into_iter().find(|s| s.name() == name)
    }

    /// The title in the screen's top border.
    pub fn title(self) -> &'static str {
        match self {
            ScreenId::Home => "locus",
            ScreenId::System => "system",
            ScreenId::Budget => "budget",
            ScreenId::Approve => "approve",
            ScreenId::AutoLog => "automatic actions",
            ScreenId::ApproveItem => "approve",
            ScreenId::WorkChart => "work · 8 weeks",
            ScreenId::Learning => "learning",
            ScreenId::Tasks => "tasks",
            ScreenId::Projects => "projects",
            ScreenId::Chat => "chat",
        }
    }

    /// Home is the root: Esc stops there.
    pub fn is_root(self) -> bool {
        self == ScreenId::Home
    }

    pub fn sections(self) -> &'static [Section] {
        match self {
            ScreenId::Home => HOME,
            ScreenId::System => SYSTEM,
            ScreenId::Learning => LEARNING,
            _ => &[],
        }
    }
}

use ScreenId as S;
// In the order Tab and the arrows visit them, top to bottom.
const HOME: &[Section] = &[
    s('w', "work", Some(S::WorkChart)),
    s('p', "projects", Some(S::Projects)),
    s('c', "careers", None),
    s('s', "studies", None),
    s('a', "approve", Some(S::Approve)),
    s('t', "tasks", Some(S::Tasks)),
    s('m', "system", Some(S::System)),
];
const SYSTEM: &[Section] = &[
    s('r', "running", None),
    s('t', "timers", None),
    s('h', "recent", None),
    s('b', "usage", Some(S::Budget)),
];
const LEARNING: &[Section] = &[s('g', "goals", None), s('t', "tracks", None), s('s', "sheets", None)];

impl fmt::Display for ScreenId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for s in ScreenId::ALL {
            assert_eq!(ScreenId::from_name(s.name()), Some(s));
        }
    }

    #[test]
    fn section_letters_are_unique_per_screen() {
        for s in ScreenId::ALL {
            let mut seen = std::collections::HashSet::new();
            for sec in s.sections() {
                assert!(seen.insert(sec.letter), "{s}: letter {} twice", sec.letter);
            }
        }
    }
}
