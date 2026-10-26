//! What a thing is for (ADR-009): a registry item, or one of Personal's streams. kairos asks it
//! of every event and task, keryx of every mail thread it files (keryx-joins-locus X7), so the
//! options and the way typing matches them live here, once.
//!
//! Where filed mail goes follows from the answer (X-b, your call of 26 Oct: "the same high level
//! ones as calendar, ie the areas, and then personal's threads"): the item's area, or for
//! Personal its stream. An item says what it is for; the folder only says roughly where.

use crate::common::{Project, ProjectKind, Taxonomy};
use crate::taxonomy::Shared;

/// One answer to "what is it for".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct For {
    /// The registry id, or the stream's name in lower case.
    pub id: String,
    /// What is shown and typed against: `B15 Control Systems`, `Errands`.
    pub label: String,
    /// The registry item, `None` for a stream.
    pub item: Option<String>,
    pub kind: Option<ProjectKind>,
    /// Where its mail is filed: the area, or the stream within Personal.
    pub folder: String,
    pub active: bool,
}

/// Every registry item (active ones first), then Personal's streams.
pub fn fors(shared: &Shared) -> Vec<For> {
    let mut v: Vec<For> = shared
        .registry
        .projects
        .iter()
        .map(|p| For {
            id: p.id.clone(),
            label: p.name.clone(),
            item: Some(p.id.clone()),
            kind: Some(p.kind),
            folder: folder_of(&shared.taxonomy, p),
            active: p.active,
        })
        .collect();
    v.sort_by_key(|f| !f.active);
    for a in shared.taxonomy.areas.iter().filter(|a| a.filter_by_stream) {
        for s in &a.streams {
            v.push(For {
                id: s.to_lowercase(),
                label: s.clone(),
                item: None,
                kind: None,
                folder: s.clone(),
                active: true,
            });
        }
    }
    v
}

/// The areas not filtered by stream (Studies, Projects, Careers) as answers of their own, for
/// what belongs to an area and to no item (a department newsletter). keryx offers them after
/// [`fors`]; kairos does not, since an event always has a calendar more specific than that.
pub fn area_fors(shared: &Shared) -> Vec<For> {
    shared
        .taxonomy
        .areas
        .iter()
        .filter(|a| !a.filter_by_stream)
        .map(|a| For {
            id: a.id.to_lowercase(),
            label: a.id.clone(),
            item: None,
            kind: None,
            folder: a.id.clone(),
            active: true,
        })
        .collect()
}

/// The folder an item's mail goes in: its area, or for an area filtered by stream (Personal)
/// its stream, else that area's inbox stream (Errands, where kairos puts what has no calendar).
pub fn folder_of(t: &Taxonomy, p: &Project) -> String {
    match t.area(&p.area) {
        Some(a) if a.filter_by_stream => p
            .stream
            .clone()
            .filter(|s| a.streams.contains(s))
            .or_else(|| a.streams.iter().find(|s| *s == "Errands").cloned())
            .or_else(|| a.streams.first().cloned())
            .unwrap_or_else(|| a.id.clone()),
        Some(a) => a.id.clone(),
        None => p.area.clone(),
    }
}

/// Every folder filed mail can go in: each area not filtered by stream, and each stream of those
/// that are. These are the only folders keryx may create on an account.
pub fn folders(t: &Taxonomy) -> Vec<String> {
    t.areas
        .iter()
        .flat_map(|a| {
            if a.filter_by_stream {
                a.streams.clone()
            } else {
                vec![a.id.clone()]
            }
        })
        .collect()
}

/// The options matching what is typed, best first: the id, the label or a word of it starting
/// with it, then anything containing it. Empty: all of them. `key` gives an option's (id, label).
pub fn matching<'a, T>(options: &'a [T], q: &str, key: impl Fn(&T) -> (&str, &str)) -> Vec<&'a T> {
    let q = q.trim().to_lowercase();
    if q.is_empty() {
        return options.iter().collect();
    }
    let starts = |o: &T| {
        let (id, label) = key(o);
        let label = label.to_lowercase();
        id.to_lowercase().starts_with(&q)
            || label.starts_with(&q)
            || label.split_whitespace().any(|w| w.starts_with(&q))
    };
    let mut v: Vec<&T> = options.iter().filter(|o| starts(o)).collect();
    v.extend(
        options
            .iter()
            .filter(|o| !starts(o) && key(o).1.to_lowercase().contains(&q)),
    );
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared() -> Shared {
        crate::taxonomy::load(&crate::taxonomy::root()).unwrap()
    }

    #[test]
    fn folders_are_the_areas_and_personals_streams() {
        let s = shared();
        let f = folders(&s.taxonomy);
        for want in ["Studies", "Projects", "Careers", "Family", "Sport", "Social", "Errands"] {
            assert!(f.iter().any(|x| x == want), "{want} in {f:?}");
        }
        assert!(!f.iter().any(|x| x == "Personal"), "Personal is its streams: {f:?}");
    }

    #[test]
    fn an_items_mail_goes_to_its_area_and_a_stream_is_its_own_folder() {
        let s = shared();
        let all = fors(&s);
        for o in &all {
            assert!(
                folders(&s.taxonomy).contains(&o.folder),
                "{} files to {}",
                o.id,
                o.folder
            );
        }
        let errands = all.iter().find(|o| o.id == "errands").unwrap();
        assert_eq!((errands.item.as_deref(), errands.folder.as_str()), (None, "Errands"));
        let course = all
            .iter()
            .find(|o| o.kind == Some(ProjectKind::Course))
            .expect("a course in the registry");
        assert_eq!(course.folder, "Studies");
    }

    #[test]
    fn typing_matches_ids_and_words_first() {
        let v = vec![
            ("b14", "B14 Signals"),
            ("quast", "QUAST"),
            ("x", "Applied signals work"),
        ];
        let m = matching(&v, "sig", |o| (o.0, o.1));
        assert_eq!(m.iter().map(|o| o.0).collect::<Vec<_>>(), vec!["b14", "x"]);
        assert_eq!(matching(&v, "qu", |o| (o.0, o.1))[0].0, "quast");
        assert_eq!(matching(&v, "", |o| (o.0, o.1)).len(), 3);
    }
}
