//! The taxonomy files at the repository root (`taxonomy/areas.toml`, `taxonomy/registry.toml`;
//! ADR-006, ADR-008): the one copy every app reads. locus's fixtures load them from here; kairosd
//! loads them on the server and serves them (`GET /api/taxonomy`) to the Mac, which keeps the last
//! copy it fetched for when the daemon cannot be reached.

use crate::{Registry, Taxonomy};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The taxonomy and the registry together: what kairosd serves and the Mac caches.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shared {
    pub taxonomy: Taxonomy,
    pub registry: Registry,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}: {1}")]
    Read(PathBuf, std::io::Error),
    #[error("{0}: {1}")]
    Parse(PathBuf, toml::de::Error),
    #[error("taxonomy: {0}")]
    Invalid(String),
}

/// `$LOCUS_TAXONOMY` if set, else `taxonomy/` in the checkout this was built from (the server
/// and the Mac both build from their checkout, so the path is the live one).
pub fn root() -> PathBuf {
    std::env::var_os("LOCUS_TAXONOMY")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../taxonomy"))
}

fn read<T: for<'de> Deserialize<'de>>(path: PathBuf) -> Result<T, Error> {
    let text = std::fs::read_to_string(&path).map_err(|e| Error::Read(path.clone(), e))?;
    toml::from_str(&text).map_err(|e| Error::Parse(path, e))
}

/// Load both files from `dir` and check the registry against the taxonomy.
pub fn load(dir: &Path) -> Result<Shared, Error> {
    let shared = Shared {
        taxonomy: read(dir.join("areas.toml"))?,
        registry: read(dir.join("registry.toml"))?,
    };
    let bad = shared.problems();
    if bad.is_empty() {
        Ok(shared)
    } else {
        Err(Error::Invalid(bad.join("; ")))
    }
}

impl Shared {
    /// Every way the registry disagrees with the taxonomy, in words.
    pub fn problems(&self) -> Vec<String> {
        let mut bad = Vec::new();
        let mut ids = std::collections::BTreeSet::new();
        for p in &self.registry.projects {
            if !ids.insert(p.id.as_str()) {
                bad.push(format!("`{}` registered twice", p.id));
            }
            match self.taxonomy.area(&p.area) {
                None => bad.push(format!("`{}` has area `{}`, not in areas.toml", p.id, p.area)),
                Some(a) => {
                    if let Some(s) = &p.stream
                        && !a.streams.contains(s)
                    {
                        bad.push(format!("`{}` has stream `{s}`, not under {}", p.id, a.id));
                    }
                }
            }
            if let Some(a) = &p.application
                && !self
                    .registry
                    .projects
                    .iter()
                    .any(|q| &q.id == a && q.kind == crate::ProjectKind::Application)
            {
                bad.push(format!("`{}` names application `{a}`, which is not a registered application", p.id));
            }
            if p.kind == crate::ProjectKind::Application && p.company.is_none() {
                bad.push(format!("application `{}` has no company", p.id));
            }
            match &p.folder {
                None => bad.push(format!("`{}` has no folder", p.id)),
                Some(f) if !crate::common::is_folder_name(f) => bad.push(format!(
                    "`{}` has folder `{f}`: lowercase words joined by hyphens, no spaces",
                    p.id
                )),
                Some(_) => {}
            }
        }
        bad.extend(self.shared_folders());
        bad
    }

    /// Two entries in one area with one folder, unless the folder is one thing (see
    /// `Project::folder`): applications to the same company, or a repo and its course.
    fn shared_folders(&self) -> Vec<String> {
        let ps = &self.registry.projects;
        let mut bad = Vec::new();
        for (i, a) in ps.iter().enumerate() {
            for b in &ps[i + 1..] {
                if a.area != b.area || a.folder.is_none() || a.folder != b.folder {
                    continue;
                }
                let same_company = a.kind == crate::ProjectKind::Application
                    && b.kind == crate::ProjectKind::Application
                    && a.company.is_some()
                    && a.company == b.company;
                let repo_and_course = |r: &crate::Project, c: &crate::Project| {
                    r.kind == crate::ProjectKind::Repo
                        && c.kind == crate::ProjectKind::Course
                        && r.course.as_deref() == Some(c.id.as_str())
                };
                if !(same_company || repo_and_course(a, b) || repo_and_course(b, a)) {
                    bad.push(format!(
                        "`{}` and `{}` share folder `{}`",
                        a.id,
                        b.id,
                        a.folder.as_deref().unwrap_or_default()
                    ));
                }
            }
        }
        bad
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_repository_files_load_and_agree() {
        let s = load(&root()).expect("taxonomy/ loads");
        assert_eq!(
            s.taxonomy.areas.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
            ["Studies", "Projects", "Careers", "Personal"]
        );
        assert!(s.taxonomy.resolves("Errands"));
        assert_eq!(s.taxonomy.area_of("Sport").map(|a| a.id.as_str()), Some("Personal"));
        let types: Vec<_> = s.taxonomy.task_types.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(types, ["sheet", "coursework", "admin", "chore"]);
        assert!(s.registry.get("tanker-flow").is_some() && s.registry.get("jane-street").is_some());
    }

    #[test]
    fn the_registry_is_clean() {
        let s = load(&root()).expect("taxonomy/ loads");
        assert_eq!(s.problems(), Vec::<String>::new());
    }

    #[test]
    fn folder_names_have_no_spaces_and_one_name_is_one_thing() {
        assert!(crate::common::is_folder_name("b1-engineering-computation"));
        for bad in ["B1 Engineering", "b1_x", "-a", "a-", "a--b", ""] {
            assert!(!crate::common::is_folder_name(bad), "{bad:?}");
        }
        let mut s = load(&root()).expect("taxonomy/ loads");
        let quast = s.registry.projects.iter().position(|p| p.id == "quast").unwrap();
        let nabla = s.registry.projects.iter().position(|p| p.id == "nabla").unwrap();
        s.registry.projects[nabla].folder = s.registry.projects[quast].folder.clone();
        assert!(s.problems().iter().any(|b| b.contains("share folder")));
        s.registry.projects[nabla].folder = Some("Nabla Repo".into());
        assert!(s.problems().iter().any(|b| b.contains("no spaces")));
    }

    #[test]
    fn a_registry_entry_outside_the_taxonomy_is_named() {
        let mut s = load(&root()).expect("taxonomy/ loads");
        if let Some(p) = s.registry.projects.first_mut() {
            p.area = "Travel".into();
        }
        assert!(s.problems().iter().any(|b| b.contains("`Travel`")));
    }
}
