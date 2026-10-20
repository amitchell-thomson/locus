//! Explore (C.7): the scout's queue graded with reasons, your ideas, and what is on the tablet in
//! /Reading. Every card carries its reasoning; a reading find goes to the tablet only once
//! ticked.

use crate::app::{App, Mark, Mode, nav};
use crate::ui::home::{find_key, tick};
use crate::ui::widgets::*;
use crate::ui::{esc_back, frame, hints, lines, title};
use locus_contract::{IdeaFrom, IdeaState, ReadingState};
use locus_theme::{SEP, glyph};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};

pub const KEYS: &[(&str, &str)] = &[
    ("y", "tick: keep it (reading goes to the tablet)"),
    ("n", "strike, then a reason"),
    ("p", "park on the board"),
    ("v", "move to reading"),
];

fn tab(app: &App) -> char {
    // Explore's only sections are its three tabs.
    app.focus.unwrap_or('q')
}

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let ex = &app.fixtures.explore;
    let summary = format!(
        "queue {} of {}{SEP}ideas {}{SEP}reading {} of {}",
        ex.queue.len(),
        ex.queue_cap,
        ex.ideas.len(),
        ex.reading.len(),
        ex.reading_cap
    );
    let inner = frame(f, area, title("explore", &summary), Some(esc_back()));
    let [tabs_r, body, foot] =
        Layout::vertical([Constraint::Length(2), Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    let t = tab(app);
    let mut tabs = vec![Span::raw(" ")];
    for (c, name) in [('q', "queue"), ('i', "ideas"), ('r', "reading")] {
        let s = if c == t { bold(keyst()) } else { dim() };
        tabs.push(Span::styled(format!("[{c}] {name}   "), s));
    }
    if ex.queue.len() as u32 >= ex.queue_cap {
        tabs.push(Span::styled("the queue is full: the scout waits", st("peach")));
    }
    lines(f, tabs_r, vec![Line::from(tabs)]);
    match t {
        'i' => draw_ideas(f, body, app),
        'r' => draw_reading(f, body, app),
        _ => draw_queue(f, body, app),
    }
    let h = match t {
        'q' => hints(&[("y", "✓"), ("n", "✗ then a reason"), ("p", "park"), ("v", "to reading")]),
        'i' => hints(&[("⏎", "discuss"), ("p", "park")]),
        _ => hints(&[("⏎", "open on the tablet")]),
    };
    lines(f, foot, vec![h.right_aligned()]);
}

fn draw_queue(f: &mut Frame, area: Rect, app: &App) {
    let ex = &app.fixtures.explore;
    if ex.queue.is_empty() {
        lines(
            f,
            area,
            vec![Line::from(Span::styled(
                "  nothing waiting; about one find a day",
                dim(),
            ))],
        );
        return;
    }
    let [list, card] = Layout::horizontal([Constraint::Length(34), Constraint::Min(0)]).areas(area);
    let sel = app.sel('q').min(ex.queue.len() - 1);
    let mut out = Vec::new();
    for (i, find) in ex.queue.iter().enumerate() {
        let (g, gc, _) = find_glyph(find.kind);
        let style = match app.marked(&find_key(find)) {
            Some(Mark::Struck(_)) => struck(text()),
            Some(Mark::Ticked) => st("green"),
            Some(Mark::Parked) => dim(),
            _ => text(),
        };
        out.push(Line::from(vec![
            pointer(i, Some(sel)),
            Span::styled(format!("{g} "), st(gc)),
            Span::styled(crate::ui::truncate(&find.title, 29), style),
        ]));
    }
    lines(f, list, out);

    let find = &ex.queue[sel];
    let (g, gc, kind) = find_glyph(find.kind);
    let w = usize::from(card.width).saturating_sub(12);
    let head = match &find.project {
        Some(p) => format!("{g} for {p}{SEP}{kind}"),
        None => format!("{g} {kind}"),
    };
    let mut out = vec![
        Line::from(vec![
            Span::raw("│ "),
            Span::styled(head, st(gc)),
            Span::styled(format!("{SEP}{}", day(find.at.date_naive())), dim()),
        ]),
        Line::from("│"),
    ];
    for l in wrap(&find.title, w + 9) {
        out.push(Line::from(vec![Span::raw("│ "), Span::styled(l, bold(text()))]));
    }
    out.push(Line::from("│"));
    for l in wrap(&find.summary, w + 9) {
        out.push(Line::from(vec![Span::raw("│ "), Span::styled(l, text())]));
    }
    out.push(Line::from("│"));
    let mut field = |label: &str, v: &str| {
        for (i, l) in wrap(v, w).into_iter().enumerate() {
            out.push(Line::from(vec![
                Span::raw("│ "),
                Span::styled(format!("{:<9}", if i == 0 { label } else { "" }), dim()),
                Span::styled(l, text()),
            ]));
        }
    };
    field("why", &find.why);
    if let Some(d) = &find.differs {
        field("differs", d);
    }
    if let Some(t) = &find.touches {
        field("touches", t);
    }
    field("cost", &format!("about {} minutes", find.minutes));
    if let Some(m) = app.marked(&find_key(find)) {
        out.push(Line::from("│"));
        let s = match m {
            Mark::Struck(r) => format!("struck: {r}"),
            Mark::Ticked => "ticked".into(),
            Mark::Parked => "parked on the board".into(),
            _ => String::new(),
        };
        out.push(Line::from(vec![Span::raw("│ "), Span::styled(s, st("peach"))]));
    }
    if matches!(app.mode, Mode::Strike(_)) {
        out.push(Line::from("│"));
        out.push(Line::from(vec![
            Span::raw("│ "),
            Span::styled("why not? i irrelevant · k knew it · l level · t too long", st("peach")),
        ]));
    }
    lines(f, card, out);
}

fn draw_ideas(f: &mut Frame, area: Rect, app: &App) {
    let ex = &app.fixtures.explore;
    let now = app.fixtures.now();
    let sel = app.sel('i');
    let mut out = Vec::new();
    for (i, idea) in ex.ideas.iter().enumerate() {
        let from = match idea.from {
            IdeaFrom::You => "you",
            IdeaFrom::Jot => "jot",
            IdeaFrom::Scout => "scout",
        };
        let state = match idea.state {
            IdeaState::Open => "open",
            IdeaState::Handed => "with the scout",
            IdeaState::Parked => "parked",
        };
        let mut left = vec![
            pointer(i, Some(sel)),
            Span::styled(format!("{} ", glyph::IDEA), st("yellow")),
            Span::styled(format!("{:<6}", from), dim()),
            Span::styled(idea.title.clone(), text()),
        ];
        if let Some(p) = &idea.project {
            left.push(Span::styled(format!("{SEP}{p}"), dim()));
        }
        if idea.best_of_fortnight {
            left.push(Span::styled("  best of the fortnight", st("peach")));
        }
        out.push(row(
            left,
            vec![Span::styled(format!("{state:<15}{:>4} ", ago(now, idea.at)), dim())],
            area.width,
        ));
    }
    if ex.ideas.is_empty() {
        out.push(Line::from(Span::styled(
            "  no ideas yet · Jot or Alt-j adds one",
            dim(),
        )));
    }
    lines(f, area, out);
}

fn draw_reading(f: &mut Frame, area: Rect, app: &App) {
    let ex = &app.fixtures.explore;
    let now = app.fixtures.now().date_naive();
    let sel = app.sel('r');
    let mut out = Vec::new();
    for (i, r) in ex.reading.iter().enumerate() {
        let (state, col) = match r.state {
            ReadingState::Untouched => ("untouched", "peach"),
            ReadingState::InProgress => ("reading", "blue"),
            ReadingState::Done => ("done", "green"),
        };
        let weeks = (now - r.since).num_weeks();
        let mut right = state.to_string();
        if let Some(p) = &r.progress {
            right = format!("{p}{SEP}{state}");
        }
        if r.state == ReadingState::Untouched && weeks >= 3 {
            right.push_str(&format!("{SEP}{weeks} weeks: the review asks"));
        }
        out.push(row(
            vec![
                pointer(i, Some(sel)),
                Span::styled(format!("{} ", glyph::READING), st("teal")),
                Span::styled(r.title.clone(), text()),
            ],
            vec![Span::styled(format!("{right} "), st(col))],
            area.width,
        ));
    }
    if ex.reading.is_empty() {
        out.push(Line::from(Span::styled("  nothing on the tablet in /Reading", dim())));
    }
    lines(f, area, out);
}

pub fn key(app: &mut App, k: KeyEvent) -> bool {
    let t = tab(app);
    let len = match t {
        'i' => app.fixtures.explore.ideas.len(),
        'r' => app.fixtures.explore.reading.len(),
        _ => app.fixtures.explore.queue.len(),
    };
    if let Some(d) = nav(&k) {
        app.move_sel(t, len, d);
        return true;
    }
    let i = app.sel(t);
    match (t, k.code) {
        ('q', KeyCode::Char('y')) => {
            if let Some(find) = app.fixtures.explore.queue.get(i).cloned() {
                tick(app, &find);
            }
        }
        ('q', KeyCode::Char('n')) => {
            if let Some(find) = app.fixtures.explore.queue.get(i) {
                app.mode = Mode::Strike(find.id.clone());
                app.note = Some("why? i irrelevant · k knew it · l level · t too long".into());
            }
        }
        ('q', KeyCode::Char('p')) => {
            if let Some(find) = app.fixtures.explore.queue.get(i) {
                let k = find_key(find);
                app.mark(k, Mark::Parked, "parked on the board; it leaves the queue");
            }
        }
        ('q', KeyCode::Char('v')) => {
            if let Some(find) = app.fixtures.explore.queue.get(i) {
                app.note = Some(format!("would move \"{}\" to reading; it waits for a tick", find.title));
            }
        }
        ('i', KeyCode::Enter) => {
            if let Some(idea) = app.fixtures.explore.ideas.get(i) {
                app.note = Some(format!("would open a chat to discuss \"{}\"", idea.title));
            }
        }
        ('r', KeyCode::Enter) => {
            if let Some(r) = app.fixtures.explore.reading.get(i) {
                app.note = Some(format!("\"{}\" is on the tablet in /Reading", r.title));
            }
        }
        _ => return false,
    }
    true
}
