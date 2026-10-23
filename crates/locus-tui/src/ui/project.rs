//! The Project view (Alt-2): one screen in every state. Where you are (now, the shortfall, what
//! is new, next), the open questions, the branch graph (plan D.1), the decision log, this
//! project's sessions, and the ask line.
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
    ("n", "the full timeline"),
    ("r", "mark the selected new item read"),
    ("d", "decisions: the full log"),
    ("a", "ask; ⏎ there opens answered"),
    ("o", "the dossier"),
    ("p", "interview prep (an application)"),
    ("m", "pin or follow"),
    ("← →", "branches: older and newer, through the whole history"),
    ("⏎ on a branch", "its change records; ↑ ↓ scroll, esc closes"),
];

pub fn view(app: &App) -> Option<&ProjectView> {
    let id = app.project_id()?;
    app.fixtures.projects.get(&id).map(|p| &p.view)
}

fn new_key(view: &ProjectView, i: usize) -> String {
    format!("new:{}:{i}", view.project)
}

pub fn answer_key(project: &str, i: usize) -> String {
    format!("answer:{project}:{i}")
}

/// The newest answer not yet read, with its place in the log: what Enter on ask opens.
fn new_answer<'a>(app: &'a App, v: &ProjectView) -> Option<(usize, &'a locus_contract::Answer)> {
    let ans = app.fixtures.projects.get(&v.project)?.answered.as_ref()?;
    ans.items
        .iter()
        .enumerate()
        .filter(|(i, a)| a.unread && app.marked(&answer_key(&v.project, *i)).is_none())
        .max_by_key(|(_, a)| a.at)
}

fn is_unread(app: &App, view: &ProjectView, i: usize) -> bool {
    view.new.get(i).is_some_and(|it| it.unread) && app.marked(&new_key(view, i)).is_none()
}

/// New, grouped by kind (round 2: "new being grouped by kind would be good"): each group's
/// heading and the indexes of its items, groups in this order, items newest first.
fn new_groups(v: &ProjectView) -> Vec<(&'static str, Vec<usize>)> {
    // Decisions are never in New (round 2l): the decisions section marks a new one. Should one
    // arrive anyway, it is filed with the dossier rather than lost.
    let group = |k: ItemKind| match k {
        ItemKind::Answer => 0,
        ItemKind::ScoutFind | ItemKind::Reading | ItemKind::Idea => 1,
        ItemKind::Commits | ItemKind::Chat => 2,
        ItemKind::Dossier | ItemKind::Ink | ItemKind::Recheck | ItemKind::Redecided | ItemKind::Decision => 3,
        ItemKind::Mail | ItemKind::Calendar | ItemKind::AutoAction => 4,
    };
    let names = ["answer", "find, idea", "work", "dossier", "elsewhere"];
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

/// The decision log, if this project keeps one.
fn decisions<'a>(app: &'a App, v: &ProjectView) -> Option<&'a locus_contract::Decisions> {
    app.fixtures.projects.get(&v.project).and_then(|p| p.decisions.as_ref())
}

/// A repo always has the section, saying so when it has no log yet (round 2g: where things sit
/// does not change from project to project); an application or a course has none.
fn has_decisions(app: &App, v: &ProjectView) -> bool {
    decisions(app, v).is_some()
        || app
            .fixtures
            .registry
            .get(&v.project)
            .is_some_and(|p| p.kind == ProjectKind::Repo)
}

/// The log in the section's order: what to re-check, then what is new, then the rest newest
/// first, a replaced decision last (round 2l).
fn decision_order(ds: &locus_contract::Decisions) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..ds.items.len()).collect();
    idx.sort_by_key(|&i| {
        let d = &ds.items[i];
        let rank = match d.status {
            DecisionStatus::Recheck | DecisionStatus::Review => 0,
            _ if d.new => 1,
            DecisionStatus::Holding => 2,
            DecisionStatus::Redecided { .. } => 3,
        };
        (rank, std::cmp::Reverse(d.date))
    });
    idx
}

/// Where you are's selectable parts run now, the shortfall (if any), each new item, next:
/// this is the first new item's place.
fn first_new(v: &ProjectView) -> usize {
    1 + usize::from(v.where_you_are.as_ref().is_some_and(|w| w.short.is_some()))
}

/// Where you are's selectable rows on a screen that draws it (the Project view, an application).
pub(crate) fn section_len_where(app: &App, screen: ScreenId, v: &ProjectView) -> usize {
    let drawn = app.shown.borrow().get(&(screen, 'y')).map_or(0, |v| v.len());
    first_new(v) + drawn + usize::from(v.where_you_are.is_some())
}

/// What a section drew, recorded by the draw so the arrows walk only that.
fn shown(app: &App, sec: char) -> Vec<usize> {
    app.shown
        .borrow()
        .get(&(ScreenId::Project, sec))
        .cloned()
        .unwrap_or_default()
}

fn sorted_sessions(v: &ProjectView) -> Vec<&locus_contract::Session> {
    let mut ss: Vec<_> = v.sessions.iter().collect();
    ss.sort_by_key(|s| (s.state, std::cmp::Reverse(s.since)));
    ss
}

pub fn section_len(app: &App, sec: char) -> usize {
    let Some(v) = view(app) else { return 0 };
    match sec {
        // Now, each new item drawn, then next.
        'y' => first_new(v) + shown(app, 'y').len() + usize::from(v.where_you_are.is_some()),
        'q' => v.questions.len(),
        'g' => v.graph.as_ref().map_or(0, |g| g.lanes.len()),
        'd' => shown(app, 'd').len(),
        's' => v.sessions.len(),
        'a' => 1,
        _ => 0,
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
    // everything, centred at the top; the timeline with what is new, in where you are; the full
    // decision log on the decisions section; answered at the end of the ask line.
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

    let mut base = where_you_are(app, v, w, 0, false).0.len() as u16;
    let has_graph = v.graph.is_some();
    let has_dec = has_decisions(app, v);
    let question_lines = questions(app, v, w);
    // Sections size to their content (round 2f, a rule for all of locus): each gets its least
    // useful height, then spare rows go to whichever has more to show, and only what is left
    // becomes space above the ask line.
    let ss = v.sessions.len() as u16;
    let nn = v.new.len() as u16;
    let nd = decisions(app, v).map_or(0, |d| d.items.len()) as u16;
    // A re-check carries its reason on a line of its own.
    let nr = decisions(app, v).map_or(0, |d| {
        d.items
            .iter()
            .filter(|x| x.status == DecisionStatus::Recheck && x.recheck.is_some())
            .count()
    }) as u16;
    let mins = [
        if has_graph { graph_need(app, v, w, 3, 3) } else { 0 },
        1 + ss.clamp(1, 3) + u16::from(ss > 3),
        // What is new, inside where you are: three rows, the last counting the rest (round 2l).
        if nn > 3 { 3 } else { nn.max(1) },
        if has_dec {
            1 + nd.clamp(1, 2) + u16::from(nd > 2) + nr.min(2)
        } else {
            0
        },
    ];
    let wants = [
        if has_graph { graph_need(app, v, w, 8, 30) } else { 0 },
        1 + ss.max(1),
        nn.max(1),
        if has_dec { 1 + nd.max(1) + nr } else { 0 },
    ];
    // Everything that is not allocated: where you are without its new rows, the questions, the
    // ask line and the gaps.
    let mut fixed = base
        + question_lines.len() as u16
        + 4
        + u16::from(!question_lines.is_empty())
        + u16::from(has_graph)
        + u16::from(has_dec);
    let mut heights = mins;
    // Short of room even for those, the sections give way a row at a time, decisions first,
    // then what is new, the branch's story, sessions, each down to a heading and one
    // row, so the open questions always draw.
    let floors = [
        if has_graph { graph_need(app, v, w, 2, 0) } else { 0 },
        2,
        1,
        u16::from(has_dec) * 2,
    ];
    let mut over = (fixed + heights.iter().sum::<u16>()).saturating_sub(inner.height);
    for i in [3usize, 2, 0, 1] {
        let give = heights[i].saturating_sub(floors[i]).min(over);
        heights[i] -= give;
        over -= give;
    }
    // Still short: where you are keeps a line per part.
    let tight = over > 0;
    if tight {
        let short = where_you_are(app, v, w, 0, true).0.len() as u16;
        fixed -= base - short;
        base = short;
    }
    let _ = base;
    let mut spare = inner.height.saturating_sub(fixed + heights.iter().sum::<u16>());
    // What is new first (everything new should show), then sessions, decisions, the
    // branch's story; an open branch is what you are reading, so it takes spare rows first.
    let order = if app.lane_open && app.focus == Some('g') {
        [0usize, 2, 1, 3]
    } else {
        [2, 1, 3, 0]
    };
    for i in order {
        let more = wants[i].saturating_sub(heights[i]).min(spare);
        heights[i] += more;
        spare -= more;
    }
    let [graph_h, ses_h, new_h, dec_h] = heights;
    let (where_lines, new_shown) = where_you_are(app, v, w, usize::from(new_h), tight);
    app.shown.borrow_mut().insert((ScreenId::Project, 'y'), new_shown);
    let gap = Constraint::Length(1);
    let none = Constraint::Length(0);
    let rects = Layout::vertical([
        Constraint::Length(where_lines.len() as u16),
        gap,
        Constraint::Length(question_lines.len() as u16),
        if question_lines.is_empty() { none } else { gap },
        Constraint::Length(graph_h),
        if has_graph { gap } else { none },
        Constraint::Length(dec_h),
        if has_dec { gap } else { none },
        Constraint::Length(ses_h),
        // What is still spare sits here, so the ask line is always at the foot (round 2b).
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .split(inner);
    let (where_r, q_r, graph_r, dec_r, ses_r, spare_r, ask_r) =
        (rects[0], rects[2], rects[4], rects[6], rects[8], rects[9], rects[10]);
    crate::ui::summary_box(f, spare_r, project_summary(app, v));

    lines(f, where_r, where_lines);
    lines(f, q_r, question_lines);
    if let Some(g) = &v.graph {
        draw_graph(f, graph_r, app, g);
    }
    if has_dec {
        draw_decisions(f, dec_r, app, v);
    }
    draw_sessions(f, ses_r, app, v);
    draw_ask(f, ask_r, app, v);

    let focused = match app.focus {
        Some('y') => Some(where_r),
        Some('q') => Some(q_r),
        Some('g') => Some(graph_r),
        Some('d') => Some(dec_r),
        Some('s') => Some(ses_r),
        Some('a') => Some(ask_r),
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
/// apart by a dashed line. Since round 2l it is the whole state in one place: now, the
/// shortfall, what is new since you last looked, then next ("so you see where you are, being
/// now, shortfall, new, next"). New is read more than worked in, so it has no section of its
/// own; its rows are still walkable, and Enter still opens one. `budget` is the rows new was
/// given (0 measures the rest); the new items drawn are returned for the arrows.
pub(crate) fn where_you_are(
    app: &App,
    v: &ProjectView,
    w: u16,
    budget: usize,
    tight: bool,
) -> (Vec<Line<'static>>, Vec<usize>) {
    // Tight, on a short window, each part keeps one line so the sections below still draw.
    let body = usize::from(w).saturating_sub(16);
    let fit = |t: &str, n: usize| -> Vec<String> {
        if tight {
            vec![crate::ui::truncate(t, body)]
        } else {
            wrap(t, body).into_iter().take(n).collect()
        }
    };
    let now = app.fixtures.now();
    let focus = app.focus == Some('y');
    let sel = focus.then(|| app.sel('y'));
    let unread = (0..v.new.len()).filter(|&i| is_unread(app, v, i)).count();
    // The full timeline sits with what is new, whose history it is (round 2d); narrow, its
    // state goes and the key stays.
    let room = usize::from(w).saturating_sub(20);
    let lead = if unread > 0 {
        format!(" · {unread} unread")
    } else {
        String::new()
    };
    // An application's timeline is on its own page, so it names no key here (round 5).
    let extra = more_rows(app, v)
        .into_iter()
        .filter(|_| v.application.is_none())
        .find(|r| r.0 == "n")
        .map(|(k, _, _, name, state)| {
            let full = format!("{lead} · {name} {state} [{k}]");
            if full.width() <= room {
                full
            } else {
                format!("{lead} · {name} [{k}]")
            }
        })
        .unwrap_or(lead);
    let head_c = if focus { "blue" } else { "surface2" };
    let mut head = vec![
        Span::styled("─ ", st(head_c)),
        Span::styled("where you are", bold(st(if focus { "blue" } else { "peach" }))),
        Span::styled(extra, dim()),
    ];
    let used: usize = head.iter().map(|s| s.content.width()).sum();
    head.push(Span::styled(
        format!(" {}", "─".repeat(usize::from(w).saturating_sub(used + 1))),
        st(head_c),
    ));
    let mut out = vec![Line::from(head)];
    // One selectable part: its first line carries the pointer, and every line of it the band.
    let part = |out: &mut Vec<Line<'static>>, at: usize, label: &str, lines_: Vec<String>, ls: Style, ts: Style| {
        for (j, l) in lines_.into_iter().enumerate() {
            let lead = if j == 0 { pointer(at, sel) } else { Span::raw("  ") };
            let line = row(
                vec![
                    lead,
                    Span::styled(format!(" {:<11}", if j == 0 { label } else { "" }), ls),
                    Span::styled(l, ts),
                ],
                vec![],
                w,
            );
            out.push(if j > 0 && sel == Some(at) {
                line.patch_style(Style::default().bg(c("surface0")))
            } else {
                line
            });
        }
    };
    match &v.where_you_are {
        Some(wh) => {
            part(&mut out, 0, "now", fit(&wh.state, 3), dim(), text());
            // The shortfall is a part of its own, selected apart from now (round 2p).
            if let Some(sh) = &wh.short {
                part(&mut out, 1, "shortfall", fit(sh, 2), dim(), st("subtext0"));
            }
        }
        None => part(&mut out, 0, "now", vec!["nothing written yet".into()], dim(), dim()),
    }
    // What is new: each kind named in a column on its first item, so grouping costs no height
    // (round 2: "new being grouped by kind").
    let mut drawn = Vec::new();
    if budget > 0 {
        let order: Vec<(&str, usize, bool)> = new_groups(v)
            .into_iter()
            .flat_map(|(h, idx)| idx.into_iter().enumerate().map(move |(k, i)| (h, i, k == 0)))
            .collect();
        let take = if order.len() > budget && budget > 1 {
            budget - 1
        } else {
            order.len().min(budget)
        };
        for (pos, &(heading, i, first)) in order.iter().take(take).enumerate() {
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
                    pointer(first_new(v) + pos, sel),
                    Span::styled(format!(" {:<11}", if pos == 0 { "new" } else { "" }), dim()),
                    Span::styled(
                        format!("{:<11}", if first || pos == 0 { heading } else { "" }),
                        bold(st("subtext1")),
                    ),
                    Span::styled(format!("{g} "), st(gc)),
                    Span::styled(it.text.clone(), if unread { text() } else { dim() }),
                ],
                right,
                w,
            ));
            drawn.push(i);
        }
        if take < order.len() {
            out.push(Line::from(Span::styled(
                format!("              + {} more", order.len() - take),
                dim(),
            )));
        }
        if v.new.is_empty() {
            let last = app
                .fixtures
                .projects
                .get(&v.project)
                .and_then(|p| p.changes.as_ref())
                .and_then(|c| c.entries.first().map(|e| e.at));
            let msg = match last {
                Some(t) => format!("nothing since you last looked; the last change was {} ago", ago(now, t)),
                None => "nothing since you last looked".to_string(),
            };
            out.push(Line::from(vec![
                Span::styled(format!("   {:<11}", "new"), dim()),
                Span::styled(msg, dim()),
            ]));
        }
    }
    let Some(wh) = &v.where_you_are else {
        return (out, drawn);
    };
    // Next, set apart by a dashed line rather than a band (round 2b).
    out.push(Line::from(Span::styled(
        format!("   {}", "╌".repeat(usize::from(w).saturating_sub(6))),
        st("surface2"),
    )));
    let at = first_new(v) + drawn.len();
    part(
        &mut out,
        at,
        "next ▸",
        fit(&wh.next, 2),
        bold(st("peach")),
        bold(text()),
    );
    (out, drawn)
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
            let unread = a
                .items
                .iter()
                .enumerate()
                .filter(|(i, x)| x.unread && app.marked(&answer_key(&v.project, *i)).is_none())
                .count();
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
    out.push(("n", glyph::COMMITS, "green", "full timeline", since.unwrap_or_default()));
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
/// always remind me of what is next to address bigger picture"). Decisions used to sit at the
/// right of its heading and read as the questions' answers, so they have their own section
/// further down (rounds 2k, 2l).
fn questions(app: &App, v: &ProjectView, w: u16) -> Vec<Line<'static>> {
    if v.questions.is_empty() {
        return vec![];
    }
    let focus = app.focus == Some('q');
    let mut head = vec![
        Span::styled("─ ", st(if focus { "blue" } else { "surface2" })),
        Span::styled("open questions", bold(if focus { st("blue") } else { text() })),
        Span::styled(" · from the dossier ", dim()),
    ];
    let used: usize = head.iter().map(|s| s.content.width()).sum();
    head.push(Span::styled(
        "─".repeat(usize::from(w).saturating_sub(used)),
        st("surface2"),
    ));
    let mut out = vec![Line::from(head)];
    let body = usize::from(w).saturating_sub(10);
    // Each question is a row you can select, like any other section (round 2i); Enter opens the
    // dossier where it is asked.
    let sel = focus.then(|| app.sel('q'));
    for (i, q) in v.questions.iter().enumerate() {
        for (j, l) in wrap(q, body).into_iter().take(2).enumerate() {
            let lead = if j == 0 { pointer(i, sel) } else { Span::raw("  ") };
            let num = if j == 0 { format!(" {}  ", i + 1) } else { "    ".into() };
            let line = row(
                vec![lead, Span::styled(num, bold(st("mauve"))), Span::styled(l, text())],
                vec![],
                w,
            );
            out.push(if j > 0 && sel == Some(i) {
                line.patch_style(Style::default().bg(c("surface0")))
            } else {
                line
            });
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
fn graph_need(app: &App, v: &ProjectView, w: u16, cap: usize, recs: usize) -> u16 {
    let Some(g) = &v.graph else { return 0 };
    let gw = usize::from(w).saturating_sub(15).max(10);
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
    let detail = g.lanes.get(sel).map_or(0, |l| {
        if app.lane_open && focus.is_some() {
            // Open: the whole summary, a heading, and up to twelve records.
            summary_lines(l, w, usize::MAX).len() + 2 + l.records.len().min(recs)
        } else {
            summary_lines(l, w, cap).len()
        }
    });
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
    const MIN_GAP: usize = 11;
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
    // Main starts in column 3, where every section's content starts (round 2h).
    let lead = 9;
    let gw = w.saturating_sub(lead + 6).max(10);
    let sel_all = lane_at(app, g);
    let shown_i = if focus {
        sel_all
    } else {
        g.lanes.len().saturating_sub(1)
    };
    let detail_h = g
        .lanes
        .get(shown_i)
        .map_or(0, |l| summary_lines(l, area.width, 3).len());
    let room = usize::from(area.height).saturating_sub(4 + detail_h).max(1);
    // No limit on overlapping rows (round 2h); the gap alone decides how many lanes are drawn.
    let _ = room;
    let (first, end) = window(g, now, gw, usize::MAX, focus.then_some(sel_all));
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
                format!("   {:>4} {}", g.trunk, if earlier > 0 { "◂" } else { "●" }),
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
        if app.lane_open && focus {
            out.extend(story(l, area.width, usize::MAX));
            // The change records, scrolled; ↑ ↓ scroll, Esc closes (round 2h).
            let room = usize::from(area.height).saturating_sub(out.len() + 1);
            let at = app.records_at.min(l.records.len().saturating_sub(room.max(1)));
            out.push(Line::from(vec![
                Span::styled(
                    format!("     change records · {}", l.records.len()),
                    bold(st("subtext1")),
                ),
                Span::styled(
                    if l.records.len() > room {
                        format!(
                            "   {} to {} · ↑ ↓ scroll · esc closes",
                            at + 1,
                            (at + room).min(l.records.len())
                        )
                    } else {
                        "   esc closes".into()
                    },
                    dim(),
                ),
            ]));
            for r in l.records.iter().skip(at).take(room) {
                out.push(Line::from(vec![
                    Span::styled(format!("       {:<8}", date(r.date)), dim()),
                    Span::styled(crate::ui::truncate(&r.text, w.saturating_sub(17)), text()),
                ]));
            }
            if l.records.is_empty() {
                out.push(Line::from(Span::styled("       no records for this branch", dim())));
            }
            lines(f, area, out);
            return;
        }
        // The story gets whatever rows the graph was given beyond its branches.
        let cap = usize::from(area.height).saturating_sub(out.len()).max(1);
        out.extend(story(l, area.width, cap));
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

/// Where a lane stands, for the right of its story: "merged · 45 commits", "open · 2 commits".
fn lane_status(l: &locus_contract::Lane) -> String {
    let state = if l.merged.is_some() { "merged" } else { "open" };
    let commits = if l.commits == 1 {
        "1 commit".to_string()
    } else {
        format!("{} commits", l.commits)
    };
    format!("{state}{SEP}{commits}")
}

/// The lane's summary wrapped to the space beside its name and status, at most `cap` lines.
fn summary_lines(l: &locus_contract::Lane, w: u16, cap: usize) -> Vec<String> {
    let used = 3 + short_name(l).width() + 2 + lane_status(l).width() + 2;
    let mut v = wrap(&l.summary, usize::from(w).saturating_sub(used).max(20));
    if v.len() > cap {
        v.truncate(cap);
        if let Some(last) = v.last_mut() {
            last.push(locus_theme::ELLIPSIS);
        }
    }
    v
}

/// The selected lane's story: its name, then its summary on the same line and wrapped beneath,
/// with merged or open and the commit count at the right (round 2i: "the branch name and the
/// summary should be on one line"; the branch's git name and dates dropped).
fn story(l: &locus_contract::Lane, w: u16, cap: usize) -> Vec<Line<'static>> {
    let name = short_name(l);
    let indent = " ".repeat(3 + name.width() + 2);
    summary_lines(l, w, cap)
        .into_iter()
        .enumerate()
        .map(|(i, line)| {
            if i == 0 {
                row(
                    vec![
                        Span::raw("   "),
                        Span::styled(name.clone(), bold(text())),
                        Span::raw("  "),
                        Span::styled(line, text()),
                    ],
                    vec![Span::styled(format!("{} ", lane_status(l)), dim())],
                    w,
                )
            } else {
                Line::from(vec![Span::raw(indent.clone()), Span::styled(line, text())])
            }
        })
        .collect()
}

/// A connector in column `x` from main (row 0) down to lane row `r`, through empty cells only.
fn join(cv: &mut [Vec<(char, Style)>], x: usize, r: usize, style: Style) {
    for rr in cv.iter_mut().take(r).skip(1) {
        if rr[x].0 == ' ' {
            rr[x] = ('│', style);
        }
    }
}

/// The decision log, what to act on first (round 2l: "decisions does deserve its own section"):
/// what is worth re-checking, with why in a few words, then what was decided since you last
/// looked, then the rest while they fit. Enter opens one on the Decisions page; d opens the log.
fn draw_decisions(f: &mut Frame, area: Rect, app: &App, v: &ProjectView) {
    let focus = app.focus == Some('d');
    let ds = decisions(app, v);
    let n = ds.map_or(0, |d| d.items.len());
    let re = ds.map_or(0, |d| {
        d.items.iter().filter(|x| x.status == DecisionStatus::Recheck).count()
    });
    let mut extra = n.to_string();
    if re > 0 {
        extra += &format!(" · {re} to re-check");
    }
    extra += " · full log [d]";
    let mut out = vec![rule("decisions", &extra, None, focus, area.width)];
    let mut drawn = Vec::new();
    match ds {
        Some(ds) if !ds.items.is_empty() => {
            let order = decision_order(ds);
            let rows = usize::from(area.height.saturating_sub(1));
            let lines_of = |i: usize| {
                let d = &ds.items[i];
                1 + usize::from(d.status == DecisionStatus::Recheck && d.recheck.is_some())
            };
            let all: usize = order.iter().map(|&i| lines_of(i)).sum();
            let room = if all > rows && rows > 1 { rows - 1 } else { rows };
            let mut take = 0;
            let mut used = 0;
            // The first always draws, its reason cut if it must be.
            while take < order.len() && (take == 0 || used + lines_of(order[take]) <= room) {
                used += lines_of(order[take]);
                take += 1;
            }
            let sel = focus.then(|| app.sel('d'));
            for (pos, &i) in order.iter().take(take).enumerate() {
                let d = &ds.items[i];
                // Reopened on the Decisions page shows here too.
                let st_now = crate::ui::decisions::status_of(app, &v.project, d);
                let (g, gc, _) = decision_glyph(&st_now);
                // No decision numbers on screen (round 2): a replaced one just says so.
                let status = match st_now {
                    DecisionStatus::Holding => "holding",
                    DecisionStatus::Recheck => "re-check",
                    DecisionStatus::Review => "reviewing",
                    DecisionStatus::Redecided { .. } => "replaced",
                };
                let left = vec![
                    pointer(pos, sel),
                    Span::styled(format!(" {g} "), st(gc)),
                    Span::styled(
                        d.title.clone(),
                        if matches!(st_now, DecisionStatus::Redecided { .. }) {
                            dim()
                        } else {
                            text()
                        },
                    ),
                ];
                let why = d.recheck.as_ref().filter(|_| st_now == DecisionStatus::Recheck);
                let mut right = vec![];
                if d.new {
                    right.push(Span::styled(format!("{} ", glyph::UNREAD), st("peach")));
                }
                right.push(Span::styled(format!("{status:<9}"), st(gc)));
                right.push(Span::styled(format!("{:>7} ", date(d.date)), dim()));
                out.push(row(left, right, area.width));
                if let Some(why) = why {
                    let line = row(
                        vec![
                            Span::raw("     "),
                            Span::styled(format!("why: {}", why.label), st("yellow")),
                        ],
                        vec![],
                        area.width,
                    );
                    out.push(if sel == Some(pos) {
                        line.patch_style(Style::default().bg(c("surface0")))
                    } else {
                        line
                    });
                }
                drawn.push(i);
            }
            if take < order.len() {
                out.push(Line::from(Span::styled(
                    // The heading already says where the full log is.
                    format!("   + {} more", order.len() - take),
                    dim(),
                )));
            }
        }
        _ => out.push(Line::from(Span::styled("   no decisions logged yet", dim()))),
    }
    app.shown.borrow_mut().insert((ScreenId::Project, 'd'), drawn);
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
    // One row shows the most pressing session rather than only a count of them.
    let overflow = !focus && ss.len() > rows && rows > 1;
    let room = rows.saturating_sub(usize::from(overflow));
    let sel = focus.then(|| app.sel('s'));
    // The name column fits the longest name, within reason (round 2h: names were cut).
    let nw = ss
        .iter()
        .map(|s| s.name.width() + if s.background { 5 } else { 0 } + 1)
        .max()
        .unwrap_or(12)
        .clamp(12, 30);
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
                Span::styled(format!(" {g} "), st(gc)),
                Span::styled(format!("{:<nw$}", crate::ui::truncate(&name, nw - 1)), text()),
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

pub(crate) fn draw_ask(f: &mut Frame, area: Rect, app: &App, v: &ProjectView) {
    // Tab reaches it like any section, and focused its whole rule lights (round 2l).
    let asking = matches!(app.mode, Mode::Ask(_)) || app.focus == Some('a');
    let what = if v.application.is_some() {
        "application"
    } else {
        "project"
    };
    let left = match &app.mode {
        Mode::Ask(q) => vec![
            Span::styled("   ask › ", st("mauve")),
            Span::styled(q.clone(), text()),
            Span::styled("▏", st("mauve")),
        ],
        // Space asks from anywhere, so Enter here reads what was asked before (round 2m: "we
        // need a better result of enter when on ask").
        _ if app.focus == Some('a') => vec![
            Span::styled("   ask › ", dim()),
            Span::styled("␣", keyst()),
            Span::styled(format!(" to ask about this {what} · "), dim()),
            Span::styled("⏎", keyst()),
            // With an answer waiting, Enter opens it (round 2o).
            Span::styled(
                match new_answer(app, v) {
                    Some((_, a)) => format!(" to read the new answer: {}", a.title),
                    None => " for what you asked before, in answered".into(),
                },
                dim(),
            ),
        ],
        _ => vec![
            Span::styled("   ask › ", dim()),
            Span::styled("␣", keyst()),
            Span::styled(format!(" to ask about this {what} · the answer comes back here"), dim()),
        ],
    };
    // What was asked before sits at the right of the ask rule (round 2e); a reaches the line
    // like any section's letter, and only there does Enter open answered (round 2n).
    let mut head = vec![
        Span::styled("─ ", st(if asking { "blue" } else { "surface2" })),
        Span::styled("ask", bold(if asking { st("blue") } else { text() })),
    ];
    let right = match more_rows(app, v).into_iter().find(|r| r.0 == "a") {
        Some((_, g, col, name, state)) => vec![
            Span::styled(format!("{g} "), st(col)),
            Span::styled(format!("{name} "), text()),
            Span::styled(state, dim()),
        ],
        None => vec![],
    };
    let rule_c = st(if asking { "blue" } else { "surface2" });
    let tail = Span::styled(" [a] ─", rule_c);
    let used: usize = head
        .iter()
        .chain(right.iter())
        .map(|s| s.content.width())
        .sum::<usize>()
        + tail.content.width();
    let pad = if right.is_empty() { 1 } else { 2 };
    head.push(Span::styled(
        format!(" {}", "─".repeat(usize::from(area.width).saturating_sub(used + pad))),
        rule_c,
    ));
    if !right.is_empty() {
        head.push(Span::raw(" "));
        head.extend(right);
    }
    head.push(tail);
    lines(f, area, vec![Line::from(head), Line::from(left)]);
}

pub fn key(app: &mut App, k: KeyEvent) -> bool {
    let Some(v) = view(app).cloned() else {
        return matches!(k.code, KeyCode::Char('m')) && toggle_pin(app);
    };
    let focus = app.focus;
    match (focus, k.code) {
        (_, KeyCode::Char('n')) => app.open(ScreenId::Changes),
        // Where you are: now, the new items, next. A new item opens or is marked read; now and
        // next open the dossier where they are written.
        (Some('y'), KeyCode::Enter | KeyCode::Char('r')) => {
            let at = app.sel('y');
            match shown(app, 'y').get(at.wrapping_sub(first_new(&v))).copied() {
                Some(i) => {
                    let note = if k.code == KeyCode::Enter {
                        format!("would open: {}", v.new[i].text)
                    } else {
                        "marked read".into()
                    };
                    app.mark(new_key(&v, i), Mark::Read, note);
                }
                None if k.code == KeyCode::Enter => {
                    app.note = Some("would open the dossier at where this project stands".into())
                }
                None => app.note = Some("r marks a new item read".into()),
            }
        }
        (Some('d'), KeyCode::Enter) => {
            let at = shown(app, 'd').get(app.sel('d')).copied();
            app.open(ScreenId::Decisions);
            if let Some(i) = at {
                crate::ui::decisions::focus_on(app, i);
            }
        }
        (Some('a'), KeyCode::Enter) => {
            let waiting = new_answer(app, &v).map(|(i, _)| i);
            app.open(ScreenId::Answered);
            if let Some(i) = waiting {
                app.sel.insert((ScreenId::Answered, ' '), i);
                app.mark(answer_key(&v.project, i), Mark::Read, "");
                app.note = None;
            }
        }
        (Some('s'), KeyCode::Enter) => {
            if let Some(s) = sorted_sessions(&v).get(app.sel('s')) {
                app.note = Some(format!("would jump your project window to \"{}\"", s.name));
            }
        }
        // ← and → walk the branches from anywhere on the page (round 2d), focusing the graph.
        (_, KeyCode::Left | KeyCode::Right) => {
            app.focus = Some('g');
            let n = v.graph.as_ref().map_or(0, |g| g.lanes.len());
            let d = if k.code == KeyCode::Left { 1 } else { -1 };
            app.move_sel('g', n, d);
            app.records_at = 0;
        }
        (Some('g'), KeyCode::Enter) => {
            app.lane_open = !app.lane_open;
            app.records_at = 0;
        }
        (Some('g'), KeyCode::Esc) if app.lane_open => app.lane_open = false,
        (Some('g'), KeyCode::Up | KeyCode::Down | KeyCode::Char('j' | 'k')) if app.lane_open => {
            let d = crate::app::nav(&k).unwrap_or(0);
            app.records_at = (app.records_at as isize + d).max(0) as usize;
        }
        (Some('q'), KeyCode::Enter) => {
            if let Some(q) = v.questions.get(app.sel('q')) {
                app.note = Some(format!(
                    "would open the dossier where it asks: {}",
                    crate::ui::truncate(q, 60)
                ));
            }
        }
        // Space asks (round 2: "a better key than i"); i still works.
        (_, KeyCode::Char(' ' | 'i')) => app.start_ask(),
        (_, KeyCode::Char('d')) => app.open(ScreenId::Decisions),
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
