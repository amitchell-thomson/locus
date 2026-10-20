//! The Project view (Alt-2): one screen in every state. Where you are, the branch graph (plan
//! D.1), one list of everything new, this project's sessions, chores, and the ask line.

use crate::app::{App, Mark, Mode, nav};
use crate::screen::ScreenId;
use crate::ui::widgets::*;
use crate::ui::{follow_badge, frame, lines, project_title};
use chrono::{Duration, NaiveDate};
use locus_contract::{BranchGraph, ProjectKind, ProjectView, SessionState};
use locus_theme::{SEP, c, glyph};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

pub const KEYS: &[(&str, &str)] = &[
    ("n n", "new; again for the full timeline"),
    ("r", "mark the selected new item read"),
    ("x", "a chore done"),
    ("i", "ask a question"),
    ("d", "decisions"),
    ("a", "answered"),
    ("p", "interview prep (an opportunity)"),
    ("o", "the dossier"),
    ("m", "pin or follow"),
];

/// Lanes drawn at once; older ones are counted in the rule.
const LANES: usize = 6;

pub fn view(app: &App) -> Option<&ProjectView> {
    let id = app.project_id()?;
    app.fixtures.projects.get(&id).map(|p| &p.view)
}

fn new_key(view: &ProjectView, i: usize) -> String {
    format!("new:{}:{i}", view.project)
}

fn chore_key(view: &ProjectView, i: usize) -> String {
    format!("chore:{}:{i}", view.project)
}

fn is_unread(app: &App, view: &ProjectView, i: usize) -> bool {
    view.new.get(i).is_some_and(|it| it.unread) && app.marked(&new_key(view, i)).is_none()
}

pub fn section_len(app: &App, sec: char) -> usize {
    let Some(v) = view(app) else { return 0 };
    match sec {
        'n' => v.new.len(),
        'g' => v.graph.as_ref().map_or(0, |g| g.lanes.len().min(LANES)),
        's' => v.sessions.len().min(8),
        _ => v.chores.len().min(3),
    }
}

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let now = app.fixtures.now();
    let view = view(app);
    let mut right = Vec::new();
    if let Some(v) = view
        && let Some(s) = v.sessions.iter().find(|s| s.state == SessionState::Working)
    {
        right.push(Span::styled(
            format!(" {} claude working{SEP}{} ", glyph::WORKING, ago(now, s.since)),
            st("blue"),
        ));
    }
    if app.peek.is_some() && app.stack.first() == Some(&ScreenId::Project) {
        right.push(Span::styled(" from home · esc returns ", st("peach")));
    } else if app.stack.first() == Some(&ScreenId::Project) {
        right.extend(follow_badge(app).spans);
    } else {
        right.push(Span::styled(" esc back ", dim()));
    }
    let mut t = vec![Span::styled(format!(" {} ", project_title(app)), bold(text()))];
    if let Some(stage) = view.and_then(|v| v.stage.as_deref()) {
        t.push(Span::styled(format!("· {stage} "), dim()));
    }
    let inner = frame(f, area, Line::from(t), Some(Line::from(right)));
    let Some(v) = view else {
        draw_missing(f, inner, app);
        return;
    };
    let w = inner.width;
    let graph_h = v.graph.as_ref().map_or(0, |g| g.lanes.len().min(LANES) as u16 + 5);
    let sessions_h = if app.focus == Some('s') {
        v.sessions.len().min(8)
    } else {
        v.sessions.len().min(3)
    } as u16
        + 1
        + u16::from(app.focus != Some('s') && v.sessions.len() > 3);
    let chores_h = v.chores.len().min(3) as u16 + 1;
    let [where_r, graph_r, new_r, ses_r, ch_r, ask_r] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Length(graph_h),
        Constraint::Min(3),
        Constraint::Length(sessions_h.max(2)),
        Constraint::Length(chores_h.max(2)),
        Constraint::Length(2),
    ])
    .areas(inner);

    let mut wy = vec![rule("where you are", "", None, false, w)];
    match &v.where_you_are {
        Some(wh) => {
            wy.push(Line::from(Span::styled(
                format!(" {}", crate::ui::truncate(&wh.state, usize::from(w) - 2)),
                text(),
            )));
            if let Some(s) = &wh.short {
                wy.push(Line::from(Span::styled(
                    format!(" {}", crate::ui::truncate(s, usize::from(w) - 2)),
                    st("subtext0"),
                )));
            }
            wy.push(Line::from(vec![
                Span::styled(" next ▸ ", st("peach")),
                Span::styled(crate::ui::truncate(&wh.next, usize::from(w) - 9), text()),
            ]));
        }
        None => wy.push(Line::from(Span::styled(" nothing written yet", dim()))),
    }
    lines(f, where_r, wy);

    if let Some(g) = &v.graph {
        draw_graph(f, graph_r, app, g);
    }
    draw_new(f, new_r, app, v);
    draw_sessions(f, ses_r, app, v);
    draw_chores(f, ch_r, app, v);

    let ask = match &app.mode {
        Mode::Ask(q) => Line::from(vec![
            Span::styled(" ask › ", st("mauve")),
            Span::styled(q.clone(), text()),
            Span::styled("▏", st("mauve")),
        ]),
        _ => Line::from(vec![
            Span::styled(" ask › ", dim()),
            Span::styled("i", keyst()),
            Span::styled(" to ask about this project · the answer comes back here", dim()),
        ]),
    };
    lines(
        f,
        ask_r,
        vec![rule("ask", "", None, matches!(app.mode, Mode::Ask(_)), w), ask],
    );
}

/// The view has nothing to show: say which of the three reasons it is.
fn draw_missing(f: &mut Frame, area: Rect, app: &App) {
    let reg = &app.fixtures.registry;
    let msg = match (app.follow.session.as_deref(), app.project_id()) {
        (None, _) if app.stack.first() == Some(&ScreenId::Project) => {
            "No project window seen yet. Switch your project window to a project session and this view follows it."
                .to_string()
        }
        (Some(s), None) if reg.by_session(s).is_none() => format!(
            "The project window is on `{s}`, which is not a registered project. Register it, or switch to one that is."
        ),
        (_, Some(id)) => format!("{id} is registered but not active, so it has no view yet."),
        (Some(s), None) => format!("{s} is registered but not active, so it has no view yet."),
        (None, None) => "No project chosen.".to_string(),
    };
    let mut out = vec![Line::from("")];
    for l in wrap(&msg, usize::from(area.width).saturating_sub(4)) {
        out.push(Line::from(Span::styled(format!("  {l}"), dim())));
    }
    lines(f, area, out);
}

fn draw_graph(f: &mut Frame, area: Rect, app: &App, g: &BranchGraph) {
    let now = app.fixtures.now().date_naive();
    let focus = app.focus == Some('g');
    let w = usize::from(area.width);
    let hidden = g.lanes.len().saturating_sub(LANES);
    let lanes = &g.lanes[hidden..];
    let extra = if hidden > 0 {
        format!("{} lanes, {hidden} older not drawn", g.lanes.len())
    } else {
        String::new()
    };
    let mut out = vec![rule("branches", &extra, Some('g'), focus, area.width)];
    let label_w = 30;
    let gw = w.saturating_sub(label_w + 8).max(10);
    // Time runs from just before the oldest drawn fork (or the graph's start) to now.
    let start = lanes
        .iter()
        .map(|l| l.forked - Duration::days(2))
        .min()
        .unwrap_or(g.from)
        .max(g.from);
    let span = (now - start).num_days().max(1) as usize;
    let x = |d: NaiveDate| -> usize { ((d - start).num_days().max(0) as usize * (gw - 1) / span).min(gw - 1) };

    let mut trunk: Vec<char> = vec!['─'; gw];
    for l in lanes {
        let fx = x(l.forked);
        trunk[fx] = if trunk[fx] == '┴' { '┼' } else { '┬' };
        if let Some(m) = l.merged {
            let mx = x(m);
            trunk[mx] = if trunk[mx] == '┬' { '┼' } else { '┴' };
        }
    }
    let mut trunk_spans = vec![Span::styled(
        format!("{:>w$} ", g.trunk, w = label_w),
        bold(st("subtext1")),
    )];
    let earlier = g.milestones.iter().filter(|m| m.date < start).count();
    trunk_spans.push(Span::styled(if earlier > 0 { "◂" } else { " " }, dim()));
    let ms: Vec<usize> = g
        .milestones
        .iter()
        .filter(|m| m.date >= start)
        .map(|m| x(m.date))
        .collect();
    for (i, ch) in trunk.iter().enumerate() {
        if ms.contains(&i) && *ch == '─' {
            trunk_spans.push(Span::styled("●", st("green")));
        } else {
            trunk_spans.push(Span::styled(ch.to_string(), st("overlay2")));
        }
    }
    trunk_spans.push(Span::styled(" now", dim()));
    out.push(Line::from(trunk_spans));

    let sel = app.sel('g').min(lanes.len().saturating_sub(1));
    for (i, l) in lanes.iter().enumerate() {
        let selected = focus && i == sel;
        let col = c(PROJECT_COLOURS[i % PROJECT_COLOURS.len()]);
        let label = crate::ui::truncate(&l.stage, label_w - 2);
        let mut spans = vec![
            Span::styled(if selected { "▌ " } else { "  " }, keyst()),
            Span::styled(
                format!("{label:>w$} ", w = label_w - 2),
                if selected {
                    bold(Style::default().fg(col))
                } else {
                    Style::default().fg(col)
                },
            ),
            Span::raw(" "),
        ];
        let a = x(l.forked);
        let end = l.merged.or(l.tip).unwrap_or(now);
        let b = x(end).max(a);
        let mut cells = String::new();
        for i in 0..gw {
            cells.push(if i < a {
                ' '
            } else if i == a && a == b {
                if l.merged.is_some() { '┃' } else { '╽' }
            } else if i == a {
                '╰'
            } else if i < b {
                '━'
            } else if i == b {
                if l.merged.is_some() { '╯' } else { '○' }
            } else {
                ' '
            });
        }
        let trimmed = cells.trim_end().to_string();
        let pad = gw - trimmed.chars().count();
        spans.push(Span::styled(trimmed, Style::default().fg(col)));
        let n = format!(" {}", l.commits);
        spans.push(Span::styled(n.clone(), dim()));
        spans.push(Span::raw(" ".repeat(pad.saturating_sub(n.width()))));
        out.push(Line::from(spans));
    }
    // The axis: the first and last day, and the middle.
    let mid = start + Duration::days((span / 2) as i64);
    let mut axis = format!("{:w$}  ", "", w = label_w);
    let a = date(start);
    let m = date(mid);
    let e = date(now);
    let gap1 = (gw / 2).saturating_sub(a.width() + m.width() / 2);
    let gap2 = gw.saturating_sub(a.width() + gap1 + m.width() + e.width());
    axis.push_str(&format!("{a}{}{m}{}{e}", " ".repeat(gap1), " ".repeat(gap2)));
    out.push(Line::from(Span::styled(axis, dim())));
    // What happened in the selected lane (or the newest, unfocused).
    let shown = if focus { lanes.get(sel) } else { lanes.last() };
    if let Some(l) = shown {
        let state = match l.merged {
            Some(m) => format!("merged {}", date(m)),
            None => format!("open{SEP}last {}", l.tip.map(date).unwrap_or_default()),
        };
        let happened = l.happened.join("; ");
        out.push(Line::from(vec![
            Span::styled(format!(" {}", l.branch), st("subtext1")),
            Span::styled(format!("{SEP}{}{SEP}{state}{SEP}", date(l.forked)), dim()),
            Span::styled(
                crate::ui::truncate(&happened, w.saturating_sub(l.branch.width() + 40)),
                text(),
            ),
        ]));
    }
    lines(f, area, out);
}

fn draw_new(f: &mut Frame, area: Rect, app: &App, v: &ProjectView) {
    let now = app.fixtures.now();
    let focus = app.focus == Some('n');
    let unread = (0..v.new.len()).filter(|&i| is_unread(app, v, i)).count();
    let extra = if v.new.is_empty() {
        "nothing new".to_string()
    } else {
        format!("{unread} unread")
    };
    let mut out = vec![rule("new", &extra, Some('n'), focus, area.width)];
    let sel = focus.then(|| app.sel('n'));
    for (i, it) in v.new.iter().enumerate() {
        let (g, gc) = item_glyph(it.kind);
        let unread = is_unread(app, v, i);
        out.push(row(
            vec![
                pointer(i, sel),
                Span::styled(
                    if unread {
                        format!("{} ", glyph::UNREAD)
                    } else {
                        "  ".into()
                    },
                    st("peach"),
                ),
                Span::styled(format!("{g} "), st(gc)),
                Span::styled(it.text.clone(), if unread { text() } else { dim() }),
            ],
            vec![Span::styled(format!("{} ", ago(now, it.at)), dim())],
            area.width,
        ));
    }
    if v.new.is_empty() {
        let last = app
            .fixtures
            .projects
            .get(&v.project)
            .and_then(|p| p.changes.as_ref())
            .and_then(|c| c.entries.first().map(|e| e.at));
        let msg = match last {
            Some(t) => format!("  nothing new; last change {} ago · n n for the timeline", ago(now, t)),
            None => "  nothing new".to_string(),
        };
        out.push(Line::from(Span::styled(msg, dim())));
    }
    lines(f, area, out);
}

fn draw_sessions(f: &mut Frame, area: Rect, app: &App, v: &ProjectView) {
    let now = app.fixtures.now();
    let focus = app.focus == Some('s');
    let mut out = vec![rule(
        "sessions",
        &v.sessions.len().to_string(),
        Some('s'),
        focus,
        area.width,
    )];
    let mut ss: Vec<_> = v.sessions.iter().collect();
    ss.sort_by_key(|s| s.state);
    let room = if focus { 8 } else { 3 };
    let sel = focus.then(|| app.sel('s'));
    for (i, s) in ss.iter().take(room).enumerate() {
        let (g, gc, word) = session_glyph(s.state);
        let name = format!("{}{}", s.name, if s.background { " (bg)" } else { "" });
        let right = if s.state == SessionState::NeedsYou {
            vec![Span::styled("⏎ jump ", keyst())]
        } else {
            vec![]
        };
        out.push(row(
            vec![
                pointer(i, sel),
                Span::styled(format!("{g} "), st(gc)),
                Span::styled(format!("{:<20}", crate::ui::truncate(&name, 20)), text()),
                Span::styled(format!("{:<18}", format!("{word}{SEP}{}", ago(now, s.since))), st(gc)),
                Span::styled(s.doing.clone().unwrap_or_default(), dim()),
            ],
            right,
            area.width,
        ));
    }
    if ss.len() > room {
        out.push(Line::from(Span::styled(
            format!("    + {} more · s to see them", ss.len() - room),
            dim(),
        )));
    }
    if ss.is_empty() {
        out.push(Line::from(Span::styled("  no sessions", dim())));
    }
    lines(f, area, out);
}

fn draw_chores(f: &mut Frame, area: Rect, app: &App, v: &ProjectView) {
    let focus = app.focus == Some('c');
    let mut out = vec![rule("chores", "", Some('c'), focus, area.width)];
    let sel = focus.then(|| app.sel('c'));
    for (i, ch) in v.chores.iter().enumerate().take(3) {
        let done = app.marked(&chore_key(v, i)) == Some(&Mark::Done);
        let mut t = format!("{}{SEP}{} min", ch.title, ch.minutes);
        if let Some(d) = ch.due {
            t.push_str(&format!("{SEP}due {}", day(d)));
        }
        out.push(row(
            vec![
                pointer(i, sel),
                Span::styled(format!("{} ", glyph::CHORES), st("teal")),
                Span::styled(t, if done { struck(text()) } else { text() }),
            ],
            vec![Span::styled("⏎ open · x done ", dim())],
            area.width,
        ));
    }
    if v.chores.is_empty() {
        out.push(Line::from(Span::styled("  none fit now", dim())));
    }
    lines(f, area, out);
}

pub fn key(app: &mut App, k: KeyEvent) -> bool {
    let Some(v) = view(app).cloned() else {
        return matches!(k.code, KeyCode::Char('m')) && toggle_pin(app);
    };
    if let Some(d) = nav(&k) {
        let len = match app.focus {
            Some('n') => v.new.len(),
            Some('s') => v.sessions.len(),
            Some('c') => v.chores.len(),
            Some('g') => v.graph.as_ref().map_or(0, |g| g.lanes.len().min(LANES)),
            _ => return false,
        };
        if let Some(sec) = app.focus {
            app.move_sel(sec, len, d);
        }
        return true;
    }
    let focus = app.focus;
    match (focus, k.code) {
        (Some('n'), KeyCode::Char('n')) => app.open(ScreenId::Changes),
        (Some('n'), KeyCode::Enter | KeyCode::Char('r')) => {
            let i = app.sel('n');
            if let Some(it) = v.new.get(i) {
                let note = if k.code == KeyCode::Enter {
                    format!("would open: {}", it.text)
                } else {
                    "marked read".into()
                };
                app.mark(new_key(&v, i), Mark::Read, note);
            }
        }
        (Some('s'), KeyCode::Enter) => {
            let mut ss: Vec<_> = v.sessions.iter().collect();
            ss.sort_by_key(|s| s.state);
            if let Some(s) = ss.get(app.sel('s')) {
                app.note = Some(format!("would jump your project window to \"{}\"", s.name));
            }
        }
        (Some('c'), KeyCode::Char('x')) => {
            let i = app.sel('c');
            if let Some(ch) = v.chores.get(i) {
                app.mark(
                    chore_key(&v, i),
                    Mark::Done,
                    format!("done: would tell kairos \"{}\"", ch.title),
                );
            }
        }
        (Some('c'), KeyCode::Enter) => {
            if let Some(ch) = v.chores.get(app.sel('c')) {
                app.note = Some(format!("would open \"{}\" in kairos", ch.title));
            }
        }
        (Some('g'), KeyCode::Enter) => app.note = Some("would show the lane's change records".into()),
        (_, KeyCode::Char('i')) => app.mode = Mode::Ask(String::new()),
        (_, KeyCode::Char('d')) => app.open(ScreenId::Decisions),
        (_, KeyCode::Char('a')) => app.open(ScreenId::Answered),
        (_, KeyCode::Char('o')) => {
            let has = app.fixtures.registry.get(&v.project).is_some_and(|p| p.depth.is_some());
            app.note = Some(if has {
                format!("would open the {} dossier PDF", v.project)
            } else {
                format!("{} has no dossier", v.project)
            });
        }
        (_, KeyCode::Char('p')) => {
            let kind = app.fixtures.registry.get(&v.project).map(|p| p.kind);
            if kind == Some(ProjectKind::Opportunity) {
                app.open(ScreenId::Prep);
            } else {
                app.note = Some("p is interview prep, for an opportunity".into());
            }
        }
        (_, KeyCode::Char('m')) => return toggle_pin(app),
        _ => return false,
    }
    true
}

fn toggle_pin(app: &mut App) -> bool {
    if app.stack.first() != Some(&ScreenId::Project) {
        app.note = Some("m pins the Project view in its own window (alt-2)".into());
        return true;
    }
    app.follow.toggle_pin();
    app.note = Some(
        if app.follow.pinned {
            "pinned: stays on this project"
        } else {
            "following the project window"
        }
        .into(),
    );
    true
}
