//! Projects: what Alec is working on, or should be. A project is stored as
//! a task of kind `project` (no deadline, no estimate, never scheduled);
//! [`Project`] is the view the API and the TUI's Projects list use. Its time
//! on the calendar is sessions: events linked by `Meta.project_id`.

use crate::model::Author;
use crate::task::{PROJECT_KIND, Task, TaskStatus};
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
    /// The calendar (key) its sessions go in (default: the projects key),
    /// e.g. Engineering for the 3rd-year project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Project {
    /// The project view of a project task. `projects_key` is the default
    /// calendar for sessions; a project in it reports `key: None`.
    pub fn of(t: &Task, projects_key: Option<&str>) -> Project {
        Project {
            id: t.id.clone(),
            name: t.title.clone(),
            status: match t.status {
                TaskStatus::Done => ProjectStatus::Done,
                _ => t.stage.unwrap_or_default(),
            },
            note: t.notes.clone(),
            key: (!t.key.is_empty() && Some(t.key.as_str()) != projects_key).then(|| t.key.clone()),
            created_at: t.created_at,
            updated_at: t.updated_at,
        }
    }
}

/// A new project, as a task (`key` = the calendar its sessions go in).
pub fn new_task(name: &str, status: ProjectStatus, note: &str, key: &str, by: Author, now: DateTime<Utc>) -> Task {
    let mut t = Task::new(name.trim(), key, 0, by, now);
    t.id = format!("p-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    t.kind = Some(PROJECT_KIND.into());
    t.notes = note.trim().to_string();
    set_status(&mut t, status);
    t
}

/// Set a project task's status: done closes the task, the rest are stages
/// of an open one.
pub fn set_status(t: &mut Task, s: ProjectStatus) {
    if s == ProjectStatus::Done {
        t.status = TaskStatus::Done;
    } else {
        t.status = TaskStatus::Open;
        t.stage = Some(s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_are_tasks() {
        let now = Utc::now();
        let mut t = new_task(" QUAST ", ProjectStatus::Next, "thesis", "Projects", Author::Alec, now);
        assert!(t.is_project() && t.id.starts_with("p-"));
        let p = Project::of(&t, Some("Projects"));
        assert_eq!((p.name.as_str(), p.status, p.key), ("QUAST", ProjectStatus::Next, None));
        assert_eq!(Project::of(&t, Some("Engineering")).key.as_deref(), Some("Projects"));
        set_status(&mut t, ProjectStatus::Done);
        assert_eq!(
            (t.status, Project::of(&t, None).status),
            (TaskStatus::Done, ProjectStatus::Done)
        );
        set_status(&mut t, ProjectStatus::Paused);
        assert_eq!(
            (t.status, Project::of(&t, None).status),
            (TaskStatus::Open, ProjectStatus::Paused)
        );
    }

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
