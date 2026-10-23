//! Answered (`a`, then Enter on the Project view): a list on top and the selected answer below
//! it. Decisions and Changes have modules of their own.

use crate::app::{App, nav};
use crate::screen::ScreenId;
use crate::ui::widgets::*;
use crate::ui::{esc_back, frame, lines, title};
use locus_contract::AskedFrom;
use locus_theme::glyph;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};

pub const ANSWER_KEYS: &[(&str, &str)] = &[
    ("␣", "follow up: continues this answer's thread, here"),
    ("→ or ⏎", "read a long answer; ↑ ↓ scroll, ← back"),
    ("t", "send to the tablet (a long explanation only)"),
    (
        "c",
        "build in project window: the thread becomes a working session there",
    ),
];

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

/// An answer long enough to be worth reading on paper; only then is `t` offered (round 2zd:
/// "if I asked a more explanatory question ... those would make longer explanations that it
/// would actually make sense to send to the tablet").
fn long(a: &locus_contract::Answer) -> bool {
    a.body.iter().map(|p| p.split_whitespace().count()).sum::<usize>() > 150
}

fn thread_key(project: &str, thread: &str) -> String {
    format!("answer:{project}:{thread}")
}

/// A follow-up typed on the answer's line: it continues that answer's thread, here in the
/// locus window, where questions and learning live; building goes to the project window
/// (ADR-007).
pub fn follow_up(app: &mut App, q: String) {
    let Some(id) = app.project_id() else { return };
    let Some(a) = app
        .fixtures
        .projects
        .get(&id)
        .and_then(|p| p.answered.as_ref())
        .and_then(|x| x.items.get(app.sel(' ')))
        .cloned()
    else {
        return;
    };
    app.asked.push((thread_key(&id, &a.thread), q));
    app.note = Some(format!(
        "would continue \"{}\" with heuris; the reply comes back under it",
        crate::ui::truncate(&a.title, 40)
    ));
}

/// Answered: the list on top, the chosen answer below it in full, a follow-up line pinned at
/// the foot (round 2zd: "I feel I would be following up a lot and would want it to be ergonomic
/// to do so"). Long answers scroll: → into the answer, ↑ ↓, ← back to the list.
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
        none(f, inner, "nothing answered yet · ␣ on the Project view asks");
        return;
    };
    let list_h = (ans.items.len() as u16).min(8);
    let [list, sep, body, ask_r] = Layout::vertical([
        Constraint::Length(list_h),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(2),
    ])
    .areas(inner);
    let sel = app.sel(' ').min(ans.items.len() - 1);
    let reading = app.focus == Some('b');
    let mut out = Vec::new();
    for (i, a) in ans.items.iter().enumerate().take(usize::from(list_h)) {
        let from = match a.asked {
            AskedFrom::Ask => "ask".to_string(),
            AskedFrom::Ink => format!("{} ink ?", glyph::INK),
        };
        let mut right = vec![];
        if a.unread {
            right.push(Span::styled(format!("{} ", glyph::UNREAD), st("peach")));
        }
        right.push(Span::styled(format!("{:>4} ", ago(now, a.at)), dim()));
        let lead = if reading && i == sel {
            Span::styled("▌ ", st("blue"))
        } else {
            pointer(i, Some(sel))
        };
        let line = row(
            vec![
                lead,
                Span::styled(format!("{} ", glyph::ANSWER), st("sky")),
                Span::styled(
                    format!("{:<48}", crate::ui::truncate(&a.title, 46)),
                    if a.unread { bold(text()) } else { text() },
                ),
                Span::styled(format!("{from:<9}"), dim()),
                Span::styled(a.passage.clone().unwrap_or_default(), dim()),
            ],
            right,
            list.width,
        );
        out.push(if reading && i == sel {
            line.patch_style(ratatui::style::Style::default().bg(locus_theme::c("surface0")))
        } else {
            line
        });
    }
    lines(f, list, out);
    lines(f, sep, vec![rule("", "", None, reading, sep.width)]);
    let a = &ans.items[sel];
    let w = usize::from(body.width).saturating_sub(4);
    // The question in full first, then where it came from and what it was about.
    let mut out = Vec::new();
    for l in wrap(&a.title, w) {
        out.push(Line::from(Span::styled(
            format!("  {l}"),
            bold(st(if reading { "blue" } else { "text" })),
        )));
    }
    let mut meta = vec![Span::styled(
        format!(
            "  {}",
            match (&a.asked, &a.passage) {
                (AskedFrom::Ink, Some(p)) => format!("circled in your ink, {p}"),
                (AskedFrom::Ink, None) => "circled in your ink".to_string(),
                (AskedFrom::Ask, _) => "asked on the ask line".to_string(),
            }
        ),
        dim(),
    )];
    meta.push(Span::styled(format!(" · {} ago", ago(now, a.at)), dim()));
    if let Some(d) = a.decision.as_ref().and_then(|id| {
        pf.and_then(|p| p.decisions.as_ref())
            .and_then(|ds| ds.items.iter().find(|x| &x.id == id))
    }) {
        meta.push(Span::styled(
            format!(" · about the decision: {}", d.title),
            st("subtext0"),
        ));
    }
    out.push(Line::from(meta));
    out.push(Line::from(""));
    for p in &a.body {
        for l in wrap(p, w) {
            out.push(Line::from(Span::styled(format!("  {l}"), text())));
        }
        out.push(Line::from(""));
    }
    for l in &a.links {
        for (i, line) in wrap(l, w.saturating_sub(2)).into_iter().enumerate() {
            out.push(Line::from(Span::styled(
                format!("  {}{line}", if i == 0 { "↳ " } else { "  " }),
                st("subtext0"),
            )));
        }
    }
    // Follow-ups you asked, waiting for their reply in this thread.
    let key = pf.map(|p| thread_key(&p.view.project, &a.thread)).unwrap_or_default();
    let pending: Vec<&String> = app.asked.iter().filter(|(k, _)| *k == key).map(|(_, q)| q).collect();
    if !pending.is_empty() {
        out.push(Line::from(""));
        for q in pending {
            for l in wrap(&format!("you: “{q}”"), w) {
                out.push(Line::from(Span::styled(format!("  {l}"), st("pink"))));
            }
            out.push(Line::from(Span::styled(
                "  on its way · the reply comes back here",
                dim(),
            )));
        }
    }
    while out.last().is_some_and(|l| l.width() == 0) {
        out.pop();
    }
    let h = usize::from(body.height);
    let max_scroll = out.len().saturating_sub(h);
    app.shown
        .borrow_mut()
        .insert((ScreenId::Answered, 'b'), vec![max_scroll]);
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
    lines(f, body, view);
    // The follow-up line, as the ask line is on the Project view.
    let asking = matches!(app.mode, crate::app::Mode::Ask(_));
    // Just "follow up", with what it does as grey text inside the line (round 3a), and the
    // page's keys at the right end of its rule, as on the Project view's ask line (round 3b).
    let mut keys: Vec<(&str, &str)> = vec![];
    if reading {
        keys.push(("←", "the list"));
    } else if max_scroll > 0 {
        keys.push(("→", "read on"));
    }
    if long(a) {
        keys.push(("t", "send to tablet"));
    }
    keys.push(("c", "build in project window"));
    let head = crate::ui::ask_rule("follow up", asking, &keys, "", ask_r.width);
    let hint = if app.draft.is_empty() {
        "continue this thread".to_string()
    } else {
        format!("your draft: {}", crate::ui::truncate(&app.draft, 50))
    };
    let line = match &app.mode {
        crate::app::Mode::Ask(q) if !q.is_empty() => Line::from(vec![
            Span::styled("   follow up › ", st("mauve")),
            Span::styled(q.clone(), text()),
            Span::styled("▏", st("mauve")),
        ]),
        crate::app::Mode::Ask(_) => Line::from(vec![
            Span::styled("   follow up › ", st("mauve")),
            Span::styled("▏", st("mauve")),
            Span::styled("continue this thread", st("overlay0")),
        ]),
        _ => Line::from(vec![
            Span::styled("   follow up › ", dim()),
            Span::styled("␣ ", keyst()),
            Span::styled(hint, st("overlay0")),
        ]),
    };
    lines(f, ask_r, vec![head, line]);
}

pub fn key(app: &mut App, k: KeyEvent) -> bool {
    let pf = app.project_id().and_then(|id| app.fixtures.projects.get(&id)).cloned();
    let Some(pf) = pf else { return false };
    let screen = app.screen();
    let len = match screen {
        ScreenId::Answered => pf.answered.as_ref().map_or(0, |a| a.items.len()),
        _ => pf.changes.as_ref().map_or(0, |c| c.entries.len()),
    };
    // Reading a long answer: ↑ ↓ scroll it, ← goes back to the list (round 2zd).
    if screen == ScreenId::Answered && app.focus == Some('b') {
        let max = app
            .shown
            .borrow()
            .get(&(ScreenId::Answered, 'b'))
            .and_then(|v| v.first().copied())
            .unwrap_or(0);
        if let Some(d) = nav(&k) {
            let at = (app.sel('b') as isize + d).clamp(0, max as isize) as usize;
            app.sel.insert((ScreenId::Answered, 'b'), at);
            return true;
        }
        if k.code == KeyCode::Left {
            app.focus = None;
            return true;
        }
    }
    if let Some(d) = nav(&k) {
        app.move_sel(' ', len, d);
        return true;
    }
    let i = app.sel(' ');
    match (screen, k.code) {
        (ScreenId::Answered, KeyCode::Right | KeyCode::Enter) => {
            app.focus = Some('b');
            app.sel.insert((ScreenId::Answered, 'b'), 0);
            true
        }
        // Space follows up on the selected answer; asking always happens on Space.
        (ScreenId::Answered, KeyCode::Char(' ')) => {
            app.start_ask();
            true
        }
        (ScreenId::Answered, KeyCode::Char(c @ ('t' | 'c'))) => {
            let Some(a) = pf.answered.as_ref().and_then(|a| a.items.get(i)) else {
                return false;
            };
            app.note = Some(match c {
                't' if long(a) => format!("would send \"{}\" to the writer agent for the tablet", a.title),
                't' => "this answer is short; t is for an explanation worth reading on paper".into(),
                _ => format!(
                    "would open thread {} as a working session in your project window, to build on it",
                    a.thread
                ),
            });
            true
        }
        _ => false,
    }
}
