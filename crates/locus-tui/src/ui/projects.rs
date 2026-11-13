//! Projects (`p` on Home, 10 Nov): every project you build, in sections by where it stands. Home
//! lists only the live ones; this is where the parked and done ones are, and what is running or
//! sitting in `~/local-projects` that the registry does not know ("quickly make a projects view
//! on pressing p that shows them all in sections").

use crate::app::App;
use crate::ui::widgets::*;
use crate::ui::{frame, lines, title};
use locus_contract::{ProjectKind, ProjectState, SessionState};
use locus_theme::glyph;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

pub const KEYS: &[(&str, &str)] = &[
    ("↑ ↓", "walk the projects"),
    ("⏎", "open it; on one not registered, ask the chat to register it"),
    ("esc", "back to Home"),
];

/// One selectable row: a registry id, `tmux:<session>`, or `dir:<folder>`.
pub fn rows(app: &App) -> Vec<(&'static str, Vec<String>)> {
    let reg = &app.fixtures.registry;
    let built = |s: ProjectState| -> Vec<String> {
        reg.projects
            .iter()
            .filter(|p| p.kind == ProjectKind::Repo && p.state == Some(s))
            .map(|p| p.id.clone())
            .collect()
    };
    let mut out = vec![
        ("live", built(ProjectState::Live)),
        ("parked", built(ProjectState::Parked)),
        ("done", built(ProjectState::Done)),
    ];
    let sessions: Vec<String> = app.unregistered.iter().map(|(n, _)| format!("tmux:{n}")).collect();
    out.push(("running, not registered", sessions));
    let dirs: Vec<String> = app
        .other_repos
        .iter()
        .filter(|(n, _)| !app.unregistered.iter().any(|(t, _)| t == n))
        .map(|(n, _)| format!("dir:{n}"))
        .collect();
    out.push(("in ~/local-projects, not registered", dirs));
    out
}

fn flat(app: &App) -> Vec<String> {
    rows(app).into_iter().flat_map(|(_, v)| v).collect()
}

pub fn len(app: &App) -> usize {
    flat(app).len()
}

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let live = rows(app).first().map_or(0, |(_, v)| v.len());
    let inner = frame(f, area, title("projects", &format!("{live} live")), None);
    let now = app.fixtures.now();
    let sel = app.sel(' ');
    let mut out: Vec<Line> = Vec::new();
    let mut i = 0usize;
    let mut sel_line = 0usize;
    for (title, ids) in rows(app) {
        let extra = if ids.is_empty() {
            "none".to_string()
        } else {
            ids.len().to_string()
        };
        out.push(rule(title, &extra, None, false, inner.width));
        for id in &ids {
            if i == sel {
                sel_line = out.len();
            }
            out.push(row_for(app, id, i, sel, now, inner.width));
            i += 1;
        }
        out.push(Line::raw(""));
    }
    // Keep the selection in view.
    let h = usize::from(inner.height);
    let skip = (sel_line + 2).saturating_sub(h);
    lines(f, inner, out.into_iter().skip(skip).collect());
}

fn row_for(app: &App, id: &str, i: usize, sel: usize, now: locus_contract::Time, width: u16) -> Line<'static> {
    let fx = &app.fixtures;
    let mut left = vec![pointer(i, Some(sel))];
    let mut right = vec![];
    if let Some(name) = id.strip_prefix("dir:") {
        left.push(Span::styled(format!(" {} ", glyph::UNATTENDED), st("overlay1")));
        left.push(Span::styled(
            format!("{:<26}", crate::ui::truncate(name, 25)),
            st("overlay2"),
        ));
        if let Some((_, Some((t, what)))) = app.other_repos.iter().find(|(n, _)| n == name) {
            left.push(Span::styled(format!("last {}  ", date(t.date_naive())), dim()));
            left.push(Span::styled(what.clone(), st("subtext0")));
        }
        return row(left, right, width);
    }
    let (name, sessions, area_col) = match id.strip_prefix("tmux:") {
        Some(n) => (
            n.to_string(),
            app.unregistered
                .iter()
                .find(|(t, _)| t == n)
                .map(|(_, s)| s.clone())
                .unwrap_or_default(),
            "overlay2".to_string(),
        ),
        None => {
            let Some(p) = fx.registry.get(id) else {
                return Line::raw("");
            };
            (
                p.name.clone(),
                fx.projects
                    .get(id)
                    .map(|pf| pf.view.sessions.clone())
                    .unwrap_or_default(),
                crate::ui::home::area_colour(app, &p.area).to_string(),
            )
        }
    };
    left.push(Span::styled(format!(" {} ", glyph::PROJECT), st(&area_col)));
    left.push(Span::styled(
        format!("{:<26}", crate::ui::truncate(&name, 25)),
        bold(st(&area_col)),
    ));
    let mut ss = sessions.clone();
    ss.sort_by_key(|s| s.state);
    match ss.first() {
        Some(s) => {
            let (g, c, word) = session_glyph(s.state);
            let g = if s.state == SessionState::Working {
                spin(app.frame)
            } else {
                g
            };
            left.push(Span::styled(format!("{g} {word:<9}"), st(c)));
            left.push(Span::styled(format!("{:>4}  ", ago(now, s.since)), dim()));
            left.push(Span::styled(s.summary.clone().unwrap_or_default(), text()));
            if ss.len() > 1 {
                right.push(Span::styled(format!("{} sessions ", ss.len()), dim()));
            }
        }
        None => {
            left.push(Span::styled(format!("{} {:<14}", glyph::IDLE, "no sessions"), dim()));
            if let Some((t, what)) = app.last_commit.get(id) {
                left.push(Span::styled(format!("last {}  ", date(t.date_naive())), dim()));
                left.push(Span::styled(what.clone(), st("subtext0")));
            }
        }
    }
    row(left, right, width)
}

pub fn key(app: &mut App, k: KeyEvent) -> bool {
    let n = len(app);
    match k.code {
        KeyCode::Down | KeyCode::Char('j') => {
            app.move_sel(' ', n, 1);
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.move_sel(' ', n, -1);
            true
        }
        KeyCode::Enter => {
            match flat(app).get(app.sel(' ')).cloned() {
                Some(id) if id.starts_with("tmux:") || id.starts_with("dir:") => {
                    let name = id.split_once(':').map_or("", |x| x.1).to_string();
                    app.mode = crate::app::Mode::Chat(format!(
                        "`{name}` is not in the registry. Look at what it is and help me register it: "
                    ));
                }
                Some(id) => app.open_item(&id),
                None => {}
            }
            true
        }
        _ => false,
    }
}
