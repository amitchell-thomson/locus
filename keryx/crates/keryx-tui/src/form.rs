//! Guided forms for the kairos link (SPEC §9): `t` task, `i` event and `w`
//! free times. Adapted from kairos-tui's `form.rs` (fields, pickers with
//! ◀ ▶, a live preview line); the preview shows exactly what is sent.

use crate::app::Key;
use chrono::{DateTime, Datelike, Duration, Timelike};
use chrono_tz::Tz;
use keryx_core::config::Config;
use keryx_core::when::{Canon, canonical};
use keryx_store::api::{FreeReq, KairosEventReq, KairosTaskReq};

/// Task types the form offers (kairos's built-in ones).
pub const TASK_TYPES: [&str; 4] = ["none", "sheet", "coursework", "admin"];
pub const WINDOWS: [&str; 3] = ["this week", "next week", "pick"];

#[derive(Debug, Clone, PartialEq)]
pub enum FieldKind {
    Text,
    /// A time (required).
    When,
    /// A time or empty.
    OptWhen,
    /// Minutes ("2h", "90m", "1h30"), empty allowed.
    Minutes,
    /// A whole number.
    Count,
    /// One of these (◀ ▶ or the first letter).
    Pick(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub label: &'static str,
    pub value: String,
    pub kind: FieldKind,
    pub hint: &'static str,
}

impl Field {
    fn new(label: &'static str, value: impl Into<String>, kind: FieldKind, hint: &'static str) -> Field {
        Field {
            label,
            value: value.into(),
            kind,
            hint,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Purpose {
    Task {
        r#ref: String,
        suggestion: Option<String>,
    },
    Event {
        r#ref: String,
        suggestion: Option<String>,
    },
    /// Free times, then a reply in `$EDITOR`.
    Free {
        r#ref: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Form {
    pub purpose: Purpose,
    pub fields: Vec<Field>,
    pub focus: usize,
    /// Claude's reason, shown above the fields when accepting a suggestion.
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FormAction {
    Continue,
    Submit,
    Cancel,
}

/// What a valid form sends.
#[derive(Debug, Clone, PartialEq)]
pub enum Built {
    Task(KairosTaskReq),
    Event(KairosEventReq),
    Free(FreeReq),
}

pub const WHEN_HINT: &str = "fri 17:00 · 23 oct · tomorrow 9 · 2026-10-23T17:00";

/// kairos calendars: the keys' `kairos_key`s plus kairos's defaults.
pub fn calendars(cfg: &Config) -> Vec<String> {
    let mut v: Vec<String> = ["Personal", "Engineering", "Careers", "Projects", "Sport", "Social"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    for k in &cfg.keys {
        if let Some(c) = &k.kairos_key
            && !v.contains(c)
        {
            v.push(c.clone());
        }
    }
    v
}

/// "2 h 30 min" for a preview.
pub fn minutes_label(m: u32) -> String {
    match (m / 60, m % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// The free-times window for a choice: (from, to) as sent to kairos.
pub fn window(choice: &str, now: DateTime<Tz>) -> (String, String) {
    let start_of_hour = now
        .with_minute(0)
        .and_then(|d| d.with_second(0))
        .map(|d| d + Duration::hours(1))
        .unwrap_or(now);
    let monday = now.date_naive() - Duration::days(i64::from(now.weekday().num_days_from_monday()));
    match choice {
        "next week" => (
            (monday + Duration::days(7)).format("%Y-%m-%d").to_string(),
            (monday + Duration::days(14)).format("%Y-%m-%d").to_string(),
        ),
        _ => (
            start_of_hour.format("%Y-%m-%dT%H:%M").to_string(),
            (monday + Duration::days(7)).format("%Y-%m-%d").to_string(),
        ),
    }
}

impl Form {
    pub fn task(cfg: &Config, r#ref: &str, title: &str, calendar: &str, suggestion: Option<(String, String)>) -> Form {
        let cals = calendars(cfg);
        let (sid, reason) = suggestion.map(|(a, b)| (Some(a), Some(b))).unwrap_or((None, None));
        Form {
            purpose: Purpose::Task {
                r#ref: r#ref.into(),
                suggestion: sid,
            },
            fields: vec![
                Field::new("Title", title, FieldKind::Text, ""),
                Field::new(
                    "Calendar",
                    if cals.iter().any(|c| c == calendar) {
                        calendar
                    } else {
                        "Personal"
                    },
                    FieldKind::Pick(cals),
                    "◀ ▶ or first letter",
                ),
                Field::new(
                    "Type",
                    "none",
                    FieldKind::Pick(TASK_TYPES.iter().map(|s| s.to_string()).collect()),
                    "sheet ≈ 5 h in 2–3 h blocks · coursework needs an estimate · admin one 30–60 min go",
                ),
                Field::new(
                    "Estimate",
                    "",
                    FieldKind::Minutes,
                    "2h · 90m · 1h30 · empty = the type's default",
                ),
                Field::new("Due", "", FieldKind::OptWhen, WHEN_HINT),
            ],
            focus: 0,
            reason,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn event(
        cfg: &Config,
        r#ref: &str,
        title: &str,
        calendar: &str,
        start: &str,
        end: &str,
        location: &str,
        suggestion: Option<(String, String)>,
    ) -> Form {
        let cals = calendars(cfg);
        let (sid, reason) = suggestion.map(|(a, b)| (Some(a), Some(b))).unwrap_or((None, None));
        Form {
            purpose: Purpose::Event {
                r#ref: r#ref.into(),
                suggestion: sid,
            },
            fields: vec![
                Field::new("Title", title, FieldKind::Text, ""),
                Field::new(
                    "Calendar",
                    if cals.iter().any(|c| c == calendar) {
                        calendar
                    } else {
                        "Personal"
                    },
                    FieldKind::Pick(cals),
                    "◀ ▶ or first letter",
                ),
                Field::new("Start", start, FieldKind::When, WHEN_HINT),
                Field::new("End", end, FieldKind::OptWhen, "empty = kairos's default length"),
                Field::new("Location", location, FieldKind::Text, ""),
            ],
            focus: 0,
            reason,
        }
    }

    pub fn free(r#ref: &str, now: DateTime<Tz>) -> Form {
        let (from, to) = window("this week", now);
        Form {
            purpose: Purpose::Free { r#ref: r#ref.into() },
            fields: vec![
                Field::new(
                    "Window",
                    "this week",
                    FieldKind::Pick(WINDOWS.iter().map(|s| s.to_string()).collect()),
                    "◀ ▶ · editing From / To picks your own",
                ),
                Field::new("From", from, FieldKind::When, WHEN_HINT),
                Field::new("To", to, FieldKind::When, WHEN_HINT),
                Field::new("Duration", "1h", FieldKind::Minutes, "how long each slot must be"),
                Field::new("How many", "4", FieldKind::Count, "slots to offer (spread across days)"),
            ],
            focus: 0,
            reason: None,
        }
    }

    pub fn title(&self) -> &'static str {
        match self.purpose {
            Purpose::Task { .. } => "kairos task",
            Purpose::Event { .. } => "kairos event",
            Purpose::Free { .. } => "reply with free times",
        }
    }

    fn value(&self, label: &str) -> &str {
        self.fields
            .iter()
            .find(|f| f.label == label)
            .map(|f| f.value.trim())
            .unwrap_or("")
    }

    fn set(&mut self, label: &str, v: String) {
        if let Some(f) = self.fields.iter_mut().find(|f| f.label == label) {
            f.value = v;
        }
    }

    fn cycle(&mut self, d: isize) {
        let f = &mut self.fields[self.focus];
        if let FieldKind::Pick(opts) = &f.kind
            && !opts.is_empty()
        {
            let i = opts.iter().position(|o| *o == f.value).unwrap_or(0) as isize;
            let n = opts.len() as isize;
            f.value = opts[((i + d).rem_euclid(n)) as usize].clone();
        }
    }

    fn jump(&mut self, c: char) {
        let f = &mut self.fields[self.focus];
        if let FieldKind::Pick(opts) = &f.kind {
            let lc = c.to_ascii_lowercase();
            let start = opts.iter().position(|o| *o == f.value).map_or(0, |i| i + 1);
            let hit = (0..opts.len())
                .map(|k| (start + k) % opts.len())
                .find(|&k| opts[k].to_ascii_lowercase().starts_with(lc));
            if let Some(k) = hit {
                f.value = opts[k].clone();
            }
        }
    }

    /// The free-times window follows the choice; typing a date means "pick".
    fn after_edit(&mut self, now: DateTime<Tz>, label: &str) {
        if !matches!(self.purpose, Purpose::Free { .. }) {
            return;
        }
        match label {
            "Window" => {
                let w = self.value("Window").to_string();
                if w != "pick" {
                    let (a, b) = window(&w, now);
                    self.set("From", a);
                    self.set("To", b);
                }
            }
            "From" | "To" => self.set("Window", "pick".into()),
            _ => {}
        }
    }

    pub fn key(&mut self, k: Key, now: DateTime<Tz>) -> FormAction {
        let n = self.fields.len();
        let picker = matches!(self.fields[self.focus].kind, FieldKind::Pick(_));
        let label = self.fields[self.focus].label;
        match k {
            Key::Esc => return FormAction::Cancel,
            Key::Enter => return FormAction::Submit,
            Key::Tab | Key::Down => self.focus = (self.focus + 1) % n,
            Key::BackTab | Key::Up => self.focus = (self.focus + n - 1) % n,
            Key::Left if picker => self.cycle(-1),
            Key::Right if picker => self.cycle(1),
            Key::Char(c) if picker => self.jump(c),
            Key::Char(c) => self.fields[self.focus].value.push(c),
            Key::Backspace if !picker => {
                self.fields[self.focus].value.pop();
            }
            _ => return FormAction::Continue,
        }
        if !matches!(k, Key::Tab | Key::Down | Key::BackTab | Key::Up) {
            self.after_edit(now, label);
        }
        FormAction::Continue
    }

    fn when(&self, cfg: &Config, now: DateTime<Tz>, label: &str, required: bool) -> Result<Option<Canon>, String> {
        let v = self.value(label);
        if v.is_empty() {
            return if required {
                Err(format!("{label} is needed"))
            } else {
                Ok(None)
            };
        }
        canonical(cfg, now, v).map(Some).map_err(|e| format!("{label}: {e}"))
    }

    fn minutes(&self, label: &str) -> Result<Option<u32>, String> {
        let v = self.value(label);
        if v.is_empty() {
            return Ok(None);
        }
        eco_dates::parse_minutes(v)
            .map(Some)
            .map_err(|_| format!("{label}: try 2h, 90m or 1h30"))
    }

    /// The request this form sends, and a one-line preview of it.
    pub fn build(&self, cfg: &Config, now: DateTime<Tz>) -> Result<(Built, String), String> {
        let title = self.value("Title").to_string();
        match &self.purpose {
            Purpose::Task { r#ref, suggestion } => {
                if title.is_empty() {
                    return Err("the task needs a title".into());
                }
                let kind = Some(self.value("Type").to_string()).filter(|k| k != "none" && !k.is_empty());
                let est = self.minutes("Estimate")?;
                if kind.as_deref() == Some("coursework") && est.is_none() {
                    return Err("coursework needs an estimate".into());
                }
                let due = self.when(cfg, now, "Due", false)?;
                if let Some(d) = &due
                    && d.ts < now.timestamp()
                    && d.iso.contains('T')
                {
                    return Err("Due: that's in the past".into());
                }
                let cal = self.value("Calendar").to_string();
                let mut p = format!("{title} · {cal}");
                if let Some(k) = &kind {
                    p.push_str(&format!(" · {k}"));
                }
                if let Some(m) = est {
                    p.push_str(&format!(" · {}", minutes_label(m)));
                }
                if let Some(d) = &due {
                    p.push_str(&format!(" · due {}", d.label));
                }
                Ok((
                    Built::Task(KairosTaskReq {
                        r#ref: r#ref.clone(),
                        title,
                        key: Some(cal),
                        kind,
                        estimate_min: est,
                        due: due.map(|d| d.iso),
                        suggestion: suggestion.clone(),
                    }),
                    p,
                ))
            }
            Purpose::Event { r#ref, suggestion } => {
                if title.is_empty() {
                    return Err("the event needs a title".into());
                }
                let start = self.when(cfg, now, "Start", true)?.ok_or("Start is needed")?;
                let end = self.when(cfg, now, "End", false)?;
                if let Some(e) = &end
                    && e.ts <= start.ts
                {
                    return Err("End is before the start".into());
                }
                let cal = self.value("Calendar").to_string();
                let loc = self.value("Location").to_string();
                let mut p = format!("{title} · {cal} · {}", start.label);
                if let Some(e) = &end {
                    let same_day = e.iso.get(..10) == start.iso.get(..10);
                    p.push_str(&format!(
                        "–{}",
                        if same_day {
                            e.label.rsplit(' ').next().unwrap_or(&e.label).to_string()
                        } else {
                            e.label.clone()
                        }
                    ));
                }
                if !loc.is_empty() {
                    p.push_str(&format!(" · {loc}"));
                }
                Ok((
                    Built::Event(KairosEventReq {
                        r#ref: r#ref.clone(),
                        title,
                        start: start.iso,
                        end: end.map(|e| e.iso),
                        location: Some(loc).filter(|l| !l.is_empty()),
                        key: Some(cal),
                        suggestion: suggestion.clone(),
                    }),
                    p,
                ))
            }
            Purpose::Free { .. } => {
                let from = self.when(cfg, now, "From", true)?.ok_or("From is needed")?;
                let to = self.when(cfg, now, "To", true)?.ok_or("To is needed")?;
                if to.ts <= from.ts {
                    return Err("To is before From".into());
                }
                let dur = self.minutes("Duration")?.ok_or("Duration is needed")?;
                if dur == 0 {
                    return Err("Duration must be over 0".into());
                }
                let count: usize = self
                    .value("How many")
                    .parse()
                    .ok()
                    .filter(|n| (1..=10).contains(n))
                    .ok_or("How many: 1 to 10")?;
                Ok((
                    Built::Free(FreeReq {
                        from: from.iso,
                        to: to.iso,
                        duration_min: dur,
                        count,
                    }),
                    format!(
                        "up to {count} slots of {} between {} and {}",
                        minutes_label(dur),
                        from.label,
                        to.label
                    ),
                ))
            }
        }
    }
}

/// An invite time for the form: London wall time, as typed.
pub fn invite_time(w: &keryx_core::invite::When, tz: Tz) -> String {
    match w {
        keryx_core::invite::When::Timed(d) => d.with_timezone(&tz).format("%Y-%m-%dT%H:%M").to_string(),
        keryx_core::invite::When::AllDay(d) => d.format("%Y-%m-%d").to_string(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use chrono::TimeZone;
    use chrono_tz::Europe::London;

    fn cfg() -> Config {
        Config::load(&std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config/config.test.toml"))
            .unwrap()
    }

    fn now() -> DateTime<Tz> {
        London.with_ymd_and_hms(2026, 10, 14, 15, 20, 0).unwrap()
    }

    fn typ(f: &mut Form, s: &str) {
        for c in s.chars() {
            f.key(Key::Char(c), now());
        }
    }

    #[test]
    fn task_form_preview_is_what_is_sent() {
        let c = cfg();
        let mut f = Form::task(&c, "oxford:9", "Lab partner", "Careers", None);
        f.focus = 2;
        f.key(Key::Right, now()); // none → sheet
        f.key(Key::Right, now()); // → coursework
        let e = f.build(&c, now()).unwrap_err();
        assert_eq!(e, "coursework needs an estimate");
        f.focus = 3;
        typ(&mut f, "1h30");
        f.focus = 4;
        typ(&mut f, "fri 17:00");
        let (b, p) = f.build(&c, now()).unwrap();
        assert_eq!(
            p,
            "Lab partner · Careers · coursework · 1 h 30 min · due Fri 16 Oct 17:00"
        );
        let Built::Task(t) = b else { panic!() };
        assert_eq!(t.due.as_deref(), Some("2026-10-16T17:00"));
        assert_eq!(t.estimate_min, Some(90));
        assert_eq!(t.key.as_deref(), Some("Careers"));
        f.fields[4].value = "wed".into();
        assert!(f.build(&c, now()).unwrap_err().contains("ambiguous"));
    }

    #[test]
    fn calendar_picker_jumps_by_letter() {
        let c = cfg();
        let mut f = Form::task(&c, "oxford:9", "x", "Nope", None);
        assert_eq!(f.fields[1].value, "Personal", "unknown calendar → Personal");
        f.focus = 1;
        f.key(Key::Char('e'), now());
        assert_eq!(f.fields[1].value, "Engineering");
    }

    #[test]
    fn event_form_from_an_invite() {
        let c = cfg();
        let f = Form::event(
            &c,
            "oxford:9",
            "Design review",
            "Personal",
            "2026-10-20T14:00",
            "2026-10-20T15:30",
            "LR6",
            None,
        );
        let (b, p) = f.build(&c, now()).unwrap();
        assert_eq!(p, "Design review · Personal · Tue 20 Oct 14:00–15:30 · LR6");
        let Built::Event(e) = b else { panic!() };
        assert_eq!(e.start, "2026-10-20T14:00");
        let mut f2 = f.clone();
        f2.fields[3].value = "2026-10-20T13:00".into();
        assert_eq!(f2.build(&c, now()).unwrap_err(), "End is before the start");
    }

    #[test]
    fn free_window_choices() {
        let c = cfg();
        let mut f = Form::free("oxford:9", now());
        assert_eq!(f.fields[1].value, "2026-10-14T16:00");
        assert_eq!(f.fields[2].value, "2026-10-19");
        f.key(Key::Right, now());
        assert_eq!(f.fields[0].value, "next week");
        assert_eq!(
            (f.fields[1].value.as_str(), f.fields[2].value.as_str()),
            ("2026-10-19", "2026-10-26")
        );
        let (b, p) = f.build(&c, now()).unwrap();
        assert_eq!(p, "up to 4 slots of 1 h between Mon 19 Oct and Mon 26 Oct");
        assert!(matches!(
            b,
            Built::Free(FreeReq {
                duration_min: 60,
                count: 4,
                ..
            })
        ));
        // Editing a date switches the window to "pick".
        f.focus = 2;
        f.key(Key::Backspace, now());
        assert_eq!(f.fields[0].value, "pick");
        f.fields[4].value = "40".into();
        assert!(f.build(&c, now()).is_err());
    }

    #[test]
    fn suggestion_reason_is_carried() {
        let c = cfg();
        let f = Form::task(
            &c,
            "oxford:9",
            "x",
            "Personal",
            Some(("sg-1".into(), "Jane asked".into())),
        );
        assert_eq!(f.reason.as_deref(), Some("Jane asked"));
        let Built::Task(t) = f.build(&c, now()).unwrap().0 else {
            panic!()
        };
        assert_eq!(t.suggestion.as_deref(), Some("sg-1"));
    }
}
