//! Overlays, key toggles and the deduplicated view.

use crate::config::{Config, Overlay, SourceKind};
use crate::dedup::{Cluster, cluster};
use crate::expand::{SourceTag, expand};
use crate::model::{EventSeries, Instance};
use crate::normalize::Warning;
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Parsed contents of one configured source.
#[derive(Debug, Clone)]
pub struct LoadedSource {
    pub id: String,
    pub kind: SourceKind,
    pub overlay: Overlay,
    pub key: Option<String>,
    pub series: Vec<EventSeries>,
}

/// What's visible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Filter {
    pub personal: bool,
    pub family: bool,
    pub background: bool,
    pub imported: bool,
    /// Lower-cased key names that are toggled off.
    pub hidden_keys: BTreeSet<String>,
}

impl Default for Filter {
    fn default() -> Self {
        Filter {
            personal: true,
            family: true,
            background: false,
            imported: false,
            hidden_keys: BTreeSet::new(),
        }
    }
}

impl Filter {
    pub fn from_config(c: &Config) -> Filter {
        Filter {
            background: c.overlays.show_background,
            imported: c.overlays.show_imported,
            ..Filter::default()
        }
    }

    pub fn overlay_on(&self, o: Overlay) -> bool {
        match o {
            Overlay::Personal => self.personal,
            Overlay::Family => self.family,
            Overlay::Background => self.background,
            Overlay::Imported => self.imported,
        }
    }

    pub fn allows(&self, i: &Instance) -> bool {
        if !self.overlay_on(i.overlay) {
            return false;
        }
        match (&i.overlay, &i.key) {
            (Overlay::Personal, Some(k)) => !self.hidden_keys.contains(&k.to_lowercase()),
            _ => true,
        }
    }

    pub fn toggle_key(&mut self, key: &str) {
        let k = key.to_lowercase();
        if !self.hidden_keys.remove(&k) {
            self.hidden_keys.insert(k);
        }
    }
}

/// Display precedence: Family > iCloud personal > read-only personal import
/// (OQTS) > Imported > Background.
pub fn rank(i: &Instance, sources: &[LoadedSource]) -> u8 {
    rank_for(i, sources.iter().find(|s| s.id == i.source).map(|s| s.kind))
}

pub fn rank_for(i: &Instance, kind: Option<SourceKind>) -> u8 {
    match (i.overlay, kind) {
        (Overlay::Family, _) => 0,
        (Overlay::Personal, Some(SourceKind::Icloud)) => 1,
        (Overlay::Personal, _) => 2,
        (Overlay::Imported, _) => 3,
        (Overlay::Background, _) => 4,
    }
}

/// All instances in `[from, to)` across sources (no filtering, no dedup).
pub fn instances(
    sources: &[LoadedSource],
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    tz: Tz,
    warnings: &mut Vec<Warning>,
) -> Vec<Instance> {
    let mut out = Vec::new();
    for s in sources {
        let tag = SourceTag {
            id: &s.id,
            overlay: s.overlay,
            key: s.key.as_deref(),
        };
        for series in &s.series {
            out.extend(expand(series, tag, from, to, tz, warnings));
        }
    }
    out
}

/// Filter, then deduplicate (so hiding Family reveals the personal copy).
pub fn view(
    sources: &[LoadedSource],
    filter: &Filter,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    tz: Tz,
    warnings: &mut Vec<Warning>,
) -> Vec<Cluster> {
    let all = instances(sources, from, to, tz, warnings);
    view_of(all, sources, filter, tz)
}

pub fn view_of(all: Vec<Instance>, sources: &[LoadedSource], filter: &Filter, tz: Tz) -> Vec<Cluster> {
    view_with(
        all,
        |id| sources.iter().find(|s| s.id == id).map(|s| s.kind),
        filter,
        tz,
    )
}

/// Like [`view_of`] but with a source-kind lookup (for cached instances).
pub fn view_with(
    all: Vec<Instance>,
    kind_of: impl Fn(&str) -> Option<SourceKind>,
    filter: &Filter,
    tz: Tz,
) -> Vec<Cluster> {
    let visible: Vec<Instance> = all.into_iter().filter(|i| filter.allows(i)).collect();
    cluster(visible, tz, |i| rank_for(i, kind_of(&i.source)))
}
