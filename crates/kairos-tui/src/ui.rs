//! Drawing. Theme: Catppuccin Mocha, transparent background (Color::Reset),
//! rounded borders, blue focus / mauve popups, ` · ` separators, `…` truncation.

use crate::app::{App, Confirm, Mode, PromptKind, View, is_deadline, is_dimmed, overlay_flags};
use crate::form::{FieldKind, Form};
use chrono::{DateTime, Datelike, Duration, NaiveDate, Timelike, Utc};
use kairos_core::colour::Rgb;
use kairos_core::config::Overlay;
use kairos_core::dedup::Cluster;
use kairos_core::model::{EventKind, Instance, Span};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span as TSpan};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub fn c(name: &str) -> Color {
    rgb(Rgb::mocha(name))
}
pub fn rgb(r: Rgb) -> Color {
    Color::Rgb(r.0, r.1, r.2)
}

/// Blend a colour towards the Mocha base (0 = unchanged, 1 = base).
pub fn fade(col: Color, t: f32) -> Color {
    let Rgb(br, bg, bb) = Rgb::mocha("base");
    match col {
        Color::Rgb(r, g, b) => {
            let m = |a: u8, z: u8| (f32::from(a) + (f32::from(z) - f32::from(a)) * t).round() as u8;
            Color::Rgb(m(r, br), m(g, bg), m(b, bb))
        }
        c => c,
    }
}

/// How much past days and not-attending Family events recede.
const PAST_FADE: f32 = 0.35;
const FAMILY_FADE: f32 = 0.45;

fn colour_of(app: &App, i: &Instance) -> Color {
    if i.meta.kind == EventKind::Proposal {
        return c("overlay1");
    }
    let r = match i.overlay {
        Overlay::Background => app.cfg.overlays.background_colour,
        Overlay::Imported => app.cfg.overlays.imported_colour,
        // Family events you're attending (blocking, TUI `B`) stay bright.
        Overlay::Family if i.meta.origin_key.is_none() && !i.meta.blocking => {
            return fade(rgb(app.cfg.overlays.family_colour), FAMILY_FADE);
        }
        Overlay::Family if i.meta.origin_key.is_none() => app.cfg.overlays.family_colour,
        _ => i
            .colour_key()
            .and_then(|k| app.cfg.key(k))
            .map_or(Rgb::mocha("text"), |k| k.colour),
    };
    rgb(r)
}

/// Truncate to `w` columns with `…`.
pub fn fit(s: &str, w: usize) -> String {
    if s.width() <= w {
        return s.to_string();
    }
    if w == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in s.chars() {
        let cw = ch.width().unwrap_or(0);
        if used + cw + 1 > w {
            break;
        }
        out.push(ch);
        used += cw;
    }
    out.push('…');
    out
}

fn put(buf: &mut Buffer, x: u16, y: u16, w: u16, s: &str, style: Style) {
    if w == 0 {
        return;
    }
    let s = fit(s, w as usize);
    buf.set_stringn(x, y, &s, w as usize, style);
}

fn panel(title: &str, focus: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if focus { c("blue") } else { c("overlay0") }))
        .title(
            Line::from(TSpan::styled(
                format!(" {title} "),
                Style::default().fg(c("blue")).add_modifier(Modifier::BOLD),
            ))
            .alignment(Alignment::Center),
        )
}

fn popup_block(title: &str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(c("mauve")))
        .title(
            Line::from(TSpan::styled(
                format!(" {title} "),
                Style::default().fg(c("mauve")).add_modifier(Modifier::BOLD),
            ))
            .alignment(Alignment::Center),
        )
}

fn centred(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h)
}

fn label(i: &Instance) -> String {
    let t = i.title.trim();
    match i.meta.kind {
        EventKind::Deadline => format!("⚑ {}", t.trim_start_matches('⚑').trim()),
        EventKind::TaskBlock => format!("~ {t}"),
        EventKind::Event => t.to_string(),
        EventKind::Proposal => format!("◌ {t}?"),
    }
}

/// Title without the parts that matter least when space is short:
/// bracketed asides and `#n` counters ("B10 Basic Soil Mechanics #1 (Knappett) LR2"
/// → "B10 Basic Soil Mechanics LR2").
pub fn compact(title: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    for ch in title.chars() {
        match ch {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out.split_whitespace()
        .filter(|w| !(w.starts_with('#') && w[1..].chars().all(|c| c.is_ascii_digit()) && w.len() > 1))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Word-wrap into at most `rows` lines of width `w`; the last line ends in `…` if cut.
pub fn wrap_lines(s: &str, w: usize, rows: usize) -> Vec<String> {
    if w == 0 || rows == 0 {
        return Vec::new();
    }
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in s.split_whitespace() {
        let candidate = if cur.is_empty() {
            word.to_string()
        } else {
            format!("{cur} {word}")
        };
        if candidate.width() <= w {
            cur = candidate;
            continue;
        }
        if !cur.is_empty() {
            lines.push(std::mem::take(&mut cur));
        }
        // A word longer than the line is truncated, not split mid-word.
        cur = fit(word, w);
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.len() > rows {
        let rest = lines[rows - 1..].join(" ");
        lines.truncate(rows - 1);
        lines.push(fit(&rest, w));
    }
    lines
}

/// Where an event sits relative to now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Past,
    Now,
    /// Starts within `tui.soon_minutes`; minutes until it starts.
    Soon(i64),
    Later,
}

/// Family events you're not attending (TUI `B`) never get now/soon emphasis.
fn attending(i: &Instance) -> bool {
    i.meta.kind != EventKind::Proposal
        && (i.overlay != Overlay::Family || i.meta.origin_key.is_some() || i.meta.blocking)
}

pub fn phase(app: &App, i: &Instance) -> Phase {
    let (s, e) = i.span.bounds(app.tz);
    let now = app.now;
    if e <= now && !(s == e && s > now) {
        return Phase::Past;
    }
    if s <= now {
        return Phase::Now;
    }
    let mins = (s - now).num_minutes();
    if mins <= i64::from(app.cfg.tui.soon_minutes) {
        Phase::Soon(mins.max(1))
    } else {
        Phase::Later
    }
}

fn in_text(mins: i64) -> String {
    format!("in {}", crate::form::fmt_minutes(mins))
}

/// Like [`wrap_lines`], but the first line is only `w0` wide (room for a tag).
pub fn wrap_first_narrow(s: &str, w0: usize, w: usize, rows: usize) -> Vec<String> {
    let mut first = String::new();
    let mut rest = Vec::new();
    for word in s.split_whitespace() {
        let cand = if first.is_empty() {
            word.to_string()
        } else {
            format!("{first} {word}")
        };
        if rest.is_empty() && cand.width() <= w0 {
            first = cand;
        } else {
            rest.push(word);
        }
    }
    if first.is_empty() && !rest.is_empty() {
        first = fit(rest.remove(0), w0);
    }
    let mut out = vec![first];
    out.extend(wrap_lines(&rest.join(" "), w, rows.saturating_sub(1)));
    out
}

/// One line describing an event, for the week view's bottom edge.
fn peek(app: &App, cl: &Cluster) -> String {
    let i = &cl.primary;
    let tz = app.tz;
    let when = match i.span {
        Span::AllDay { .. } => "all day".to_string(),
        Span::Timed { start, end } if start == end => start.with_timezone(&tz).format("%H:%M").to_string(),
        Span::Timed { start, end } => format!(
            "{}–{}",
            start.with_timezone(&tz).format("%H:%M"),
            end.with_timezone(&tz).format("%H:%M")
        ),
    };
    let mut parts = vec![when, label(i)];
    if i.meta.kind == EventKind::Proposal {
        parts.push("proposed project time: y accept · d drop".into());
        return parts.join(" · ");
    }
    if let Some(l) = &i.location {
        parts.push(l.clone());
    }
    if let Some(r) = app.squeeze_of(i) {
        parts.push(format!("⚠ squeezed {}", r.join(", ")));
    }
    match (i.colour_key(), i.overlay) {
        (Some(k), _) => parts.push(k.to_string()),
        (None, Overlay::Family) => parts.push("Family".into()),
        (None, o) => parts.push(o.as_str().to_string()),
    }
    if cl.is_merged() {
        parts.push(format!("⧉ {}", cl.others.len() + 1));
    }
    match phase(app, i) {
        Phase::Now if !i.span.is_all_day() => {
            parts.push(format!("now, until {}", i.end(tz).with_timezone(&tz).format("%H:%M")))
        }
        Phase::Soon(m) => parts.push(in_text(m)),
        _ => {}
    }
    parts.join(" · ")
}

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    if area.height < 4 || area.width < 20 {
        return;
    }
    let main = Rect::new(area.x, area.y, area.width, area.height - 1);
    let status = Rect::new(area.x, area.y + area.height - 1, area.width, 1);
    match app.view {
        View::Week => week(f, app, main, false),
        View::Day => week(f, app, main, true),
        View::Agenda => agenda(f, app, main),
        View::Tasks => tasks(f, app, main),
        View::Approvals => approvals(f, app, main),
    }
    if app.expanded
        && matches!(app.view, View::Week | View::Day | View::Agenda)
        && let Some(cl) = app.selected()
    {
        details(f, app, main, &cl);
    }
    match &app.mode {
        Mode::Help => help(f, main),
        Mode::Checkin(i) => checkin(f, app, main, *i),
        Mode::Form(form) => form_popup(f, app, main, form),
        _ => {}
    }
    status_bar(f, app, status);
}

// ---------------------------------------------------------------- week / day grid

struct Placed<'a> {
    cl: &'a Cluster,
    lane: usize,
    lanes: usize,
    r0: usize,
    r1: usize,
}

fn lanes<'a>(items: &'a [(usize, usize, &'a Cluster)]) -> Vec<Placed<'a>> {
    // Greedy interval colouring, then each group's lane count.
    let mut ends: Vec<usize> = Vec::new();
    let mut out: Vec<Placed<'a>> = Vec::new();
    for &(r0, r1, cl) in items {
        let lane = match ends.iter().position(|&e| e <= r0) {
            Some(l) => {
                ends[l] = r1;
                l
            }
            None => {
                ends.push(r1);
                ends.len() - 1
            }
        };
        out.push(Placed {
            cl,
            lane,
            lanes: 1,
            r0,
            r1,
        });
    }
    for i in 0..out.len() {
        let n = out
            .iter()
            .filter(|o| o.r0 < out[i].r1 && out[i].r0 < o.r1)
            .map(|o| o.lane + 1)
            .max()
            .unwrap_or(1);
        out[i].lanes = n;
    }
    out
}

fn week(f: &mut Frame, app: &App, area: Rect, single: bool) {
    let tz = app.tz;
    // A selected work block lights up its task's deadline(s) (red), and a
    // selected deadline its task's work blocks (blue, like a selected block).
    let sel = app.selected().map(|c| c.primary);
    let linked_task = sel.as_ref().and_then(|i| i.meta.task_id.clone());
    let sel_kind = sel.as_ref().map(|i| i.meta.kind);
    let linked = |i: &Instance| {
        let pair = matches!(
            (sel_kind, i.meta.kind),
            (Some(EventKind::TaskBlock), EventKind::Deadline) | (Some(EventKind::Deadline), EventKind::TaskBlock)
        );
        pair && i.meta.task_id.is_some() && i.meta.task_id == linked_task
    };
    let inner_w = area.width.saturating_sub(2);
    let gutter: u16 = 6;
    let ndays: usize = if single {
        1
    } else if app.work_week && inner_w >= gutter + 5 * 13 {
        5
    } else if inner_w >= gutter + 7 * 13 {
        7
    } else if inner_w >= gutter + 3 * 12 {
        3
    } else {
        1
    };
    let monday = app.cursor - Duration::days(i64::from(app.cursor.weekday().num_days_from_monday()));
    let first = match ndays {
        7 => monday,
        // Mon–Fri; on a weekend show the coming week.
        5 if app.cursor.weekday().num_days_from_monday() >= 5 => monday + Duration::days(7),
        5 => monday,
        3 => app.cursor - Duration::days(1),
        _ => app.cursor,
    };
    let last = first + Duration::days(ndays as i64 - 1);
    let mut title = if single {
        app.cursor.format("%A %-d %B").to_string()
    } else {
        format!(
            "Week {} · {}–{}",
            first.iso_week().week(),
            first.format("%-d %b"),
            last.format("%-d %b")
        )
    };
    // Oxford week (Sun–Sat) of the selected day.
    if let Some(w) = app.cfg.tui.term_week(app.cursor) {
        title.push_str(&format!(" · {w}"));
    }
    let mut block = panel(&title, true);
    if let Some(cl) = app.selected() {
        let text = fit(&peek(app, &cl), area.width.saturating_sub(6) as usize);
        block = block.title_bottom(Line::from(vec![
            TSpan::styled(" ", Style::default()),
            TSpan::styled("● ", Style::default().fg(colour_of(app, &cl.primary))),
            TSpan::styled(format!("{text} "), Style::default().fg(c("text"))),
        ]));
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height < 4 || inner.width <= gutter {
        return;
    }
    let buf = f.buffer_mut();
    // Column k spans [col_x(k), col_x(k+1)): the remainder is spread, not dumped on the last day.
    let avail = (inner.width - gutter) as usize;
    let col_x = |k: usize| inner.x + gutter + (avail * k / ndays) as u16;
    let col_w_of = |k: usize| (col_x(k + 1) - col_x(k)).max(1);
    let days: Vec<NaiveDate> = (0..ndays).map(|k| first + Duration::days(k as i64)).collect();
    let per_day: Vec<Vec<Cluster>> = days.iter().map(|d| app.day_clusters(*d)).collect();
    let ghosts = app.ghosts();
    let today = app.today();

    // Header.
    let y = inner.y;
    for (k, d) in days.iter().enumerate() {
        let x = col_x(k);
        let col_w = col_w_of(k);
        let mut label = if col_w >= 10 {
            d.format("%a %-d").to_string()
        } else {
            d.format("%-d").to_string()
        };
        let holiday = per_day[k]
            .iter()
            .find(|c| c.primary.overlay == Overlay::Background && c.primary.span.is_all_day());
        let style = if *d == today && *d == app.cursor {
            // Block = today; mauve = selected. Today and selected: a mauve block.
            Style::default()
                .fg(c("base"))
                .bg(c("mauve"))
                .add_modifier(Modifier::BOLD)
        } else if *d == today {
            Style::default()
                .fg(c("base"))
                .bg(c("blue"))
                .add_modifier(Modifier::BOLD)
        } else if *d == app.cursor {
            Style::default()
                .fg(c("mauve"))
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else if *d < today {
            Style::default().fg(c("overlay0"))
        } else {
            Style::default().fg(c("subtext0"))
        };
        label = format!(" {label} ");
        put(buf, x, y, col_w.saturating_sub(1), &label, style);
        if let Some(h) = holiday {
            let used = label.width() as u16;
            put(
                buf,
                x + used,
                y,
                col_w.saturating_sub(1 + used),
                &format!("· {}", h.primary.title.trim()),
                Style::default().fg(c("peach")).add_modifier(Modifier::ITALIC),
            );
        }
    }

    // All-day strip. An event spanning several visible days is ONE bar across
    // them (not its title repeated per day); bars are packed into rows.
    let ad = |c: &Cluster| c.primary.span.is_all_day() && c.primary.overlay != Overlay::Background;
    let key_of = |c: &Cluster| {
        (
            c.primary.source.clone(),
            c.primary.uid.clone(),
            c.primary.recurrence_id.clone(),
        )
    };
    struct Bar<'a> {
        k0: usize,
        k1: usize,
        cl: &'a Cluster,
        key: (String, String, Option<String>),
    }
    let mut bars: Vec<Bar> = Vec::new();
    for (k, v) in per_day.iter().enumerate() {
        for cl in v.iter().filter(|c| ad(c)) {
            let key = key_of(cl);
            match bars.iter_mut().find(|b| b.key == key && b.k1 + 1 == k) {
                Some(b) => b.k1 = k,
                None => bars.push(Bar { k0: k, k1: k, cl, key }),
            }
        }
    }
    bars.sort_by_key(|b| (b.k0, std::cmp::Reverse(b.k1 - b.k0)));
    let cap = if single { 4 } else { 2 };
    let mut occupied: Vec<Vec<bool>> = Vec::new();
    let mut placed: Vec<(usize, &Bar)> = Vec::new();
    let mut hidden = vec![0usize; ndays];
    for b in &bars {
        let row = (0..)
            .find(|&r| occupied.get(r).is_none_or(|o: &Vec<bool>| (b.k0..=b.k1).all(|k| !o[k])))
            .unwrap_or(0);
        if row >= cap {
            for h in &mut hidden[b.k0..=b.k1] {
                *h += 1;
            }
            continue;
        }
        if occupied.len() <= row {
            occupied.resize(row + 1, vec![false; ndays]);
        }
        for o in &mut occupied[row][b.k0..=b.k1] {
            *o = true;
        }
        placed.push((row, b));
    }
    let strip_rows = occupied.len().min(cap);
    // Days with hidden bars give up their last row to "+N more".
    let more_row = strip_rows.saturating_sub(1);
    for (row, b) in &placed {
        let i = &b.cl.primary;
        let x0 = col_x(b.k0);
        let x1 = col_x(b.k1 + 1).saturating_sub(1);
        let width = x1.saturating_sub(x0);
        let cursor_k = days.iter().position(|d| *d == app.cursor);
        let selected = app.view != View::Agenda
            && cursor_k.is_some_and(|ck| {
                (b.k0..=b.k1).contains(&ck) && per_day[ck].iter().position(|c| key_of(c) == b.key) == Some(app.sel)
            });
        let base_col = if is_deadline(i) { c("red") } else { colour_of(app, i) };
        let past = days[b.k1] < today;
        let col = if past { fade(base_col, PAST_FADE) } else { base_col };
        // A soft tint of the event's colour across every day it covers, with
        // the title on the same tint: a span, not a line.
        let mut style = Style::default().fg(col).bg(fade(col, 0.7));
        if is_dimmed(i) {
            style = style.add_modifier(Modifier::DIM);
        }
        if is_deadline(i) && (selected || linked(i)) {
            // Deadlines highlight red, like the timed ⚑ line.
            style = Style::default().fg(c("base")).bg(c("red")).add_modifier(Modifier::BOLD);
        } else if selected {
            style = Style::default()
                .fg(c("base"))
                .bg(c("blue"))
                .add_modifier(Modifier::BOLD);
        }
        let mut text = label(i);
        if b.cl.is_merged() {
            text.push_str(" ⧉");
        }
        let mut s = format!("▌{text}");
        let pad = (width as usize).saturating_sub(s.width());
        s.push_str(&" ".repeat(pad));
        put(buf, x0, y + 1 + *row as u16, width, &s, style);
    }
    for (k, n) in hidden.iter().enumerate() {
        if *n > 0 && strip_rows > 0 {
            let x = col_x(k);
            let w = col_w_of(k).saturating_sub(1);
            let blank = " ".repeat(w as usize);
            put(buf, x, y + 1 + more_row as u16, w, &blank, Style::default());
            put(
                buf,
                x,
                y + 1 + more_row as u16,
                w,
                &format!("▎+{} more", n + usize::from(occupied[more_row][k])),
                Style::default().fg(c("overlay1")),
            );
        }
    }
    let sep_y = y + 1 + strip_rows as u16;
    for xx in inner.x..inner.x + inner.width {
        buf.set_string(xx, sep_y, "─", Style::default().fg(c("surface1")));
    }

    // Time grid.
    let grid_y = sep_y + 1;
    let rows = inner.y + inner.height - grid_y;
    if rows == 0 {
        return;
    }
    // Hours the events need, and the usual day (08–22) around them.
    let (mut e0, mut e1) = (i64::MAX, i64::MIN);
    for v in &per_day {
        for cl in v {
            if let Span::Timed { start, end } = cl.primary.span {
                let s = start.with_timezone(&tz);
                let e = end.with_timezone(&tz);
                if s.date_naive() == e.date_naive() {
                    e0 = e0.min(s.hour() as i64 * 60);
                    e1 = e1.max((e.hour() as i64 * 60 + i64::from(e.minute() > 0) * 60).min(24 * 60));
                }
            }
        }
    }
    let nrows = rows as i64;
    // Layout: 2 rows per hour (1 if space is short), with a divider row at
    // each section boundary (config `tui.section_hours`, default 12/18/22:
    // morning, afternoon, evening, night). Events crossing a boundary run
    // through it; events ending exactly on it stop above it.
    let sections: Vec<i64> = app
        .cfg
        .tui
        .section_hours
        .iter()
        .filter(|h| (1..24).contains(*h))
        .map(|h| i64::from(*h) * 60)
        .collect();
    let is_boundary = |m: i64| sections.contains(&m);
    let gaps_between = |a: i64, b: i64| -> i64 {
        // Section boundaries strictly inside (a, b).
        sections.iter().filter(|&&x| a < x && x < b).count() as i64
    };
    let needed = |a: i64, b: i64, rph: i64| (b - a) / 60 * rph + gaps_between(a, b);
    let usual = if e0 <= e1 {
        (e0.min(8 * 60), e1.max(22 * 60))
    } else {
        (8 * 60, 22 * 60)
    };
    let tight = if e0 <= e1 { (e0, e1.max(e0 + 60)) } else { usual };
    let mut layout = None;
    'pick: for rph in [2, 1] {
        for (a, b) in [usual, tight] {
            if needed(a, b, rph) <= nrows {
                layout = Some((rph, a, b));
                break 'pick;
            }
        }
    }
    let (rph, mut h0, mut h1) = layout.unwrap_or((0, tight.0, tight.1));
    if rph > 0 {
        // Spare rows add hours: towards the usual day, then 06:00–24:00.
        for (lo, hi) in [(8 * 60, 22 * 60), (6 * 60, 24 * 60)] {
            loop {
                if h1 < hi && needed(h0, h1 + 60, rph) <= nrows {
                    h1 += 60;
                } else if h0 > lo && needed(h0 - 60, h1, rph) <= nrows {
                    h0 -= 60;
                } else {
                    break;
                }
            }
        }
    }
    let span = (h1 - h0).max(60);
    let nrows = nrows as usize;
    let row_of_min = |m: i64| -> i64 {
        if rph > 0 {
            ((m - h0) * rph).div_euclid(60) + gaps_between(h0, m.min(h1) + 1)
        } else {
            // Too short even for one row per hour: squeeze, no gaps.
            ((m - h0) * nrows as i64).div_euclid(span)
        }
    };
    let mins_of = |t: DateTime<Utc>, d: NaiveDate| -> i64 {
        let l = t.with_timezone(&tz);
        if l.date_naive() < d {
            0
        } else if l.date_naive() > d {
            24 * 60
        } else {
            l.hour() as i64 * 60 + l.minute() as i64
        }
    };
    let row_of = |t: DateTime<Utc>, d: NaiveDate| -> i64 { row_of_min(mins_of(t, d)) };
    // Exclusive end row; an event ending exactly on a block boundary stops
    // above the blank row.
    let end_row = |s: DateTime<Utc>, e: DateTime<Utc>, d: NaiveDate| -> i64 {
        let r0 = row_of(s, d);
        let em = mins_of(e, d);
        let mut r1 = row_of_min(em);
        if rph > 0 && is_boundary(em) && em > h0 && em < h1 && r1 - 1 > r0 {
            r1 -= 1;
        }
        r1.max(r0 + 1)
    };
    let mut last_label: i64 = -2;
    // Divider rows: the row just above each section boundary's hour.
    let divider_rows: Vec<i64> = if rph > 0 {
        sections
            .iter()
            .filter(|&&m| m > h0 && m < h1)
            .map(|&m| row_of_min(m) - 1)
            .collect()
    } else {
        Vec::new()
    };
    for r in 0..nrows {
        if divider_rows.contains(&(r as i64)) {
            // One continuous line, straight through the column guides.
            for xx in inner.x + gutter..inner.x + inner.width {
                buf.set_string(xx, grid_y + r as u16, "┄", Style::default().fg(c("surface2")));
            }
            continue;
        }
        for k in 0..ndays {
            let x = col_x(k);
            buf.set_string(x, grid_y + r as u16, "┊", Style::default().fg(c("surface0")));
        }
    }
    let mut hour = (h0 + 59) / 60 * 60;
    while hour < h1 {
        let r = row_of_min(hour);
        if r > last_label && (0..nrows as i64).contains(&r) {
            put(
                buf,
                inner.x,
                grid_y + r as u16,
                gutter - 1,
                &format!("{:02}:00", hour / 60),
                Style::default().fg(c("overlay0")),
            );
            last_label = r;
        }
        hour += 60;
    }
    for (k, d) in days.iter().enumerate() {
        let x0 = col_x(k) + 1;
        let w = col_w_of(k).saturating_sub(2);
        let timed: Vec<(usize, usize, &Cluster)> = per_day[k]
            .iter()
            .filter(|c| !c.primary.span.is_all_day())
            .filter_map(|cl| {
                let (s, e) = cl.primary.span.bounds(tz);
                let r0 = row_of(s, *d).clamp(0, nrows as i64 - 1) as usize;
                // End row rounds down like the start, so back-to-back events never
                // share a row (which would split them into side-by-side lanes).
                let r1 = end_row(s, e, *d).clamp(r0 as i64 + 1, nrows as i64) as usize;
                (row_of(s, *d) < nrows as i64).then_some((r0, r1, cl))
            })
            .collect();
        let deadlines_first: Vec<(usize, usize, &Cluster)> =
            timed.iter().copied().filter(|t| !is_deadline(&t.2.primary)).collect();
        for p in lanes(&deadlines_first) {
            let lane_w = (w / p.lanes as u16).max(1);
            let x = x0 + lane_w * p.lane as u16;
            let i = &p.cl.primary;
            let idx = per_day[k].iter().position(|x| std::ptr::eq(x, p.cl));
            // Solid blue highlight when selected or linked to the selected deadline.
            let hl = (*d == app.cursor && idx == Some(app.sel) || linked(i)).then(|| c("blue"));
            let selected = hl.is_some();
            let ph = phase(app, i);
            let lively = attending(i);
            let col = colour_of(app, i);
            let col = if *d < today || ph == Phase::Past {
                fade(col, PAST_FADE)
            } else {
                col
            };
            // Now: the whole block tinted in its colour.
            let now_block = ph == Phase::Now && lively && !selected;
            let mut style = Style::default().fg(col);
            if now_block {
                style = style.bg(fade(col, 0.7));
            }
            if is_dimmed(i) {
                style = style.add_modifier(Modifier::DIM);
            }
            if let Some(bg) = hl {
                style = Style::default().fg(c("base")).bg(bg).add_modifier(Modifier::BOLD);
            }
            let t = i.start(tz).with_timezone(&tz);
            // Text area: lane minus the ▌ bar and a gap column.
            let tw = lane_w.saturating_sub(2) as usize;
            let rows_avail = p.r1 - p.r0;
            let full = label(i);
            // The real title, wrapped; `…` marks where it's cut (details on the bottom edge).
            let mut title = if app.squeeze_of(i).is_some() {
                format!("⚠ {full}")
            } else {
                full
            };
            if p.cl.is_merged() {
                title.push_str(" ⧉");
            }
            // The grid already says the hour; only off-the-hour starts (12:30)
            // need spelling out. Exact times are on the bottom edge anyway.
            let mut text = if lane_w >= 12 && t.minute() != 0 {
                format!("{} {title}", t.format("%H:%M"))
            } else {
                title
            };
            // Soon: "in 25m" at the end of the first line (peach, like the
            // now-line); the title wraps around it rather than being cut.
            // (Location isn't drawn in blocks; it's on the bottom edge.)
            let soon_tag = match ph {
                Phase::Soon(m) if lively && lane_w >= 14 => Some(in_text(m)),
                _ => None,
            };
            let mut lines = match &soon_tag {
                Some(tag) => wrap_first_narrow(&text, tw.saturating_sub(tag.width() + 1), tw, rows_avail),
                None => wrap_lines(&text, tw, rows_avail),
            };
            if lines.is_empty() {
                text = String::new();
                lines.push(text);
            }
            for r in p.r0..p.r1 {
                let k = r - p.r0;
                let body = match lines.get(k) {
                    Some(l) => format!("▌{l}"),
                    None if i.meta.kind == EventKind::TaskBlock => {
                        format!("▌{}", "░".repeat(tw))
                    }
                    None => "▌".to_string(),
                };
                let st = if selected {
                    style
                } else if k == 0 {
                    // Bold first line: back-to-back events stay distinguishable.
                    style.add_modifier(Modifier::BOLD)
                } else if k < lines.len() || now_block {
                    style.remove_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(col).add_modifier(Modifier::DIM)
                };
                // On a divider row only the bar (and any text) interrupts the
                // line; elsewhere pad so the event reads as one block. The
                // selected event stays a solid highlight.
                let lw = lane_w.saturating_sub(1) as usize;
                let on_divider = divider_rows.contains(&(r as i64));
                let body = if on_divider && !selected && !now_block {
                    body
                } else {
                    format!("{}{}", body, " ".repeat(lw.saturating_sub(body.width())))
                };
                put(buf, x, grid_y + r as u16, lane_w.saturating_sub(1), &body, st);
                if k == 0
                    && let Some(tag) = &soon_tag
                {
                    let tx = x + lane_w.saturating_sub(1 + tag.width() as u16);
                    let tag_style = if selected {
                        st
                    } else {
                        Style::default().fg(c("peach")).add_modifier(Modifier::BOLD)
                    };
                    put(buf, tx, grid_y + r as u16, tag.width() as u16, tag, tag_style);
                }
            }
        }
        // Timed deadlines: a ⚑ marker line across the column.
        // Bold (with "in 5h") only when due within 24 h; past ones fade.
        for (r0, _, cl) in timed.iter().filter(|t| is_deadline(&t.2.primary)) {
            let i = &cl.primary;
            let due = i.start(tz);
            let t = due.with_timezone(&tz).format("%H:%M");
            let left = due - app.now;
            let urgent = left > Duration::zero() && left <= Duration::hours(24);
            let when = if urgent {
                format!(" ({})", in_text(left.num_minutes().max(1)))
            } else {
                String::new()
            };
            // `Style::reset()`: the line is drawn over any event block in its
            // row, and must not inherit that block's background (e.g. the blue
            // of a selected event) or its bold.
            let mut st = Style::reset().fg(if due <= app.now {
                fade(c("red"), PAST_FADE)
            } else {
                c("red")
            });
            if urgent {
                st = st.add_modifier(Modifier::BOLD);
            }
            // Selected (or its work block is): a solid red block (like the
            // blue one for events), padded across the column.
            let idx = per_day[k].iter().position(|x| std::ptr::eq(x, *cl));
            let selected = (*d == app.cursor && idx == Some(app.sel)) || linked(i);
            let title = i.title.trim_start_matches('⚑').trim();
            let text = if selected {
                st = Style::default().fg(c("base")).bg(c("red")).add_modifier(Modifier::BOLD);
                let s = format!("⚑ {t} {title}{when}");
                format!("{s}{}", " ".repeat((w as usize).saturating_sub(s.width())))
            } else {
                format!("⚑ {t} {title}{when} ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")
            };
            put(buf, x0, grid_y + *r0 as u16, w, &text, st);
        }
        // Ghost blocks (pending changes).
        for g in ghosts
            .iter()
            .filter(|g| g.span.local_dates(tz).contains(d) && !g.span.is_all_day())
        {
            let (s, e) = g.span.bounds(tz);
            let r0 = row_of(s, *d);
            if r0 < 0 || r0 >= nrows as i64 {
                continue;
            }
            let r1 = end_row(s, e, *d).clamp(r0 + 1, nrows as i64);
            let st = Style::default().fg(c("overlay1")).add_modifier(Modifier::ITALIC);
            put(
                buf,
                x0 + w / 2,
                grid_y + r0 as u16,
                w - w / 2,
                &format!("◌ {}", g.title),
                st,
            );
            for r in r0 + 1..r1 {
                put(buf, x0 + w / 2, grid_y + r as u16, 1, "┆", st);
            }
        }
        // Now line.
        if *d == today {
            let r = row_of(app.now, *d);
            if (0..nrows as i64).contains(&r) {
                // Now: a peach line across today's column, under any event text.
                let y = grid_y + r as u16;
                let st = Style::default().fg(c("peach"));
                buf.set_string(col_x(k), y, "●", st);
                // The line only runs through free time, never across an event.
                let busy = timed.iter().any(|&(r0, r1, _)| (r0 as i64) <= r && r < r1 as i64);
                if !busy {
                    for xx in col_x(k) + 1..col_x(k) + col_w_of(k).saturating_sub(1) {
                        buf.set_string(xx, y, "─", st);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------- agenda

fn agenda(f: &mut Frame, app: &App, area: Rect) {
    let (a, b) = app.agenda_days();
    let block = panel(&format!("Agenda · {}–{}", a.format("%-d %b"), b.format("%-d %b")), true);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let rows = app.agenda_rows();
    let mut lines: Vec<Line> = Vec::new();
    let mut sel_line = 0;
    let mut last: Option<NaiveDate> = None;
    for (k, (d, cl)) in rows.iter().enumerate() {
        if last != Some(*d) {
            if last.is_some() {
                lines.push(Line::raw(""));
            }
            let today = app.today();
            let tag = if *d == today {
                " · today"
            } else if *d == today + Duration::days(1) {
                " · tomorrow"
            } else {
                ""
            };
            lines.push(Line::from(TSpan::styled(
                format!("{}{tag}", d.format("%a %-d %b")),
                Style::default().fg(c("blue")).add_modifier(Modifier::BOLD),
            )));
            last = Some(*d);
        }
        if k == app.sel {
            sel_line = lines.len();
        }
        lines.push(event_line(app, cl, k == app.sel, inner.width as usize));
    }
    if rows.is_empty() {
        lines.push(Line::from(TSpan::styled(
            "Nothing scheduled.",
            Style::default().fg(c("overlay1")),
        )));
    }
    let h = inner.height as usize;
    let scroll = sel_line.saturating_sub(h.saturating_sub(3)) as u16;
    f.render_widget(Paragraph::new(lines).scroll((scroll, 0)), inner);
}

fn event_line(app: &App, cl: &Cluster, selected: bool, width: usize) -> Line<'static> {
    let i = &cl.primary;
    let tz = app.tz;
    let when = match i.span {
        Span::AllDay { .. } => "all day    ".to_string(),
        Span::Timed { start, end } if start == end => format!("{}      ", start.with_timezone(&tz).format("%H:%M")),
        Span::Timed { start, end } => format!(
            "{}–{}",
            start.with_timezone(&tz).format("%H:%M"),
            end.with_timezone(&tz).format("%H:%M")
        ),
    };
    let mut title = label(i);
    if cl.is_merged() {
        title.push_str(" ⧉");
    }
    let mut meta = Vec::new();
    if let Some(l) = &i.location {
        meta.push(l.clone());
    }
    match (i.colour_key(), i.overlay) {
        (Some(k), _) => meta.push(k.to_string()),
        (None, Overlay::Family) => meta.push("Family".into()),
        (None, o) => meta.push(o.as_str().to_string()),
    }
    let meta = format!("  · {}", meta.join(" · "));
    let col = if is_deadline(i) { c("red") } else { colour_of(app, i) };
    let base = if selected {
        Style::default()
            .fg(c("base"))
            .bg(c("blue"))
            .add_modifier(Modifier::BOLD)
    } else if is_dimmed(i) {
        Style::default().fg(c("overlay1"))
    } else {
        Style::default().fg(c("text"))
    };
    let room = width.saturating_sub(when.width() + 4 + meta.width());
    Line::from(vec![
        TSpan::styled(
            format!(" {when} "),
            if selected {
                base
            } else {
                Style::default().fg(c("overlay1"))
            },
        ),
        TSpan::styled("● ", if selected { base } else { Style::default().fg(col) }),
        TSpan::styled(fit(&title, room.max(8)), base),
        TSpan::styled(
            meta,
            if selected {
                base
            } else {
                Style::default().fg(c("overlay0"))
            },
        ),
    ])
}

// ---------------------------------------------------------------- details (dedup expansion)

fn details(f: &mut Frame, app: &App, area: Rect, cl: &Cluster) {
    let i = &cl.primary;
    let tz = app.tz;
    let mut lines = vec![Line::from(TSpan::styled(
        label(i),
        Style::default().fg(colour_of(app, i)).add_modifier(Modifier::BOLD),
    ))];
    let when = match i.span {
        Span::AllDay { start, end } => {
            if end - start <= Duration::days(1) {
                start.format("%a %-d %b · all day").to_string()
            } else {
                format!(
                    "{} – {} · all day",
                    start.format("%a %-d %b"),
                    (end - Duration::days(1)).format("%a %-d %b")
                )
            }
        }
        Span::Timed { start, end } => format!(
            "{} – {}",
            start.with_timezone(&tz).format("%a %-d %b %H:%M"),
            end.with_timezone(&tz).format("%H:%M")
        ),
    };
    lines.push(Line::from(TSpan::styled(when, Style::default().fg(c("subtext0")))));
    if let Some(l) = &i.location {
        lines.push(Line::from(TSpan::styled(l.clone(), Style::default().fg(c("subtext0")))));
    }
    let mut facts = vec![format!("{} · {}", i.overlay.as_str(), i.colour_key().unwrap_or("—"))];
    if let Some(a) = i.meta.created_by {
        facts.push(format!("by {}", a.as_str()));
    }
    if i.recurring {
        facts.push("recurring".into());
    }
    if i.meta.flex {
        facts.push("flex".into());
    }
    lines.push(Line::from(TSpan::styled(
        facts.join(" · "),
        Style::default().fg(c("overlay1")),
    )));
    lines.push(Line::raw(""));
    lines.push(Line::from(TSpan::styled(
        if cl.is_merged() {
            format!("⧉ {} copies (showing the highest-precedence one)", cl.others.len() + 1)
        } else {
            "no duplicates".into()
        },
        Style::default().fg(c("mauve")),
    )));
    for o in cl.all() {
        lines.push(Line::from(vec![
            TSpan::styled("● ", Style::default().fg(colour_of(app, o))),
            TSpan::styled(format!("{} ", o.source), Style::default().fg(c("text"))),
            TSpan::styled(
                format!("{} · {}", o.overlay.as_str(), fit(&o.title, 30)),
                Style::default().fg(c("overlay1")),
            ),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(TSpan::styled(
        format!("uid {}", i.uid),
        Style::default().fg(c("overlay0")),
    )));
    let h = (lines.len() as u16 + 2).min(area.height);
    let r = centred(area, 64, h);
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(lines)
            .block(popup_block("event"))
            .wrap(Wrap { trim: true }),
        r,
    );
}

// ---------------------------------------------------------------- tasks / approvals

fn tasks(f: &mut Frame, app: &App, area: Rect) {
    let block = panel("Tasks", true);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let mut lines: Vec<Line> = Vec::new();
    if let Some(e) = &app.daemon_error {
        lines.push(Line::from(TSpan::styled(
            format!("kairosd: {e}"),
            Style::default().fg(c("red")),
        )));
    }
    if app.tasks.is_empty() {
        lines.push(Line::from(TSpan::styled(
            "No open tasks. a to add (title #key ~4h due fri 17:00).",
            Style::default().fg(c("overlay1")),
        )));
    }
    for (k, t) in app.tasks.iter().enumerate() {
        let sel = k == app.sel;
        let col = app.cfg.key(&t.key).map_or(c("text"), |k| rgb(k.colour));
        let done = t.estimate_min.saturating_sub(t.remaining_min);
        let bar_w = 12usize;
        let filled = ((done as usize * bar_w) / t.estimate_min.max(1) as usize).min(bar_w);
        let bar = format!(
            "{}{}{}",
            "━".repeat(filled),
            if filled < bar_w { "●" } else { "" },
            "─".repeat(bar_w.saturating_sub(filled + 1))
        );
        let due = t
            .due
            .map(|d| format!("due {}", d.describe(app.tz)))
            .unwrap_or_else(|| "no deadline".into());
        let base = if sel {
            Style::default()
                .fg(c("base"))
                .bg(c("blue"))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(c("text"))
        };
        let used_up = t.remaining_min == 0;
        lines.push(Line::from(vec![
            TSpan::styled("● ", if sel { base } else { Style::default().fg(col) }),
            TSpan::styled(format!("{:<28} ", fit(&t.title, 28)), base),
            TSpan::styled(
                format!("P{} ", t.priority),
                if sel { base } else { Style::default().fg(c("overlay1")) },
            ),
            TSpan::styled(bar, if sel { base } else { Style::default().fg(c("blue")) }),
            TSpan::styled(
                format!(" {}/{} min · {due}", done, t.estimate_min),
                if sel { base } else { Style::default().fg(c("overlay1")) },
            ),
            TSpan::styled(
                if used_up {
                    "  planned time used up — d finished · + more time"
                } else {
                    ""
                },
                if sel { base } else { Style::default().fg(c("peach")) },
            ),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(TSpan::styled(
        "a add · d done · + more time · n start now · x drop · P plan · Esc back",
        Style::default().fg(c("overlay0")),
    )));
    f.render_widget(Paragraph::new(lines), inner);
}

fn approvals(f: &mut Frame, app: &App, area: Rect) {
    let left_w = (area.width * 2 / 5).max(24).min(area.width);
    let left = Rect::new(area.x, area.y, left_w, area.height);
    let right = Rect::new(area.x + left_w, area.y, area.width - left_w, area.height);
    let block = panel(&format!("Approvals · {}", app.pending.len()), true);
    let inner = block.inner(left);
    f.render_widget(block, left);
    let mut lines: Vec<Line> = Vec::new();
    if let Some(e) = &app.daemon_error {
        lines.push(Line::from(TSpan::styled(
            format!("kairosd: {e}"),
            Style::default().fg(c("red")),
        )));
    }
    if app.pending.is_empty() {
        lines.push(Line::from(TSpan::styled(
            "Nothing waiting for approval.",
            Style::default().fg(c("overlay1")),
        )));
    }
    for (k, cs) in app.pending.iter().enumerate() {
        let sel = k == app.sel;
        let st = if sel {
            Style::default()
                .fg(c("base"))
                .bg(c("blue"))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(c("text"))
        };
        lines.push(Line::from(vec![
            TSpan::styled("◌ ", if sel { st } else { Style::default().fg(c("peach")) }),
            TSpan::styled(
                fit(cs.summary.lines().next().unwrap_or(""), inner.width as usize - 3),
                st,
            ),
        ]));
    }
    f.render_widget(Paragraph::new(lines), inner);
    let block = panel("diff", false);
    let inner = block.inner(right);
    f.render_widget(block, right);
    let Some(cs) = app.pending.get(app.sel) else { return };
    let mut lines = vec![
        Line::from(TSpan::styled(
            cs.summary.clone(),
            Style::default().fg(c("text")).add_modifier(Modifier::BOLD),
        )),
        Line::from(TSpan::styled(
            format!(
                "{} · by {} · {}",
                cs.id,
                cs.author.as_str(),
                cs.created_at.with_timezone(&app.tz).format("%a %-d %b %H:%M")
            ),
            Style::default().fg(c("overlay1")),
        )),
    ];
    for r in &cs.reasons {
        lines.push(Line::from(TSpan::styled(
            format!("! {r}"),
            Style::default().fg(c("peach")),
        )));
    }
    lines.push(Line::raw(""));
    // Big batches: counts by kind, then a few examples.
    const SHOW: usize = 4;
    if cs.ops.len() > SHOW + 2 {
        let mut kinds: std::collections::BTreeMap<&str, usize> = Default::default();
        for op in &cs.ops {
            let k = match op {
                kairos_core::changeset::Op::CreateEvent { .. } => "create",
                kairos_core::changeset::Op::UpdateEvent { .. } => "edit",
                kairos_core::changeset::Op::DeleteEvent { .. } => "delete",
                kairos_core::changeset::Op::MoveEvent { .. } => "move",
                _ => "other",
            };
            *kinds.entry(k).or_default() += 1;
        }
        let parts: Vec<String> = kinds.iter().map(|(k, n)| format!("{n} {k}")).collect();
        lines.push(Line::from(TSpan::styled(
            format!("{} changes: {} · first {SHOW} shown", cs.ops.len(), parts.join(" · ")),
            Style::default().fg(c("blue")).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::raw(""));
    }
    for op in cs
        .ops
        .iter()
        .take(if cs.ops.len() > SHOW + 2 { SHOW } else { usize::MAX })
    {
        lines.push(Line::from(TSpan::styled(
            format!("• {}", op.describe()),
            Style::default().fg(c("mauve")),
        )));
        let (before, after) = match op {
            kairos_core::changeset::Op::UpdateEvent { before, after, .. }
            | kairos_core::changeset::Op::MoveEvent { before, after, .. } => {
                (Some(before.as_str()), Some(after.as_str()))
            }
            kairos_core::changeset::Op::CreateEvent { after, .. } => (None, Some(after.as_str())),
            kairos_core::changeset::Op::DeleteEvent { before, .. } => (Some(before.as_str()), None),
            _ => (None, None),
        };
        let interesting = |l: &&str| {
            ["SUMMARY", "DTSTART", "DTEND", "LOCATION", "RRULE", "STATUS"]
                .iter()
                .any(|k| l.starts_with(k))
        };
        let bl: Vec<&str> = before
            .map(|b| b.lines().filter(interesting).collect())
            .unwrap_or_default();
        let al: Vec<&str> = after
            .map(|b| b.lines().filter(interesting).collect())
            .unwrap_or_default();
        for l in &bl {
            if !al.contains(l) {
                lines.push(Line::from(TSpan::styled(
                    format!("  - {l}"),
                    Style::default().fg(c("red")),
                )));
            }
        }
        for l in &al {
            if !bl.contains(l) {
                lines.push(Line::from(TSpan::styled(
                    format!("  + {l}"),
                    Style::default().fg(c("green")),
                )));
            }
        }
    }
    if cs.ops.len() > SHOW + 2 {
        lines.push(Line::from(TSpan::styled(
            format!(
                "… and {} more — `kairos show {}` lists every one",
                cs.ops.len() - SHOW,
                cs.id
            ),
            Style::default().fg(c("overlay1")),
        )));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(TSpan::styled(
        "y approve · n reject · j/k select · Esc back",
        Style::default().fg(c("overlay0")),
    )));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

// ---------------------------------------------------------------- popups

fn help(f: &mut Frame, area: Rect) {
    let rows = [
        ("h l / H L", "day / week back, forward"),
        ("t", "today"),
        ("j k", "select event"),
        ("Tab", "week → day → agenda"),
        ("Enter", "details / expand duplicates ⧉"),
        ("w", "week: Mon–Sun ↔ Mon–Fri (wider)"),
        ("p f b i", "show/hide: Personal, Family, Holidays, Gmail"),
        ("1–7", "show/hide one calendar (see the bar below)"),
        ("a", "add event (a form; Tab between fields)"),
        ("e K", "edit event (K jumps to its calendar)"),
        ("m", "move to another time"),
        ("d s", "delete, share to Family"),
        ("B", "Family event: block / unblock my time"),
        ("y d", "proposed project time ◌: accept (say what) / drop"),
        ("n", "start the selected task now (block from now; plan reflows)"),
        ("P", "run the scheduler"),
        ("T A", "tasks, approvals"),
        ("C", "check in today's work blocks"),
        ("u", "undo the last applied change"),
        ("/", "search"),
        ("r", "sync now"),
        ("q", "quit"),
    ];
    let lines: Vec<Line> = rows
        .iter()
        .map(|(k, d)| {
            Line::from(vec![
                TSpan::styled(
                    format!("{k:>10}  "),
                    Style::default().fg(c("blue")).add_modifier(Modifier::BOLD),
                ),
                TSpan::styled(*d, Style::default().fg(c("text"))),
            ])
        })
        .collect();
    let r = centred(area, 64, rows.len() as u16 + 2);
    f.render_widget(Clear, r);
    f.render_widget(Paragraph::new(lines).block(popup_block("keys")), r);
}

fn checkin(f: &mut Frame, app: &App, area: Rect, i: usize) {
    let open: Vec<_> = app.checkins.iter().filter(|c| c.outcome.is_none()).collect();
    let mut lines = vec![
        Line::from(TSpan::styled(
            "How did today's blocks go?",
            Style::default().fg(c("text")).add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
    ];
    for (k, it) in open.iter().enumerate() {
        let st = if k == i {
            Style::default()
                .fg(c("base"))
                .bg(c("blue"))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(c("text"))
        };
        lines.push(Line::from(TSpan::styled(
            format!(
                "{}–{}  {}",
                it.start.with_timezone(&app.tz).format("%H:%M"),
                it.end.with_timezone(&app.tz).format("%H:%M"),
                it.title
            ),
            st,
        )));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(TSpan::styled(
        "d done · p partial · m missed · s skip · Esc later",
        Style::default().fg(c("overlay0")),
    )));
    let r = centred(area, 60, lines.len() as u16 + 2);
    f.render_widget(Clear, r);
    f.render_widget(Paragraph::new(lines).block(popup_block("check-in")), r);
}

fn form_popup(f: &mut Frame, app: &App, area: Rect, form: &Form) {
    let w = 72.min(area.width.saturating_sub(2));
    let label_w = 10usize;
    let val_w = (w as usize).saturating_sub(label_w + 6);
    let mut lines: Vec<Line> = vec![Line::raw("")];
    for (k, fld) in form.fields.iter().enumerate() {
        let focus = k == form.focus;
        let lab = TSpan::styled(
            format!("{:>label_w$}  ", fld.label),
            if focus {
                Style::default().fg(c("mauve")).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(c("overlay1"))
            },
        );
        let mut spans = vec![lab];
        match fld.kind {
            FieldKind::Key | FieldKind::Type | FieldKind::Repeat => {
                let col = app.cfg.key(&fld.value).map_or(c("text"), |k| rgb(k.colour));
                let arrow = Style::default().fg(if focus { c("mauve") } else { c("surface2") });
                spans.push(TSpan::styled("◀ ", arrow));
                if fld.kind == FieldKind::Key {
                    spans.push(TSpan::styled("● ", Style::default().fg(col)));
                }
                spans.push(TSpan::styled(
                    fld.value.clone(),
                    Style::default()
                        .fg(c("text"))
                        .add_modifier(if focus { Modifier::BOLD } else { Modifier::empty() }),
                ));
                spans.push(TSpan::styled(" ▶", arrow));
            }
            _ => {
                let v = if focus {
                    // Keep the end (where typing happens) visible.
                    let chars: Vec<char> = fld.value.chars().collect();
                    let mut start = 0;
                    while chars[start..].iter().collect::<String>().width() + 1 > val_w && start < chars.len() {
                        start += 1;
                    }
                    chars[start..].iter().collect::<String>()
                } else {
                    fit(&fld.value, val_w)
                };
                spans.push(TSpan::styled(v, Style::default().fg(c("text"))));
                if focus {
                    spans.push(TSpan::styled("█", Style::default().fg(c("mauve"))));
                }
            }
        }
        lines.push(Line::from(spans));
        if focus && !fld.hint.is_empty() {
            lines.push(Line::from(TSpan::styled(
                format!("{:>label_w$}  {}", "", fit(fld.hint, val_w)),
                Style::default().fg(c("overlay0")).add_modifier(Modifier::ITALIC),
            )));
        }
    }
    lines.push(Line::raw(""));
    let preview = match app.form_preview(form) {
        Ok(p) => TSpan::styled(format!("  → {p}"), Style::default().fg(c("green"))),
        Err(e) => TSpan::styled(format!("  ✗ {e}"), Style::default().fg(c("peach"))),
    };
    lines.push(Line::from(preview));
    lines.push(Line::raw(""));
    lines.push(Line::from(TSpan::styled(
        "  Tab next · ⇧Tab back · Enter save · Esc cancel",
        Style::default().fg(c("overlay0")),
    )));
    let h = (lines.len() as u16 + 2).min(area.height);
    let r = centred(area, w, h);
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(lines)
            .block(popup_block(form.title()))
            .wrap(Wrap { trim: false }),
        r,
    );
}

// ---------------------------------------------------------------- status bar

fn status_bar(f: &mut Frame, app: &App, area: Rect) {
    let buf = f.buffer_mut();
    let y = area.y;
    let mut x = area.x;
    let push = |buf: &mut Buffer, x: &mut u16, s: &str, st: Style| {
        let w = s.width() as u16;
        if *x + w <= area.x + area.width {
            buf.set_string(*x, y, s, st);
            *x += w;
        }
    };
    match &app.mode {
        Mode::Prompt(kind, b) => {
            push(
                buf,
                &mut x,
                &format!(
                    " {} ",
                    if *kind == PromptKind::Search {
                        "search"
                    } else {
                        kind.label()
                    }
                ),
                Style::default()
                    .fg(c("base"))
                    .bg(c("mauve"))
                    .add_modifier(Modifier::BOLD),
            );
            push(buf, &mut x, "\u{e0b0}", Style::default().fg(c("mauve")));
            push(buf, &mut x, &format!(" {b}█"), Style::default().fg(c("text")));
            return;
        }
        Mode::Form(form) => {
            push(
                buf,
                &mut x,
                &format!(" {} ", form.title()),
                Style::default()
                    .fg(c("base"))
                    .bg(c("mauve"))
                    .add_modifier(Modifier::BOLD),
            );
            push(buf, &mut x, "\u{e0b0}", Style::default().fg(c("mauve")));
            if let Some((m, err)) = &app.message {
                push(
                    buf,
                    &mut x,
                    &format!(" {m}"),
                    Style::default().fg(if *err { c("red") } else { c("text") }),
                );
            }
            return;
        }
        Mode::Confirm(cf) => {
            let q = match cf {
                Confirm::Delete => "delete?",
                Confirm::Share => "share to Family?",
                Confirm::Undo(_) => "undo?",
                Confirm::DropTask(_) => "drop task?",
            };
            push(
                buf,
                &mut x,
                &format!(" {q} "),
                Style::default()
                    .fg(c("base"))
                    .bg(c("peach"))
                    .add_modifier(Modifier::BOLD),
            );
            push(buf, &mut x, "\u{e0b0}", Style::default().fg(c("peach")));
            if let Some((m, _)) = &app.message {
                push(buf, &mut x, &format!(" {m}"), Style::default().fg(c("text")));
            }
            return;
        }
        _ => {}
    }
    push(
        buf,
        &mut x,
        &format!(" {} ", app.view.name().to_uppercase()),
        Style::default()
            .fg(c("base"))
            .bg(c("blue"))
            .add_modifier(Modifier::BOLD),
    );
    push(
        buf,
        &mut x,
        "\u{e0b0}",
        Style::default().fg(c("blue")).bg(c("surface0")),
    );
    // Legend: toggle key + name; hidden ones are struck through. Names shrink to fit.
    let flags = overlay_flags(app);
    let room = (area.x + area.width).saturating_sub(x + 12) as usize;
    let full: usize = flags.iter().map(|f| f.0.width() + f.1.width() + 3).sum::<usize>() + 2;
    let short_len = |n: &str| n.chars().take(3).collect::<String>();
    let shortw: usize = flags
        .iter()
        .map(|f| f.0.width() + short_len(&f.1).width() + 3)
        .sum::<usize>()
        + 2;
    let mode = if full <= room {
        0
    } else if shortw <= room {
        1
    } else {
        2
    };
    let bg = Style::default().bg(c("surface0"));
    push(buf, &mut x, " ", bg);
    let mut prev_group = None;
    for (key, name, on, col, group) in flags {
        let key = key.as_str();
        if prev_group.is_some() && prev_group != Some(group) {
            push(buf, &mut x, "│ ", Style::default().fg(c("surface2")).bg(c("surface0")));
        }
        prev_group = Some(group);
        push(buf, &mut x, key, Style::default().fg(c("overlay1")).bg(c("surface0")));
        let shown = match mode {
            0 => format!(" {name}"),
            1 => format!(" {}", short_len(&name)),
            _ => String::new(),
        };
        let st = if on {
            Style::default()
                .fg(rgb(col))
                .bg(c("surface0"))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(c("surface2"))
                .bg(c("surface0"))
                .add_modifier(Modifier::CROSSED_OUT)
        };
        if mode == 2 {
            // Letters only: colour the key itself.
            x -= key.width() as u16;
            push(buf, &mut x, key, st);
        } else {
            push(buf, &mut x, &shown, st);
        }
        if key == "p" {
            push(buf, &mut x, " ▸ ", Style::default().fg(c("overlay1")).bg(c("surface0")));
        } else {
            push(buf, &mut x, "  ", bg);
        }
    }
    if let Some(q) = &app.search {
        push(
            buf,
            &mut x,
            &format!("/{q} "),
            Style::default().fg(c("yellow")).bg(c("surface0")),
        );
    }
    push(buf, &mut x, "\u{e0b0}", Style::default().fg(c("surface0")));
    // Right side: message, else pending + help hint.
    let (text, st) = match &app.message {
        Some((m, true)) => (format!(" {m} "), Style::default().fg(c("red"))),
        Some((m, false)) => (format!(" {m} "), Style::default().fg(c("green"))),
        None => {
            let mut parts = Vec::new();
            if !app.pending.is_empty() {
                parts.push(format!("◌ {} pending", app.pending.len()));
            }
            if app.daemon_error.is_some() {
                parts.push("kairosd offline".into());
            }
            if !app.index.missing.is_empty() {
                parts.push(format!("{} source(s) not synced", app.index.missing.len()));
            }
            parts.push("? help".into());
            (format!(" {} ", parts.join(" · ")), Style::default().fg(c("overlay1")))
        }
    };
    let w = text.width() as u16;
    let start = (area.x + area.width).saturating_sub(w).max(x);
    put(buf, start, y, area.x + area.width - start, &text, st);
}
