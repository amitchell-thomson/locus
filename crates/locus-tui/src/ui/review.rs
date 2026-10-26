//! Search (`/` anywhere) and the weekly review (`W` from Home in its slot).
//!
//! Search covers decisions, answers, Explore, notes, the library, chats and dossiers; Enter says
//! where it would jump (19 Oct). Typing edits the query, but the results are the fixture's: the
//! query is not run until Slice 7.
//!
//! The review runs five steps in order of value, each skippable; it remembers where you stopped.

use crate::app::{App, nav};
use crate::screen::ScreenId;
use crate::ui::widgets::*;
use crate::ui::{frame, hints, lines, title, work};
use locus_contract::{HitSource, StepState};
use locus_theme::{SEP, c, glyph};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

pub const SEARCH_KEYS: &[(&str, &str)] = &[
    ("type", "the query"),
    ("⇥", "this project or everything"),
    ("⏎", "jump to the result"),
];
pub const KEYS: &[(&str, &str)] = &[
    ("← →", "previous or next step"),
    ("1-5", "a step"),
    ("s", "skip this step"),
];

fn source_name(s: HitSource) -> &'static str {
    match s {
        HitSource::Decisions => "decisions",
        HitSource::Answers => "answers",
        HitSource::Explore => "explore",
        HitSource::Notes => "notes",
        HitSource::Library => "library",
        HitSource::Chats => "chats",
        HitSource::Dossiers => "dossiers",
    }
}

pub fn draw_search(f: &mut Frame, area: Rect, app: &App) {
    let s = &app.fixtures.search;
    let scope = if app.scope_all {
        "everything".to_string()
    } else {
        app.project_id().unwrap_or_else(|| "everything".into())
    };
    let t = Line::from(vec![
        Span::styled(" / ", bold(keyst())),
        Span::styled(app.query.clone(), bold(text())),
        Span::styled("▏ ", keyst()),
    ]);
    let right = Line::from(vec![
        Span::styled(format!(" scope: {scope} "), dim()),
        Span::styled("⇥ ", keyst()),
    ]);
    let inner = frame(f, area, t, Some(right));
    let [body, foot] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    let hits: Vec<_> = s
        .hits
        .iter()
        .filter(|h| app.scope_all || app.project_id().is_none() || h.project == app.project_id())
        .collect();
    let sel = app.sel(' ').min(hits.len().saturating_sub(1));
    let mut out = Vec::new();
    for (i, h) in hits.iter().enumerate() {
        let mut left = vec![
            pointer(i, Some(sel)),
            Span::styled(format!("{:<11}", source_name(h.source)), st("subtext0")),
        ];
        if let Some(p) = h.project.as_ref().filter(|p| !h.title.starts_with(p.as_str())) {
            left.push(Span::styled(format!("{p} "), dim()));
        }
        left.push(Span::styled(h.title.clone(), text()));
        let right = h.at.map(|t| format!("{} ", day(t.date_naive()))).unwrap_or_default();
        out.push(row(left, vec![Span::styled(right, dim())], body.width));
    }
    if hits.is_empty() {
        let msg = if app.query.is_empty() {
            "  type to search everything"
        } else {
            "  nothing found"
        };
        out.push(Line::from(Span::styled(msg, dim())));
    }
    if app.query != s.query && !s.hits.is_empty() {
        out.push(Line::from(""));
        out.push(Line::from(Span::styled(
            format!(
                "  (the prototype shows the results for \"{}\"; queries run from Slice 7)",
                s.query
            ),
            Style::default().fg(c("overlay0")),
        )));
    }
    lines(f, body, out);
    lines(
        f,
        foot,
        vec![hints(&[("⏎", "jump"), ("⇥", "scope"), ("esc", "back")]).right_aligned()],
    );
}

const STEPS: [&str; 5] = ["week", "decisions", "explore", "locus", "next week"];

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let fx = &app.fixtures;
    let r = &fx.review;
    let inner = frame(
        f,
        area,
        title("weekly review", &format!("{} to {}", date(r.from), date(r.to))),
        Some(Line::from(vec![
            Span::styled(" esc", keyst()),
            Span::styled(" pause ", dim()),
        ])),
    );
    let [steps_r, body, foot] =
        Layout::vertical([Constraint::Length(2), Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    let cur = app.review_step.clamp(1, 5);
    let mut spans = vec![Span::raw(" ")];
    for (i, name) in STEPS.iter().enumerate() {
        let state = r.steps.get(i).copied().unwrap_or(StepState::Todo);
        let (mark, col) = match state {
            StepState::Done => (glyph::DONE, "green"),
            StepState::Skipped => ("–", "overlay1"),
            _ if i + 1 == cur => (glyph::ATTENDED, "blue"),
            _ => (glyph::UNATTENDED, "overlay1"),
        };
        if i > 0 {
            spans.push(Span::styled(" ── ", st("surface2")));
        }
        let style = if i + 1 == cur { bold(st("blue")) } else { st(col) };
        spans.push(Span::styled(format!("{} {name} {mark}", i + 1), style));
    }
    lines(f, steps_r, vec![Line::from(spans)]);
    let w = usize::from(body.width);
    let mut out = Vec::new();
    match cur {
        1 => {
            let grid = Rect { height: 9, ..body };
            work::draw_week(f, grid, app, false);
            let below = Rect {
                y: body.y + 10,
                height: body.height.saturating_sub(10),
                ..body
            };
            let wk = &r.week;
            lines(
                f,
                below,
                vec![
                    Line::from(Span::styled(
                        format!(
                            " blocks kept on their project   {} of {}",
                            wk.blocks_kept, wk.blocks_planned
                        ),
                        text(),
                    )),
                    Line::from(Span::styled(
                        format!(" budget                         {:.1}% of 15%", wk.budget_pct),
                        text(),
                    )),
                    Line::from(Span::styled(
                        format!(
                            " dossiers rebuilt               {}",
                            if wk.dossiers_rebuilt.is_empty() {
                                "none".into()
                            } else {
                                wk.dossiers_rebuilt.join(", ")
                            }
                        ),
                        text(),
                    )),
                ],
            );
            return draw_foot(f, foot);
        }
        2 => {
            out.push(rule(
                "2 decisions worth re-checking",
                &r.rechecks.len().to_string(),
                None,
                true,
                body.width,
            ));
            for rc in &r.rechecks {
                out.push(Line::from(""));
                out.push(row(
                    vec![
                        Span::styled(format!(" {} ", glyph::RECHECK), st("yellow")),
                        Span::styled(format!("{}{SEP}{} {}", rc.project, rc.decision, rc.title), bold(text())),
                    ],
                    vec![Span::styled(format!("agreed {} ", date(rc.agreed)), dim())],
                    body.width,
                ));
                for (i, l) in wrap(&rc.flagged, w.saturating_sub(14)).into_iter().enumerate() {
                    out.push(Line::from(vec![
                        Span::styled(format!("   {:<10}", if i == 0 { "flagged" } else { "" }), dim()),
                        Span::styled(l, text()),
                    ]));
                }
                if let Some(e) = &rc.effect {
                    out.push(Line::from(vec![
                        Span::styled(format!("   {:<10}", "effect"), dim()),
                        Span::styled(e.clone(), text()),
                    ]));
                }
                out.push(Line::from(""));
                out.push(Line::from(
                    [
                        vec![Span::raw("   ")],
                        hints(&[("w", "ask why it was chosen"), ("o", "reopen"), ("→", "next")]).spans,
                    ]
                    .concat(),
                ));
            }
            if r.rechecks.is_empty() {
                out.push(Line::from(Span::styled("  nothing to re-check this week", dim())));
            }
        }
        3 => {
            let ex = &r.explore;
            out.push(rule(
                "3 explore",
                &format!("{} left in the queue", ex.left),
                None,
                true,
                body.width,
            ));
            out.push(Line::from(""));
            if let Some(b) = &ex.best_idea {
                out.push(Line::from(vec![
                    Span::styled(format!(" {} best idea of the fortnight  ", glyph::IDEA), st("yellow")),
                    Span::styled(b.title.clone(), bold(text())),
                ]));
                for l in wrap(&b.summary, w.saturating_sub(4)) {
                    out.push(Line::from(Span::styled(format!("   {l}"), text())));
                }
                for l in wrap(&format!("why: {}", b.why), w.saturating_sub(4)) {
                    out.push(Line::from(Span::styled(format!("   {l}"), dim())));
                }
                out.push(Line::from(
                    [
                        vec![Span::raw("   ")],
                        hints(&[("p", "park"), ("c", "discuss"), ("x", "drop")]).spans,
                    ]
                    .concat(),
                ));
                out.push(Line::from(""));
            }
            out.push(Line::from(Span::styled(
                format!(
                    " then the {} finds still in the queue, graded in turn, as on the Explore page",
                    ex.left
                ),
                dim(),
            )));
            for u in &ex.untouched {
                out.push(Line::from(vec![
                    Span::styled(format!(" {} ", glyph::READING), st("teal")),
                    Span::styled(format!("untouched for three weeks: {u}"), text()),
                    Span::styled("   k keep · x take it off the tablet", dim()),
                ]));
            }
        }
        4 => {
            let lo = &r.locus;
            out.push(rule("4 locus itself", "", None, true, body.width));
            out.push(Line::from(""));
            out.push(Line::from(vec![
                Span::styled(" what did locus get wrong this week? › ", text()),
                Span::styled("_", keyst()),
            ]));
            out.push(Line::from(""));
            for u in &lo.unused {
                out.push(row(
                    vec![Span::styled(
                        format!(" you never opened {u} this week. Keep it?"),
                        text(),
                    )],
                    vec![Span::styled("y n ", keyst())],
                    body.width,
                ));
            }
            for s in &lo.strikes {
                out.push(Line::from(Span::styled(format!(" strikes: {s}"), text())));
            }
            if !lo.orphans.is_empty() {
                out.push(Line::from(""));
                out.push(Line::from(Span::styled(
                    format!(" captured with no area, {}: where does each belong?", lo.orphans.len()),
                    st("peach"),
                )));
                for o in &lo.orphans {
                    let (g, gc) = item_glyph(o.kind);
                    out.push(Line::from(vec![
                        Span::styled(format!("   {g} "), st(gc)),
                        Span::styled(o.text.clone(), text()),
                        Span::styled(format!("{SEP}{}", day(o.at.date_naive())), dim()),
                    ]));
                }
            }
        }
        _ => {
            out.push(rule("5 next week", "deep blocks", None, true, body.width));
            out.push(Line::from(""));
            let mut spans = vec![Span::styled(" deep blocks   ", text())];
            for b in &r.next_week {
                spans.push(Span::styled(
                    format!("{} ", b.project),
                    Style::default().fg(project_colour(&fx.registry, &b.project)),
                ));
                spans.push(Span::styled(
                    format!("{} {}     ", "━".repeat(b.blocks as usize * 2), b.blocks),
                    Style::default().fg(project_colour(&fx.registry, &b.project)),
                ));
            }
            if r.next_week.is_empty() {
                spans.push(Span::styled("none proposed", dim()));
            }
            out.push(Line::from(spans));
            out.push(Line::from(""));
            out.push(Line::from(
                [
                    vec![Span::raw("               ")],
                    hints(&[("← →", "project"), ("↑ ↓", "blocks"), ("⏎", "plan them in kairos")]).spans,
                ]
                .concat(),
            ));
        }
    }
    lines(f, body, out);
    draw_foot(f, foot);
}

fn draw_foot(f: &mut Frame, foot: Rect) {
    lines(
        f,
        foot,
        vec![hints(&[("← →", "steps"), ("s", "skip"), ("esc", "pause")]).right_aligned()],
    );
}

pub fn key(app: &mut App, k: KeyEvent) -> bool {
    if app.screen() == ScreenId::Search {
        let n = app.fixtures.search.hits.len();
        if let KeyCode::Up | KeyCode::Down = k.code
            && let Some(d) = nav(&k)
        {
            app.move_sel(' ', n, d);
            return true;
        }
        match k.code {
            KeyCode::Tab => {
                app.scope_all = !app.scope_all;
                app.sel.insert((ScreenId::Search, ' '), 0);
            }
            KeyCode::Enter => {
                if let Some(h) = app.fixtures.search.hits.get(app.sel(' ')) {
                    app.note = Some(format!("would jump to: {}", h.jump));
                }
            }
            KeyCode::Backspace => {
                app.query.pop();
            }
            KeyCode::Char(c) if !k.modifiers.contains(ratatui::crossterm::event::KeyModifiers::CONTROL) => {
                app.query.push(c)
            }
            _ => return false,
        }
        return true;
    }
    match k.code {
        KeyCode::Right | KeyCode::Char('l') => app.review_step = (app.review_step + 1).min(5),
        KeyCode::Left | KeyCode::Char('h') => app.review_step = app.review_step.saturating_sub(1).max(1),
        KeyCode::Char(c @ '1'..='5') => app.review_step = c as usize - '0' as usize,
        KeyCode::Char('s') => {
            app.note = Some(format!("skipped step {}; nothing piles up", app.review_step));
            app.review_step = (app.review_step + 1).min(5);
        }
        // w why and c discuss (not ? and d, which mean keys and done everywhere: L4, 26 Oct).
        KeyCode::Char('w' | 'o' | 'p' | 'c' | 'x' | 'y' | 'n' | 'k') => {
            app.note = Some("would act on it; the review records your answer".into());
        }
        KeyCode::Enter if app.review_step == 5 => app.note = Some("would ask kairos to plan these blocks".into()),
        _ => return false,
    }
    true
}
