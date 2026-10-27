//! View-only duplicate detection. **Never deletes anything**: it groups
//! instances into clusters and picks one to display.
//!
//! Two instances are duplicates if
//! 1. same UID and same recurrence id, or
//! 2. normalised titles match (Jaro–Winkler ≥ 0.92 or equal token sets) and
//!    starts are within ±15 min, on the same local date, both timed or both
//!    all-day — and, unless titles are identical, their locations don't
//!    contradict each other.
//!
//! Two different work blocks are never duplicates (2026-10-27): kairos wrote both, so they
//! are two claims on your time, not two copies of one event. Folding them hid two B14 blocks
//! under a third and their time was counted without being seen.

use crate::model::{EventKind, Instance};
use chrono::Duration;
use chrono_tz::Tz;
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};
use unicode_segmentation::UnicodeSegmentation;

pub const JW_THRESHOLD: f64 = 0.92;
pub const START_TOLERANCE_MIN: i64 = 15;

/// Case-fold, strip punctuation/emoji/⚑, collapse whitespace.
pub fn normalise_title(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for w in s.unicode_words() {
        let w: String = w
            .chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect();
        if w.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&w);
    }
    out
}

pub fn titles_match(a: &str, b: &str) -> bool {
    let (na, nb) = (normalise_title(a), normalise_title(b));
    if na.is_empty() || nb.is_empty() {
        return false;
    }
    if na == nb {
        return true;
    }
    let ta: BTreeSet<&str> = na.split(' ').collect();
    let tb: BTreeSet<&str> = nb.split(' ').collect();
    ta == tb || strsim::jaro_winkler(&na, &nb) >= JW_THRESHOLD
}

fn locations_conflict(a: &Instance, b: &Instance) -> bool {
    match (&a.location, &b.location) {
        (Some(x), Some(y)) => {
            let (x, y) = (normalise_title(x), normalise_title(y));
            !(x.is_empty() || y.is_empty() || x.contains(&y) || y.contains(&x))
        }
        _ => false,
    }
}

pub fn is_duplicate(a: &Instance, b: &Instance, tz: Tz) -> bool {
    if a.uid == b.uid && a.recurrence_id == b.recurrence_id {
        return true;
    }
    if a.meta.kind == EventKind::TaskBlock || b.meta.kind == EventKind::TaskBlock {
        return false;
    }
    if a.span.is_all_day() != b.span.is_all_day() {
        return false;
    }
    let (sa, sb) = (a.start(tz), b.start(tz));
    if (sa - sb).abs() > Duration::minutes(START_TOLERANCE_MIN) {
        return false;
    }
    if sa.with_timezone(&tz).date_naive() != sb.with_timezone(&tz).date_naive() {
        return false;
    }
    if !titles_match(&a.title, &b.title) {
        return false;
    }
    normalise_title(&a.title) == normalise_title(&b.title) || !locations_conflict(a, b)
}

/// A displayed item plus the duplicates it stands for.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Cluster {
    pub primary: Instance,
    /// Hidden duplicates (empty for most events).
    pub others: Vec<Instance>,
}

impl Cluster {
    pub fn is_merged(&self) -> bool {
        !self.others.is_empty()
    }
    pub fn all(&self) -> impl Iterator<Item = &Instance> {
        std::iter::once(&self.primary).chain(self.others.iter())
    }
}

/// Group duplicates. `rank` gives display precedence (lower wins), e.g.
/// Family 0 > iCloud personal 1 > OQTS import 2 > Imported 3.
pub fn cluster(instances: Vec<Instance>, tz: Tz, rank: impl Fn(&Instance) -> u8) -> Vec<Cluster> {
    let n = instances.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut [usize], mut x: usize) -> usize {
        while p[x] != x {
            p[x] = p[p[x]];
            x = p[x];
        }
        x
    }
    // Bucket by local start date to keep this near-linear.
    let mut buckets: HashMap<chrono::NaiveDate, Vec<usize>> = HashMap::new();
    for (i, inst) in instances.iter().enumerate() {
        buckets
            .entry(inst.start(tz).with_timezone(&tz).date_naive())
            .or_default()
            .push(i);
    }
    let mut by_uid: HashMap<(&str, Option<&str>), usize> = HashMap::new();
    for (i, inst) in instances.iter().enumerate() {
        if let Some(&j) = by_uid.get(&(inst.uid.as_str(), inst.recurrence_id.as_deref())) {
            let (a, b) = (find(&mut parent, i), find(&mut parent, j));
            parent[a] = b;
        } else {
            by_uid.insert((inst.uid.as_str(), inst.recurrence_id.as_deref()), i);
        }
    }
    for idx in buckets.values() {
        for (k, &i) in idx.iter().enumerate() {
            for &j in &idx[k + 1..] {
                if is_duplicate(&instances[i], &instances[j], tz) {
                    let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                    if a != b {
                        parent[a] = b;
                    }
                }
            }
        }
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        let r = find(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }
    let mut slots: Vec<Option<Instance>> = instances.into_iter().map(Some).collect();
    let mut out: Vec<Cluster> = groups
        .into_values()
        .filter_map(|mut members| {
            members.sort_by_key(|&i| (slots[i].as_ref().map(&rank).unwrap_or(u8::MAX), i));
            let mut it = members.into_iter();
            let primary = slots[it.next()?].take()?;
            let others = it.filter_map(|i| slots[i].take()).collect();
            Some(Cluster { primary, others })
        })
        .collect();
    out.sort_by(|a, b| {
        a.primary
            .start(tz)
            .cmp(&b.primary.start(tz))
            .then_with(|| a.primary.title.cmp(&b.primary.title))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalisation() {
        assert_eq!(normalise_title("⚑  B4 Sheet!!"), "b4 sheet");
        assert_eq!(normalise_title("OQTS committee meeting! 🎉"), "oqts committee meeting");
        assert_eq!(normalise_title("Café — Talk"), "café talk");
    }

    fn block(uid: &str, start: &str, end: &str) -> Instance {
        let t = |s: &str| {
            chrono::DateTime::parse_from_rfc3339(s)
                .map(|d| d.with_timezone(&chrono::Utc))
                .unwrap_or_default()
        };
        Instance {
            uid: uid.into(),
            recurrence_id: None,
            source: "engineering".into(),
            overlay: crate::config::Overlay::Personal,
            key: Some("Studies".into()),
            title: "☐ B14 Image Processing".into(),
            location: None,
            description: None,
            span: crate::model::Span::Timed {
                start: t(start),
                end: t(end),
            },
            recurring: false,
            transparent: false,
            tentative: false,
            meta: crate::model::Meta {
                kind: EventKind::TaskBlock,
                task_id: Some("t-1".into()),
                ..Default::default()
            },
            href: None,
            alarms: vec![],
        }
    }

    #[test]
    fn two_work_blocks_are_never_one_event() {
        // 2026-10-27: 11:00–13:00 and two blocks at 11:10 of the same task drew as one.
        let tz = chrono_tz::Europe::London;
        let a = block("a", "2026-10-27T11:00:00Z", "2026-10-27T13:00:00Z");
        let b = block("b", "2026-10-27T11:10:00Z", "2026-10-27T12:00:00Z");
        assert!(!is_duplicate(&a, &b, tz));
        assert_eq!(cluster(vec![a.clone(), b], tz, |_| 0).len(), 2);
        // The same block twice (one uid) is still one.
        assert_eq!(cluster(vec![a.clone(), a], tz, |_| 0).len(), 1);
    }

    #[test]
    fn title_matching() {
        assert!(titles_match("OQTS committee meeting!", "OQTS Committee Meeting"));
        assert!(titles_match("Meeting OQTS committee", "oqts committee meeting"));
        assert!(!titles_match("A1 Mathematics Lecture", "A2 Electronics Lecture"));
        assert!(!titles_match("Squash", "Squash practice session with team"));
        assert!(!titles_match("", ""));
    }
}
