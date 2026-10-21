//! Fixture sets: a fixed, realistic snapshot that every screen runs on until its backend exists
//! (plan D). A set is a directory of JSON files, one per contract, under
//! `crates/locus-tui/fixtures/<name>/`:
//!
//! ```text
//! manifest.json taxonomy.json registry.json
//! home.json work.json system.json usage.json explore.json approve.json autolog.json
//! search.json learning.json review.json
//! ink/<name>.json                      one per mark waiting in the approve list
//! prep/<opportunity>.json              one per opportunity with a plan
//! projects/<id>/view.json              every active project
//! projects/<id>/{decisions,answered,changes}.json   where the project has them
//! ```
//!
//! Loading is strict: every contract refuses unknown fields, and `check` then cross-references
//! the files (a project id, a track, an ink fixture named in one file must exist in another), so
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
    pub explore: Explore,
    pub approve: Approve,
    pub autolog: AutoLog,
    pub search: Search,
    pub learning: Learning,
    pub review: Review,
    pub ink: BTreeMap<String, InkConfirm>,
    pub prep: BTreeMap<String, Prep>,
    pub projects: BTreeMap<String, ProjectFixture>,
}

#[derive(Debug, Clone)]
pub struct ProjectFixture {
    pub view: ProjectView,
    pub decisions: Option<Decisions>,
    pub answered: Option<Answered>,
    pub changes: Option<Changes>,
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
        let registry: Registry = read(dir, "registry.json")?;
        let mut projects = BTreeMap::new();
        for p in registry.projects.iter().filter(|p| p.active) {
            let base = format!("projects/{}", p.id);
            projects.insert(
                p.id.clone(),
                ProjectFixture {
                    view: read(dir, &format!("{base}/view.json"))?,
                    decisions: read_opt(dir, &format!("{base}/decisions.json"))?,
                    answered: read_opt(dir, &format!("{base}/answered.json"))?,
                    changes: read_opt(dir, &format!("{base}/changes.json"))?,
                },
            );
        }
        Ok(Fixtures {
            dir: dir.to_path_buf(),
            manifest: read(dir, "manifest.json")?,
            taxonomy: read(dir, "taxonomy.json")?,
            registry,
            home: read(dir, "home.json")?,
            work: read(dir, "work.json")?,
            system: read(dir, "system.json")?,
            usage: read(dir, "usage.json")?,
            explore: read(dir, "explore.json")?,
            approve: read(dir, "approve.json")?,
            autolog: read(dir, "autolog.json")?,
            search: read(dir, "search.json")?,
            learning: read(dir, "learning.json")?,
            review: read(dir, "review.json")?,
            ink: read_dir(dir, "ink")?,
            prep: read_dir(dir, "prep")?,
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
            "manifest", "taxonomy", "registry", "home", "work", "system", "usage", "explore", "approve", "autolog",
            "search", "learning", "review",
        ]
        .iter()
        .map(|f| format!("{f}.json"))
        .collect();
        out.extend(self.ink.keys().map(|k| format!("ink/{k}.json")));
        out.extend(self.prep.keys().map(|k| format!("prep/{k}.json")));
        for (id, p) in &self.projects {
            out.insert(format!("projects/{id}/view.json"));
            let optional = [
                ("decisions", p.decisions.is_some()),
                ("answered", p.answered.is_some()),
                ("changes", p.changes.is_some()),
            ];
            for (f, present) in optional {
                if present {
                    out.insert(format!("projects/{id}/{f}.json"));
                }
            }
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
                bad.push(format!("{what}: no project `{id}` in registry.json"));
            }
        };

        if ids.len() != self.registry.projects.len() {
            bad.push("registry.json: a project id appears twice".into());
        }
        let mut sessions = BTreeSet::new();
        for p in &self.registry.projects {
            match self.taxonomy.areas.iter().find(|a| a.id == p.area) {
                None => bad.push(format!(
                    "registry.json: {} has area `{}`, not in taxonomy.json",
                    p.id, p.area
                )),
                Some(a) => {
                    if let Some(s) = &p.stream
                        && !a.streams.contains(s)
                    {
                        bad.push(format!("registry.json: {} has stream `{s}`, not under {}", p.id, a.id));
                    }
                }
            }
            if let Some(c) = &p.course
                && self.registry.get(c).is_none_or(|x| x.kind != ProjectKind::Course)
            {
                bad.push(format!(
                    "registry.json: {} is linked to `{c}`, which is not a course",
                    p.id
                ));
            }
            for s in &p.sessions {
                if !sessions.insert(s.as_str()) {
                    bad.push(format!("registry.json: session `{s}` belongs to two projects"));
                }
            }
        }

        if let Some(a) = &self.home.attended {
            project("home.json attended", a, &mut bad);
        }
        if let Some(n) = &self.home.next
            && !self.taxonomy.areas.iter().any(|a| a.id == n.area)
        {
            bad.push(format!("home.json next: area `{}` not in taxonomy.json", n.area));
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

        if self.explore.queue.len() > self.explore.queue_cap as usize {
            bad.push(format!(
                "explore.json: {} finds in a queue that holds {}",
                self.explore.queue.len(),
                self.explore.queue_cap
            ));
        }
        for f in &self.explore.queue {
            if let Some(p) = &f.project {
                project(&format!("explore.json find {}", f.id), p, &mut bad);
            }
        }
        for i in &self.explore.ideas {
            if let Some(p) = &i.project {
                project("explore.json idea", p, &mut bad);
            }
        }
        for g in &self.approve.groups {
            for i in &g.items {
                if let Some(o) = &i.opens
                    && !self.ink.contains_key(o)
                {
                    bad.push(format!(
                        "approve.json: `{}` opens ink/{o}.json, which does not exist",
                        i.text
                    ));
                }
            }
        }
        for (name, ink) in &self.ink {
            if let Some(img) = &ink.image
                && !self.dir.join("ink").join(img).is_file()
            {
                bad.push(format!("ink/{name}.json: image {img} is not beside it"));
            }
        }
        for h in &self.search.hits {
            if let Some(p) = &h.project {
                project("search.json", p, &mut bad);
            }
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
        for (name, p) in &self.prep {
            if p.opportunity != *name {
                bad.push(format!("prep/{name}.json: says opportunity `{}`", p.opportunity));
            }
            match self.registry.get(&p.opportunity) {
                Some(r) if r.kind == ProjectKind::Application => {}
                Some(_) => bad.push(format!("prep/{name}.json: {} is not an application", p.opportunity)),
                None => bad.push(format!(
                    "prep/{name}.json: no project `{}` in registry.json",
                    p.opportunity
                )),
            }
            for t in &p.topics {
                track(&format!("prep/{name}.json"), &t.track, &mut bad);
            }
            for s in &p.log {
                track(&format!("prep/{name}.json log"), &s.track, &mut bad);
            }
        }

        for (id, pf) in &self.projects {
            let named = [
                Some(("view", pf.view.project.as_str())),
                pf.decisions.as_ref().map(|d| ("decisions", d.project.as_str())),
                pf.answered.as_ref().map(|d| ("answered", d.project.as_str())),
                pf.changes.as_ref().map(|d| ("changes", d.project.as_str())),
            ];
            for (file, p) in named.into_iter().flatten() {
                if p != id {
                    bad.push(format!("projects/{id}/{file}.json: says project `{p}`"));
                }
            }
            // An answer asked about a decision names one that is in the log (round 2v).
            if let Some(a) = &pf.answered {
                for x in &a.items {
                    if let Some(id_) = &x.decision
                        && !pf
                            .decisions
                            .as_ref()
                            .is_some_and(|d| d.items.iter().any(|i| &i.id == id_))
                    {
                        bad.push(format!(
                            "projects/{id}/answered.json: \"{}\" asks about {id_}, not in decisions.json",
                            x.title
                        ));
                    }
                }
            }
            // One decision, one surface (round 2l): the log marks a new one, New never lists it.
            for it in &pf.view.new {
                if matches!(it.kind, ItemKind::Decision | ItemKind::Recheck | ItemKind::Redecided) {
                    bad.push(format!(
                        "projects/{id}/view.json: new lists a decision ({}); mark it new in decisions.json",
                        it.text
                    ));
                }
            }
            if let Some(g) = &pf.view.graph {
                for l in &g.lanes {
                    if l.merged.is_some() == l.tip.is_some() {
                        bad.push(format!(
                            "projects/{id}/view.json: lane {} needs exactly one of merged, tip",
                            l.branch
                        ));
                    }
                }
            }
        }
        for r in &self.review.rechecks {
            project("review.json recheck", &r.project, &mut bad);
        }
        for b in &self.review.next_week {
            project("review.json next week", &b.project, &mut bad);
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

/// Every `<sub>/<name>.json`, keyed by name. A missing directory is an empty map.
fn read_dir<T: serde::de::DeserializeOwned>(dir: &Path, sub: &str) -> Result<BTreeMap<String, T>> {
    let mut out = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(dir.join(sub)) else {
        return Ok(out);
    };
    for e in entries {
        let path = e?.path();
        if path.extension().is_some_and(|x| x == "json")
            && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
        {
            out.insert(stem.to_string(), read(dir, &format!("{sub}/{stem}.json"))?);
        }
    }
    Ok(out)
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
        fx.learning.tracks.retain(|t| t.id != "microstructure");
        assert!(fx.check().iter().any(|b| b.contains("no track `microstructure`")));
    }

    #[test]
    fn an_overfull_queue_is_caught() {
        let mut fx = default_set();
        fx.explore.queue_cap = 3;
        assert!(fx.check().iter().any(|b| b.contains("in a queue that holds 3")));
    }

    #[test]
    fn usage_that_does_not_add_up_is_caught() {
        let mut fx = default_set();
        fx.usage.week_pct = 12.0;
        assert_eq!(fx.check().iter().filter(|b| b.starts_with("usage.json")).count(), 2);
    }

    #[test]
    fn an_ink_item_with_no_fixture_is_caught() {
        let mut fx = default_set();
        fx.ink.clear();
        assert!(fx.check().iter().any(|b| b.contains("does not exist")));
    }

    #[test]
    fn an_unknown_field_fails_the_load() {
        let r: Result<Home, _> = serde_json::from_str(
            r#"{"chores_fit":0,"mail":{"need_reply":0,"deadlines_proposed":0},"atended":"quast"}"#,
        );
        assert!(r.unwrap_err().to_string().contains("unknown field `atended`"));
    }
}
