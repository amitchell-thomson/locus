//! The screens a Project view opens: Answered (`a`, then Enter) and Changes (`n`). Each is a
//! list on top and the selected entry explained below. Decisions has its own module.

use crate::app::{App, nav};
use crate::screen::ScreenId;
use crate::ui::widgets::*;
use crate::ui::{esc_back, frame, hints, lines, title};
use locus_contract::AskedFrom;
use locus_theme::glyph;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};

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
        ScreenId::Answered => pf.answered.as_ref().map_or(0, |a| a.items.len()),
        _ => pf.changes.as_ref().map_or(0, |c| c.entries.len()),
    };
    if let Some(d) = nav(&k) {
        app.move_sel(' ', len, d);
        return true;
    }
    let i = app.sel(' ');
    match (screen, k.code) {
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
