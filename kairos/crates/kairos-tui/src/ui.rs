//! Drawing, in the look every locus app shares (locus-theme, L1/L2): Catppuccin Mocha on a
//! transparent background, rounded grey frames, mauve popups, ` · ` separators, `…` truncation,
//! key hints as blue keys with dim words.

use crate::app::{App, Mode, PromptKind, View, is_deadline, is_dimmed, overlay_flags};
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
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

pub use locus_theme::{c, fade, fit};

pub fn rgb(r: Rgb) -> Color {
    Color::Rgb(r.0, r.1, r.2)
}

/// How much past days and not-attending Family events recede.
const PAST_FADE: f32 = 0.35;
const FAMILY_FADE: f32 = 0.45;

fn colour_of(app: &App, i: &Instance) -> Color {
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

fn put(buf: &mut Buffer, x: u16, y: u16, w: u16, s: &str, style: Style) {
    if w == 0 {
        return;
    }
    let s = fit(s, w as usize);
    buf.set_stringn(x, y, &s, w as usize, style);
}

/// The keys for what's selected, on the bottom-right edge in locus's hint format — unless the
/// window is too narrow to keep ~40 columns for the details on the left.
fn with_hints<'a>(block: Block<'a>, app: &App, width: u16) -> Block<'a> {
    let pairs = app.key_hint_pairs();
    let mut line = locus_theme::hints(&pairs);
    if pairs.is_empty() || (width as usize) < line.width() + 48 {
        return block;
    }
    line.spans.insert(0, TSpan::raw(" "));
    line.spans.push(TSpan::raw(" "));
    block.title_bottom(line.right_aligned())
}

/// The selected item: a solid fill in its own colour, softened a little so
/// it isn't glaring, with dark bold text — the strongest thing on screen, and
/// never a different calendar's colour.
pub fn selected_style(col: Color) -> Style {
    Style::default()
        .fg(c("base"))
        .bg(fade(col, 0.15))
        .add_modifier(Modifier::BOLD)
}

/// Linked to the selection (a task's ⚑ ↔ its work blocks): a medium tint of
/// its own colour, bold.
pub fn linked_style(col: Color) -> Style {
    Style::default()
        .fg(c("text"))
        .bg(fade(col, 0.55))
        .add_modifier(Modifier::BOLD)
}

/// A view's frame: locus's grey screen frame, `kairos` on the left as locus's screens name
/// themselves, and the view's own title (the week, the day) in the centre in blue so it stands
/// out (spec K6; your call 25 Oct: "make the border grey like the rest, but keep the week
/// heading in the middle blue").
fn panel(title: &str) -> Block<'static> {
    locus_theme::screen(locus_theme::title("kairos", ""), None).title(
        Line::from(TSpan::styled(
            format!(" {title} "),
            Style::default().fg(c("blue")).add_modifier(Modifier::BOLD),
        ))
        .alignment(Alignment::Center),
    )
}

fn popup_block(title: &str) -> Block<'static> {
    locus_theme::popup(title)
}

/// A date the locus way: the day lowercase, the month as written (`fri 23 Oct`, `sunday 25
/// October`), as locus's screens write them (L2).
fn lower_day(s: &str) -> String {
    match s.split_once(' ') {
        Some((day, rest)) => format!("{} {rest}", day.to_lowercase()),
        None => s.to_lowercase(),
    }
}

fn centred(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h)
}

pub fn label(app: &App, i: &Instance) -> String {
    let t = i.title.trim();
    match i.meta.kind {
        EventKind::Deadline => format!("⚑ {}", t.trim_start_matches('⚑').trim()),
        // Work: a box — 󰄱 to do (also past, until checked in), 󰄵 done.
        EventKind::TaskBlock => kairos_core::build::block_label(t, i.end(app.tz) <= app.now),
        // Project sessions (removed 2026-10-25) stored a diamond in the title; their events are
        // still on the calendar and are drawn as the ordinary events they now are.
        EventKind::Event => t.trim_start_matches(['◇', '◈', '◆']).trim_start().to_string(),
        EventKind::Worked => format!("worked · {t}"),
    }
}

/// A scheduler slot: a work block kairos placed and may still move — not
/// one Alec placed himself (pinned), and not one that has started (history:
/// done work shows ✓, solid like everything else in the past).
pub fn is_slot(i: &Instance, now: DateTime<Utc>) -> bool {
    i.meta.kind == EventKind::TaskBlock && !i.meta.pinned && i.span.bounds(chrono_tz::UTC).0 > now
}

/// A future work block Alec placed (moved by hand, started now): stays put.
pub fn is_placed(i: &Instance, now: DateTime<Utc>) -> bool {
    i.meta.kind == EventKind::TaskBlock && i.meta.pinned && i.span.bounds(chrono_tz::UTC).0 > now
}

/// The marker before an item in lists and the bottom edge (matches `edge`).
fn dot(i: &Instance, now: DateTime<Utc>) -> &'static str {
    if is_slot(i, now) {
        "┆ "
    } else if is_placed(i, now) {
        "╏ "
    } else {
        "● "
    }
}

/// The left edge of a block: who controls it. `┆` kairos may move it, `╏`
/// Alec placed it (stays), `▌` events and anything that has started.
fn edge(i: &Instance, now: DateTime<Utc>) -> &'static str {
    if is_slot(i, now) {
        "┆"
    } else if is_placed(i, now) {
        "╏"
    } else {
        "▌"
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
    !i.meta.not_going && (i.overlay != Overlay::Family || i.meta.origin_key.is_some() || i.meta.blocking)
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
    // No row left for the rest: cut the first line with `…`, so a title
    // that doesn't fit beside the tag never looks complete when it isn't.
    if rows <= 1 && !rest.is_empty() {
        return vec![fit(&format!("{first} {}", rest.join(" ")), w0)];
    }
    let mut out = vec![first];
    out.extend(wrap_lines(&rest.join(" "), w, rows.saturating_sub(1)));
    out
}

/// Where a work block's task stands: now ("task: 30m/1h done · 30m left")
/// for blocks under way or past; for a future block, as it will be when that
/// block starts, counting the task's blocks planned before it ("task: 2h50 of
/// 6h done by then · 1h10 left after this").
fn task_progress(app: &App, i: &Instance) -> Option<String> {
    let id = i.meta.task_id.as_deref()?;
    if let Some(t) = app.tasks.iter().find(|t| t.id == id) {
        let done = t.estimate_min.saturating_sub(t.remaining_min);
        let f = |m: u32| crate::form::fmt_minutes(i64::from(m));
        let due = t
            .due
            .map_or_else(|| "no due date".to_string(), |d| format!("due {}", d.describe(app.tz)));
        let (s, e) = i.span.bounds(app.tz);
        if s > app.now {
            // This task's work before this block that isn't counted yet:
            // planned blocks, and past ones not checked in (they count as done).
            let before: i64 = app
                .index
                .instances
                .iter()
                .filter(|b| {
                    b.meta.task_id.as_deref() == Some(id)
                        && b.meta.kind == EventKind::TaskBlock
                        && !kairos_core::build::is_checked_in(&b.title)
                        && b.uid != i.uid
                        && b.span.bounds(app.tz).1 <= s
                })
                .map(|b| {
                    let (bs, be) = b.span.bounds(app.tz);
                    (be - bs).num_minutes().max(0)
                })
                .sum();
            let by_then = (i64::from(done) + before).min(i64::from(t.estimate_min)) as u32;
            let this = (e - s).num_minutes().max(0);
            let after = (i64::from(t.remaining_min) - before - this).max(0) as u32;
            let after = if after == 0 {
                "done after this".to_string()
            } else {
                format!("{} left after", f(after))
            };
            return Some(format!(
                "task: {}/{} done by then · {after} · {due}",
                f(by_then),
                f(t.estimate_min)
            ));
        }
        let left = if t.remaining_min == 0 {
            "planned time used up".to_string()
        } else {
            format!("{} left", f(t.remaining_min))
        };
        return Some(format!("task: {}/{} done · {left} · {due}", f(done), f(t.estimate_min)));
    }
    app.done_tasks
        .iter()
        .find(|t| t.id == id)
        .map(|_| "task finished ✓".to_string())
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
    let mut parts = vec![when, label(app, i)];
    // Work for a task: where the task stands, not just this block.
    if i.meta.kind == EventKind::TaskBlock
        && let Some(info) = task_progress(app, i)
    {
        parts.push(info);
        if let Some(r) = app.squeeze_of(i) {
            parts.push(format!("⚠ squeezed {}", r.join(", ")));
        }
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
    if let Some(l) = app.locus_link(i) {
        parts.push(format!("→ {l}"));
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
        View::Month => month(f, app, main),
        View::Agenda => agenda(f, app, main),
    }
    if app.expanded
        && let Some(cl) = app.selected()
    {
        details(f, app, main, &cl);
    }
    match &app.mode {
        Mode::Help => help(f, app, main),
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
        lower_day(&app.cursor.format("%A %-d %B").to_string())
    } else {
        format!(
            "week {} · {}–{}",
            first.iso_week().week(),
            first.format("%-d %b"),
            last.format("%-d %b")
        )
    };
    // Oxford week (Sun–Sat) of the selected day.
    if let Some(w) = app.cfg.tui.term_week(app.cursor) {
        title.push_str(&format!(" · {w}"));
    }
    let mut block = with_hints(panel(&title), app, area.width);
    if let Some(cl) = app.selected() {
        let h = app.key_hints().width() as u16;
        let room = if area.width >= h + 48 {
            area.width.saturating_sub(8 + h)
        } else {
            area.width.saturating_sub(6)
        };
        let text = fit(&peek(app, &cl), room as usize);
        block = block.title_bottom(Line::from(vec![
            TSpan::styled(" ", Style::default()),
            TSpan::styled(
                dot(&cl.primary, app.now),
                Style::default().fg(colour_of(app, &cl.primary)),
            ),
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
            lower_day(&d.format("%a %-d").to_string())
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
        if selected {
            style = selected_style(base_col);
        } else if linked(i) {
            style = linked_style(base_col);
        }
        let mut text = label(app, i);
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
    // Layout: every hour two rows (spec K6, "even hour height"), with a divider row at
    // each section boundary (config `tui.section_hours`, default 12/18/22: morning, afternoon,
    // evening, night). A tall window shows more hours, never taller ones; a day that does not
    // fit scrolls (↑/↓ past the first or last item) instead of squashing to one row an hour.
    // Events crossing a boundary run through it; events ending exactly on it stop above it.
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
    // The day's extent: the usual day (08–22) and every event in view.
    let (ext0, ext1) = if e0 <= e1 {
        (e0.min(8 * 60), e1.max(22 * 60))
    } else {
        (8 * 60, 22 * 60)
    };
    let rph: i64 = if needed(ext0, ext0 + 60, 2) <= nrows { 2 } else { 0 };
    let (mut h0, mut h1) = (ext0, ext1);
    if rph > 0 && needed(ext0, ext1, rph) <= nrows {
        // Spare rows add hours, never blank rows: on to midnight, then earlier into the morning.
        loop {
            if h1 < 24 * 60 && needed(h0, h1 + 60, rph) <= nrows {
                h1 += 60;
            } else if h0 > 0 && needed(h0 - 60, h1, rph) <= nrows {
                h0 -= 60;
            } else {
                break;
            }
        }
    } else if rph > 0 {
        // Not all of it fits: a window of whole hours, scrolled to where you are.
        let anchor = app.day_top.get().unwrap_or_else(|| {
            let sel_start = app.selected().and_then(|c| match c.primary.span {
                Span::Timed { start, .. } => {
                    let l = start.with_timezone(&tz);
                    days.contains(&l.date_naive()).then(|| l.hour() as i64 * 60 - 60)
                }
                Span::AllDay { .. } => None,
            });
            let now_l = app.now.with_timezone(&tz);
            let now_start = days
                .contains(&now_l.date_naive())
                .then(|| now_l.hour() as i64 * 60 - 60);
            // Else the first timed thing in view, else the usual morning.
            let first = (e0 <= e1).then(|| e0 - 60);
            sel_start.or(now_start).or(first).unwrap_or(8 * 60)
        });
        let mut len = 60;
        while ext0 + len < ext1 && needed(anchor.max(ext0), anchor.max(ext0) + len + 60, rph) <= nrows {
            len += 60;
        }
        h0 = (anchor / 60 * 60).clamp(ext0, (ext1 - len).max(ext0));
        h1 = h0 + len;
    }
    let (h0, h1) = (h0, h1);
    app.grid.set((h0, h1, ext0.min(h0), ext1.max(h1)));
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
    // Hours above or below the window: a mark beside the first or last hour shown.
    let (_, _, x0_ext, x1_ext) = app.grid.get();
    if rph > 0 && h0 > x0_ext {
        put(
            buf,
            inner.x + gutter - 1,
            grid_y,
            1,
            "▲",
            Style::default().fg(c("overlay1")),
        );
    }
    if rph > 0 && h1 < x1_ext && nrows > 0 {
        put(
            buf,
            inner.x + gutter - 1,
            grid_y + nrows as u16 - 1,
            1,
            "▼",
            Style::default().fg(c("overlay1")),
        );
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
                // Scrolled out of the window (above or below it): not drawn, `▲`/`▼` say so.
                let visible = row_of(s, *d) < nrows as i64 && end_row(s, e, *d) > 0;
                visible.then_some((r0, r1, cl))
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
            // Selected: a solid fill in its own colour; linked: a medium tint.
            let own = colour_of(app, i);
            let hl = if *d == app.cursor && idx == Some(app.sel) {
                Some(selected_style(own))
            } else if linked(i) {
                Some(linked_style(own))
            } else {
                None
            };
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
            if let Some(h) = hl {
                style = h;
            }
            // Work for a task: italic (and a dashed edge while kairos may move it).
            if i.meta.kind == EventKind::TaskBlock {
                style = style.add_modifier(Modifier::ITALIC);
            }
            let t = i.start(tz).with_timezone(&tz);
            // Text area: lane minus the ▌ bar and a gap column.
            let tw = lane_w.saturating_sub(2) as usize;
            let rows_avail = p.r1 - p.r0;
            let full = label(app, i);
            // The real title, wrapped; `…` marks where it's cut (details on the bottom edge).
            // Squeezed: ⚠ after the box ("󰄱 ⚠ B4 sheet").
            let mut title = if app.squeeze_of(i).is_some() {
                match full.split_once(' ') {
                    Some((b, rest)) if b == kairos_core::build::TODO_ICON => format!("{b} ⚠ {rest}"),
                    _ => format!("⚠ {full}"),
                }
            } else {
                full
            };
            if p.cl.is_merged() {
                title.push_str(" ⧉");
            }
            let mut text = title;
            // A tag at the end of the first line (the title wraps around it):
            // "in 25m" when it starts soon (peach, like the now-line), else
            // the minutes when the start falls between grid rows — ":15" for
            // 18:15, dim — since the block is drawn from the row above. On a
            // grid row the position says the time. (Location and exact times
            // are on the bottom edge.)
            let off_grid = if rph > 0 {
                (i64::from(t.minute()) * rph) % 60 != 0
            } else {
                t.minute() != 0
            };
            let soon_tag: Option<(String, Style)> = match ph {
                Phase::Soon(m) if lively && lane_w >= 14 => {
                    Some((in_text(m), Style::default().fg(c("peach")).add_modifier(Modifier::BOLD)))
                }
                _ if off_grid && lane_w >= 10 => {
                    Some((format!(":{:02}", t.minute()), Style::default().fg(c("overlay1"))))
                }
                _ => None,
            };
            let mut lines = match soon_tag.as_ref().map(|t| &t.0) {
                Some(tag) => wrap_first_narrow(&text, tw.saturating_sub(tag.width() + 1), tw, rows_avail),
                None => wrap_lines(&text, tw, rows_avail),
            };
            if lines.is_empty() {
                text = String::new();
                lines.push(text);
            }
            for r in p.r0..p.r1 {
                let k = r - p.r0;
                let bar = edge(i, app.now);
                let body = match lines.get(k) {
                    Some(l) => format!("{bar}{l}"),
                    None => bar.to_string(),
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
                    && let Some((tag, tag_st)) = &soon_tag
                {
                    let tx = x + lane_w.saturating_sub(1 + tag.width() as u16);
                    let tag_style = if selected { st } else { *tag_st };
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
            let is_sel = *d == app.cursor && idx == Some(app.sel);
            let selected = is_sel || linked(i);
            let title = i.title.trim_start_matches('⚑').trim();
            let text = if selected {
                st = if is_sel {
                    selected_style(c("red"))
                } else {
                    linked_style(c("red"))
                };
                let s = format!("⚑ {t} {title}{when}");
                format!("{s}{}", " ".repeat((w as usize).saturating_sub(s.width())))
            } else {
                format!("⚑ {t} {title}{when} ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")
            };
            put(buf, x0, grid_y + *r0 as u16, w, &text, st);
        }
        // Worked time (spec K6): measured from your Claude sessions after the fact, drawn on the
        // day's own left guide (so it reads as that day's) in a faint tint of the project's area,
        // in place of the dotted guide. Never selectable, never over an event.
        for wk in app.worked_on(*d) {
            let (s, e) = wk.span.bounds(tz);
            let r0 = row_of(s, *d).max(0);
            let r1 = end_row(s, e, *d).min(nrows as i64);
            let col = wk
                .key
                .as_deref()
                .and_then(|k| app.cfg.key(k))
                .map_or(c("overlay0"), |k| rgb(k.colour));
            let st = Style::default().fg(fade(col, 0.72));
            let x = col_x(k);
            for r in r0..r1 {
                let y = grid_y + r as u16;
                if buf[(x, y)].symbol() == "┊" {
                    buf.set_string(x, y, "│", st);
                }
            }
            if single && r0 < r1 {
                let text = format!("worked · {} ", wk.title);
                let tw = text.width() as u16;
                let right = x0 + w.saturating_sub(1);
                let tx = right.saturating_sub(tw);
                let y = grid_y + r0 as u16;
                if tx > x0 && (tx..right).all(|xx| buf[(xx, y)].symbol() == " ") {
                    put(buf, tx, y, tw, &text, st);
                }
            }
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

/// Month view: Monday-first weeks covering the cursor's month; each day lists
/// what's on (time + title, coloured by calendar; `┆` work, `⚑` deadlines),
/// with "+N" when it doesn't fit. The cursor day is mauve, today blue.
fn month(f: &mut Frame, app: &App, area: Rect) {
    let tz = app.tz;
    let today = app.today();
    let cur = app.cursor;
    let first = cur.with_day(1).unwrap_or(cur);
    let start = first - Duration::days(i64::from(first.weekday().num_days_from_monday()));
    let next_month = if first.month() == 12 {
        NaiveDate::from_ymd_opt(first.year() + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(first.year(), first.month() + 1, 1)
    }
    .unwrap_or(first + Duration::days(31));
    let weeks = ((next_month - start).num_days() + 6) / 7;
    let mut title = first.format("%B %Y").to_string();
    if let Some(w) = app.cfg.tui.term_week(cur) {
        title.push_str(&format!(" · {w}"));
    }
    let mut block = with_hints(panel(&title), app, area.width);
    if let Some(cl) = app.selected() {
        let h = app.key_hints().width() as u16;
        let room = if area.width >= h + 48 {
            area.width.saturating_sub(8 + h)
        } else {
            area.width.saturating_sub(6)
        };
        block = block.title_bottom(Line::from(vec![
            TSpan::styled(" ", Style::default()),
            TSpan::styled(
                dot(&cl.primary, app.now),
                Style::default().fg(colour_of(app, &cl.primary)),
            ),
            TSpan::styled(
                format!("{} ", fit(&peek(app, &cl), room as usize)),
                Style::default().fg(c("text")),
            ),
        ]));
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height < 4 || inner.width < 21 {
        return;
    }
    let buf = f.buffer_mut();
    // Weekday header.
    let col_x = |k: u16| inner.x + inner.width * k / 7;
    let col_w = |k: u16| col_x(k + 1) - col_x(k);
    for (k, d) in ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"].iter().enumerate() {
        put(
            buf,
            col_x(k as u16),
            inner.y,
            col_w(k as u16),
            &format!(" {d}"),
            Style::default().fg(c("overlay1")),
        );
    }
    let top = inner.y + 1;
    let avail = inner.height - 1;
    let row_y = |r: u16| top + avail * r / weeks as u16;
    let row_h = |r: u16| row_y(r + 1) - row_y(r);
    for r in 0..weeks as u16 {
        for k in 0..7u16 {
            let d = start + Duration::days(i64::from(r * 7 + k));
            let (x, y, w, h) = (col_x(k), row_y(r), col_w(k), row_h(r));
            let inside = d.month() == first.month();
            // Cell rule on top, date in its corner.
            let rule_st = Style::default().fg(c("surface1"));
            put(buf, x, y, w, &"─".repeat(w as usize), rule_st);
            let date_st = if d == today && d == cur {
                Style::default()
                    .fg(c("base"))
                    .bg(c("mauve"))
                    .add_modifier(Modifier::BOLD)
            } else if d == today {
                Style::default()
                    .fg(c("base"))
                    .bg(c("blue"))
                    .add_modifier(Modifier::BOLD)
            } else if d == cur {
                Style::default()
                    .fg(c("mauve"))
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else if !inside || d < today {
                Style::default().fg(c("overlay0"))
            } else {
                Style::default().fg(c("subtext0"))
            };
            put(buf, x, y, 4.min(w), &format!("{:>3} ", d.day()), date_st);
            if h < 2 {
                continue;
            }
            let items = app.day_clusters(d);
            let lines = (h - 1) as usize;
            let shown = if items.len() > lines {
                lines.saturating_sub(1)
            } else {
                items.len()
            };
            for (n, cl) in items.iter().take(shown).enumerate() {
                let i = &cl.primary;
                let selected = d == cur && n == app.sel;
                let when = match i.span {
                    Span::Timed { start, .. } if !i.span.is_all_day() => {
                        format!("{} ", start.with_timezone(&tz).format("%H:%M"))
                    }
                    _ => String::new(),
                };
                let mark = match edge(i, app.now) {
                    "▌" => " ",
                    e => e,
                };
                let text = format!("{mark}{when}{}", label(app, i));
                let col = if is_deadline(i) { c("red") } else { colour_of(app, i) };
                let col = if d < today || !inside {
                    fade(col, PAST_FADE)
                } else {
                    col
                };
                let st = if selected {
                    selected_style(if is_deadline(i) { c("red") } else { colour_of(app, i) })
                } else if is_dimmed(i) {
                    Style::default().fg(col).add_modifier(Modifier::DIM)
                } else {
                    Style::default().fg(col)
                };
                let st = if i.meta.kind == EventKind::TaskBlock {
                    st.add_modifier(Modifier::ITALIC)
                } else {
                    st
                };
                put(
                    buf,
                    x,
                    y + 1 + n as u16,
                    w.saturating_sub(1),
                    &fit(&text, w.saturating_sub(1) as usize),
                    st,
                );
            }
            if items.len() > shown {
                put(
                    buf,
                    x,
                    y + 1 + shown as u16,
                    w.saturating_sub(1),
                    &format!(" +{} more", items.len() - shown),
                    Style::default().fg(c("overlay1")),
                );
            }
        }
    }
}

fn agenda(f: &mut Frame, app: &App, area: Rect) {
    let (a, b) = app.agenda_days();
    let block = with_hints(
        panel(&format!("agenda · {}–{}", a.format("%-d %b"), b.format("%-d %b"))),
        app,
        area.width,
    );
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
                format!("{}{tag}", lower_day(&d.format("%a %-d %b").to_string())),
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
            "nothing scheduled",
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
    let mut title = label(app, i);
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
        selected_style(col)
    } else if is_dimmed(i) {
        Style::default().fg(c("overlay1"))
    } else {
        Style::default().fg(c("text"))
    };
    let base = if i.meta.kind == EventKind::TaskBlock {
        base.add_modifier(Modifier::ITALIC)
    } else {
        base
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
        TSpan::styled(dot(i, app.now), if selected { base } else { Style::default().fg(col) }),
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
        label(app, i),
        Style::default().fg(colour_of(app, i)).add_modifier(Modifier::BOLD),
    ))];
    let when = match i.span {
        Span::AllDay { start, end } => {
            if end - start <= Duration::days(1) {
                lower_day(&start.format("%a %-d %b · all day").to_string())
            } else {
                format!(
                    "{} – {} · all day",
                    lower_day(&start.format("%a %-d %b").to_string()),
                    lower_day(&(end - Duration::days(1)).format("%a %-d %b").to_string())
                )
            }
        }
        Span::Timed { start, end } => format!(
            "{} – {}",
            lower_day(&start.with_timezone(&tz).format("%a %-d %b %H:%M").to_string()),
            end.with_timezone(&tz).format("%H:%M")
        ),
    };
    lines.push(Line::from(TSpan::styled(when, Style::default().fg(c("subtext0")))));
    if let Some(l) = &i.location {
        lines.push(Line::from(TSpan::styled(l.clone(), Style::default().fg(c("subtext0")))));
    }
    // What it is in the taxonomy and what it is linked to, row by row (your call 25 Oct: "I
    // would rather see the taxonomy clearly laid out, with a clear idea of what it is linked
    // to"), then who made it.
    let row = |name: &str, value: Vec<TSpan<'static>>| {
        let mut v = vec![TSpan::styled(format!("{name:<9}"), locus_theme::dim())];
        v.extend(value);
        Line::from(v)
    };
    let text = |s: String| TSpan::styled(s, Style::default().fg(c("text")));
    let dimmed = |s: String| TSpan::styled(s, locus_theme::dim());
    lines.push(Line::raw(""));
    // Area, and its stream for Personal: Studies · Personal › Errands · Personal › Family.
    let key = i.colour_key();
    let area_name = match i.overlay {
        Overlay::Family => "Personal › Family".to_string(),
        Overlay::Background => "a background calendar".to_string(),
        Overlay::Imported => "imported (read only)".to_string(),
        Overlay::Personal => match key.and_then(|k| app.cfg.key(k)) {
            Some(k) => match &k.area {
                Some(a) => format!("{a} › {}", k.name),
                None => k.name.clone(),
            },
            None => key.unwrap_or("—").to_string(),
        },
    };
    lines.push(row(
        "area",
        vec![
            TSpan::styled("● ", Style::default().fg(colour_of(app, i))),
            text(area_name),
        ],
    ));
    // What it is for: the registry item (and its kind), else not linked yet.
    let link = app.locus_link(i);
    let for_ = match link
        .as_deref()
        .and_then(|l| app.fors.iter().find(|o| o.locus.as_deref() == Some(l)))
    {
        Some(o) => {
            let kind = if o.course {
                "course"
            } else if o.application {
                "application"
            } else {
                "project"
            };
            vec![
                text(o.label.clone()),
                dimmed(format!(" · {kind}")),
                TSpan::styled("   ⏎", locus_theme::keyst()),
                dimmed(" opens it in locus".into()),
            ]
        }
        None => match link {
            Some(l) => vec![text(l)],
            // No item: its calendar is the answer, an area (general within it) or a stream.
            None if matches!(i.overlay, Overlay::Personal) && !app.for_label(None, key).is_empty() => vec![
                text(app.for_label(None, key)),
                dimmed("  ".into()),
                TSpan::styled("L", locus_theme::keyst()),
                dimmed(" links it to something specific".into()),
            ],
            None => vec![
                dimmed("not linked to anything yet  ".into()),
                TSpan::styled("L", locus_theme::keyst()),
                dimmed(" says what it is for".into()),
            ],
        },
    };
    lines.push(row("for", for_));
    // Its task, for a work block or a due date.
    if let Some(p) = task_progress(app, i) {
        let title = i
            .meta
            .task_id
            .as_deref()
            .and_then(|id| app.tasks.iter().chain(app.done_tasks.iter()).find(|t| t.id == id))
            .map(|t| t.title.clone())
            .unwrap_or_default();
        let p = p.trim_start_matches("task: ").to_string();
        lines.push(row("task", vec![text(title), dimmed(format!(" · {p}"))]));
    }
    // Who made it: you, or an agent.
    let by = match i.meta.created_by {
        Some(kairos_core::model::Author::Alec) => "you, in kairos".to_string(),
        Some(kairos_core::model::Author::Claude) => "Claude (an agent)".to_string(),
        Some(kairos_core::model::Author::Scheduler) => "the scheduler (kairos)".to_string(),
        None => "you, on the phone or in Calendar".to_string(),
    };
    let mut flags = Vec::new();
    if i.recurring {
        flags.push("repeats");
    }
    if is_slot(i, app.now) {
        flags.push("kairos may move it");
    } else if i.meta.pinned {
        flags.push("you placed it");
    }
    if i.meta.not_going {
        flags.push("not going");
    }
    if i.overlay == Overlay::Family {
        flags.push(if i.meta.blocking {
            "blocks your time"
        } else {
            "doesn't block your time"
        });
    }
    let mut made = vec![text(by)];
    if !flags.is_empty() {
        made.push(dimmed(format!(" · {}", flags.join(" · "))));
    }
    lines.push(row("made by", made));
    // Copies, only when there are any.
    if cl.is_merged() {
        lines.push(Line::raw(""));
        lines.push(Line::from(TSpan::styled(
            format!("⧉ {} copies, showing the one that wins", cl.others.len() + 1),
            Style::default().fg(c("mauve")),
        )));
        for o in cl.all() {
            lines.push(Line::from(vec![
                TSpan::styled("  ● ", Style::default().fg(colour_of(app, o))),
                TSpan::styled(format!("{} ", o.source), Style::default().fg(c("text"))),
                TSpan::styled(fit(&o.title, 30), locus_theme::dim()),
            ]));
        }
    }
    let h = (lines.len() as u16 + 2).min(area.height);
    let r = centred(area, 74, h);
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(lines)
            .block(popup_block("event").padding(ratatui::widgets::Padding::horizontal(1)))
            .wrap(Wrap { trim: true }),
        r,
    );
}

// ---------------------------------------------------------------- popups

/// The keys overlay, as locus's (L2): this screen, everywhere, symbols. The digit filters are
/// here with each area in its colour, since the footer no longer carries a legend.
fn help(f: &mut Frame, app: &App, area: Rect) {
    let key = locus_theme::keyst();
    let text = Style::default().fg(c("text"));
    let dim = locus_theme::dim();
    let pair = |k: &str, what: &str| {
        Line::from(vec![
            TSpan::styled(format!("  {k:<10}"), key),
            TSpan::styled(what.to_string(), text),
        ])
    };
    let mut lines = vec![Line::from(TSpan::styled(" this screen", dim))];
    for (k, what) in [
        ("← → [ ]", "day · week back, forward"),
        ("⇧↑↓ ⇧←→", "move the selected item 30 min · a day"),
        ("⌥↑↓", "shorter · longer by 15 min (a block: its task's time)"),
        ("↑ ↓", "select; past the first or last, scroll the hours"),
        ("⇥ ⇧⇥", "week → day → month → agenda"),
        ("t w", "today · week Mon–Sun or Mon–Fri"),
        ("+", "add an event or a task (it asks what it is for)"),
        ("e m L", "edit · move · say what it is for"),
        ("d x", "done (a task) · remove (asks first)"),
        ("y x d", "a past block (✓ too): done · didn't happen · task done"),
        ("n W S", "start a task now · log work · run the scheduler"),
        ("s B", "share to Family · going or not"),
        ("C u r", "check in · undo · sync"),
        ("T P A", "tasks · projects · approve, in locus"),
    ] {
        lines.push(pair(k, what));
    }
    // The filters: each digit with its area or stream, in its colour.
    let mut legend = vec![TSpan::styled(format!("  {:<10}", "1–8 p f"), key)];
    for (k, name, _, col, _) in overlay_flags(app) {
        legend.push(TSpan::styled(format!("{k} "), dim));
        legend.push(TSpan::styled(
            format!("{}  ", name.to_lowercase()),
            Style::default().fg(rgb(col)),
        ));
    }
    lines.push(Line::from(legend));
    lines.push(Line::from(vec![
        TSpan::raw(format!("  {:<10}", "")),
        TSpan::styled("show or hide each; p all yours, f Family", dim),
    ]));
    lines.push(Line::raw(""));
    lines.push(Line::from(TSpan::styled(" everywhere", dim)));
    for (k, what) in [
        ("⏎", "open in locus, else the details"),
        ("esc", "back"),
        ("/", "search"),
        ("?", "keys"),
        ("alt-1…4", "home · project · calendar · mail"),
    ] {
        lines.push(pair(k, what));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(TSpan::styled(" symbols", dim)));
    for (sym, what) in [
        (
            format!(
                "{} {} {}",
                locus_theme::glyph::TODO_BOX,
                locus_theme::glyph::WAITING_BOX,
                locus_theme::glyph::DONE
            ),
            "a work block: to do · over, not checked in · done",
        ),
        ("┆ ╏ ▌".to_string(), "kairos may move it · you placed it · an event"),
        ("⚑".to_string(), "a task's due date"),
        ("◌".to_string(), "a change waiting in Approve"),
        ("⧉".to_string(), "copies of one event (only the winner is drawn)"),
    ] {
        lines.push(Line::from(vec![
            TSpan::styled(format!("  {sym:<10}"), key),
            TSpan::styled(what, text),
        ]));
    }
    let h = (lines.len() as u16 + 2).min(area.height);
    let r = centred(area, 80, h);
    f.render_widget(Clear, r);
    f.render_widget(Paragraph::new(lines).block(popup_block("keys")), r);
}

fn checkin(f: &mut Frame, app: &App, area: Rect, i: usize) {
    let open: Vec<_> = app.checkins.iter().filter(|c| c.outcome.is_none()).collect();
    let mut lines = vec![
        Line::from(TSpan::styled(
            "How did these go?",
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
                "{} {}–{}  {}",
                it.start.with_timezone(&app.tz).format("%a"),
                it.start.with_timezone(&app.tz).format("%H:%M"),
                it.end.with_timezone(&app.tz).format("%H:%M"),
                it.title
            ),
            st,
        )));
    }
    lines.push(Line::raw(""));
    lines.push(locus_theme::hints(&[
        ("y", "done"),
        ("x", "missed"),
        ("d", "task done"),
        ("⏎", "fix its time"),
        ("↓", "next"),
        ("esc", "later"),
    ]));
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
            FieldKind::Key | FieldKind::Type | FieldKind::Repeat | FieldKind::Project => {
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
        // For (ADR-009): what it would be, as you type: the matches, the first one taken.
        if focus && fld.kind == FieldKind::For && !form.fors.is_empty() {
            let matches = form.for_matches(&fld.value);
            let mut m: Vec<TSpan> = vec![TSpan::raw(format!("{:>label_w$}  ", ""))];
            if matches.is_empty() {
                m.push(TSpan::styled("nothing matches", Style::default().fg(c("peach"))));
            }
            let mut used = label_w + 2;
            for (n, o) in matches.iter().take(5).enumerate() {
                let col = app.cfg.key(&o.key).map_or(c("text"), |k| rgb(k.colour));
                let text = format!("{}{} ", if n == 0 { "" } else { "· " }, o.label);
                if used + text.width() + 2 > val_w + label_w + 2 {
                    break;
                }
                used += text.width() + 2;
                m.push(TSpan::styled("● ", Style::default().fg(col)));
                m.push(TSpan::styled(
                    text,
                    if n == 0 {
                        Style::default().fg(c("text")).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(c("overlay1"))
                    },
                ));
            }
            lines.push(Line::from(m));
        }
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
    let mut keys = locus_theme::hints(&[("⇥", "next"), ("⇧⇥", "back"), ("⏎", "save"), ("esc", "cancel")]);
    keys.spans.insert(0, TSpan::raw("  "));
    lines.push(keys);
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

/// The footer, as locus's (L2; your call 25 Oct: no legend, say only what is hidden): the keys
/// that always work on the left; on the right the message, else what is hidden by the digit
/// filters and what waits for you. While typing or answering, what finishes it.
fn status_bar(f: &mut Frame, app: &App, area: Rect) {
    let (left, typed): (Line<'static>, Option<Line<'static>>) = match &app.mode {
        Mode::Prompt(kind, b) => {
            let label = if *kind == PromptKind::Search {
                "search"
            } else {
                kind.label()
            };
            (
                locus_theme::hints(&[("⏎", "done"), ("esc", "cancel")]),
                Some(Line::from(vec![
                    TSpan::styled(format!("{label}: "), locus_theme::dim()),
                    TSpan::styled(format!("{b}█"), Style::default().fg(c("text"))),
                ])),
            )
        }
        Mode::Form(_) => (
            locus_theme::hints(&[("⇥", "next field"), ("⏎", "save"), ("esc", "cancel")]),
            None,
        ),
        Mode::Confirm(_) => (locus_theme::hints(&[("y", "yes"), ("any other key", "no")]), None),
        Mode::AddChoice => (
            locus_theme::hints(&[("e", "event"), ("t", "task"), ("esc", "cancel")]),
            None,
        ),
        _ => (
            locus_theme::hints(&[("⏎", "open"), ("⇥", "view"), ("[ ]", "week"), ("?", "keys")]),
            None,
        ),
    };
    let mut spans = vec![TSpan::raw(" ")];
    spans.extend(left.spans);
    if let Some(t) = typed {
        spans.push(TSpan::raw("   "));
        spans.extend(t.spans);
    } else if let Some(q) = &app.search {
        spans.push(TSpan::styled(format!("   /{q}"), Style::default().fg(c("yellow"))));
    }
    let left = Line::from(spans);
    let used = left.width() as u16;
    f.render_widget(Paragraph::new(left), area);
    // Right: the message, else what is hidden and what waits for you.
    let right = match &app.message {
        Some((m, _)) => m.clone(),
        None => {
            let mut parts = Vec::new();
            let hidden = hidden_filters(app);
            if !hidden.is_empty() {
                parts.push(format!("hidden: {}", hidden.join(" · ")));
            }
            let n = app.to_check_in();
            if n > 0 {
                parts.push(format!("{n} to check in · C"));
            }
            if !app.pending.is_empty() {
                parts.push(format!("{} to approve · A", app.pending.len()));
            }
            if app.daemon_error.is_some() {
                parts.push("kairosd offline".into());
            }
            if !app.index.missing.is_empty() {
                parts.push(format!("{} calendar(s) not synced", app.index.missing.len()));
            }
            parts.join("   ")
        }
    };
    let room = area.width.saturating_sub(used + 2);
    if room > 4 && !right.is_empty() {
        let text = fit(&right, room as usize);
        let w = text.width() as u16 + 1;
        let r = Rect::new(area.x + area.width - w, area.y, w, 1);
        f.render_widget(
            Paragraph::new(TSpan::styled(format!("{text} "), Style::default().fg(c("peach")))),
            r,
        );
    }
}

/// What the digit filters hide, lowercase with its key (`careers 3`); a hidden Personal is said
/// once, not with each of its streams.
fn hidden_filters(app: &App) -> Vec<String> {
    let flags = overlay_flags(app);
    let personal_off = flags.iter().any(|f| f.1 == "Personal" && !f.2);
    flags
        .iter()
        .filter(|f| !f.2)
        .filter(|f| !(personal_off && f.4 == 1 && f.1 != "Personal"))
        .map(|f| format!("{} {}", f.1.to_lowercase(), f.0))
        .collect()
}
