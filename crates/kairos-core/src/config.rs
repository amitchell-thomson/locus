//! `config.toml` schema and validation. Nothing about Alec's calendars is
//! hard-coded: sources, keys and colours all come from here.

use crate::colour::Rgb;
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Which overlay a source belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Overlay {
    Personal,
    Family,
    Background,
    Imported,
}

impl Overlay {
    pub fn as_str(self) -> &'static str {
        match self {
            Overlay::Personal => "personal",
            Overlay::Family => "family",
            Overlay::Background => "background",
            Overlay::Imported => "imported",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// An iCloud calendar synced by vdirsyncer into a vdir collection.
    Icloud,
    /// A read-only `.ics` URL (Google secret iCal address), fetched by kairos.
    IcsReadonly,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub id: String,
    /// Display name (status-bar legend); defaults to the id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub kind: SourceKind,
    /// iCloud: the calendar's display name (matched against vdir `displayname` files).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar: Option<String>,
    /// iCloud: explicit collection directory, relative to `paths.vdir` (overrides `calendar`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// ics_readonly: a shell command printing the secret URL (keeps it out of the repo).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url_cmd: Option<String>,
    /// ics_readonly: a literal URL (only for public feeds, e.g. holidays).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub overlay: Overlay,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default)]
    pub writable: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyDef {
    pub name: String,
    /// 1–7: the TUI toggle key.
    pub index: u8,
    pub colour: Rgb,
    /// Source id of the iCloud calendar that holds this key's events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlaySettings {
    #[serde(default = "default_family_colour")]
    pub family_colour: Rgb,
    #[serde(default = "default_background_colour")]
    pub background_colour: Rgb,
    #[serde(default = "default_imported_colour")]
    pub imported_colour: Rgb,
    #[serde(default)]
    pub show_background: bool,
    #[serde(default)]
    pub show_imported: bool,
}

fn default_family_colour() -> Rgb {
    Rgb::mocha("yellow")
}
fn default_background_colour() -> Rgb {
    Rgb::mocha("overlay1")
}
fn default_imported_colour() -> Rgb {
    Rgb::mocha("overlay0")
}

/// Full-screen calendar layout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TuiSettings {
    /// Hours where the week/day grid draws a section divider
    /// (default: afternoon 12, evening 18, night 22).
    #[serde(default = "default_section_hours")]
    pub section_hours: Vec<u32>,
    /// University terms, for "1st week MT" in the title bar. `start` is the
    /// Sunday of 1st week (Oxford weeks run Sunday–Saturday).
    #[serde(default)]
    pub terms: Vec<Term>,
    /// Events starting within this many minutes get an "in 25m" marker.
    #[serde(default = "default_soon_minutes")]
    pub soon_minutes: u32,
}

fn default_soon_minutes() -> u32 {
    60
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Term {
    pub name: String,
    pub start: chrono::NaiveDate,
}

impl TuiSettings {
    /// Oxford-style week label for `day`: "1st week MT", "0th week HT" …
    /// Covers 0th–9th week of each term; `None` outside.
    pub fn term_week(&self, day: chrono::NaiveDate) -> Option<String> {
        self.terms.iter().find_map(|t| {
            let wk = (day - t.start).num_days().div_euclid(7) + 1;
            (0..=9).contains(&wk).then(|| {
                let suffix = match wk {
                    1 => "st",
                    2 => "nd",
                    3 => "rd",
                    _ => "th",
                };
                format!("{wk}{suffix} week {}", t.name)
            })
        })
    }
}

fn default_section_hours() -> Vec<u32> {
    vec![12, 18, 22]
}

impl Default for TuiSettings {
    fn default() -> Self {
        TuiSettings {
            section_hours: default_section_hours(),
            terms: Vec::new(),
            soon_minutes: default_soon_minutes(),
        }
    }
}

impl Default for OverlaySettings {
    fn default() -> Self {
        OverlaySettings {
            family_colour: default_family_colour(),
            background_colour: default_background_colour(),
            imported_colour: default_imported_colour(),
            show_background: false,
            show_imported: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Paths {
    /// vdirsyncer storage root (one subdirectory per collection).
    #[serde(default = "default_vdir")]
    pub vdir: String,
    /// kairos data dir (SQLite, read-only feed cache).
    #[serde(default = "default_data")]
    pub data: String,
    /// Cache dir for the fast `next`/`today` index.
    #[serde(default = "default_cache")]
    pub cache: String,
}

fn default_vdir() -> String {
    "~/.local/share/kairos/vdir".into()
}
fn default_data() -> String {
    "~/.local/share/kairos".into()
}
fn default_cache() -> String {
    "~/.cache/kairos".into()
}

impl Default for Paths {
    fn default() -> Self {
        Paths {
            vdir: default_vdir(),
            data: default_data(),
            cache: default_cache(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct DaemonSettings {
    /// Where clients reach kairosd, e.g. `http://compute-node:7420`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Shell command printing the bearer token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_cmd: Option<String>,
    /// kairosd only: listen address (should be the Tailscale IP).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyncSettings {
    /// Command used to sync one collection after a write; `{collection}` is
    /// replaced by the vdirsyncer `pair/collection` name. Empty disables.
    #[serde(default = "default_sync_cmd")]
    pub collection_cmd: String,
    /// Command for a full sync (`kairos sync`).
    #[serde(default = "default_full_sync_cmd")]
    pub full_cmd: String,
    /// vdirsyncer pair name for iCloud.
    #[serde(default = "default_pair")]
    pub pair: String,
    /// Warn in `doctor` if the vdir is older than this.
    #[serde(default = "default_stale_minutes")]
    pub stale_after_minutes: u32,
    /// kairosd: run `full_cmd` this often (iCloud ⇄ server). Phone edits reach
    /// kairos within this; the Mac then copies from the server.
    #[serde(default = "default_every_minutes")]
    pub every_minutes: u64,
}

fn default_every_minutes() -> u64 {
    2
}

fn default_sync_cmd() -> String {
    "vdirsyncer sync {collection}".into()
}
fn default_full_sync_cmd() -> String {
    "vdirsyncer sync".into()
}
fn default_pair() -> String {
    "icloud".into()
}
fn default_stale_minutes() -> u32 {
    15
}

impl Default for SyncSettings {
    fn default() -> Self {
        SyncSettings {
            collection_cmd: default_sync_cmd(),
            full_cmd: default_full_sync_cmd(),
            pair: default_pair(),
            stale_after_minutes: default_stale_minutes(),
            every_minutes: default_every_minutes(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotifySettings {
    /// ntfy server, e.g. `https://ntfy.sh`.
    #[serde(default = "default_ntfy_server")]
    pub server: String,
    /// Command printing the (secret) topic name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic_cmd: Option<String>,
    /// Command printing an ntfy access token, if the topic is protected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_cmd: Option<String>,
    /// URL the phone uses to reach kairosd over Tailscale (for action buttons).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
    #[serde(default = "default_morning")]
    pub morning_summary: String,
    #[serde(default = "default_checkin")]
    pub evening_checkin: String,
    /// When the daily tidy-up runs that counts week-old unanswered blocks
    /// as done (see `autodone_after_days`).
    #[serde(default = "default_autodone")]
    pub autodone_at: String,
    /// A work block nobody checked in on counts as done after this many
    /// days (Alec checks in when it suits him — yesterday's at 10:00 is
    /// fine). Until then the scheduler assumes it's done, and it waits (󰡖).
    #[serde(default = "default_autodone_days")]
    pub autodone_after_days: u32,
}

fn default_autodone_days() -> u32 {
    7
}

fn default_ntfy_server() -> String {
    "https://ntfy.sh".into()
}
fn default_morning() -> String {
    "07:30".into()
}
fn default_checkin() -> String {
    "21:30".into()
}
fn default_autodone() -> String {
    "09:00".into()
}

impl Default for NotifySettings {
    fn default() -> Self {
        NotifySettings {
            server: default_ntfy_server(),
            topic_cmd: None,
            token_cmd: None,
            public_url: None,
            morning_summary: default_morning(),
            evening_checkin: default_checkin(),
            autodone_at: default_autodone(),
            autodone_after_days: default_autodone_days(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeadlineSettings {
    /// VALARM offsets for timed deadlines, in minutes before.
    #[serde(default = "default_alarms")]
    pub alarms_minutes_before: Vec<i64>,
}

fn default_alarms() -> Vec<i64> {
    vec![24 * 60, 120]
}

impl Default for DeadlineSettings {
    fn default() -> Self {
        DeadlineSettings {
            alarms_minutes_before: default_alarms(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_tz")]
    pub timezone: String,
    /// When false, kairosd writes only to `test_calendar`.
    #[serde(default)]
    pub live: bool,
    #[serde(default = "default_test_calendar")]
    pub test_calendar: String,
    #[serde(default)]
    pub paths: Paths,
    #[serde(default)]
    pub daemon: DaemonSettings,
    #[serde(default)]
    pub sync: SyncSettings,
    #[serde(default)]
    pub notify: NotifySettings,
    #[serde(default)]
    pub deadlines: DeadlineSettings,
    #[serde(default)]
    pub overlays: OverlaySettings,
    #[serde(default)]
    pub tui: TuiSettings,
    #[serde(default)]
    pub keys: Vec<KeyDef>,
    #[serde(default)]
    pub sources: Vec<Source>,
}

fn default_tz() -> String {
    "Europe/London".into()
}
fn default_test_calendar() -> String {
    "ZZ-kairos-test".into()
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read {path}: {msg}")]
    Read { path: String, msg: String },
    #[error("TOML error: {0}")]
    Toml(String),
    #[error("unknown timezone {0:?}")]
    Timezone(String),
    #[error("duplicate source id {0:?}")]
    DuplicateSource(String),
    #[error("source {source_id:?} refers to unknown key {key:?}")]
    UnknownKey { source_id: String, key: String },
    #[error("key {key:?} refers to unknown source {source_id:?}")]
    UnknownKeySource { key: String, source_id: String },
    #[error("duplicate key name {0:?}")]
    DuplicateKey(String),
    #[error("key {key:?}: index {index} must be 1–7 and unique")]
    KeyIndex { key: String, index: u8 },
    #[error("more than 7 keys defined ({0})")]
    TooManyKeys(usize),
    #[error("source {0:?}: read-only (ics_readonly) sources must have writable = false")]
    ReadonlyWritable(String),
    #[error("source {0:?}: background/imported overlays cannot be writable")]
    LayerWritable(String),
    #[error("source {0:?}: ics_readonly needs exactly one of url / url_cmd")]
    MissingUrl(String),
    #[error("source {0:?}: icloud sources need calendar or path")]
    MissingCalendar(String),
    #[error("source {0:?}: personal-overlay sources should have a key")]
    PersonalWithoutKey(String),
    #[error("more than one family source ({0}); kairos supports one Family calendar")]
    MultipleFamily(usize),
}

impl Config {
    pub fn from_toml(s: &str) -> Result<Config, ConfigError> {
        toml::from_str(s).map_err(|e| ConfigError::Toml(e.to_string()))
    }

    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        let s = std::fs::read_to_string(path).map_err(|e| ConfigError::Read {
            path: path.display().to_string(),
            msg: e.to_string(),
        })?;
        Config::from_toml(&s)
    }

    /// All problems, not just the first.
    pub fn validate(&self) -> Vec<ConfigError> {
        let mut errs = Vec::new();
        if self.timezone.parse::<Tz>().is_err() {
            errs.push(ConfigError::Timezone(self.timezone.clone()));
        }
        if self.keys.len() > 7 {
            errs.push(ConfigError::TooManyKeys(self.keys.len()));
        }
        let mut key_names = HashSet::new();
        let mut key_idx = HashSet::new();
        for k in &self.keys {
            if !key_names.insert(k.name.to_lowercase()) {
                errs.push(ConfigError::DuplicateKey(k.name.clone()));
            }
            if !(1..=7).contains(&k.index) || !key_idx.insert(k.index) {
                errs.push(ConfigError::KeyIndex {
                    key: k.name.clone(),
                    index: k.index,
                });
            }
        }
        let mut ids = HashSet::new();
        for s in &self.sources {
            if !ids.insert(s.id.as_str()) {
                errs.push(ConfigError::DuplicateSource(s.id.clone()));
            }
            if let Some(k) = &s.key
                && self.key(k).is_none()
            {
                errs.push(ConfigError::UnknownKey {
                    source_id: s.id.clone(),
                    key: k.clone(),
                });
            }
            match s.kind {
                SourceKind::IcsReadonly => {
                    if s.writable {
                        errs.push(ConfigError::ReadonlyWritable(s.id.clone()));
                    }
                    if s.url.is_some() == s.url_cmd.is_some() {
                        errs.push(ConfigError::MissingUrl(s.id.clone()));
                    }
                }
                SourceKind::Icloud => {
                    if s.calendar.is_none() && s.path.is_none() {
                        errs.push(ConfigError::MissingCalendar(s.id.clone()));
                    }
                }
            }
            if s.writable && matches!(s.overlay, Overlay::Background | Overlay::Imported) {
                errs.push(ConfigError::LayerWritable(s.id.clone()));
            }
            if s.overlay == Overlay::Personal && s.key.is_none() {
                errs.push(ConfigError::PersonalWithoutKey(s.id.clone()));
            }
        }
        let fam = self.sources.iter().filter(|s| s.overlay == Overlay::Family).count();
        if fam > 1 {
            errs.push(ConfigError::MultipleFamily(fam));
        }
        for k in &self.keys {
            if let Some(src) = &k.calendar
                && !self.sources.iter().any(|s| &s.id == src)
            {
                errs.push(ConfigError::UnknownKeySource {
                    key: k.name.clone(),
                    source_id: src.clone(),
                });
            }
        }
        errs
    }

    pub fn tz(&self) -> Tz {
        self.timezone.parse().unwrap_or(chrono_tz::Europe::London)
    }

    pub fn key(&self, name: &str) -> Option<&KeyDef> {
        self.keys.iter().find(|k| k.name.eq_ignore_ascii_case(name))
    }

    /// The inbox: key 1, where events without an explicit key land.
    pub fn default_key(&self) -> Option<&KeyDef> {
        self.keys.iter().min_by_key(|k| k.index)
    }

    pub fn source(&self, id: &str) -> Option<&Source> {
        self.sources.iter().find(|s| s.id == id)
    }

    /// The writable iCloud source holding a key's events.
    pub fn source_for_key(&self, key: &str) -> Option<&Source> {
        let k = self.key(key)?;
        if let Some(id) = &k.calendar {
            return self.source(id);
        }
        self.sources.iter().find(|s| {
            s.writable
                && s.kind == SourceKind::Icloud
                && s.key.as_deref().is_some_and(|sk| sk.eq_ignore_ascii_case(key))
        })
    }

    pub fn family_source(&self) -> Option<&Source> {
        self.sources.iter().find(|s| s.overlay == Overlay::Family)
    }

    pub fn vdir(&self) -> PathBuf {
        expand_home(&self.paths.vdir)
    }
    pub fn data_dir(&self) -> PathBuf {
        expand_home(&self.paths.data)
    }
    pub fn cache_dir(&self) -> PathBuf {
        expand_home(&self.paths.cache)
    }

    /// Default config path: `$KAIROS_CONFIG` or `~/.config/kairos/config.toml`.
    pub fn default_path() -> PathBuf {
        if let Ok(p) = std::env::var("KAIROS_CONFIG") {
            return PathBuf::from(p);
        }
        expand_home("~/.config/kairos/config.toml")
    }
}

/// Expand a leading `~/`.
pub fn expand_home(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oxford_term_weeks() {
        let t = TuiSettings {
            terms: vec![Term {
                name: "MT".into(),
                start: chrono::NaiveDate::from_ymd_opt(2026, 10, 11).unwrap(),
            }],
            ..TuiSettings::default()
        };
        let d = |m, day| chrono::NaiveDate::from_ymd_opt(2026, m, day).unwrap();
        assert_eq!(t.term_week(d(10, 4)).as_deref(), Some("0th week MT"));
        assert_eq!(
            t.term_week(d(10, 11)).as_deref(),
            Some("1st week MT"),
            "Sunday starts the week"
        );
        assert_eq!(t.term_week(d(10, 17)).as_deref(), Some("1st week MT"));
        assert_eq!(t.term_week(d(10, 18)).as_deref(), Some("2nd week MT"));
        assert_eq!(t.term_week(d(12, 5)).as_deref(), Some("8th week MT"));
        assert_eq!(t.term_week(d(12, 12)).as_deref(), Some("9th week MT"));
        assert_eq!(t.term_week(d(12, 20)), None);
        assert_eq!(t.term_week(d(9, 30)), None);
    }

    const EXAMPLE: &str = include_str!("../../../config/config.example.toml");

    fn example() -> Config {
        Config::from_toml(EXAMPLE).unwrap()
    }

    #[test]
    fn example_parses_and_validates() {
        let c = example();
        assert_eq!(c.validate(), vec![]);
        assert!(!c.live, "example must not be live");
        assert_eq!(c.test_calendar, "ZZ-kairos-test");
        assert_eq!(c.keys.len(), 6);
        assert_eq!(c.tz(), chrono_tz::Europe::London);
        assert_eq!(c.source_for_key("sport").unwrap().id, "sport");
        assert_eq!(c.family_source().unwrap().id, "family");
    }

    #[test]
    fn serde_round_trip() {
        let c = example();
        let s = toml::to_string(&c).unwrap();
        assert_eq!(Config::from_toml(&s).unwrap(), c);
    }

    #[test]
    fn defaults_are_safe() {
        let c = Config::from_toml("").unwrap();
        assert!(!c.live);
        assert_eq!(c.timezone, "Europe/London");
        assert_eq!(c.validate(), vec![]);
    }

    fn with(f: impl FnOnce(&mut Config)) -> Vec<ConfigError> {
        let mut c = example();
        f(&mut c);
        c.validate()
    }

    #[test]
    fn rejects_bad_timezone() {
        let e = with(|c| c.timezone = "Europe/Londn".into());
        assert_eq!(e, vec![ConfigError::Timezone("Europe/Londn".into())]);
    }

    #[test]
    fn rejects_duplicate_source() {
        let e = with(|c| {
            let s = c.sources[0].clone();
            c.sources.push(s);
        });
        assert!(matches!(e[0], ConfigError::DuplicateSource(_)), "{e:?}");
    }

    #[test]
    fn rejects_unknown_key() {
        let e = with(|c| c.sources[0].key = Some("Nope".into()));
        assert!(e.iter().any(|x| matches!(x, ConfigError::UnknownKey { .. })), "{e:?}");
    }

    #[test]
    fn rejects_writable_readonly_source() {
        let e = with(|c| {
            let s = c
                .sources
                .iter_mut()
                .find(|s| s.kind == SourceKind::IcsReadonly)
                .unwrap();
            s.writable = true;
        });
        assert!(e.iter().any(|x| matches!(x, ConfigError::ReadonlyWritable(_))), "{e:?}");
    }

    #[test]
    fn rejects_bad_key_index() {
        let e = with(|c| c.keys[1].index = c.keys[0].index);
        assert!(e.iter().any(|x| matches!(x, ConfigError::KeyIndex { .. })), "{e:?}");
        let e = with(|c| c.keys[0].index = 9);
        assert!(e.iter().any(|x| matches!(x, ConfigError::KeyIndex { .. })), "{e:?}");
    }

    #[test]
    fn rejects_duplicate_key_name() {
        let e = with(|c| c.keys[1].name = c.keys[0].name.to_uppercase());
        assert!(e.iter().any(|x| matches!(x, ConfigError::DuplicateKey(_))), "{e:?}");
    }

    #[test]
    fn rejects_too_many_keys() {
        let e = with(|c| {
            while c.keys.len() < 8 {
                let mut k = c.keys[0].clone();
                k.index = c.keys.len() as u8 + 1;
                k.name = format!("Extra{}", k.index);
                c.keys.push(k);
            }
        });
        assert!(e.iter().any(|x| matches!(x, ConfigError::TooManyKeys(8))), "{e:?}");
    }

    #[test]
    fn rejects_missing_url() {
        let e = with(|c| {
            let s = c
                .sources
                .iter_mut()
                .find(|s| s.kind == SourceKind::IcsReadonly)
                .unwrap();
            s.url = None;
            s.url_cmd = None;
        });
        assert!(e.iter().any(|x| matches!(x, ConfigError::MissingUrl(_))), "{e:?}");
    }

    #[test]
    fn rejects_icloud_without_calendar() {
        let e = with(|c| {
            c.sources[0].calendar = None;
            c.sources[0].path = None;
        });
        assert!(e.iter().any(|x| matches!(x, ConfigError::MissingCalendar(_))), "{e:?}");
    }

    #[test]
    fn rejects_writable_background() {
        let e = with(|c| {
            let s = c.sources.iter_mut().find(|s| s.overlay == Overlay::Background).unwrap();
            s.kind = SourceKind::Icloud;
            s.calendar = Some("x".into());
            s.url = None;
            s.writable = true;
        });
        assert!(e.iter().any(|x| matches!(x, ConfigError::LayerWritable(_))), "{e:?}");
    }

    #[test]
    fn rejects_personal_without_key() {
        let e = with(|c| c.sources[0].key = None);
        assert!(
            e.iter().any(|x| matches!(x, ConfigError::PersonalWithoutKey(_))),
            "{e:?}"
        );
    }

    #[test]
    fn rejects_two_family_sources() {
        let e = with(|c| {
            let mut s = c.family_source().unwrap().clone();
            s.id = "family2".into();
            c.sources.push(s);
        });
        assert!(e.iter().any(|x| matches!(x, ConfigError::MultipleFamily(2))), "{e:?}");
    }

    #[test]
    fn rejects_key_with_unknown_calendar() {
        let e = with(|c| c.keys[0].calendar = Some("ghost".into()));
        assert!(
            e.iter().any(|x| matches!(x, ConfigError::UnknownKeySource { .. })),
            "{e:?}"
        );
    }

    #[test]
    fn unknown_fields_are_errors() {
        assert!(Config::from_toml("timezon = \"Europe/London\"").is_err());
    }
}
