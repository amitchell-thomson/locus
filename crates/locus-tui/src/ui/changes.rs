//! Changes (`n` on the Project view): the project's whole timeline, newest first, in the shape
//! of the Decisions page (round 4, 23 Oct: "same shape as decisions would be good"). The timeline
//! down the left, grouped by day, with a line where you were last here ("a you were here last
//! would be good too, showing what is new since I last opened the page"); the selected entry in
//! full on the right. Routine items fold into one row, so they do not crowd out your own work
//! ("definitely fold routine items, they would crowd out actual interesting work by me").

use crate::app::App;
use crate::screen::ScreenId;
use crate::ui::widgets::*;
use crate::ui::{esc_back, frame, lines, title};
use chrono::NaiveDate;
use locus_contract::{Changes, Item, Time};
use locus_theme::{SEP, c, glyph};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

pub const KEYS: &[(&str, &str)] = &[
    ("↑ ↓", "walk the timeline; a folded routine opens on the right"),
    ("→ or ⏎", "read the entry; ↑ ↓ scroll it, ← back"),
    ("o", "open it where it lives"),
];

/// One row of the timeline: an entry, or a routine's entries folded together.
#[derive(Clone)]
enum Row {
    One(usize),
    Fold(String, Vec<usize>),
}

impl Row {
    /// The newest entry in the row, which places it in time.
    fn first(&self) -> usize {
        match self {
            Row::One(i) => *i,
            Row::Fold(_, v) => v[0],
        }
    }
}

fn log(app: &App) -> Option<&Changes> {
    let id = app.project_id()?;
    app.fixtures.projects.get(&id)?.changes.as_ref()
}

/// The timeline's rows: each routine folded at the place of its newest entry.
fn rows(ch: &Changes) -> Vec<Row> {
    let mut out: Vec<Row> = Vec::new();
    for (i, e) in ch.entries.iter().enumerate() {
        match &e.routine {
            Some(name) => {
                if let Some(Row::Fold(_, v)) = out.iter_mut().find(|r| matches!(r, Row::Fold(n, _) if n == name)) {
                    v.push(i);
                } else {
                    out.push(Row::Fold(name.clone(), vec![i]));
                }
            }
            None => out.push(Row::One(i)),
        }
    }
    out
}

fn is_new(ch: &Changes, e: &Item) -> bool {
    ch.last_seen.is_some_and(|t| e.at > t)
}

pub fn section_len(app: &App) -> usize {
    log(app).map_or(0, |c| rows(c).len())
}

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let now = app.fixtures.now();
    let name = app
        .project_id()
        .and_then(|id| app.fixtures.registry.get(&id).map(|p| p.name.clone()))
        .unwrap_or_else(|| "no project".into());
    let ch = log(app);
    let extra = ch
        .map(|c| {
            let n = c.entries.iter().filter(|e| is_new(c, e)).count();
            let since = format!("since {}", day(c.since));
            if n > 0 {
                format!("{n} new since you were here{SEP}{since}")
            } else {
                since
            }
        })
        .unwrap_or_default();
    let inner = frame(f, area, title(&format!("{name} · changes"), &extra), Some(esc_back()));
    let Some(ch) = ch.filter(|c| !c.entries.is_empty()) else {
        lines(
            f,
            inner,
            vec![
                Line::from(""),
                Line::from(Span::styled("   nothing recorded yet", dim())),
            ],
        );
        return;
    };
    let rs = rows(ch);
    let sel = app.sel(' ').min(rs.len() - 1);
    // Side by side when there is room, as on Decisions; stacked below that.
    let (list_r, div_r, entry_r) = if inner.width >= 120 {
        let lw = (inner.width * 9 / 20).clamp(40, 76);
        (
            Rect { width: lw, ..inner },
            Rect {
                x: inner.x + lw,
                width: 3,
                ..inner
            },
            Rect {
                x: inner.x + lw + 3,
                width: inner.width.saturating_sub(lw + 3),
                ..inner
            },
        )
    } else {
        let lh = inner.height / 2;
        (
            Rect { height: lh, ..inner },
            Rect {
                y: inner.y + lh,
                height: 1,
                ..inner
            },
            Rect {
                y: inner.y + lh + 1,
                height: inner.height.saturating_sub(lh + 1),
                ..inner
            },
        )
    };
    draw_list(f, list_r, app, ch, &rs, sel, now);
    let reading = app.focus == Some('b');
    let col = st(if reading { "blue" } else { "surface2" });
    if div_r.width == 3 {
        let side = if reading { " ┃ " } else { " │ " };
        lines(
            f,
            div_r,
            (0..div_r.height).map(|_| Line::from(Span::styled(side, col))).collect(),
        );
    } else {
        let bar = if reading { "━" } else { "─" };
        lines(
            f,
            div_r,
            vec![Line::from(Span::styled(bar.repeat(usize::from(div_r.width)), col))],
        );
    }
    draw_entry(f, entry_r, app, ch, &rs[sel], now);
}

fn draw_list(f: &mut Frame, area: Rect, app: &App, ch: &Changes, rs: &[Row], sel: usize, now: Time) {
    let reading = app.focus == Some('b');
    let w = area.width;
    // Every line drawn, with the row it belongs to, so the view can follow the selection.
    let mut out: Vec<(Option<usize>, Line<'static>)> = Vec::new();
    let mut last_day: Option<NaiveDate> = None;
    let mut marked = false;
    for (k, r) in rs.iter().enumerate() {
        let e = &ch.entries[r.first()];
        // The line where you were last here: everything above it is new.
        if !marked
            && let Some(t) = ch.last_seen
            && e.at <= t
        {
            marked = true;
            if k > 0 {
                let label = format!(" you were here, {} ", day(t.date_naive()));
                let fill = usize::from(w).saturating_sub(label.chars().count() + 4);
                out.push((
                    None,
                    Line::from(vec![
                        Span::styled("──", st("peach")),
                        Span::styled(label, bold(st("peach"))),
                        Span::styled("─".repeat(fill), st("peach")),
                    ]),
                ));
            }
        }
        let d = e.at.date_naive();
        let label = if last_day == Some(d) { String::new() } else { day(d) };
        last_day = Some(d);
        let (g, gc, text_, dimmed) = match r {
            Row::One(i) => {
                let e = &ch.entries[*i];
                let (g, gc) = item_glyph(e.kind);
                (g, gc, e.text.clone(), false)
            }
            Row::Fold(name, v) => {
                let (g, _) = item_glyph(ch.entries[v[0]].kind);
                (g, "overlay1", format!("{name} · {} runs", v.len()), true)
            }
        };
        let mut right = vec![];
        if is_new(ch, e) {
            right.push(Span::styled(format!("{} ", glyph::UNREAD), st("peach")));
        }
        right.push(Span::styled(format!("{:>4} ", ago(now, e.at)), dim()));
        let showing = reading && k == sel;
        let lead = if showing {
            Span::styled("▌ ", st("blue"))
        } else {
            pointer(k, Some(sel))
        };
        let line = row(
            vec![
                lead,
                Span::styled(format!("{label:<8}"), dim()),
                Span::styled(format!("{g} "), st(gc)),
                Span::styled(text_, if dimmed { dim() } else { text() }),
            ],
            right,
            w,
        );
        out.push((
            Some(k),
            if showing {
                line.patch_style(Style::default().bg(c("surface0")))
            } else {
                line
            },
        ));
    }
    // Scroll so the selected row stays in view, a few rows of context above it.
    let h = usize::from(area.height);
    let at = out.iter().position(|(k, _)| *k == Some(sel)).unwrap_or(0);
    let first = at.saturating_sub(h.saturating_sub(4)).min(out.len().saturating_sub(h));
    lines(f, area, out.into_iter().skip(first).take(h).map(|(_, l)| l).collect());
}

/// The entry in full: what it was, when, and its detail; a folded routine lists its runs.
fn draw_entry(f: &mut Frame, area: Rect, app: &App, ch: &Changes, r: &Row, now: Time) {
    let reading = app.focus == Some('b');
    let w = usize::from(area.width).saturating_sub(4);
    let mut out: Vec<Line<'static>> = Vec::new();
    let head_style = bold(st(if reading { "blue" } else { "text" }));
    match r {
        Row::One(i) => {
            let e = &ch.entries[*i];
            let (g, gc) = item_glyph(e.kind);
            for l in wrap(&e.text, w) {
                out.push(Line::from(vec![Span::raw("  "), Span::styled(l, head_style)]));
            }
            let mut meta = vec![
                Span::raw("  "),
                Span::styled(format!("{g} "), st(gc)),
                Span::styled(format!("{} {}", day(e.at.date_naive()), e.at.format("%H:%M")), dim()),
                Span::styled(format!("{SEP}{} ago", ago(now, e.at)), dim()),
            ];
            if is_new(ch, e) {
                meta.push(Span::styled(
                    format!("{SEP}{} new since you were here", glyph::UNREAD),
                    st("peach"),
                ));
            }
            out.push(Line::from(meta));
            out.push(Line::from(""));
            if e.detail.is_empty() {
                out.push(Line::from(Span::styled("  nothing more recorded", dim())));
            }
            for (k, p) in e.detail.iter().enumerate() {
                if k > 0 {
                    out.push(Line::from(""));
                }
                for l in wrap(p, w) {
                    out.push(Line::from(vec![Span::raw("  "), Span::styled(l, text())]));
                }
            }
        }
        Row::Fold(name, v) => {
            out.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(name.clone(), head_style),
            ]));
            out.push(Line::from(Span::styled(
                format!(
                    "  routine · {} runs, folded so they do not crowd out your work",
                    v.len()
                ),
                dim(),
            )));
            out.push(Line::from(""));
            for &i in v {
                let e = &ch.entries[i];
                out.push(row(
                    vec![
                        Span::styled(format!("  {:<8}", day(e.at.date_naive())), dim()),
                        Span::styled(e.text.clone(), text()),
                    ],
                    vec![],
                    area.width,
                ));
            }
        }
    }
    let h = usize::from(area.height);
    let max_scroll = out.len().saturating_sub(h);
    app.shown
        .borrow_mut()
        .insert((ScreenId::Changes, 'b'), vec![max_scroll]);
    let scroll = if reading { app.sel('b').min(max_scroll) } else { 0 };
    let mut view: Vec<Line<'static>> = out.into_iter().skip(scroll).take(h).collect();
    if scroll < max_scroll
        && let Some(l) = view.last_mut()
    {
        *l = Line::from(Span::styled(
            if reading {
                "  ↓ more"
            } else {
                "  ↓ more · → to read on"
            },
            dim(),
        ));
    }
    lines(f, area, view);
}

pub fn key(app: &mut App, k: KeyEvent) -> bool {
    let Some(ch) = log(app).cloned() else { return false };
    let rs = rows(&ch);
    if app.focus == Some('b') {
        let max = app
            .shown
            .borrow()
            .get(&(ScreenId::Changes, 'b'))
            .and_then(|v| v.first().copied())
            .unwrap_or(0);
        if let Some(d) = crate::app::nav(&k) {
            let at = (app.sel('b') as isize + d).clamp(0, max as isize) as usize;
            app.sel.insert((ScreenId::Changes, 'b'), at);
            return true;
        }
        if k.code == KeyCode::Left {
            app.focus = None;
            return true;
        }
    }
    if let Some(d) = crate::app::nav(&k) {
        app.move_sel(' ', rs.len(), d);
        return true;
    }
    match k.code {
        KeyCode::Right | KeyCode::Enter if app.focus.is_none() => {
            app.focus = Some('b');
            app.sel.insert((ScreenId::Changes, 'b'), 0);
            true
        }
        KeyCode::Char('o') => {
            if let Some(r) = rs.get(app.sel(' ')) {
                let e = &ch.entries[r.first()];
                app.note = Some(format!("would open where it lives: {}", e.text));
            }
            true
        }
        _ => false,
    }
}
