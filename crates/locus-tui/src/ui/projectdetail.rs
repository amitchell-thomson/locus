//! The screens a Project view opens: Decisions (`d`), Answered (`a`), Changes (`n n`).
//! Each is a list on top and the selected entry explained below.

use crate::app::{App, nav};
use crate::screen::ScreenId;
use crate::ui::widgets::*;
use crate::ui::{esc_back, frame, hints, lines, title};
use locus_contract::{AskedFrom, DecisionStatus};
use locus_theme::{SEP, glyph};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};

pub const DECISION_KEYS: &[(&str, &str)] = &[
    ("?", "ask why it was made (lands in Answered)"),
    ("o", "reopen it in the log"),
    ("c", "put it into the project chat as context"),
    ("⏎", "the full entry"),
];
pub const ANSWER_KEYS: &[(&str, &str)] = &[
    ("f", "follow up in the same thread"),
    ("t", "send to the tablet as an explainer"),
    ("c", "open as a session in the project window"),
];
pub const CHANGE_KEYS: &[(&str, &str)] = &[("⏎", "open the entry where it lives")];

fn project_name(app: &App) -> String {
    app.project_id()
        .and_then(|id| app.fixtures.registry.get(&id).map(|p| p.name.clone()))
        .unwrap_or_else(|| "no project".into())
}

fn none(f: &mut Frame, area: Rect, what: &str) {
    lines(
        f,
        area,
        vec![Line::from(""), Line::from(Span::styled(format!("  {what}"), dim()))],
    );
}

pub fn draw_decisions(f: &mut Frame, area: Rect, app: &App) {
    let pf = app.project_id().and_then(|id| app.fixtures.projects.get(&id));
    let ds = pf.and_then(|p| p.decisions.as_ref());
    let counts = ds
        .map(|d| {
            let n = |f: fn(&DecisionStatus) -> bool| d.items.iter().filter(|x| f(&x.status)).count();
            format!(
                "{} re-check{SEP}{} holding{SEP}{} re-decided",
                n(|s| *s == DecisionStatus::Recheck),
                n(|s| *s == DecisionStatus::Holding),
                n(|s| matches!(s, DecisionStatus::Redecided { .. }))
            )
        })
        .unwrap_or_default();
    let inner = frame(
        f,
        area,
        title(&format!("{} · decisions", project_name(app)), &counts),
        Some(esc_back()),
    );
    let Some(ds) = ds else {
        none(f, inner, "this project has no decision log");
        return;
    };
    if ds.items.is_empty() {
        none(f, inner, &format!("no decisions in {} yet", ds.log));
        return;
    }
    let list_h = (ds.items.len() as u16).min(10);
    let [list, sep, detail, foot] = Layout::vertical([
        Constraint::Length(list_h),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(inner);
    let sel = app.sel(' ').min(ds.items.len() - 1);
    // Re-checks first, then by date, newest first.
    let first = sel.saturating_sub(usize::from(list_h).saturating_sub(1));
    let mut out = Vec::new();
    for (i, d) in ds.items.iter().enumerate().skip(first).take(usize::from(list_h)) {
        let (g, gc, status) = decision_glyph(&d.status);
        out.push(row(
            vec![
                pointer(i, Some(sel)),
                Span::styled(format!("{g} "), st(gc)),
                Span::styled(format!("{:<8}", d.id), st("subtext1")),
                Span::styled(d.title.clone(), text()),
            ],
            vec![Span::styled(format!("{:>7}  {:<10}", date(d.date), status), st(gc))],
            list.width,
        ));
    }
    lines(f, list, out);
    lines(f, sep, vec![rule("", "", None, false, sep.width)]);
    let d = &ds.items[sel];
    let w = usize::from(detail.width).saturating_sub(12);
    let mut out = vec![Line::from(vec![
        Span::styled(format!(" {}", d.id), bold(st("subtext1"))),
        Span::styled(format!("{SEP}agreed {}", date(d.date)), dim()),
    ])];
    let mut field = |label: &str, value: &str, style| {
        for (i, l) in wrap(value, w).into_iter().enumerate() {
            out.push(Line::from(vec![
                Span::styled(format!(" {:<10}", if i == 0 { label } else { "" }), dim()),
                Span::styled(l, style),
            ]));
        }
    };
    field("chose", &d.chose, text());
    for o in &d.over {
        field("over", o, st("subtext0"));
    }
    if let Some(q) = &d.you {
        field("you", &format!("\"{}\" [{}]", q.text, q.source), st("lavender"));
    }
    for e in &d.evidence {
        field("evidence", e, text());
    }
    if let Some(r) = &d.recheck {
        field("re-check", r, st("yellow"));
    }
    if let Some(a) = &d.affects {
        field("affects", a, text());
    }
    if let DecisionStatus::Redecided { by } = &d.status {
        field("replaced", &format!("by {by}"), st("mauve"));
    }
    lines(f, detail, out);
    let mut h = vec![("?", "ask why"), ("o", "reopen"), ("c", "into the chat")];
    h.push(("⏎", "full entry"));
    lines(f, foot, vec![hints(&h).right_aligned()]);
}

pub fn draw_answered(f: &mut Frame, area: Rect, app: &App) {
    let now = app.fixtures.now();
    let pf = app.project_id().and_then(|id| app.fixtures.projects.get(&id));
    let ans = pf.and_then(|p| p.answered.as_ref());
    let inner = frame(
        f,
        area,
        title(&format!("{} · answered", project_name(app)), ""),
        Some(esc_back()),
    );
    let Some(ans) = ans.filter(|a| !a.items.is_empty()) else {
        none(f, inner, "nothing answered yet · i on the Project view asks");
        return;
    };
    let list_h = (ans.items.len() as u16).min(8);
    let [list, sep, body, foot] = Layout::vertical([
        Constraint::Length(list_h),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(inner);
    let sel = app.sel(' ').min(ans.items.len() - 1);
    let mut out = Vec::new();
    for (i, a) in ans.items.iter().enumerate().take(usize::from(list_h)) {
        let from = match a.asked {
            AskedFrom::Ask => "ask".to_string(),
            AskedFrom::Ink => format!("{} ink ?", glyph::INK),
        };
        out.push(row(
            vec![
                pointer(i, Some(sel)),
                Span::styled(format!("{} ", glyph::ANSWER), st("sky")),
                Span::styled(
                    format!("{:<35}", crate::ui::truncate(&a.title, 33)),
                    if a.unread { bold(text()) } else { text() },
                ),
                Span::styled(format!("{from:<9}"), dim()),
                Span::styled(a.passage.clone().unwrap_or_else(|| "·".into()), dim()),
            ],
            vec![Span::styled(format!("{} ", ago(now, a.at)), dim())],
            list.width,
        ));
    }
    lines(f, list, out);
    lines(f, sep, vec![rule("", "", None, false, sep.width)]);
    let a = &ans.items[sel];
    let w = usize::from(body.width).saturating_sub(2);
    let mut out = Vec::new();
    for p in &a.body {
        for l in wrap(p, w) {
            out.push(Line::from(Span::styled(format!(" {l}"), text())));
        }
        out.push(Line::from(""));
    }
    for l in &a.links {
        for (i, line) in wrap(l, w.saturating_sub(2)).into_iter().enumerate() {
            out.push(Line::from(Span::styled(
                format!(" {}{line}", if i == 0 { "↳ " } else { "  " }),
                st("subtext0"),
            )));
        }
    }
    lines(f, body, out);
    lines(
        f,
        foot,
        vec![hints(&[("f", "follow up"), ("t", "send to tablet"), ("c", "open as a session")]).right_aligned()],
    );
}

pub fn draw_changes(f: &mut Frame, area: Rect, app: &App) {
    let now = app.fixtures.now();
    let pf = app.project_id().and_then(|id| app.fixtures.projects.get(&id));
    let ch = pf.and_then(|p| p.changes.as_ref());
    let since = ch.map(|c| format!("since {} ▾", day(c.since))).unwrap_or_default();
    let inner = frame(
        f,
        area,
        title(&format!("{} · changes", project_name(app)), &since),
        Some(esc_back()),
    );
    let Some(ch) = ch.filter(|c| !c.entries.is_empty()) else {
        none(f, inner, "no changes recorded");
        return;
    };
    let sel = app.sel(' ').min(ch.entries.len() - 1);
    let mut out = Vec::new();
    let mut last_day = None;
    for (i, e) in ch.entries.iter().enumerate() {
        let d = e.at.date_naive();
        let label = if last_day == Some(d) { String::new() } else { day(d) };
        last_day = Some(d);
        let (g, gc) = item_glyph(e.kind);
        out.push(row(
            vec![
                pointer(i, Some(sel)),
                Span::styled(format!("{label:<8}"), dim()),
                Span::styled(format!("{g} "), st(gc)),
                Span::styled(e.text.clone(), text()),
            ],
            vec![Span::styled(format!("{} ", ago(now, e.at)), dim())],
            inner.width,
        ));
    }
    lines(f, inner, out);
}

pub fn key(app: &mut App, k: KeyEvent) -> bool {
    let pf = app.project_id().and_then(|id| app.fixtures.projects.get(&id)).cloned();
    let Some(pf) = pf else { return false };
    let screen = app.screen();
    let len = match screen {
        ScreenId::Decisions => pf.decisions.as_ref().map_or(0, |d| d.items.len()),
        ScreenId::Answered => pf.answered.as_ref().map_or(0, |a| a.items.len()),
        _ => pf.changes.as_ref().map_or(0, |c| c.entries.len()),
    };
    if let Some(d) = nav(&k) {
        app.move_sel(' ', len, d);
        return true;
    }
    let i = app.sel(' ');
    match (screen, k.code) {
        (ScreenId::Decisions, c @ (KeyCode::Char('?' | 'o' | 'c') | KeyCode::Enter)) => {
            let Some(d) = pf.decisions.as_ref().and_then(|d| d.items.get(i)) else {
                return false;
            };
            app.note = Some(match c {
                KeyCode::Char('?') => format!("would ask why {} was made; the answer lands in Answered", d.id),
                KeyCode::Char('o') => format!(
                    "would reopen {} in the decision log, where the working chats see it",
                    d.id
                ),
                KeyCode::Char('c') => format!("would put {} into the focused project chat as context", d.id),
                _ => format!(
                    "would open {} at {}",
                    pf.decisions.as_ref().map_or("", |d| d.log.as_str()),
                    d.id
                ),
            });
            true
        }
        (ScreenId::Answered, KeyCode::Char(c @ ('f' | 't' | 'c'))) => {
            let Some(a) = pf.answered.as_ref().and_then(|a| a.items.get(i)) else {
                return false;
            };
            app.note = Some(match c {
                'f' => format!("would continue thread {}", a.thread),
                't' => format!("would send \"{}\" to the writer agent for the tablet", a.title),
                _ => format!("would open thread {} as a session in your project window", a.thread),
            });
            true
        }
        (ScreenId::Changes, KeyCode::Enter) => {
            if let Some(e) = pf.changes.as_ref().and_then(|c| c.entries.get(i)) {
                app.note = Some(format!("would open where it lives: {}", e.text));
            }
            true
        }
        _ => false,
    }
}
