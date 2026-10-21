//! Every screen the plan names, and the sections each one focuses by letter.
//!
//! The list is Slice 1 spec step 1.4's table. A section's letter is shown in its corner, which
//! is why Home's footer no longer lists them (plan D, 19 Oct: "do not need letter it says it in
//! each pane").

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScreenId {
    Home,
    Project,
    System,
    Budget,
    Explore,
    Approve,
    AutoLog,
    InkConfirm,
    Decisions,
    Answered,
    Changes,
    Search,
    WorkChart,
    Prep,
    Learning,
    Review,
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
    pub const ALL: [ScreenId; 16] = [
        ScreenId::Home,
        ScreenId::Project,
        ScreenId::System,
        ScreenId::Budget,
        ScreenId::Explore,
        ScreenId::Approve,
        ScreenId::AutoLog,
        ScreenId::InkConfirm,
        ScreenId::Decisions,
        ScreenId::Answered,
        ScreenId::Changes,
        ScreenId::Search,
        ScreenId::WorkChart,
        ScreenId::Prep,
        ScreenId::Learning,
        ScreenId::Review,
    ];

    /// The name used on the command line and in the use log.
    pub fn name(self) -> &'static str {
        match self {
            ScreenId::Home => "home",
            ScreenId::Project => "project",
            ScreenId::System => "system",
            ScreenId::Budget => "budget",
            ScreenId::Explore => "explore",
            ScreenId::Approve => "approve",
            ScreenId::AutoLog => "autolog",
            ScreenId::InkConfirm => "ink",
            ScreenId::Decisions => "decisions",
            ScreenId::Answered => "answered",
            ScreenId::Changes => "changes",
            ScreenId::Search => "search",
            ScreenId::WorkChart => "work",
            ScreenId::Prep => "prep",
            ScreenId::Learning => "learning",
            ScreenId::Review => "review",
        }
    }

    pub fn from_name(name: &str) -> Option<ScreenId> {
        ScreenId::ALL.into_iter().find(|s| s.name() == name)
    }

    /// The title in the screen's top border.
    pub fn title(self) -> &'static str {
        match self {
            ScreenId::Home => "locus",
            ScreenId::Project => "project",
            ScreenId::System => "system",
            ScreenId::Budget => "budget",
            ScreenId::Explore => "explore",
            ScreenId::Approve => "approve",
            ScreenId::AutoLog => "automatic actions",
            ScreenId::InkConfirm => "where does this mark point?",
            ScreenId::Decisions => "decisions",
            ScreenId::Answered => "answered",
            ScreenId::Changes => "changes",
            ScreenId::Search => "search",
            ScreenId::WorkChart => "work · 8 weeks",
            ScreenId::Prep => "interview prep",
            ScreenId::Learning => "learning",
            ScreenId::Review => "weekly review",
        }
    }

    /// Home and Project are roots: each has its own tmux window, and Esc stops there.
    pub fn is_root(self) -> bool {
        matches!(self, ScreenId::Home | ScreenId::Project)
    }

    pub fn sections(self) -> &'static [Section] {
        match self {
            ScreenId::Home => HOME,
            ScreenId::Project => PROJECT,
            ScreenId::System => SYSTEM,
            ScreenId::Explore => EXPLORE,
            ScreenId::Learning => LEARNING,
            ScreenId::Prep => PREP,
            _ => &[],
        }
    }
}

use ScreenId as S;
// In the order Tab and the arrows visit them, top to bottom.
const HOME: &[Section] = &[
    s('w', "work", Some(S::WorkChart)),
    s('p', "projects", Some(S::Project)),
    s('c', "careers", None),
    s('t', "studies", None),
    s('e', "explore", Some(S::Explore)),
    s('a', "waiting for you", Some(S::Approve)),
    s('s', "system", Some(S::System)),
];
// Decisions (d), Answered (a) and prep (p) open from the Project view by their own keys: they
// are screens, not panes on it.
// In layout order, so ↑ and ↓ walk them as drawn (round 2: "going up from sessions jumps
// you straight to branches").
const PROJECT: &[Section] = &[
    s('y', "where you are", None),
    s('g', "branches", None),
    s('n', "new", Some(S::Changes)),
    s('s', "sessions", None),
    s('c', "chores", None),
];
const SYSTEM: &[Section] = &[
    s('r', "running", None),
    s('e', "recent", None),
    s('u', "usage", Some(S::Budget)),
];
const EXPLORE: &[Section] = &[s('q', "queue", None), s('i', "ideas", None), s('r', "reading", None)];
const LEARNING: &[Section] = &[s('g', "goals", None), s('t', "tracks", None), s('s', "sheets", None)];
const PREP: &[Section] = &[s('t', "topics", None), s('p', "practice log", None)];

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
