//! `constraints.toml`: when the scheduler may place task blocks.

use chrono::{NaiveDate, NaiveTime, Weekday};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A wall-clock time `HH:MM`. `24:00` is allowed as an end-of-day bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Hm(pub u16); // minutes since midnight, 0..=1440

impl Hm {
    pub fn minutes(self) -> u16 {
        self.0
    }
    pub fn parse(s: &str) -> Option<Hm> {
        let (h, m) = s.trim().split_once(':')?;
        let h: u16 = h.parse().ok()?;
        let m: u16 = m.parse().ok()?;
        if m >= 60 || h > 24 || (h == 24 && m != 0) {
            return None;
        }
        Some(Hm(h * 60 + m))
    }
    pub fn to_time(self) -> NaiveTime {
        let m = u32::from(self.0.min(1439));
        NaiveTime::from_hms_opt(m / 60, m % 60, 0).unwrap_or(NaiveTime::MIN)
    }
}

impl std::fmt::Display for Hm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:02}:{:02}", self.0 / 60, self.0 % 60)
    }
}

impl Serialize for Hm {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Hm {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Hm::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("bad time {s:?}, want HH:MM")))
    }
}

/// `[start, end)` within a day. `end < start` wraps past midnight (only
/// meaningful for `sleep`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Window {
    pub start: Hm,
    pub end: Hm,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NamedWindow {
    pub name: String,
    pub start: Hm,
    pub end: Hm,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DateRange {
    pub from: NaiveDate,
    /// Inclusive.
    pub to: NaiveDate,
}

impl DateRange {
    pub fn contains(&self, d: NaiveDate) -> bool {
        self.from <= d && d <= self.to
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoSchedule {
    /// Weekdays this applies to (`mon`..`sun`); empty = every day.
    #[serde(default, with = "weekdays")]
    pub days: Vec<Weekday>,
    pub start: Hm,
    pub end: Hm,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub name: String,
    /// When this profile applies. The profile with no dates is the fallback.
    #[serde(default)]
    pub dates: Vec<DateRange>,
    pub day_window: Window,
    /// Protected sleep; nothing is scheduled inside it even if day_window says so.
    pub sleep: Window,
    #[serde(default)]
    pub meals: Vec<NamedWindow>,
    pub max_focus_hours_per_day: f32,
    #[serde(default)]
    pub deep_work: Vec<Window>,
    #[serde(default = "default_buffer")]
    pub buffer_minutes: u16,
    #[serde(default = "default_buffer_loc")]
    pub buffer_location_change_minutes: u16,
    #[serde(default)]
    pub no_schedule: Vec<NoSchedule>,
    #[serde(default = "default_lead")]
    pub deadline_lead_hours: u16,
}

fn default_buffer() -> u16 {
    10
}
fn default_buffer_loc() -> u16 {
    30
}
fn default_lead() -> u16 {
    12
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Constraints {
    /// Named preferred windows for tasks (`morning`, `afternoon`, `evening`, …).
    #[serde(default = "default_windows")]
    pub windows: Vec<NamedWindow>,
    /// Scheduling granularity in minutes.
    #[serde(default = "default_slot")]
    pub slot_minutes: u16,
    /// How far ahead the scheduler looks for tasks without a deadline.
    #[serde(default = "default_horizon")]
    pub horizon_days: u16,
    /// Never schedule within this many minutes of now.
    #[serde(default = "default_min_lead")]
    pub min_lead_minutes: u16,
    pub profiles: Vec<Profile>,
}

fn default_slot() -> u16 {
    15
}
fn default_horizon() -> u16 {
    14
}
fn default_min_lead() -> u16 {
    30
}

fn default_windows() -> Vec<NamedWindow> {
    vec![
        NamedWindow {
            name: "morning".into(),
            start: Hm(8 * 60),
            end: Hm(12 * 60),
        },
        NamedWindow {
            name: "afternoon".into(),
            start: Hm(12 * 60),
            end: Hm(17 * 60),
        },
        NamedWindow {
            name: "evening".into(),
            start: Hm(17 * 60),
            end: Hm(22 * 60),
        },
    ]
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConstraintsError {
    #[error("TOML error: {0}")]
    Toml(String),
    #[error("no profiles defined")]
    NoProfiles,
    #[error("exactly one profile must have no dates (the fallback); found {0}")]
    Fallback(usize),
    #[error("profiles {a:?} and {b:?} have overlapping date ranges")]
    Overlap { a: String, b: String },
    #[error("profile {0:?}: day_window start must be before end")]
    DayWindow(String),
    #[error("profile {0:?}: date range with from > to")]
    BadRange(String),
    #[error("slot_minutes must divide 60 (got {0})")]
    Slot(u16),
}

impl Constraints {
    pub fn from_toml(s: &str) -> Result<Constraints, ConstraintsError> {
        let c: Constraints = toml::from_str(s).map_err(|e| ConstraintsError::Toml(e.to_string()))?;
        let errs = c.validate();
        match errs.into_iter().next() {
            Some(e) => Err(e),
            None => Ok(c),
        }
    }

    pub fn validate(&self) -> Vec<ConstraintsError> {
        let mut errs = Vec::new();
        if self.profiles.is_empty() {
            errs.push(ConstraintsError::NoProfiles);
        }
        if self.slot_minutes == 0 || 60 % self.slot_minutes != 0 {
            errs.push(ConstraintsError::Slot(self.slot_minutes));
        }
        let fallbacks = self.profiles.iter().filter(|p| p.dates.is_empty()).count();
        if !self.profiles.is_empty() && fallbacks != 1 {
            errs.push(ConstraintsError::Fallback(fallbacks));
        }
        for p in &self.profiles {
            if p.day_window.start >= p.day_window.end {
                errs.push(ConstraintsError::DayWindow(p.name.clone()));
            }
            if p.dates.iter().any(|r| r.from > r.to) {
                errs.push(ConstraintsError::BadRange(p.name.clone()));
            }
        }
        for (i, a) in self.profiles.iter().enumerate() {
            for b in &self.profiles[i + 1..] {
                let overlap = a
                    .dates
                    .iter()
                    .any(|x| b.dates.iter().any(|y| x.from <= y.to && y.from <= x.to));
                if overlap {
                    errs.push(ConstraintsError::Overlap {
                        a: a.name.clone(),
                        b: b.name.clone(),
                    });
                }
            }
        }
        errs
    }

    /// The profile in force on `d`.
    pub fn profile_for(&self, d: NaiveDate) -> Option<&Profile> {
        self.profiles
            .iter()
            .find(|p| p.dates.iter().any(|r| r.contains(d)))
            .or_else(|| self.profiles.iter().find(|p| p.dates.is_empty()))
    }

    pub fn window(&self, name: &str) -> Option<&NamedWindow> {
        self.windows.iter().find(|w| w.name.eq_ignore_ascii_case(name))
    }
}

mod weekdays {
    use chrono::Weekday;
    use serde::{Deserialize, Deserializer, Serializer, ser::SerializeSeq};

    pub fn serialize<S: Serializer>(v: &[Weekday], s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(v.len()))?;
        for d in v {
            seq.serialize_element(&d.to_string().to_lowercase())?;
        }
        seq.end()
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Weekday>, D::Error> {
        let v = Vec::<String>::deserialize(d)?;
        v.iter()
            .map(|s| {
                s.parse::<Weekday>()
                    .map_err(|_| serde::de::Error::custom(format!("bad weekday {s:?}")))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = include_str!("../../../config/constraints.example.toml");

    #[test]
    fn example_parses() {
        let c = Constraints::from_toml(EXAMPLE).unwrap();
        assert_eq!(c.validate(), vec![]);
        let term_day = NaiveDate::from_ymd_opt(2026, 10, 20).unwrap();
        let vac_day = NaiveDate::from_ymd_opt(2026, 12, 25).unwrap();
        assert_eq!(c.profile_for(term_day).unwrap().name, "term");
        assert_eq!(c.profile_for(vac_day).unwrap().name, "vacation");
        assert_eq!(c.window("Morning").unwrap().start, Hm(8 * 60));
        let sat = &c.profile_for(term_day).unwrap().no_schedule[0];
        assert_eq!(sat.days, vec![Weekday::Sat]);
    }

    #[test]
    fn round_trip() {
        let c = Constraints::from_toml(EXAMPLE).unwrap();
        let s = toml::to_string(&c).unwrap();
        assert_eq!(Constraints::from_toml(&s).unwrap(), c);
    }

    #[test]
    fn hm_parse() {
        assert_eq!(Hm::parse("08:30"), Some(Hm(510)));
        assert_eq!(Hm::parse("24:00"), Some(Hm(1440)));
        assert_eq!(Hm::parse("24:01"), None);
        assert_eq!(Hm::parse("8:60"), None);
        assert_eq!(Hm(510).to_string(), "08:30");
    }

    #[test]
    fn overlapping_profiles_rejected() {
        let mut c = Constraints::from_toml(EXAMPLE).unwrap();
        let r = c.profiles[0].dates[0].clone();
        c.profiles[1].dates.push(r);
        // now no fallback and an overlap
        let errs = c.validate();
        assert!(
            errs.iter().any(|e| matches!(e, ConstraintsError::Overlap { .. })),
            "{errs:?}"
        );
        assert!(
            errs.iter().any(|e| matches!(e, ConstraintsError::Fallback(0))),
            "{errs:?}"
        );
    }

    #[test]
    fn bad_slot_rejected() {
        let mut c = Constraints::from_toml(EXAMPLE).unwrap();
        c.slot_minutes = 7;
        assert!(c.validate().contains(&ConstraintsError::Slot(7)));
    }
}
