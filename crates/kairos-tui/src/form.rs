//! The add / edit form: one field per line, Tab between fields, a live
//! preview of how the time was understood, Enter to save.

use crate::app::Key;
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use kairos_core::model::{Instance, Span};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormKind {
    AddEvent,
    EditEvent {
        source: String,
        uid: String,
    },
    AddTask,
    EditTask {
        id: String,
    },
    /// Check-in: the block took longer than planned.
    TookLonger {
        t: crate::app::CheckinTarget,
    },
    AddProject,
    EditProject {
        id: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    When,
    Duration,
    /// Picks one of the configured keys (←/→ or first letter).
    Key,
    /// Picks a task type (←/→ or first letter).
    Type,
    /// Picks how an event repeats (←/→).
    Repeat,
    /// Optional time (task due).
    OptWhen,
    /// Picks a project status (←/→ or first letter).
    Status,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub label: &'static str,
    pub value: String,
    pub kind: FieldKind,
    pub hint: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form {
    pub kind: FormKind,
    pub fields: Vec<Field>,
    pub focus: usize,
    /// Values when the form opened (edits only send what changed).
    pub original: Vec<String>,
    /// Key names in index order, for the picker.
    pub keys: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormAction {
    Continue,
    Submit,
    Cancel,
}

pub const WHEN_HINT: &str = "tue 19:00 · 23 oct 2pm · tomorrow 9am · a date alone = all day";
const DUR_HINT: &str = "60m · 1h30 · 2h (empty = 1h)";
const KEY_HINT: &str = "←/→ or type the first letter";
const STATUS_HINT: &str = "active (now) · next (soon) · paused · done — ←/→";
const TITLE_HINT: &str = "or all at once: Squash @ tue 19:00 #Sport for 90m";

fn field(label: &'static str, value: impl Into<String>, kind: FieldKind, hint: &'static str) -> Field {
    Field {
        label,
        value: value.into(),
        kind,
        hint,
    }
}

impl Form {
    fn new(kind: FormKind, fields: Vec<Field>, keys: Vec<String>) -> Form {
        let original = fields.iter().map(|f| f.value.clone()).collect();
        Form {
            kind,
            fields,
            focus: 0,
            original,
            keys,
        }
    }

    pub fn add_event(keys: Vec<String>, default_key: &str, when: String) -> Form {
        Form::new(
            FormKind::AddEvent,
            vec![
                field("Title", "", FieldKind::Text, TITLE_HINT),
                field("When", when, FieldKind::When, WHEN_HINT),
                field("Duration", "", FieldKind::Duration, DUR_HINT),
                field("Calendar", default_key, FieldKind::Key, KEY_HINT),
                field("Location", "", FieldKind::Text, "optional"),
                field("Notes", "", FieldKind::Text, "optional"),
                field(
                    "Repeat",
                    "no",
                    FieldKind::Repeat,
                    "no · weekly · weekly in term (each term in constraints.toml) · daily — ←/→",
                ),
            ],
            keys,
        )
    }

    pub fn edit_event(keys: Vec<String>, i: &Instance, tz: Tz) -> Form {
        let (when, dur) = match i.span {
            Span::AllDay { start, .. } => (start.format("%Y-%m-%d").to_string(), String::new()),
            Span::Timed { start, end } => (
                start.with_timezone(&tz).format("%Y-%m-%d %H:%M").to_string(),
                fmt_minutes((end - start).num_minutes()),
            ),
        };
        Form::new(
            FormKind::EditEvent {
                source: i.source.clone(),
                uid: i.uid.clone(),
            },
            vec![
                field("Title", i.title.clone(), FieldKind::Text, ""),
                field("When", when, FieldKind::When, WHEN_HINT),
                field("Duration", dur, FieldKind::Duration, DUR_HINT),
                field("Calendar", i.key.clone().unwrap_or_default(), FieldKind::Key, KEY_HINT),
                field(
                    "Location",
                    i.location.clone().unwrap_or_default(),
                    FieldKind::Text,
                    "empty = none",
                ),
                field(
                    "Notes",
                    i.description.clone().unwrap_or_default(),
                    FieldKind::Text,
                    "empty = none",
                ),
            ],
            keys,
        )
    }

    pub fn edit_task(keys: Vec<String>, t: &kairos_core::task::Task, tz: Tz) -> Form {
        let due = match t.due {
            None => String::new(),
            Some(kairos_core::task::Due::Date(d)) => d.format("%Y-%m-%d").to_string(),
            Some(kairos_core::task::Due::At(a)) => a.with_timezone(&tz).format("%Y-%m-%d %H:%M").to_string(),
        };
        Form::new(
            FormKind::EditTask { id: t.id.clone() },
            vec![
                field("Title", t.title.clone(), FieldKind::Text, ""),
                field("Calendar", t.key.clone(), FieldKind::Key, KEY_HINT),
                field(
                    "Type",
                    t.kind.clone().unwrap_or_else(|| "sheet".into()),
                    FieldKind::Type,
                    "sheet · coursework · admin — ←/→ (resets block sizes)",
                ),
                field(
                    "Left",
                    fmt_minutes(i64::from(t.remaining_min)),
                    FieldKind::Duration,
                    "work still to do: 3h · 90m",
                ),
                field(
                    "Due",
                    due,
                    FieldKind::OptWhen,
                    "fri 17:00 · 23 oct · empty = no deadline",
                ),
            ],
            keys,
        )
    }

    pub fn add_task(keys: Vec<String>, default_key: &str) -> Form {
        Form::new(
            FormKind::AddTask,
            vec![
                field("Title", "", FieldKind::Text, "what needs doing"),
                field("Calendar", default_key, FieldKind::Key, KEY_HINT),
                field(
                    "Type",
                    "sheet",
                    FieldKind::Type,
                    "sheet (2–3 h blocks) · coursework · admin (one go) — ←/→",
                ),
                field(
                    "Estimate",
                    "",
                    FieldKind::Duration,
                    "how long in total: 4h · 90m · empty = the type's default",
                ),
                field(
                    "Due",
                    "",
                    FieldKind::OptWhen,
                    "fri 17:00 · 23 oct · empty = no deadline",
                ),
            ],
            keys,
        )
    }

    pub fn title(&self) -> &'static str {
        match self.kind {
            FormKind::AddEvent => "new event",
            FormKind::EditEvent { .. } => "edit event",
            FormKind::AddTask => "new task",
            FormKind::EditTask { .. } => "edit task",
            FormKind::TookLonger { .. } => "took longer",
            FormKind::AddProject => "new project",
            FormKind::EditProject { .. } => "edit project",
        }
    }

    pub fn add_project() -> Form {
        Form::new(
            FormKind::AddProject,
            vec![
                field("Name", "", FieldKind::Text, "what it's called: kairos · email TUI"),
                field("Status", "active", FieldKind::Status, STATUS_HINT),
                field("Note", "", FieldKind::Text, "optional: what it is, or the next step"),
            ],
            Vec::new(),
        )
    }

    pub fn edit_project(p: &kairos_core::project::Project) -> Form {
        Form::new(
            FormKind::EditProject { id: p.id.clone() },
            vec![
                field("Name", p.name.clone(), FieldKind::Text, ""),
                field("Status", p.status.as_str(), FieldKind::Status, STATUS_HINT),
                field("Note", p.note.clone(), FieldKind::Text, "empty = none"),
            ],
            Vec::new(),
        )
    }

    /// "Took longer": time spent, how much more, plan it from when.
    pub fn took_longer(t: crate::app::CheckinTarget) -> Form {
        let spent = fmt_minutes(i64::from(t.planned_min + 60));
        let more = fmt_minutes(i64::from(t.remaining_min.saturating_sub(t.planned_min).max(60)));
        Form::new(
            FormKind::TookLonger { t },
            vec![
                field(
                    "Spent",
                    spent,
                    FieldKind::Duration,
                    "time actually spent on it: 3h · 2h30",
                ),
                field(
                    "Needs",
                    more,
                    FieldKind::Duration,
                    "how much more you think it needs (0 = finished)",
                ),
                field(
                    "From",
                    "",
                    FieldKind::OptWhen,
                    "plan it no earlier than: sat · 20 oct · empty = as soon as there's time",
                ),
            ],
            Vec::new(),
        )
    }

    pub fn get(&self, label: &str) -> &str {
        self.fields
            .iter()
            .find(|f| f.label == label)
            .map_or("", |f| f.value.trim())
    }

    pub fn changed(&self, label: &str) -> bool {
        self.fields
            .iter()
            .zip(&self.original)
            .any(|(f, o)| f.label == label && f.value.trim() != o.trim())
    }

    pub fn focus_on(&mut self, label: &str) {
        if let Some(k) = self.fields.iter().position(|f| f.label == label) {
            self.focus = k;
        }
    }

    fn set(&mut self, label: &str, v: String) {
        if let Some(f) = self.fields.iter_mut().find(|f| f.label == label) {
            f.value = v;
        }
    }

    /// `Squash @ tue 19:00 #Sport for 90m` typed into Title fills the other fields.
    pub fn expand_quick(&mut self) -> bool {
        if self.kind != FormKind::AddEvent {
            return false;
        }
        let Some((title, when, key, dur)) = crate::app::parse_add(self.get("Title")) else {
            return false;
        };
        self.set("Title", title);
        self.set("When", when);
        if let Some(d) = dur {
            self.set("Duration", fmt_minutes(i64::from(d)));
        }
        if let Some(k) = key
            && let Some(name) = self.keys.iter().find(|n| n.eq_ignore_ascii_case(&k))
        {
            let name = name.clone();
            self.set("Calendar", name);
        }
        true
    }

    /// The choices of the focused picker field.
    fn options(&self) -> Vec<String> {
        match self.fields[self.focus].kind {
            FieldKind::Type => TASK_TYPES.iter().map(|s| s.to_string()).collect(),
            FieldKind::Repeat => REPEATS.iter().map(|s| s.to_string()).collect(),
            FieldKind::Status => kairos_core::project::ProjectStatus::ALL
                .iter()
                .map(|s| s.as_str().to_string())
                .collect(),
            _ => self.keys.clone(),
        }
    }

    fn cycle_key(&mut self, step: isize) {
        let opts = self.options();
        if opts.is_empty() {
            return;
        }
        let f = &mut self.fields[self.focus];
        let n = opts.len() as isize;
        let cur = opts
            .iter()
            .position(|k| k.eq_ignore_ascii_case(&f.value))
            .map_or(if step > 0 { -1 } else { 0 }, |p| p as isize);
        f.value = opts[(cur + step).rem_euclid(n) as usize].clone();
    }

    fn jump_key(&mut self, c: char) {
        let opts = self.options();
        let c = c.to_ascii_lowercase();
        let f = &self.fields[self.focus];
        let start = opts
            .iter()
            .position(|k| k.eq_ignore_ascii_case(&f.value))
            .map_or(0, |p| p + 1);
        let n = opts.len();
        if let Some(k) = (0..n)
            .map(|o| &opts[(start + o) % n])
            .find(|k| k.to_ascii_lowercase().starts_with(c))
        {
            self.fields[self.focus].value = k.clone();
        }
    }

    pub fn key(&mut self, k: Key) -> FormAction {
        let n = self.fields.len();
        let kind = self.fields[self.focus].kind;
        match k {
            Key::Esc => return FormAction::Cancel,
            Key::Enter => {
                if self.focus == 0 {
                    self.expand_quick();
                }
                return FormAction::Submit;
            }
            Key::Tab | Key::Down => {
                if self.focus == 0 {
                    self.expand_quick();
                }
                self.focus = (self.focus + 1) % n;
            }
            Key::BackTab | Key::Up => self.focus = (self.focus + n - 1) % n,
            Key::Left if kind.is_picker() => self.cycle_key(-1),
            Key::Right if kind.is_picker() => self.cycle_key(1),
            Key::Char(c) if kind.is_picker() => self.jump_key(c),
            Key::Char(c) => self.fields[self.focus].value.push(c),
            Key::Backspace if !kind.is_picker() => {
                self.fields[self.focus].value.pop();
            }
            _ => {}
        }
        FormAction::Continue
    }
}

/// Task types the form offers (the built-in ones in constraints.toml).
pub const TASK_TYPES: [&str; 3] = ["sheet", "coursework", "admin"];

/// How a new event can repeat.
pub const REPEATS: [&str; 4] = ["no", "weekly", "weekly in term", "daily"];

/// The API repeat for a Repeat field value.
pub fn repeat_of(v: &str) -> Option<kairos_core::api::Repeat> {
    let (every, term_only) = match v {
        "weekly" => ("week", false),
        "weekly in term" => ("week", true),
        "daily" => ("day", false),
        _ => return None,
    };
    Some(kairos_core::api::Repeat {
        every: Some(every.into()),
        term_only,
        until: None,
    })
}

impl FieldKind {
    pub fn is_picker(self) -> bool {
        matches!(
            self,
            FieldKind::Key | FieldKind::Type | FieldKind::Repeat | FieldKind::Status
        )
    }
}

pub fn fmt_minutes(m: i64) -> String {
    match (m / 60, m % 60) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h{m:02}"),
    }
}

/// "today ", "tomorrow ", "23 oct " … for prefilling When from the cursor day.
pub fn when_prefill(day: chrono::NaiveDate, today: chrono::NaiveDate) -> String {
    if day == today {
        "today ".into()
    } else if day == today.succ_opt().unwrap_or(today) {
        "tomorrow ".into()
    } else if day > today {
        day.format("%-d %b ").to_string().to_lowercase()
    } else {
        day.format("%Y-%m-%d ").to_string()
    }
}

/// Human description of a parsed start/end.
pub fn describe_span(start: DateTime<Utc>, end: DateTime<Utc>, tz: Tz) -> String {
    let s = start.with_timezone(&tz);
    let e = end.with_timezone(&tz);
    if s.date_naive() == e.date_naive() {
        format!("{}–{}", s.format("%a %-d %b %H:%M"), e.format("%H:%M"))
    } else {
        format!("{} – {}", s.format("%a %-d %b %H:%M"), e.format("%a %-d %b %H:%M"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> Vec<String> {
        ["Inbox", "Engineering", "Careers", "Sport", "Social"]
            .map(String::from)
            .to_vec()
    }

    #[test]
    fn typing_tab_and_key_picker() {
        let mut f = Form::add_event(keys(), "Inbox", "today ".into());
        for c in "Squash".chars() {
            f.key(Key::Char(c));
        }
        f.key(Key::Tab);
        for c in "19:00".chars() {
            f.key(Key::Char(c));
        }
        assert_eq!(f.get("When"), "today 19:00");
        f.key(Key::Tab);
        f.key(Key::Tab);
        assert_eq!(f.fields[f.focus].label, "Calendar");
        f.key(Key::Char('s'));
        assert_eq!(f.get("Calendar"), "Sport");
        f.key(Key::Char('s'));
        assert_eq!(f.get("Calendar"), "Social", "same letter cycles");
        f.key(Key::Right);
        assert_eq!(f.get("Calendar"), "Inbox", "wraps");
        f.key(Key::Left);
        assert_eq!(f.get("Calendar"), "Social");
        f.key(Key::BackTab);
        assert_eq!(f.fields[f.focus].label, "Duration");
        assert_eq!(f.key(Key::Enter), FormAction::Submit);
        assert_eq!(f.key(Key::Esc), FormAction::Cancel);
    }

    #[test]
    fn quick_syntax_in_title_fills_fields() {
        let mut f = Form::add_event(keys(), "Inbox", "today ".into());
        for c in "Squash @ tue 19:00 #sport for 90m".chars() {
            f.key(Key::Char(c));
        }
        f.key(Key::Tab);
        assert_eq!(f.get("Title"), "Squash");
        assert_eq!(f.get("When"), "tue 19:00");
        assert_eq!(f.get("Duration"), "1h30");
        assert_eq!(f.get("Calendar"), "Sport");
    }

    #[test]
    fn prefill_reads_naturally() {
        let t = chrono::NaiveDate::from_ymd_opt(2026, 10, 11).unwrap_or_default();
        assert_eq!(when_prefill(t, t), "today ");
        assert_eq!(when_prefill(t + chrono::Duration::days(1), t), "tomorrow ");
        assert_eq!(when_prefill(t + chrono::Duration::days(12), t), "23 oct ");
        assert_eq!(when_prefill(t - chrono::Duration::days(1), t), "2026-10-10 ");
        assert_eq!(fmt_minutes(90), "1h30");
        assert_eq!(fmt_minutes(60), "1h");
        assert_eq!(fmt_minutes(45), "45m");
    }
}
