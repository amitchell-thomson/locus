//! Drawing. Theme: Catppuccin Mocha, transparent background (Color::Reset),
//! rounded borders, blue focus / mauve popups, ` · ` separators, `…` truncation.

use crate::app::{App, Confirm, Mode, PromptKind, View, is_deadline, is_dimmed, overlay_flags};
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

fn colour_of(app: &App, i: &Instance) -> Color {
    let r = match i.overlay {
        Overlay::Background => app.cfg.overlays.background_colour,
        Overlay::Imported => app.cfg.overlays.imported_colour,
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
    }
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
    let inner_w = area.width.saturating_sub(2);
    let gutter: u16 = 6;
    let ndays: usize = if single {
        1
    } else if inner_w >= gutter + 7 * 13 {
        7
    } else if inner_w >= gutter + 3 * 12 {
        3
    } else {
        1
    };
    let first = match ndays {
        7 => app.cursor - Duration::days(i64::from(app.cursor.weekday().num_days_from_monday())),
        3 => app.cursor - Duration::days(1),
        _ => app.cursor,
    };
    let last = first + Duration::days(ndays as i64 - 1);
    let title = if single {
        app.cursor.format("%A %-d %B").to_string()
    } else {
        format!(
            "Week {} · {}–{}",
            first.iso_week().week(),
            first.format("%-d %b"),
            last.format("%-d %b")
        )
    };
    let block = panel(&title, true);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height < 4 || inner.width <= gutter {
        return;
    }
    let buf = f.buffer_mut();
    let col_w = ((inner.width - gutter) as usize / ndays).max(1) as u16;
    let days: Vec<NaiveDate> = (0..ndays).map(|k| first + Duration::days(k as i64)).collect();
    let per_day: Vec<Vec<Cluster>> = days.iter().map(|d| app.day_clusters(*d)).collect();
    let ghosts = app.ghosts();
    let today = app.today();

    // Header.
    let y = inner.y;
    for (k, d) in days.iter().enumerate() {
        let x = inner.x + gutter + col_w * k as u16;
        let label = if col_w >= 10 {
            d.format("%a %-d").to_string()
        } else {
            d.format("%-d").to_string()
        };
        let style = if *d == today {
            Style::default()
                .fg(c("base"))
                .bg(c("blue"))
                .add_modifier(Modifier::BOLD)
        } else if *d == app.cursor {
            Style::default()
                .fg(c("mauve"))
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::default().fg(c("subtext0"))
        };
        put(buf, x, y, col_w.saturating_sub(1), &format!(" {label} "), style);
    }

    // All-day strip (incl. all-day deadlines and multi-day spans).
    let strip_rows = per_day
        .iter()
        .map(|v| v.iter().filter(|c| c.primary.span.is_all_day()).count())
        .max()
        .unwrap_or(0)
        .min(if single { 4 } else { 2 });
    for (k, d) in days.iter().enumerate() {
        let x = inner.x + gutter + col_w * k as u16;
        let all: Vec<&Cluster> = per_day[k].iter().filter(|c| c.primary.span.is_all_day()).collect();
        for (r, cl) in all.iter().enumerate().take(strip_rows) {
            let i = &cl.primary;
            let more = all.len() > strip_rows && r + 1 == strip_rows;
            let mut text = if more {
                format!("+{} more", all.len() - r)
            } else {
                label(i)
            };
            if cl.is_merged() && !more {
                text.push_str(" ⧉");
            }
            let selected = *d == app.cursor
                && app.view != View::Agenda
                && per_day[k].iter().position(|x| std::ptr::eq(x, *cl)) == Some(app.sel);
            let mut style = Style::default().fg(if is_deadline(i) { c("red") } else { colour_of(app, i) });
            if is_dimmed(i) {
                style = style.add_modifier(Modifier::DIM);
            }
            if selected {
                style = Style::default()
                    .fg(c("base"))
                    .bg(c("blue"))
                    .add_modifier(Modifier::BOLD);
            }
            put(
                buf,
                x,
                y + 1 + r as u16,
                col_w.saturating_sub(1),
                &format!("▎{text}"),
                style,
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
    let mut h0 = 7 * 60;
    let mut h1 = 23 * 60;
    for v in &per_day {
        for cl in v {
            if let Span::Timed { start, end } = cl.primary.span {
                let s = start.with_timezone(&tz);
                let e = end.with_timezone(&tz);
                if s.date_naive() == e.date_naive() {
                    h0 = h0.min(s.hour() as i64 * 60);
                    h1 = h1.max((e.hour() as i64 * 60 + i64::from(e.minute() > 0) * 60).min(24 * 60));
                }
            }
        }
    }
    let span = (h1 - h0).max(60);
    let nrows = rows as usize;
    // Continuous mapping: minutes → row, using every available row.
    let row_of_min = |m: i64| -> i64 { ((m - h0) * nrows as i64).div_euclid(span) };
    let row_of = |t: DateTime<Utc>, d: NaiveDate| -> i64 {
        let l = t.with_timezone(&tz);
        let mins = if l.date_naive() < d {
            0
        } else if l.date_naive() > d {
            24 * 60
        } else {
            l.hour() as i64 * 60 + l.minute() as i64
        };
        row_of_min(mins)
    };
    let mut last_label: i64 = -2;
    for r in 0..nrows {
        for k in 0..ndays {
            let x = inner.x + gutter + col_w * k as u16;
            buf.set_string(x, grid_y + r as u16, "┊", Style::default().fg(c("surface0")));
        }
    }
    let mut hour = (h0 + 59) / 60 * 60;
    while hour < h1 {
        let r = row_of_min(hour);
        if r > last_label + 1 && (0..nrows as i64).contains(&r) {
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
        let x0 = inner.x + gutter + col_w * k as u16 + 1;
        let w = col_w.saturating_sub(2);
        let timed: Vec<(usize, usize, &Cluster)> = per_day[k]
            .iter()
            .filter(|c| !c.primary.span.is_all_day())
            .filter_map(|cl| {
                let (s, e) = cl.primary.span.bounds(tz);
                let r0 = row_of(s, *d).clamp(0, nrows as i64 - 1) as usize;
                let r1 = (row_of(e - Duration::seconds(1), *d) + 1).clamp(r0 as i64 + 1, nrows as i64) as usize;
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
            let selected = *d == app.cursor && idx == Some(app.sel);
            let col = colour_of(app, i);
            let mut style = Style::default().fg(col);
            if is_dimmed(i) {
                style = style.add_modifier(Modifier::DIM);
            }
            if selected {
                style = Style::default()
                    .fg(c("base"))
                    .bg(c("blue"))
                    .add_modifier(Modifier::BOLD);
            }
            let t = i.start(tz).with_timezone(&tz);
            let mut first_line = if lane_w >= 12 {
                format!("{} {}", t.format("%H:%M"), label(i))
            } else {
                label(i)
            };
            if p.cl.is_merged() {
                first_line.push_str(" ⧉");
            }
            put(
                buf,
                x,
                grid_y + p.r0 as u16,
                lane_w.saturating_sub(1),
                &format!("▌{first_line}"),
                style,
            );
            for r in p.r0 + 1..p.r1 {
                let body = if i.meta.kind == EventKind::TaskBlock {
                    format!("▌{}", "░".repeat(lane_w.saturating_sub(2) as usize))
                } else if r == p.r0 + 1 && single {
                    format!("▌{}", i.location.clone().unwrap_or_default())
                } else {
                    "▌".to_string()
                };
                let st = if selected {
                    style
                } else {
                    Style::default().fg(col).add_modifier(Modifier::DIM)
                };
                put(buf, x, grid_y + r as u16, lane_w.saturating_sub(1), &body, st);
            }
        }
        // Timed deadlines: a ⚑ marker line across the column.
        for (r0, _, cl) in timed.iter().filter(|t| is_deadline(&t.2.primary)) {
            let i = &cl.primary;
            let t = i.start(tz).with_timezone(&tz).format("%H:%M");
            put(
                buf,
                x0,
                grid_y + *r0 as u16,
                w,
                &format!(
                    "⚑ {t} {}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━",
                    i.title.trim_start_matches('⚑').trim()
                ),
                Style::default().fg(c("red")).add_modifier(Modifier::BOLD),
            );
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
            let r1 = (row_of(e - Duration::seconds(1), *d) + 1).clamp(r0 + 1, nrows as i64);
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
                let y = grid_y + r as u16;
                let xx = inner.x + gutter + col_w * k as u16;
                buf.set_string(xx, y, "●", Style::default().fg(c("peach")));
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
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(TSpan::styled(
        "a add · d done · x drop · P plan · Esc back",
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
    for op in &cs.ops {
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
        ("p f b i", "toggle Personal, Family, background, imported"),
        ("1–7", "toggle keys"),
        ("a", "add (title @ when [#key] [for 90m])"),
        ("e m K", "rename, move, re-key"),
        ("d s", "delete, share to Family"),
        ("B", "Family event: block / unblock my time"),
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
    push(buf, &mut x, " ", Style::default().bg(c("surface0")));
    for (name, on, col) in overlay_flags(app) {
        let st = if on {
            Style::default()
                .fg(rgb(col))
                .bg(c("surface0"))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(c("surface2")).bg(c("surface0"))
        };
        push(buf, &mut x, &name, st);
        push(buf, &mut x, " ", Style::default().bg(c("surface0")));
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
                parts.push(format!("⏳ {} pending", app.pending.len()));
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
