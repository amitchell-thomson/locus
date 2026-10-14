//! Projects (stored by kairosd in SQLite): what Alec is working on, or
//! should be. Accepted project time is linked to one, so time spent is
//! tracked without logging anything.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProjectStatus {
    /// Working on it now (proposed project time suggests these).
    #[default]
    Active,
    /// Should be working on it soon.
    Next,
    Paused,
    Done,
}

impl ProjectStatus {
    pub const ALL: [ProjectStatus; 4] = [
        ProjectStatus::Active,
        ProjectStatus::Next,
        ProjectStatus::Paused,
        ProjectStatus::Done,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ProjectStatus::Active => "active",
            ProjectStatus::Next => "next",
            ProjectStatus::Paused => "paused",
            ProjectStatus::Done => "done",
        }
    }

    pub fn parse(s: &str) -> Option<ProjectStatus> {
        ProjectStatus::ALL
            .into_iter()
            .find(|p| p.as_str().eq_ignore_ascii_case(s.trim()))
    }

    /// The next status in the cycle (active → next → paused → done → active).
    pub fn cycle(self) -> ProjectStatus {
        match self {
            ProjectStatus::Active => ProjectStatus::Next,
            ProjectStatus::Next => ProjectStatus::Paused,
            ProjectStatus::Paused => ProjectStatus::Done,
            ProjectStatus::Done => ProjectStatus::Active,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub status: ProjectStatus,
    /// One line: what it is, or the next step.
    #[serde(default)]
    pub note: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Project {
    pub fn new(name: &str, status: ProjectStatus, note: &str, now: DateTime<Utc>) -> Project {
        Project {
            id: format!("p-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]),
            name: name.trim().to_string(),
            status,
            note: note.trim().to_string(),
            created_at: now,
            updated_at: now,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_round_trips_and_cycles() {
        for s in ProjectStatus::ALL {
            assert_eq!(ProjectStatus::parse(s.as_str()), Some(s));
        }
        assert_eq!(ProjectStatus::parse("Next"), Some(ProjectStatus::Next));
        assert_eq!(ProjectStatus::Done.cycle(), ProjectStatus::Active);
        let mut s = ProjectStatus::Active;
        for _ in 0..4 {
            s = s.cycle();
        }
        assert_eq!(s, ProjectStatus::Active);
    }
}
