//! The Project view (Alt-2): one screen in every state. Where you are, a strip naming what sits
//! behind its keys, the branch graph (plan D.1), everything new by kind, this project's
//! sessions, the chores that fit a gap, and the ask line.
//!
//! The same rules as Home (round 2, 22 Oct: "consistent section highlighting and spacing rules
//! as in with home"): a blank line between sections, bright section titles, the focused section
//! marked on the frame's edge. And every big capability is named somewhere on the page, even if
//! only in a strip ("every big capability should be referenced in some way on every page").

use crate::app::{App, Mark, Mode};
use crate::screen::ScreenId;
use crate::ui::widgets::*;
use crate::ui::{follow_badge, lines, project_title};
use chrono::NaiveDate;
use locus_contract::{BranchGraph, DecisionStatus, ItemKind, ProjectKind, ProjectView, SessionState};
use locus_theme::{SEP, c, glyph};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

pub const KEYS: &[(&str, &str)] = &[
    ("␣ or i", "ask about this project"),
    ("n n", "new; again for the full timeline"),
    ("r", "mark the selected new item read"),
    ("x", "a chore done"),
    ("d", "decisions"),
    ("a", "answered"),
    ("o", "the dossier"),
    ("p", "interview prep (an application)"),
    ("m", "pin or follow"),
    ("← →", "branches: older and newer, through the whole history"),
];

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

/// New, grouped by kind (round 2: "new being grouped by kind would be good"): each group's
/// heading and the indexes of its items, groups in this order, items newest first.
fn new_groups(v: &ProjectView) -> Vec<(&'static str, Vec<usize>)> {
    let group = |k: ItemKind| match k {
        ItemKind::Recheck | ItemKind::Redecided | ItemKind::Decision => 0,
        ItemKind::Answer => 1,
        ItemKind::ScoutFind | ItemKind::Reading | ItemKind::Idea => 2,
        ItemKind::Commits | ItemKind::Chat => 3,
        ItemKind::Dossier | ItemKind::Ink => 4,
        ItemKind::Mail | ItemKind::Calendar | ItemKind::AutoAction => 5,
    };
    let names = [
        "decisions",
        "answers",
        "finds and ideas",
        "work",
        "dossier and ink",
        "elsewhere",
    ];
    (0..names.len())
        .map(|g| {
            (
                names[g],
                (0..v.new.len())
                    .filter(|&i| group(v.new[i].kind) == g)
                    .collect::<Vec<_>>(),
            )
        })
        .filter(|(_, v)| !v.is_empty())
        .collect()
}

/// New's items in the order they are drawn, for selection.
fn new_order(v: &ProjectView) -> Vec<usize> {
    new_groups(v).into_iter().flat_map(|(_, v)| v).collect()
}

fn sorted_sessions(v: &ProjectView) -> Vec<&locus_contract::Session> {
    let mut ss: Vec<_> = v.sessions.iter().collect();
    ss.sort_by_key(|s| (s.state, std::cmp::Reverse(s.since)));
    ss
}

pub fn section_len(app: &App, sec: char) -> usize {
    let Some(v) = view(app) else { return 0 };
    match sec {
        'g' => v.graph.as_ref().map_or(0, |g| g.lanes.len()),
        'n' => v.new.len(),
        's' => v.sessions.len(),
        _ => v.chores.len(),
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
            format!(" {} claude working{SEP}{} ", spin(app.frame), ago(now, s.since)),
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
    // Each thing behind a key sits where its context is (round 2d): the dossier, which bears on
    // everything, centred at the top; decisions beside the open questions; the timeline with
    // New; answered at the end of the ask line.
    let centre = view.map(|v| centre_title(app, v)).unwrap_or_default();
    let block = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(st("overlay0"))
        .title(Line::from(t).left_aligned())
        .title(centre.centered())
        .title(Line::from(right).right_aligned());
    let inner = block.inner(area);
    f.render_widget(block, area);
    let Some(v) = view else {
        draw_missing(f, inner, app);
        return;
    };
    let w = inner.width;

    let where_lines = where_you_are(v, w);
    let groups = new_groups(v);
    let new_h = if v.new.is_empty() {
        2
    } else {
        1 + groups.iter().map(|(_, i)| i.len()).sum::<usize>() as u16
    };
    let has_graph = v.graph.is_some();
    let question_lines = questions(app, v, w);
    // Sections size to their content (round 2f, a rule for all of locus): each gets its least
    // useful height, then spare rows go to whichever has more to show (every session, every
    // chore, more of the branch's story), and only what is left becomes space above the ask line.
    let ss = v.sessions.len() as u16;
    let ch = v.chores.len() as u16;
    let mins = [
        if has_graph { graph_need(app, v, w, 3) } else { 0 },
        1 + ss.clamp(1, 3) + u16::from(ss > 3),
        1 + ch.clamp(1, 3),
    ];
    let wants = [
        if has_graph { graph_need(app, v, w, 8) } else { 0 },
        1 + ss.max(1),
        1 + ch.max(1),
    ];
    // Everything that is not one of the three: where, questions, new, the ask line and the gaps.
    let fixed = where_lines.len() as u16
        + question_lines.len() as u16
        + new_h
        + 2
        + 4
        + u16::from(!question_lines.is_empty())
        + u16::from(has_graph);
    let mut heights = mins;
    let mut spare = inner.height.saturating_sub(fixed + mins.iter().sum::<u16>());
    // Sessions first (live work), then chores, then the branch's story.
    for i in [1usize, 2, 0] {
        let more = wants[i].saturating_sub(heights[i]).min(spare);
        heights[i] += more;
        spare -= more;
    }
    let [graph_h, ses_h, ch_h] = heights;
    let gap = Constraint::Length(1);
    let rects = Layout::vertical([
        Constraint::Length(where_lines.len() as u16),
        gap,
        Constraint::Length(question_lines.len() as u16),
        if question_lines.is_empty() {
            Constraint::Length(0)
        } else {
            gap
        },
        Constraint::Length(graph_h),
        if has_graph { gap } else { Constraint::Length(0) },
        Constraint::Length(new_h),
        gap,
        Constraint::Length(ses_h),
        gap,
        Constraint::Length(ch_h),
        // What is still spare sits here, so the ask line is always at the foot (round 2b).
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .split(inner);
    let (where_r, q_r, graph_r, new_r, ses_r, ch_r, spare_r, ask_r) = (
        rects[0], rects[2], rects[4], rects[6], rects[8], rects[10], rects[11], rects[12],
    );
    crate::ui::summary_box(f, spare_r, project_summary(app, v));

    lines(f, where_r, where_lines);
    lines(f, q_r, question_lines);
    if let Some(g) = &v.graph {
        draw_graph(f, graph_r, app, g);
    }
    draw_new(f, new_r, app, v);
    draw_sessions(f, ses_r, app, v);
    draw_chores(f, ch_r, app, v);
    draw_ask(f, ask_r, app, v);

    let focused = match app.focus {
        Some('g') => Some(graph_r),
        Some('n') => Some(new_r),
        Some('s') => Some(ses_r),
        Some('c') => Some(ch_r),
        _ => None,
    };
    if let Some(r) = focused {
        let buf = f.buffer_mut();
        for y in r.y..r.y + r.height {
            if let Some(cell) = buf.cell_mut((area.x, y)) {
                cell.set_symbol("┃").set_style(st("blue"));
            }
        }
    }
}

/// Where you are, drawn to catch the eye (round 2: "should draw your eye, and now it reads like
/// a block of text"): labelled parts, the state free to wrap, and next on its own band, set
/// apart by a blank line.
fn where_you_are(v: &ProjectView, w: u16) -> Vec<Line<'static>> {
    let mut out = vec![Line::from(vec![
        Span::styled("─ ", st("peach")),
        Span::styled("where you are", bold(st("peach"))),
        Span::styled(
            format!(" {}", "─".repeat(usize::from(w).saturating_sub(16))),
            st("surface2"),
        ),
    ])];
    let Some(wh) = &v.where_you_are else {
        out.push(Line::from(Span::styled("   nothing written yet", dim())));
        return out;
    };
    let body = usize::from(w).saturating_sub(16);
    let mut part = |label: &str, text_: &str, style: Style, max: usize| {
        for (i, l) in wrap(text_, body).into_iter().take(max).enumerate() {
            out.push(Line::from(vec![
                Span::styled(format!("   {:<11}", if i == 0 { label } else { "" }), dim()),
                Span::styled(l, style),
            ]));
        }
    };
    part("now", &wh.state, text(), 3);
    if let Some(s) = &wh.short {
        part("shortfall", s, st("subtext0"), 2);
    }
    // Next, set apart by a dashed line rather than a band (round 2b).
    out.push(Line::from(Span::styled(
        format!("   {}", "╌".repeat(usize::from(w).saturating_sub(6))),
        st("surface2"),
    )));
    for (i, l) in wrap(&wh.next, body).into_iter().take(2).enumerate() {
        out.push(Line::from(vec![
            Span::styled(
                format!("   {:<11}", if i == 0 { "next ▸" } else { "" }),
                bold(st("peach")),
            ),
            Span::styled(l, bold(text())),
        ]));
    }
    out
}

/// What sits behind this view's keys, each with its state (round 2: "otherwise there is no
/// way to know about the two pages behind keys"; round 2b: in "a more sensible place, and laid
/// out in a way that is easier to read"). (key, glyph, colour, name, state)
fn more_rows(app: &App, v: &ProjectView) -> Vec<(&'static str, &'static str, &'static str, &'static str, String)> {
    let pf = app.fixtures.projects.get(&v.project);
    let reg = app.fixtures.registry.get(&v.project);
    let mut out = Vec::new();
    // Decisions and answered are always named, saying so when there are none, so where they
    // sit does not change from project to project (round 2g).
    let repo = reg.is_some_and(|p| p.kind == ProjectKind::Repo);
    match pf.and_then(|p| p.decisions.as_ref()) {
        Some(d) => {
            let re = d.items.iter().filter(|x| x.status == DecisionStatus::Recheck).count();
            let state = if re > 0 {
                format!("{}, {re} worth re-checking", d.items.len())
            } else {
                d.items.len().to_string()
            };
            out.push(("d", glyph::HOLDING, "green", "decisions", state));
        }
        None if repo => out.push(("d", glyph::HOLDING, "green", "decisions", "no log yet".into())),
        None => {}
    }
    match pf.and_then(|p| p.answered.as_ref()) {
        Some(a) => {
            let unread = a.items.iter().filter(|x| x.unread).count();
            let state = if unread > 0 {
                format!("{}, {unread} new", a.items.len())
            } else {
                a.items.len().to_string()
            };
            out.push(("a", glyph::ANSWER, "sky", "answered", state));
        }
        None => out.push(("a", glyph::ANSWER, "sky", "answered", "nothing yet".into())),
    }
    if reg.is_some_and(|p| p.depth.is_some()) {
        let rebuilt = v
            .new
            .iter()
            .chain(
                pf.and_then(|p| p.changes.as_ref())
                    .map(|c| c.entries.as_slice())
                    .unwrap_or(&[]),
            )
            .filter(|i| i.kind == ItemKind::Dossier)
            .map(|i| i.at)
            .max();
        let state = rebuilt
            .map(|t| format!("rebuilt {}", date(t.date_naive())))
            .unwrap_or_else(|| "not built".into());
        out.push(("o", glyph::DOSSIER, "blue", "dossier", state));
    }
    if reg.is_some_and(|p| p.kind == ProjectKind::Application) {
        let state = if app.fixtures.prep.contains_key(&v.project) {
            "a plan".into()
        } else {
            "no plan yet".into()
        };
        out.push(("p", glyph::IDEA, "peach", "interview prep", state));
    }
    let since = pf
        .and_then(|p| p.changes.as_ref())
        .map(|c| format!("since {}", day(c.since)));
    out.push((
        "n n",
        glyph::COMMITS,
        "green",
        "full timeline",
        since.unwrap_or_default(),
    ));
    out
}

/// The top border's centre: the dossier for a repo (it bears on everything), interview prep for
/// an application.
fn centre_title(app: &App, v: &ProjectView) -> Line<'static> {
    let rows = more_rows(app, v);
    let Some((k, g, col, name, state)) = rows.iter().find(|r| r.0 == "o" || r.0 == "p") else {
        return Line::default();
    };
    Line::from(vec![
        Span::styled(format!(" {g} "), st(col)),
        Span::styled(name.to_string(), bold(text())),
        Span::styled(format!(" {state} "), dim()),
        Span::styled(format!("{k} "), bold(keyst())),
    ])
}

/// Open questions from the dossier: the bigger picture behind next (round 2d: "I want it to
/// always remind me of what is next to address bigger picture"), with decisions at the right of
/// its heading, since a question and the decisions it may reopen belong together.
fn questions(app: &App, v: &ProjectView, w: u16) -> Vec<Line<'static>> {
    if v.questions.is_empty() {
        return vec![];
    }
    let right = match more_rows(app, v).into_iter().find(|r| r.0 == "d") {
        Some((k, g, col, name, state)) => vec![
            Span::styled(format!("{g} "), st(col)),
            Span::styled(format!("{name} "), text()),
            Span::styled(format!("{state}  "), dim()),
            Span::styled(format!("{k} "), bold(keyst())),
        ],
        None => vec![],
    };
    let mut head = vec![
        Span::styled("─ ", st("surface2")),
        Span::styled("open questions", bold(text())),
        Span::styled(" · from the dossier ", dim()),
    ];
    let used: usize = head.iter().chain(right.iter()).map(|s| s.content.width()).sum();
    head.push(Span::styled(
        "─".repeat(usize::from(w).saturating_sub(used + 2)),
        st("surface2"),
    ));
    head.push(Span::raw(" "));
    head.extend(right);
    let mut out = vec![Line::from(head)];
    let body = usize::from(w).saturating_sub(10);
    for (i, q) in v.questions.iter().enumerate() {
        for (j, l) in wrap(q, body).into_iter().take(2).enumerate() {
            out.push(Line::from(vec![
                Span::styled(
                    if j == 0 {
                        format!("   {}  ", i + 1)
                    } else {
                        "      ".into()
                    },
                    bold(st("mauve")),
                ),
                Span::styled(l, text()),
            ]));
        }
    }
    out
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

/// A lane's short name: the worktree's, as git has it (round 2: "labelled with its short
/// worktree name, as those fit better in small spaces"). A merge whose branch name did not
/// survive is named by its stage.
fn short_name(l: &locus_contract::Lane) -> String {
    let b = l.branch.trim_start_matches("worktree-").trim_start_matches("alec/");
    if b.starts_with("merge: ") {
        l.stage.clone()
    } else {
        b.to_string()
    }
}

/// Where each lane sits: its fork and end columns, the row it is packed into, and where its
/// name goes. Shared by drawing and by working out how many rows the graph needs.
struct Placed {
    a: usize,
    b: usize,
    row: usize,
    name_at: usize,
    name: String,
}

#[derive(Clone, Copy, PartialEq)]
enum Ev {
    Milestone(usize),
    Fork(usize),
    End(usize),
}

/// Lay the graph out across `gw` columns. Columns are events in date order, not days (on a day
/// axis nabla's lanes, all within a few days, collapsed into bars): every fork, merge, open tip
/// and milestone in view gets its own column; within a day, lane by lane, fork before end. Rows
/// are packed as git draws them: a lane takes the first row free by the time it forks (round 2:
/// one row a lane made every connector climb past all the earlier rows).
fn layout(
    g: &BranchGraph,
    lanes: &[locus_contract::Lane],
    now: NaiveDate,
    gw: usize,
) -> (Vec<(NaiveDate, u8, Ev)>, Vec<Placed>, usize) {
    let start = lanes.iter().map(|l| l.forked).min().unwrap_or(g.from).max(g.from);
    let mut evs: Vec<(NaiveDate, u8, Ev)> = Vec::new();
    for (i, m) in g.milestones.iter().enumerate().filter(|(_, m)| m.date >= start) {
        evs.push((m.date, 0, Ev::Milestone(i)));
    }
    for (i, l) in lanes.iter().enumerate() {
        evs.push((l.forked, 1, Ev::Fork(i)));
        evs.push((l.merged.or(l.tip).unwrap_or(now), 2, Ev::End(i)));
    }
    evs.sort_by_key(|(d, k, e)| {
        let lane = match e {
            Ev::Milestone(i) | Ev::Fork(i) | Ev::End(i) => *i,
        };
        (*d, *k != 0, lane, *k)
    });
    let n = evs.len().max(2);
    let col = |slot: usize| -> usize { (1 + slot * gw.saturating_sub(4) / (n - 1)).min(gw - 1) };
    let at = |e: Ev| evs.iter().position(|(_, _, x)| *x == e).map(col).unwrap_or(0);
    let mut row_end: Vec<usize> = Vec::new();
    let mut placed = Vec::new();
    for (i, l) in lanes.iter().enumerate() {
        let a = at(Ev::Fork(i));
        let b = at(Ev::End(i)).max(a);
        let name = format!(" {} ", short_name(l));
        let w = name.chars().count();
        let inside = b - a > w + 2;
        // On the bar when it is long enough, after it when there is room, else before the fork.
        let (name_at, left, extent) = if inside {
            (a + 2, a, b)
        } else if b + w < gw {
            (b + 1, a, b + w)
        } else {
            let at_ = a.saturating_sub(w);
            (at_, at_, b)
        };
        let row = match row_end.iter().position(|&e| e + 2 < left) {
            Some(r) => {
                row_end[r] = extent;
                r
            }
            None => {
                row_end.push(extent);
                row_end.len() - 1
            }
        };
        placed.push(Placed {
            a,
            b,
            row,
            name_at,
            name,
        });
    }
    let rows = row_end.len();
    (evs, placed, rows)
}

/// Rows the graph needs at this width: main, its packed rows, the axis, a blank and the
/// selected lane's heading and summary.
fn graph_need(app: &App, v: &ProjectView, w: u16, cap: usize) -> u16 {
    let Some(g) = &v.graph else { return 0 };
    let gw = usize::from(w).saturating_sub(12).max(10);
    let now = app.fixtures.now().date_naive();
    // Measure the window as it will be drawn, not every lane packed together.
    let focus = (app.focus == Some('g')).then(|| lane_at(app, g));
    let (a, b) = window(g, now, gw, usize::MAX, focus);
    let (_, _, rows) = layout(g, &g.lanes[a..b], now, gw);
    // The lane whose summary is shown: the selected one when focused, else the newest.
    let sel = if app.focus == Some('g') {
        lane_at(app, g)
    } else {
        g.lanes.len().saturating_sub(1)
    };
    let detail = 1 + g.lanes.get(sel).map_or(0, |l| summary_lines(l, w, cap).len());
    (4 + rows + detail) as u16
}

/// The project in two lines, for rows nothing else wanted: this week's time and what is live,
/// then the latest change.
fn project_summary(app: &App, v: &ProjectView) -> Vec<Line<'static>> {
    let now = app.fixtures.now();
    let hours = app
        .fixtures
        .work
        .totals()
        .into_iter()
        .find(|(p, _)| *p == v.project)
        .map_or(0.0, |(_, h)| h);
    let live = v
        .sessions
        .iter()
        .filter(|s| matches!(s.state, SessionState::Working | SessionState::NeedsYou))
        .count();
    let unread = v.unread();
    let mut out = vec![Line::from(vec![
        Span::styled(format!(" {hours:.1} h this week"), text()),
        Span::styled(format!("{SEP}{live} sessions live{SEP}{unread} new"), dim()),
    ])];
    let latest = v
        .new
        .iter()
        .chain(
            app.fixtures
                .projects
                .get(&v.project)
                .and_then(|p| p.changes.as_ref())
                .map(|c| c.entries.as_slice())
                .unwrap_or(&[]),
        )
        .max_by_key(|i| i.at);
    if let Some(it) = latest {
        out.push(Line::from(vec![
            Span::styled(format!(" latest, {} ago  ", ago(now, it.at)), dim()),
            Span::styled(it.text.clone(), text()),
        ]));
    }
    out
}

/// Which lanes are drawn: unfocused, the newest that fit; focused, a window around the selected
/// lane, so ← and → walk back through the whole history (round 2c). Each event gets at least
/// MIN_GAP columns, so close branches are not squeezed together; the cost is a longer scroll
/// (round 2e: "do not be afraid of spreading them out a little bit more").
fn window(g: &BranchGraph, now: NaiveDate, gw: usize, room: usize, sel: Option<usize>) -> (usize, usize) {
    const MIN_GAP: usize = 6;
    let fits = |a: usize, b: usize| 2 * (b - a) * MIN_GAP <= gw && layout(g, &g.lanes[a..b], now, gw).2 <= room;
    let len = g.lanes.len();
    match sel {
        Some(sel) => {
            let (mut a, mut b) = (sel, (sel + 1).min(len));
            loop {
                if b < len && fits(a, b + 1) {
                    b += 1;
                } else if a > 0 && fits(a - 1, b) {
                    a -= 1;
                } else {
                    break;
                }
            }
            (a, b)
        }
        None => {
            let mut a = 0;
            while a + 1 < len && !fits(a, len) {
                a += 1;
            }
            (a, len)
        }
    }
}

/// The branch graph: main along the top with its milestones named above it, each lane hanging
/// from main by a connector at its fork and rising back at its merge, named on the bar itself;
/// the selected lane's work summarised underneath (round 2: lanes "hover in their row only",
/// "I do not know what those two green dots are", "use the extra space to show what each branch
/// is on the branch itself", "hovering over the branch should show more detail").
fn draw_graph(f: &mut Frame, area: Rect, app: &App, g: &BranchGraph) {
    let now = app.fixtures.now().date_naive();
    let focus = app.focus == Some('g');
    let w = usize::from(area.width);
    let lead = 6;
    let gw = w.saturating_sub(lead + 6).max(10);
    let sel_all = lane_at(app, g);
    let shown_i = if focus {
        sel_all
    } else {
        g.lanes.len().saturating_sub(1)
    };
    let detail_h = 1 + g
        .lanes
        .get(shown_i)
        .map_or(0, |l| summary_lines(l, area.width, 3).len());
    let room = usize::from(area.height).saturating_sub(4 + detail_h).max(1);
    let (first, end) = window(g, now, gw, room, focus.then_some(sel_all));
    let len = g.lanes.len();
    let (evs, placed, rows) = layout(g, &g.lanes[first..end], now, gw);
    let lanes = &g.lanes[first..end];
    let extra = if first > 0 || end < len {
        format!("{len} branches · {} to {} shown · ← → older and newer", first + 1, end)
    } else {
        format!("{len} branches")
    };
    let mut out = vec![rule("branches", &extra, Some('g'), focus, area.width)];
    let n = evs.len().max(2);
    let col = |slot: usize| -> usize { (1 + slot * gw.saturating_sub(4) / (n - 1)).min(gw - 1) };

    let mut cv: Vec<Vec<(char, Style)>> = vec![vec![(' ', Style::default()); gw]; 1 + rows];
    for cell in cv[0].iter_mut() {
        *cell = ('─', st("overlay2"));
    }
    for (i, (l, p)) in lanes.iter().zip(&placed).enumerate() {
        let selected = focus && first + i == sel_all;
        // The selected branch on a light band in its own colour, the others dimmed while the
        // graph has focus (round 2b: "hard to tell what is selected"; 2c: solid was "too bold").
        let colour = c(PROJECT_COLOURS[(first + i) % PROJECT_COLOURS.len()]);
        let style = if selected {
            bold(Style::default().fg(colour).bg(c("surface1")))
        } else if focus {
            Style::default().fg(locus_theme::fade(colour, 0.55))
        } else {
            Style::default().fg(colour)
        };
        let r = 1 + p.row;
        for (x, cell) in cv[r].iter_mut().enumerate().take(p.b + 1).skip(p.a) {
            *cell = (
                if x == p.a {
                    '╰'
                } else if x < p.b {
                    '━'
                } else if l.merged.is_some() {
                    '╯'
                } else {
                    '○'
                },
                style,
            );
        }
        for (k, ch) in p.name.chars().enumerate() {
            if p.name_at + k < gw {
                cv[r][p.name_at + k] = (ch, bold(style));
            }
        }
        cv[0][p.a] = ('┬', st("overlay2"));
        join(&mut cv, p.a, r, style);
        if l.merged.is_some() {
            cv[0][p.b] = if cv[0][p.b].0 == '┬' {
                ('┼', st("overlay2"))
            } else {
                ('┴', st("overlay2"))
            };
            join(&mut cv, p.b, r, style);
        }
    }
    // Milestones are not drawn: too few and too thin to be worth the noise (round 2b).
    let earlier = first;
    for (r, cells) in cv.into_iter().enumerate() {
        // At the start of history main begins with a green dot; otherwise ◂ says there is more.
        let mut spans = vec![if r == 0 {
            Span::styled(
                format!("{:>4} {}", g.trunk, if earlier > 0 { "◂" } else { "●" }),
                bold(st(if earlier > 0 { "subtext1" } else { "green" })),
            )
        } else {
            Span::raw(" ".repeat(lead))
        }];
        spans.extend(cells.into_iter().map(|(ch, s)| Span::styled(ch.to_string(), s)));
        if r == 0 {
            spans.push(Span::styled(if end < g.lanes.len() { " ▸" } else { " now" }, dim()));
        }
        out.push(Line::from(spans));
    }
    // The axis: a date wherever it changes and there is room for it.
    // At the start of history the first label says so (round 2e: "mark the start more clearly,
    // so I know what the first branch was").
    let mut axis: Vec<char> = vec![' '; lead + gw + 14];
    let mut last_end = 0;
    let mut last_date = None;
    for (slot, (d, _, _)) in evs.iter().enumerate() {
        if last_date == Some(*d) {
            continue;
        }
        let x = lead + col(slot);
        let label = if slot == 0 && earlier == 0 {
            format!("start, {}", date(*d))
        } else {
            date(*d)
        };
        let text_: Vec<char> = label.chars().collect();
        if x >= last_end && x + text_.len() < axis.len() {
            axis[x..x + text_.len()].copy_from_slice(&text_);
            last_end = x + text_.len() + 2;
            last_date = Some(*d);
        }
    }
    out.push(Line::from(Span::styled(axis.into_iter().collect::<String>(), dim())));
    out.push(Line::from(""));
    // The selected lane's work, or the newest lane's when the graph is not focused.
    let shown = if focus { g.lanes.get(sel_all) } else { g.lanes.last() };
    if let Some(l) = shown {
        let state = match l.merged {
            Some(m) => format!("{} to {}, merged", date(l.forked), date(m)),
            None => format!(
                "open since {}, last commit {}",
                date(l.forked),
                l.tip.map(date).unwrap_or_default()
            ),
        };
        let commits = if l.commits == 1 {
            "1 commit".to_string()
        } else {
            format!("{} commits", l.commits)
        };
        out.push(Line::from(vec![
            Span::styled(format!("   {}", short_name(l)), bold(text())),
            Span::styled(format!("{SEP}{}{SEP}{state}{SEP}{commits}", l.stage), dim()),
        ]));
        // The story gets whatever rows the graph was given beyond its branches.
        let cap = usize::from(area.height).saturating_sub(out.len() + 1).max(1);
        for line in summary_lines(l, area.width, cap) {
            out.push(Line::from(Span::styled(format!("     {line}"), text())));
        }
    }
    lines(f, area, out);
}

/// The selected lane. The graph's selection counts back from the newest branch, so entering
/// the graph starts on the latest work, ← steps back in time and → forward (round 2d: entering
/// it landed on the oldest branch, and ← had nowhere to go).
fn lane_at(app: &App, g: &BranchGraph) -> usize {
    let n = g.lanes.len();
    n.saturating_sub(1)
        .saturating_sub(app.sel('g').min(n.saturating_sub(1)))
}

/// The selected lane's summary, wrapped, at most three lines.
fn summary_lines(l: &locus_contract::Lane, w: u16, cap: usize) -> Vec<String> {
    let mut v = wrap(&l.summary, usize::from(w).saturating_sub(8));
    if v.len() > cap {
        v.truncate(cap);
        if let Some(last) = v.last_mut() {
            last.push(locus_theme::ELLIPSIS);
        }
    }
    v
}

/// A connector in column `x` from main (row 0) down to lane row `r`, through empty cells only.
fn join(cv: &mut [Vec<(char, Style)>], x: usize, r: usize, style: Style) {
    for rr in cv.iter_mut().take(r).skip(1) {
        if rr[x].0 == ' ' {
            rr[x] = ('│', style);
        }
    }
}

fn draw_new(f: &mut Frame, area: Rect, app: &App, v: &ProjectView) {
    let now = app.fixtures.now();
    let focus = app.focus == Some('n');
    let unread = (0..v.new.len()).filter(|&i| is_unread(app, v, i)).count();
    // The full timeline sits with New, whose history it is (round 2d).
    let timeline = more_rows(app, v)
        .into_iter()
        .find(|r| r.0 == "n n")
        .map(|(_, _, _, _, state)| format!(" · n n full timeline {state}"))
        .unwrap_or_default();
    let extra = if v.new.is_empty() {
        format!("nothing new{timeline}")
    } else {
        format!("{unread} unread{timeline}")
    };
    let mut out = vec![rule("new", &extra, Some('n'), focus, area.width)];
    let sel = focus.then(|| app.sel('n'));
    let mut pos = 0;
    // Each kind is named in a column of its own on its first item, rather than on a row of its
    // own, so grouping costs no height.
    for (heading, idx) in new_groups(v) {
        for (k, i) in idx.into_iter().enumerate() {
            let label = if k == 0 { heading } else { "" };
            let it = &v.new[i];
            let (g, gc) = item_glyph(it.kind);
            let unread = is_unread(app, v, i);
            // The unread marker sits on the right, as the counts do on Home (round 2).
            let mut right = vec![];
            if unread {
                right.push(Span::styled(format!("{} ", glyph::UNREAD), st("peach")));
            }
            right.push(Span::styled(format!("{:>4} ", ago(now, it.at)), dim()));
            out.push(row(
                vec![
                    pointer(pos, sel),
                    Span::styled(format!(" {label:<16}"), bold(st("subtext1"))),
                    Span::styled(format!("{g} "), st(gc)),
                    Span::styled(it.text.clone(), if unread { text() } else { dim() }),
                ],
                right,
                area.width,
            ));
            pos += 1;
        }
    }
    if v.new.is_empty() {
        let last = app
            .fixtures
            .projects
            .get(&v.project)
            .and_then(|p| p.changes.as_ref())
            .and_then(|c| c.entries.first().map(|e| e.at));
        let msg = match last {
            Some(t) => format!("   nothing new; the last change was {} ago", ago(now, t)),
            None => "   nothing new".to_string(),
        };
        out.push(Line::from(Span::styled(msg, dim())));
    }
    lines(f, area, out);
}

/// Every Claude session in the project, live ones first, each with what it is for and what it
/// needs in plain words across the full line (round 2: "a good, intuitive and clear natural
/// language summary of what they need"). Unfocused, the three most pressing; focused, the rest.
fn draw_sessions(f: &mut Frame, area: Rect, app: &App, v: &ProjectView) {
    let now = app.fixtures.now();
    let focus = app.focus == Some('s');
    let ss = sorted_sessions(v);
    let live = ss
        .iter()
        .filter(|s| matches!(s.state, SessionState::Working | SessionState::NeedsYou))
        .count();
    let extra = format!("{} · {live} live", ss.len());
    let mut out = vec![rule("sessions", &extra, Some('s'), focus, area.width)];
    // As many as the section was given; a last line counts the rest when they do not fit.
    let rows = usize::from(area.height.saturating_sub(1));
    let overflow = !focus && ss.len() > rows;
    let room = rows.saturating_sub(usize::from(overflow));
    let sel = focus.then(|| app.sel('s'));
    let first = sel.map_or(0, |s| s.saturating_sub(room.saturating_sub(1)));
    for (i, s) in ss.iter().enumerate().skip(first).take(room) {
        let (g, gc, word) = session_glyph(s.state);
        let g = if s.state == SessionState::Working {
            spin(app.frame)
        } else {
            g
        };
        let name = format!("{}{}", s.name, if s.background { " (bg)" } else { "" });
        let what = s.summary.clone().or_else(|| s.doing.clone()).unwrap_or_default();
        let right = if s.state == SessionState::NeedsYou {
            vec![Span::styled("⏎ jump ", keyst())]
        } else {
            vec![]
        };
        out.push(row(
            vec![
                pointer(i, sel),
                Span::styled(format!("{g} "), st(gc)),
                Span::styled(format!("{:<20}", crate::ui::truncate(&name, 19)), text()),
                Span::styled(format!("{:<9}", word), st(gc)),
                Span::styled(format!("{:>4}   ", ago(now, s.since)), dim()),
                Span::styled(
                    what,
                    if matches!(s.state, SessionState::Done | SessionState::Idle) {
                        dim()
                    } else {
                        text()
                    },
                ),
            ],
            right,
            area.width,
        ));
    }
    if overflow {
        out.push(Line::from(Span::styled(
            format!("     + {} older · s to see them", ss.len() - room),
            dim(),
        )));
    }
    if ss.is_empty() {
        out.push(Line::from(Span::styled("   no sessions", dim())));
    }
    lines(f, area, out);
}

/// Chores are life admin that fits a gap while Claude works (round 2: "laundry, or responding to
/// an email, or picking up furniture"), the same list on every project.
fn draw_chores(f: &mut Frame, area: Rect, app: &App, v: &ProjectView) {
    let focus = app.focus == Some('c');
    let mut out = vec![rule("chores", "for a gap while you wait", Some('c'), focus, area.width)];
    let sel = focus.then(|| app.sel('c'));
    for (i, ch) in v
        .chores
        .iter()
        .enumerate()
        .take(usize::from(area.height.saturating_sub(1)))
    {
        let done = app.marked(&chore_key(v, i)) == Some(&Mark::Done);
        let mut right = vec![Span::styled(format!("{} min ", ch.minutes), dim())];
        if let Some(d) = ch.due {
            right.insert(0, Span::styled(format!("due {}   ", day(d)), st("peach")));
        }
        out.push(row(
            vec![
                pointer(i, sel),
                Span::styled(format!("  {} ", glyph::CHORES), st("teal")),
                Span::styled(ch.title.clone(), if done { struck(text()) } else { text() }),
            ],
            right,
            area.width,
        ));
    }
    if v.chores.is_empty() {
        out.push(Line::from(Span::styled("   none fit now", dim())));
    }
    lines(f, area, out);
}

fn draw_ask(f: &mut Frame, area: Rect, app: &App, v: &ProjectView) {
    let asking = matches!(app.mode, Mode::Ask(_));
    let left = match &app.mode {
        Mode::Ask(q) => vec![
            Span::styled("   ask › ", st("mauve")),
            Span::styled(q.clone(), text()),
            Span::styled("▏", st("mauve")),
        ],
        _ => vec![
            Span::styled("   ask › ", dim()),
            Span::styled("␣", keyst()),
            Span::styled(" to ask about this project · the answer comes back here", dim()),
        ],
    };
    // What was asked before sits at the right of the ask rule, as decisions sit on the open
    // questions' rule (round 2e).
    let mut head = vec![
        Span::styled("─ ", st(if asking { "blue" } else { "surface2" })),
        Span::styled("ask", bold(if asking { st("blue") } else { text() })),
    ];
    let right = match more_rows(app, v).into_iter().find(|r| r.0 == "a") {
        Some((k, g, col, name, state)) => vec![
            Span::styled(format!("{g} "), st(col)),
            Span::styled(format!("{name} "), text()),
            Span::styled(format!("{state}  "), dim()),
            Span::styled(format!("{k} "), bold(keyst())),
        ],
        None => vec![],
    };
    let used: usize = head.iter().chain(right.iter()).map(|s| s.content.width()).sum();
    let tail = if right.is_empty() { 1 } else { 2 };
    head.push(Span::styled(
        format!(" {}", "─".repeat(usize::from(area.width).saturating_sub(used + tail))),
        st("surface2"),
    ));
    if !right.is_empty() {
        head.push(Span::raw(" "));
        head.extend(right);
    }
    lines(f, area, vec![Line::from(head), Line::from(left)]);
}

pub fn key(app: &mut App, k: KeyEvent) -> bool {
    let Some(v) = view(app).cloned() else {
        return matches!(k.code, KeyCode::Char('m')) && toggle_pin(app);
    };
    let focus = app.focus;
    match (focus, k.code) {
        (Some('n'), KeyCode::Char('n')) => app.open(ScreenId::Changes),
        (Some('n'), KeyCode::Enter | KeyCode::Char('r')) => {
            if let Some(&i) = new_order(&v).get(app.sel('n')) {
                let it = &v.new[i];
                let note = if k.code == KeyCode::Enter {
                    format!("would open: {}", it.text)
                } else {
                    "marked read".into()
                };
                app.mark(new_key(&v, i), Mark::Read, note);
            }
        }
        (Some('s'), KeyCode::Enter) => {
            if let Some(s) = sorted_sessions(&v).get(app.sel('s')) {
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
        // ← and → walk the branches from anywhere on the page (round 2d), focusing the graph.
        (_, KeyCode::Left | KeyCode::Right) => {
            app.focus = Some('g');
            let n = v.graph.as_ref().map_or(0, |g| g.lanes.len());
            let d = if k.code == KeyCode::Left { 1 } else { -1 };
            app.move_sel('g', n, d);
        }
        (Some('g'), KeyCode::Enter) => app.note = Some("would list the branch's change records".into()),
        // Space asks (round 2: "a better key than i"); i still works.
        (_, KeyCode::Char(' ' | 'i')) => app.mode = Mode::Ask(String::new()),
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
            if kind == Some(ProjectKind::Application) {
                app.open(ScreenId::Prep);
            } else {
                app.note = Some("p is interview prep, for an application".into());
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
