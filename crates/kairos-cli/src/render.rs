//! Terminal rendering (Catppuccin Mocha, truecolor) and the stable JSON schema.

use chrono::{DateTime, Duration, NaiveDate, Utc};
use chrono_tz::Tz;
use kairos_core::colour::Rgb;
use kairos_core::config::{Config, Overlay};
use kairos_core::dedup::Cluster;
use kairos_core::model::{EventKind, Instance, Span};
use serde::Serialize;
use std::io::IsTerminal;

pub const JSON_SCHEMA: u32 = 1;
pub const CAL_ICON: &str = "\u{f00f0}";
pub const FLAG: &str = "⚑";

pub struct Paint {
    pub on: bool,
}

impl Paint {
    pub fn auto() -> Paint {
        Paint {
            on: std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
        }
    }
    pub fn fg(&self, c: Rgb, s: &str) -> String {
        if self.on {
            format!("\x1b[38;2;{};{};{}m{s}\x1b[0m", c.0, c.1, c.2)
        } else {
            s.to_string()
        }
    }
    pub fn bold(&self, c: Rgb, s: &str) -> String {
        if self.on {
            format!("\x1b[1;38;2;{};{};{}m{s}\x1b[0m", c.0, c.1, c.2)
        } else {
            s.to_string()
        }
    }
    pub fn dim(&self, s: &str) -> String {
        self.fg(Rgb::mocha("overlay1"), s)
    }
}

/// Colour for an instance: key colour, Family colour, or layer colours.
pub fn colour_of(cfg: &Config, i: &Instance) -> Rgb {
    match i.overlay {
        Overlay::Background => cfg.overlays.background_colour,
        Overlay::Imported => cfg.overlays.imported_colour,
        Overlay::Family if i.meta.origin_key.is_none() => cfg.overlays.family_colour,
        _ => i
            .colour_key()
            .and_then(|k| cfg.key(k))
            .map_or(Rgb::mocha("text"), |k| k.colour),
    }
}

pub fn time_range(i: &Instance, tz: Tz) -> String {
    match i.span {
        Span::AllDay { .. } => "all day    ".into(),
        Span::Timed { start, end } if start == end => format!("{}      ", start.with_timezone(&tz).format("%H:%M")),
        Span::Timed { start, end } => {
            let s = start.with_timezone(&tz);
            let e = end.with_timezone(&tz);
            if s.date_naive() == e.date_naive() || e.format("%H:%M").to_string() == "00:00" {
                format!("{}–{}", s.format("%H:%M"), e.format("%H:%M"))
            } else {
                format!("{}–{} {}", s.format("%H:%M"), e.format("%H:%M"), e.format("%a"))
            }
        }
    }
}

pub fn title_of(i: &Instance) -> String {
    let t = i.title.trim();
    match i.meta.kind {
        EventKind::Deadline => format!("{FLAG} {}", t.trim_start_matches(FLAG).trim()),
        // Work: ☐ to do, ✓ done (the line's marker says who placed it).
        EventKind::TaskBlock if t.starts_with('✓') => t.to_string(),
        EventKind::TaskBlock => kairos_core::build::todo_block_title(t),
        EventKind::Event => t.to_string(),
        EventKind::Proposal => format!("◌ {t}?"),
    }
}

/// One line per event for `today` / `week`.
pub fn line(cfg: &Config, p: &Paint, c: &Cluster, tz: Tz) -> String {
    let i = &c.primary;
    let col = colour_of(cfg, i);
    let dimmed = matches!(i.overlay, Overlay::Imported | Overlay::Background) || i.tentative;
    let mut title = title_of(i);
    if c.is_merged() {
        title.push_str(" ⧉");
    }
    let title = if dimmed {
        p.dim(&title)
    } else if i.meta.kind == EventKind::Deadline {
        p.bold(Rgb::mocha("red"), &title)
    } else {
        title
    };
    // `┆` = a scheduler slot (a work block kairos may move), as in the TUI.
    // As in the TUI: ┆ kairos may move it, ╏ Alec placed it, ● events / done work.
    let mark = match i.meta.kind {
        EventKind::TaskBlock if i.title.starts_with('✓') => "●",
        EventKind::TaskBlock if i.meta.pinned => "╏",
        EventKind::TaskBlock => "┆",
        _ => "●",
    };
    let mut s = format!(" {} {} {}", p.dim(&time_range(i, tz)), p.fg(col, mark), title);
    let mut extra = Vec::new();
    if let Some(l) = &i.location {
        extra.push(l.clone());
    }
    if let Some(k) = i.colour_key() {
        extra.push(k.to_string());
    } else if i.overlay == Overlay::Family {
        extra.push("Family".into());
    }
    if !extra.is_empty() {
        s.push_str(&p.dim(&format!("  · {}", extra.join(" · "))));
    }
    s
}

pub fn day_header(p: &Paint, d: NaiveDate, today: NaiveDate) -> String {
    let label = if d == today {
        format!("{} · today", d.format("%a %-d %b"))
    } else if d == today + Duration::days(1) {
        format!("{} · tomorrow", d.format("%a %-d %b"))
    } else {
        d.format("%a %-d %b").to_string()
    };
    p.bold(Rgb::mocha("blue"), &label)
}

pub fn humanise_until(now: DateTime<Utc>, t: DateTime<Utc>) -> String {
    let m = (t - now).num_minutes();
    if m <= 0 {
        "now".into()
    } else if m < 60 {
        format!("in {m}m")
    } else if m < 24 * 60 {
        let h = m / 60;
        let r = m % 60;
        if r >= 15 && h < 3 {
            format!("in {h}h{r:02}")
        } else {
            format!("in {}h", (m + 30) / 60)
        }
    } else {
        format!("in {}d", (m + 720) / 1440)
    }
}

// ---------------------------------------------------------------- JSON (docs/json.md)

#[derive(Serialize)]
pub struct JsonDup {
    pub uid: String,
    pub source: String,
    pub overlay: Overlay,
}

#[derive(Serialize)]
pub struct JsonEvent {
    pub uid: String,
    pub recurrence_id: Option<String>,
    pub title: String,
    pub start: String,
    pub end: String,
    pub all_day: bool,
    pub date: NaiveDate,
    pub overlay: Overlay,
    pub key: Option<String>,
    pub source: String,
    pub location: Option<String>,
    pub kind: EventKind,
    pub flex: bool,
    pub task_id: Option<String>,
    pub created_by: Option<String>,
    pub recurring: bool,
    pub tentative: bool,
    pub transparent: bool,
    pub colour: String,
    pub duplicates: Vec<JsonDup>,
}

#[derive(Serialize)]
pub struct JsonList {
    pub schema: u32,
    pub timezone: String,
    pub from: String,
    pub to: String,
    pub events: Vec<JsonEvent>,
}

pub fn json_event(cfg: &Config, c: &Cluster, tz: Tz) -> JsonEvent {
    let i = &c.primary;
    let (start, end, date) = match i.span {
        Span::AllDay { start, end } => (start.to_string(), end.to_string(), start),
        Span::Timed { start, end } => (
            start.with_timezone(&tz).to_rfc3339(),
            end.with_timezone(&tz).to_rfc3339(),
            start.with_timezone(&tz).date_naive(),
        ),
    };
    JsonEvent {
        uid: i.uid.clone(),
        recurrence_id: i.recurrence_id.clone(),
        title: i.title.clone(),
        start,
        end,
        all_day: i.span.is_all_day(),
        date,
        overlay: i.overlay,
        key: i.colour_key().map(String::from),
        source: i.source.clone(),
        location: i.location.clone(),
        kind: i.meta.kind,
        flex: i.meta.flex,
        task_id: i.meta.task_id.clone(),
        created_by: i.meta.created_by.map(|a| a.as_str().to_string()),
        recurring: i.recurring,
        tentative: i.tentative,
        transparent: i.transparent,
        colour: colour_of(cfg, i).hex(),
        duplicates: c
            .others
            .iter()
            .map(|o| JsonDup {
                uid: o.uid.clone(),
                source: o.source.clone(),
                overlay: o.overlay,
            })
            .collect(),
    }
}
