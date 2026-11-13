//! Fixture sets: a fixed, realistic snapshot that every screen runs on until its backend exists
//! (plan D). A set is a directory of JSON files, one per contract, under
//! `crates/locus-tui/fixtures/<name>/`:
//!
//! ```text
//! manifest.json
//! home.json work.json system.json usage.json approve.json autolog.json learning.json tasks.json
//! projects/<id>/view.json              every active project
//! ```
//!
//! A live window lays what it reads over these and drops what it cannot read live; the fixtures
//! are the fallback that tests and `--dump` draw.
//!
//! The taxonomy and the registry are not part of a set: every set reads the one copy in
//! `taxonomy/` at the repository root (ADR-008, spec K3), the same files kairos reads.
//!
//! Loading is strict: every contract refuses unknown fields, and `check` then cross-references
//! the files (a project id or a track named in one file must exist in another), so
//! a fixture cannot leave a section silently empty.

use anyhow::{Context, Result};
use locus_contract::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Fixtures {
    pub dir: PathBuf,
    pub manifest: Manifest,
    pub taxonomy: Taxonomy,
    pub registry: Registry,
    pub home: Home,
    pub work: Work,
    pub system: System,
    pub usage: Usage,
    pub approve: Approve,
    pub autolog: AutoLog,
    pub learning: Learning,
    pub tasks: Tasks,
    pub projects: BTreeMap<String, ProjectFixture>,
}

#[derive(Debug, Clone)]
pub struct ProjectFixture {
    pub view: ProjectView,
}

/// Where the repo's fixture sets live, found from the crate so it works from any directory.
pub fn sets_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

/// A set by name (`default`, `empty`) or by path.
pub fn resolve(set: &str) -> PathBuf {
    let p = Path::new(set);
    if p.is_dir() {
        p.to_path_buf()
    } else {
        sets_dir().join(set)
    }
}

impl Fixtures {
    pub fn load(dir: &Path) -> Result<Fixtures> {
        let shared = locus_contract::taxonomy::load(&locus_contract::taxonomy::root())?;
        let registry = shared.registry;
        let mut projects = BTreeMap::new();
        for p in registry.projects.iter().filter(|p| p.active) {
            let base = format!("projects/{}", p.id);
            projects.insert(
                p.id.clone(),
                ProjectFixture {
                    // A registry entry with no view yet gets an empty one: registering
                    // something (JCR Treasurer, 10 Nov) must never stop the window starting.
                    view: match read_opt(dir, &format!("{base}/view.json"))? {
                        Some(v) => v,
                        None => serde_json::from_value(serde_json::json!({
                            "project": p.id, "sessions": []
                        }))
                        .map_err(|e| anyhow::anyhow!("an empty view for {}: {e}", p.id))?,
                    },
                },
            );
        }
        Ok(Fixtures {
            dir: dir.to_path_buf(),
            manifest: read(dir, "manifest.json")?,
            taxonomy: shared.taxonomy,
            registry,
            home: read(dir, "home.json")?,
            work: read(dir, "work.json")?,
            system: read(dir, "system.json")?,
            usage: read(dir, "usage.json")?,
            approve: read(dir, "approve.json")?,
            autolog: read(dir, "autolog.json")?,
            learning: read(dir, "learning.json")?,
            tasks: read(dir, "tasks.json")?,
            projects,
        })
    }

    pub fn now(&self) -> Time {
        self.manifest.now
    }

    /// Every JSON file in the set, relative to it, for the validation report.
    pub fn files(dir: &Path) -> Vec<String> {
        let mut out = Vec::new();
        walk(dir, dir, &mut out);
        out.sort();
        out
    }

    /// The files `load` read, so a stray file (a typo'd name, a project not marked active) is
    /// reported as unread rather than passing as OK.
    pub fn read_files(&self) -> BTreeSet<String> {
        let mut out: BTreeSet<String> = [
            "manifest", "home", "work", "system", "usage", "approve", "autolog", "learning", "tasks",
        ]
        .iter()
        .map(|f| format!("{f}.json"))
        .collect();
        for id in self.projects.keys() {
            out.insert(format!("projects/{id}/view.json"));
        }
        out
    }

    /// Cross-file problems, each in words. Empty means the set is consistent.
    pub fn check(&self) -> Vec<String> {
        let mut bad = Vec::new();
        let ids: BTreeSet<&str> = self.registry.projects.iter().map(|p| p.id.as_str()).collect();
        let tracks: BTreeSet<&str> = self.learning.tracks.iter().map(|t| t.id.as_str()).collect();
        let project = |what: &str, id: &str, bad: &mut Vec<String>| {
            if !ids.contains(id) {
                bad.push(format!("{what}: no project `{id}` in taxonomy/registry.toml"));
            }
        };

        if ids.len() != self.registry.projects.len() {
            bad.push("registry.toml: a project id appears twice".into());
        }
        let mut sessions = BTreeSet::new();
        for p in &self.registry.projects {
            match self.taxonomy.areas.iter().find(|a| a.id == p.area) {
                None => bad.push(format!(
                    "registry.toml: {} has area `{}`, not in taxonomy/areas.toml",
                    p.id, p.area
                )),
                Some(a) => {
                    if let Some(s) = &p.stream
                        && !a.streams.contains(s)
                    {
                        bad.push(format!("registry.toml: {} has stream `{s}`, not under {}", p.id, a.id));
                    }
                }
            }
            if let Some(c) = &p.course
                && self.registry.get(c).is_none_or(|x| x.kind != ProjectKind::Course)
            {
                bad.push(format!(
                    "registry.toml: {} is linked to `{c}`, which is not a course",
                    p.id
                ));
            }
            for s in &p.sessions {
                if !sessions.insert(s.as_str()) {
                    bad.push(format!("registry.toml: session `{s}` belongs to two projects"));
                }
            }
        }

        if let Some(a) = &self.home.attended {
            project("home.json attended", a, &mut bad);
        }
        if let Some(n) = &self.home.next
            && !self.taxonomy.areas.iter().any(|a| a.id == n.area)
        {
            bad.push(format!("home.json next: area `{}` not in taxonomy/areas.toml", n.area));
        }
        for d in &self.work.week {
            for s in &d.spans {
                project(&format!("work.json {}", d.date), &s.project, &mut bad);
                if !(1..=4).contains(&s.intensity) {
                    bad.push(format!("work.json {}: intensity {} is not 1 to 4", d.date, s.intensity));
                }
                if s.end <= s.start {
                    bad.push(format!("work.json {}: a span ends before it starts", d.date));
                }
            }
        }
        let days = self.work.chart.series.first().map(|s| s.hours.len());
        for s in &self.work.chart.series {
            project("work.json chart", &s.project, &mut bad);
            if Some(s.hours.len()) != days {
                bad.push(format!(
                    "work.json chart: {} has {} days, not {:?}",
                    s.project,
                    s.hours.len(),
                    days
                ));
            }
        }

        let days: f32 = self.usage.days.iter().map(DayUse::total).sum();
        if !self.usage.days.is_empty() && (days - self.usage.week_pct).abs() > 0.05 {
            bad.push(format!(
                "usage.json: the days add to {days:.1}%, the week says {:.1}%",
                self.usage.week_pct
            ));
        }
        let projects: f32 = self.usage.by_project.iter().map(|c| c.pct).sum();
        if !self.usage.by_project.is_empty() && (projects - self.usage.week_pct).abs() > 0.05 {
            bad.push(format!(
                "usage.json: by project adds to {projects:.1}%, the week says {:.1}%",
                self.usage.week_pct
            ));
        }

        for t in &self.learning.tracks {
            if !(0.0..=1.0).contains(&t.confidence) || t.history.iter().any(|p| !(0.0..=1.0).contains(&p.confidence)) {
                bad.push(format!("learning.json: track {} has a confidence outside 0 to 1", t.id));
            }
        }
        let track = |what: &str, id: &str, bad: &mut Vec<String>| {
            if !tracks.contains(id) {
                bad.push(format!("{what}: no track `{id}` in learning.json"));
            }
        };
        for g in &self.learning.goals {
            for s in &g.syllabus {
                track(&format!("learning.json goal {}", g.id), &s.track, &mut bad);
            }
        }
        for s in &self.learning.sheets {
            for t in &s.tracks {
                track(&format!("learning.json sheet {}", s.id), t, &mut bad);
            }
        }
        // Every application names its company: the tablet's Careers folders are per company,
        // and capture matches a folder to it (Slice 0, 0.12).
        for p in &self.registry.projects {
            if p.kind == ProjectKind::Application && p.company.is_none() {
                bad.push(format!("registry.toml: application `{}` has no company", p.id));
            }
        }
        for (id, pf) in &self.projects {
            if pf.view.project != *id {
                bad.push(format!("projects/{id}/view.json: says project `{}`", pf.view.project));
            }
        }
        bad
    }
}

fn read<T: serde::de::DeserializeOwned>(dir: &Path, file: &str) -> Result<T> {
    let path = dir.join(file);
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

fn read_opt<T: serde::de::DeserializeOwned>(dir: &Path, file: &str) -> Result<Option<T>> {
    if dir.join(file).is_file() {
        read(dir, file).map(Some)
    } else {
        Ok(None)
    }
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(root, &p, out);
        } else if p.extension().is_some_and(|x| x == "json")
            && let Ok(rel) = p.strip_prefix(root)
        {
            out.push(rel.display().to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_set() -> Fixtures {
        Fixtures::load(&resolve("default")).unwrap()
    }

    #[test]
    fn both_sets_load_and_check_clean() {
        for set in ["default", "empty"] {
            let fx = Fixtures::load(&resolve(set)).unwrap();
            assert_eq!(fx.check(), Vec::<String>::new(), "{set}");
            assert_eq!(
                fx.read_files().len(),
                Fixtures::files(&fx.dir).len(),
                "{set}: a file nothing reads"
            );
        }
    }

    // The checks are only worth having if they fire: each breaks one thing and expects one problem.
    #[test]
    fn a_dangling_project_id_is_caught() {
        let mut fx = default_set();
        fx.home.attended = Some("qast".into());
        assert!(fx.check().iter().any(|b| b.contains("no project `qast`")));
    }

    #[test]
    fn a_dangling_track_is_caught() {
        let mut fx = default_set();
        fx.learning.tracks.retain(|t| t.id != "linear-algebra");
        assert!(fx.check().iter().any(|b| b.contains("no track `linear-algebra`")));
    }

    #[test]
    fn usage_that_does_not_add_up_is_caught() {
        let mut fx = default_set();
        fx.usage.week_pct = 12.0;
        assert_eq!(fx.check().iter().filter(|b| b.starts_with("usage.json")).count(), 2);
    }

    #[test]
    fn an_unknown_field_fails_the_load() {
        let r: Result<Home, _> = serde_json::from_str(r#"{"mail":{"need_reply":0},"atended":"quast"}"#);
        assert!(r.unwrap_err().to_string().contains("unknown field `atended`"));
    }
}
