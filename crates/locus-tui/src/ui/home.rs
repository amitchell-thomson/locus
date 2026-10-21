//! Home (Alt-1): only what no other view shows (plan D), and a place that sends you somewhere
//! useful rather than one you act in (round 1, 21 Oct: "not much should happen from home, it
//! should just take you to somewhere useful").
//!
//! Top to bottom: the moment and what is next; what is waiting for your approval; the week's
//! work; projects (things you build in deep blocks); careers (applications and the CV) beside
//! learning (your goals); Explore's finds by kind; the system's live state. Careers and
//! learning are kept out of projects on purpose: "they are internship applications and my CV,
//! they do not feel like projects".

use crate::app::App;
use crate::screen::ScreenId;
use crate::ui::widgets::*;
use crate::ui::{lines, work};
use locus_contract::{FindKind, GoalState, ProjectKind, SessionState};
use locus_theme::{SEP, c, glyph};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

pub const KEYS: &[(&str, &str)] = &[
    ("⇥ ↑ ↓", "move between sections and through them"),
    ("⏎", "go there: the project, the approve list, the find"),
    ("W", "the weekly review, when it is ready"),
    ("L", "learning"),
];

/// Projects in the order Home lists them: a session that needs you first, then the attended
/// project, then the rest as registered. Home's sections go by area, not kind (ADR-006): this
/// one holds everything outside Careers.
pub fn project_rows(app: &App) -> Vec<String> {
    rows_of(app, |p| p.area != "Careers")
}

/// The Careers area: applications, and the CV (a project that lives there).
pub fn career_rows(app: &App) -> Vec<String> {
    rows_of(app, |p| p.area == "Careers")
}

fn rows_of(app: &App, keep: impl Fn(&locus_contract::Project) -> bool) -> Vec<String> {
    let fx = &app.fixtures;
    let mut rows: Vec<(u8, usize, String)> = Vec::new();
    for (i, p) in fx
        .registry
        .projects
        .iter()
        .enumerate()
        .filter(|(_, p)| p.active && keep(p))
    {
        let needs = fx
            .projects
            .get(&p.id)
            .is_some_and(|pf| pf.view.sessions.iter().any(|s| s.state == SessionState::NeedsYou));
        let rank = if needs {
            0
        } else if fx.home.attended.as_deref() == Some(p.id.as_str()) {
            1
        } else {
            2
        };
        rows.push((rank, i, p.id.clone()));
    }
    rows.sort();
    rows.into_iter().map(|(_, _, id)| id).collect()
}

/// Goals still in play, for the learning section.
fn goal_rows(app: &App) -> Vec<&locus_contract::Goal> {
    app.fixtures
        .learning
        .goals
        .iter()
        .filter(|g| g.state != GoalState::Done)
        .collect()
}

/// The approve list flattened: (item key, text, destination, ink fixture it opens).
pub fn approve_rows(app: &App) -> Vec<(String, String, Option<String>, Option<String>)> {
    let mut out = Vec::new();
    for (g, group) in app.fixtures.approve.groups.iter().enumerate() {
        for (i, it) in group.items.iter().enumerate() {
            out.push((
                format!("approve:{g}:{i}"),
                it.text.clone(),
                it.to.clone(),
                it.opens.clone(),
            ));
        }
    }
    out
}

pub fn find_key(f: &locus_contract::Find) -> String {
    format!("find:{}", f.id)
}

/// Explore's queue by kind, each group in queue order: (heading, indexes into the queue).
fn find_groups(app: &App) -> Vec<(&'static str, Vec<usize>)> {
    let q = &app.fixtures.explore.queue;
    let pick = |f: fn(FindKind) -> bool| -> Vec<usize> { (0..q.len()).filter(|&i| f(q[i].kind)).collect() };
    [
        (
            "for your projects",
            pick(|k| matches!(k, FindKind::Method | FindKind::Course | FindKind::Pattern)),
        ),
        ("to read", pick(|k| k == FindKind::Reading)),
        ("new project ideas", pick(|k| k == FindKind::NewProject)),
    ]
    .into_iter()
    .filter(|(_, v)| !v.is_empty())
    .collect()
}

/// What Enter does for the focused section, for the footer.
pub fn enter_hint(app: &App) -> Option<&'static str> {
    Some(match app.focus? {
        'a' => "go through the approvals",
        'w' => "the 8-week chart",
        'p' => "open the project",
        'c' => "open it",
        'g' => "learning",
        'e' => "the find, in Explore",
        's' => "the system page",
        _ => return None,
    })
}

pub fn section_len(app: &App, sec: char) -> usize {
    match sec {
        'p' => project_rows(app).len(),
        'c' => career_rows(app).len(),
        'g' => goal_rows(app).len(),
        'e' => app.fixtures.explore.queue.len(),
        _ => 1,
    }
}

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let inner = draw_border(f, area, app);
    let n_proj = project_rows(app).len().max(1) as u16;
    let n_side = career_rows(app).len().max(goal_rows(app).len() + 1).max(1) as u16;
    // A blank line between sections (round 1b), paid for by moving next and mail into the
    // border.
    let gap = Constraint::Length(1);
    // Work heads the page; the approve line sits with the system at the foot (round 1d: he
    // checks approvals as a sign the system is working, so it belongs beside the system).
    let rects = Layout::vertical([
        Constraint::Length(9),
        gap,
        Constraint::Length(n_proj + 1),
        gap,
        Constraint::Length(n_side + 1),
        gap,
        Constraint::Min(3),
        gap,
        Constraint::Length(2),
        gap,
        Constraint::Length(3),
    ])
    .split(inner);
    let (work_r, proj_r, side_r, exp_r, wait_r, sys_r) = (rects[0], rects[2], rects[4], rects[6], rects[8], rects[10]);

    draw_waiting(f, wait_r, app);
    work::draw_week(f, work_r, app, app.focus == Some('w'));
    draw_projects(f, proj_r, app);
    // Careers and learning side by side, a line between them (round 1c).
    let [c_r, bar_r, g_r] = Layout::horizontal([
        Constraint::Percentage(50),
        Constraint::Length(3),
        Constraint::Percentage(50),
    ])
    .areas(side_r);
    draw_careers(f, c_r, app);
    draw_learning(f, g_r, app);
    // The divider is learning's edge: focused, it lights up the way the frame does for the
    // other sections (round 1d).
    let (col, top, side) = if app.focus == Some('g') {
        (st("blue"), "━┳━", " ┃ ")
    } else {
        (st("surface2"), "─┬─", " │ ")
    };
    let mut div = vec![Line::from(Span::styled(top, col))];
    for _ in 1..bar_r.height {
        div.push(Line::from(Span::styled(side, col)));
    }
    lines(f, bar_r, div);
    draw_explore(f, exp_r, app);
    draw_system(f, sys_r, app);

    // The focused section gets a thick blue bar on the frame's edge beside it: clearer than a
    // coloured rule alone, without boxing every section (round 1b).
    let focused = match app.focus {
        Some('a') => Some(wait_r),
        Some('w') => Some(work_r),
        Some('p') => Some(proj_r),
        Some('c') => Some(side_r),
        Some('e') => Some(exp_r),
        Some('s') => Some(sys_r),
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

/// The frame, with the moment centred in its top border and next and mail, short, at its right
/// (round 1b: "centered in the top bar, on the same line as locus . home"; next and mail "on the
/// top of the border ... in a shorter format"). The chores count and the proposed deadline
/// left: the deadline is an item in Approve, chores are on the Project view.
fn draw_border(f: &mut Frame, area: Rect, app: &App) -> Rect {
    let fx = &app.fixtures;
    let now = fx.now();
    let w = usize::from(area.width);
    let running = fx.system.running.len();
    // Three lengths of each part: 2 full words, 1 shorter, 0 shortest.
    let make_mid = |n: u8| -> Vec<Span<'static>> {
        let fmt = if n == 2 {
            "%A %-d %B · %H:%M"
        } else {
            "%a %-d %b · %H:%M"
        };
        let mut v = vec![Span::styled(
            format!(" {} ", now.format(fmt).to_string().to_lowercase()),
            bold(text()),
        )];
        if running > 0 {
            let word = ["", " running", " jobs running"][usize::from(n)];
            v.push(Span::styled(
                format!("  {} {running}{word} ", spin(app.frame)),
                st("blue"),
            ));
        }
        if fx.home.review_ready {
            let t = ["  review · W ", "  review ready · W ", "  weekly review ready · W "][usize::from(n)];
            v.push(Span::styled(t, bold(st("peach"))));
        }
        v
    };
    // Short, so the border stays balanced (round 1c: "something really short and 'in 15m'"):
    // the event's first word and how long until it; mail as a count.
    let until = |t: locus_contract::Time| -> String {
        let m = (t - now).num_minutes();
        match m {
            i64::MIN..=0 => "now".into(),
            1..=59 => format!("in {m}m"),
            60..=1439 => format!("in {}h", m / 60),
            _ => t.format("%a %H:%M").to_string().to_lowercase(),
        }
    };
    let make_right = |n: u8| -> Vec<Span<'static>> {
        let mut v = Vec::new();
        if let Some(e) = &fx.home.next {
            let word = e.title.split_whitespace().next().unwrap_or("").to_lowercase();
            let t = if n > 0 {
                format!("{word} {}", until(e.at))
            } else {
                until(e.at)
            };
            v.push(Span::styled(format!(" {} {t} ", glyph::CALENDAR), st("peach")));
        }
        let m = fx.home.mail.need_reply;
        if m > 0 {
            v.push(Span::styled(format!(" {} {m} ", glyph::MAIL), st("sapphire")));
        }
        v
    };
    let width = |v: &[Span]| v.iter().map(|s| s.content.width()).sum::<usize>();
    // The centre is centred on the whole width, so each side has (w - centre) / 2 for the left
    // title (16) or the right part. Take the first pairing, in order of preference, that fits.
    let fits = |mid: &[Span], right: &[Span]| {
        let side = w.saturating_sub(width(mid)) / 2;
        width(right) + 3 <= side && 19 <= side
    };
    let (mut mid, mut right) = (make_mid(0), Vec::new());
    for (m, r) in [(2, 2), (1, 2), (0, 2), (1, 0), (0, 0)] {
        let (a, b) = (make_mid(m), make_right(r));
        if fits(&a, &b) {
            (mid, right) = (a, b);
            break;
        }
    }
    let block = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(st("overlay0"))
        .title(crate::ui::title("locus", "home").left_aligned())
        .title(Line::from(mid).centered())
        .title(Line::from(right).right_aligned());
    let inner = block.inner(area);
    f.render_widget(block, area);
    inner
}

/// The approve list as one line that draws the eye; the items themselves are on the approve
/// page (round 1: "I would rather see the approves in the approve window, with a smaller
/// summary in a different location").
fn draw_waiting(f: &mut Frame, area: Rect, app: &App) {
    let focus = app.focus == Some('a');
    let ap = &app.fixtures.approve;
    let n = ap.len();
    let mut out = vec![rule("waiting for you", "", Some('a'), focus, area.width)];
    if n == 0 {
        out.push(Line::from(Span::styled("  nothing waiting for your approval", dim())));
    } else {
        let kinds: Vec<String> = ap
            .groups
            .iter()
            .filter(|g| !g.items.is_empty())
            .map(|g| {
                let name = match g.kind {
                    locus_contract::ApproveKind::Filing => "to file",
                    locus_contract::ApproveKind::Mail => "mail",
                    locus_contract::ApproveKind::Deadline => "deadline",
                    locus_contract::ApproveKind::Ink => "ink to place",
                    locus_contract::ApproveKind::StopAsking => "stop asking?",
                };
                format!("{} {name}", g.items.len())
            })
            .collect();
        let left = vec![
            pointer(0, focus.then_some(0)),
            Span::styled(format!("{} {n} to approve   ", glyph::APPROVE), bold(st("yellow"))),
            Span::styled(kinds.join(SEP), text()),
        ];
        let new = ap.groups.iter().flat_map(|g| &g.items).filter(|i| i.new).count();
        let right = if new > 0 {
            vec![Span::styled(format!("{} {new} new ", glyph::UNREAD), st("peach"))]
        } else {
            vec![]
        };
        out.push(row(left, right, area.width));
    }
    lines(f, area, out);
}

fn draw_projects(f: &mut Frame, area: Rect, app: &App) {
    let fx = &app.fixtures;
    let now = fx.now();
    let focus = app.focus == Some('p');
    let ids = project_rows(app);
    let mut out = vec![rule("projects", "what you build", Some('p'), focus, area.width)];
    let sel = focus.then(|| app.sel('p'));
    for (i, id) in ids.iter().enumerate() {
        let Some(p) = fx.registry.get(id) else { continue };
        let pf = fx.projects.get(id);
        let attended = fx.home.attended.as_deref() == Some(id.as_str());
        let (g, gc) = if attended {
            (glyph::ATTENDED, "blue")
        } else {
            (glyph::UNATTENDED, "overlay1")
        };
        let mut left = vec![
            pointer(i, sel),
            Span::styled(format!("{g} "), st(gc)),
            Span::styled(
                format!("{:<9}", crate::ui::truncate(&p.name, 9)),
                bold(st(area_colour(app, &p.area))),
            ),
        ];
        let mut sessions: Vec<_> = pf.map(|pf| pf.view.sessions.iter().collect()).unwrap_or_default();
        sessions.sort_by_key(|s| s.state);
        match sessions.first() {
            Some(s) => {
                let (sg, sc, word) = session_glyph(s.state);
                let sg = if s.state == SessionState::Working {
                    spin(app.frame)
                } else {
                    sg
                };
                left.push(Span::styled(format!("{sg} "), st(sc)));
                left.push(Span::styled(format!("{word:<9}"), st(sc)));
                left.push(Span::styled(format!("{:>4}  ", ago(now, s.since)), dim()));
                let what = s.summary.clone().or_else(|| s.doing.clone()).unwrap_or_default();
                left.push(Span::styled(what, text()));
            }
            None => {
                left.push(Span::styled(format!("{} ", glyph::IDLE), dim()));
                left.push(Span::styled(format!("{:<15}", "no sessions"), dim()));
                // What it last did, so a stalled project shows where it stopped (round 1d).
                let last = pf.and_then(|pf| {
                    let mut items: Vec<_> = pf.view.new.iter().collect();
                    if let Some(c) = &pf.changes {
                        items.extend(c.entries.iter());
                    }
                    items.into_iter().max_by_key(|i| i.at).cloned()
                });
                match last {
                    Some(it) => {
                        left.push(Span::styled(format!("last {}  ", date(it.at.date_naive())), dim()));
                        left.push(Span::styled(it.text, st("subtext0")));
                    }
                    None => left.push(Span::styled(
                        pf.and_then(|pf| pf.view.stage.clone())
                            .unwrap_or_else(|| "nothing yet".into()),
                        dim(),
                    )),
                }
            }
        }
        out.push(row(left, counts(pf.map(|p| &p.view), sessions.len()), area.width));
    }
    if ids.is_empty() {
        out.push(Line::from(Span::styled("  no active projects", dim())));
    }
    lines(f, area, out);
}

/// The two counts on a row, each saying what it counts (round 1: "+1/+7 and the
/// notifications are confusing").
fn counts(view: Option<&locus_contract::ProjectView>, sessions: usize) -> Vec<Span<'static>> {
    let mut right = Vec::new();
    if sessions > 1 {
        right.push(Span::styled(format!("{sessions} sessions"), dim()));
    }
    let unread = view.map(|v| v.unread()).unwrap_or(0);
    if unread > 0 {
        if !right.is_empty() {
            right.push(Span::styled("  ", dim()));
        }
        right.push(Span::styled(format!("{} {unread} new", glyph::UNREAD), st("peach")));
    }
    right.push(Span::raw(" "));
    right
}

fn draw_careers(f: &mut Frame, area: Rect, app: &App) {
    let fx = &app.fixtures;
    let focus = app.focus == Some('c');
    let ids = career_rows(app);
    let mut out = vec![rule(
        "careers",
        "applications and your CV",
        Some('c'),
        focus,
        area.width,
    )];
    let sel = focus.then(|| app.sel('c'));
    for (i, id) in ids.iter().enumerate() {
        let Some(p) = fx.registry.get(id) else { continue };
        let pf = fx.projects.get(id);
        let today = fx.now().date_naive();
        let mut stage = vec![];
        match pf.and_then(|pf| pf.view.status.clone()) {
            Some(st_) => {
                let days = (today - st_.since).num_days();
                stage.push(Span::styled(st_.stage.clone(), text()));
                match st_.turn {
                    Some(locus_contract::Turn::Them) => {
                        stage.push(Span::styled(format!("{SEP}waiting for them, {days} days"), dim()))
                    }
                    Some(locus_contract::Turn::You) => {
                        stage.push(Span::styled(format!("{SEP}your move, {days} days"), bold(st("peach"))))
                    }
                    None => stage.push(Span::styled(format!("{SEP}{}", date(st_.since)), dim())),
                }
            }
            None => stage.push(Span::styled(
                pf.and_then(|pf| pf.view.stage.clone()).unwrap_or_default(),
                text(),
            )),
        }
        let g = if p.kind == ProjectKind::Project {
            glyph::DOSSIER
        } else {
            glyph::MAIL
        };
        let left = vec![
            pointer(i, sel),
            Span::styled(format!("{g} "), st("peach")),
            Span::styled(format!("{:<13}", crate::ui::truncate(&p.name, 12)), bold(st("peach"))),
        ];
        let mut left = left;
        left.extend(stage);
        out.push(row(left, counts(pf.map(|p| &p.view), 0), area.width));
    }
    if ids.is_empty() {
        out.push(Line::from(Span::styled("  no applications open", dim())));
    }
    lines(f, area, out);
}

fn draw_learning(f: &mut Frame, area: Rect, app: &App) {
    let l = &app.fixtures.learning;
    let focus = app.focus == Some('g');
    let goals = goal_rows(app);
    let mut out = vec![rule(
        "learning",
        "what you are learning for",
        Some('g'),
        focus,
        area.width,
    )];
    let sel = focus.then(|| app.sel('g'));
    for (i, g) in goals.iter().enumerate() {
        let hours: f32 = g.syllabus.iter().map(|s| s.hours).sum();
        let (state, col) = match g.state {
            GoalState::Proposed => ("syllabus to agree", "peach"),
            GoalState::Active => ("active", "blue"),
            GoalState::Done => ("done", "green"),
        };
        out.push(row(
            vec![
                pointer(i, sel),
                Span::styled(format!("{} ", glyph::IDEA), st(col)),
                Span::styled(g.title.clone(), text()),
            ],
            vec![Span::styled(format!("{hours:.0} h{SEP}{state} "), st(col))],
            area.width,
        ));
    }
    if let Some(t) = l.tracks.iter().min_by(|a, b| a.confidence.total_cmp(&b.confidence)) {
        let mut right = bar(t.confidence, 10, None, c("red"));
        right.push(Span::styled(format!(" {:>3.0}% ", t.confidence * 100.0), dim()));
        out.push(row(
            vec![Span::styled(format!("    weakest  {}", t.name), dim())],
            right,
            area.width,
        ));
    }
    if goals.is_empty() {
        out.push(Line::from(Span::styled("  no goals set · L to set one", dim())));
    }
    lines(f, area, out);
}

/// Explore's finds by kind, titles only, with room between groups so they do not run into a
/// wall (round 1). Grading happens on the Explore page; Enter takes you to the find there.
fn draw_explore(f: &mut Frame, area: Rect, app: &App) {
    let ex = &app.fixtures.explore;
    let focus = app.focus == Some('e');
    let full = if ex.queue.len() as u32 >= ex.queue_cap {
        " · full, so the scout is waiting for you"
    } else {
        ""
    };
    let extra = format!("{} finds{full}", ex.queue.len());
    let mut out = vec![rule("explore", &extra, Some('e'), focus, area.width)];
    let sel = focus.then(|| app.sel('e'));
    let room = usize::from(area.height.saturating_sub(1));
    let groups = find_groups(app);
    // Every kind keeps its heading and at least one find: when they do not all fit, the
    // longest group gives up rows first, and its heading says how many more there are.
    let mut shown: Vec<usize> = groups.iter().map(|(_, v)| v.len()).collect();
    let total = |shown: &[usize], gaps: bool| -> usize {
        shown.iter().sum::<usize>() + shown.len() + if gaps { shown.len().saturating_sub(1) } else { 0 }
    };
    while total(&shown, false) > room {
        match shown
            .iter()
            .enumerate()
            .filter(|(_, n)| **n > 1)
            .max_by_key(|(_, n)| **n)
        {
            Some((i, _)) => shown[i] -= 1,
            None => break,
        }
    }
    // Then as many blank lines between groups as the spare rows allow, so space is never left
    // over at the bottom while the groups are packed (round 1e).
    let gaps = room
        .saturating_sub(total(&shown, false))
        .min(shown.len().saturating_sub(1));
    let mut pos = 0;
    for (gi, (heading, idx)) in groups.iter().enumerate() {
        if gi > 0 && gi <= gaps {
            out.push(Line::from(""));
        }
        let mut head = vec![Span::styled(
            format!("   {heading}{SEP}{}", idx.len()),
            bold(st("subtext1")),
        )];
        if shown[gi] < idx.len() {
            head.push(Span::styled(
                format!("   + {} more on the Explore page", idx.len() - shown[gi]),
                dim(),
            ));
        }
        out.push(Line::from(head));
        for &qi in idx.iter().take(shown[gi]) {
            let find = &ex.queue[qi];
            let (g, gc, _) = find_glyph(find.kind);
            let who = find.project.clone().unwrap_or_default();
            let left = vec![
                pointer(pos, sel),
                Span::styled(format!("  {g} "), st(gc)),
                Span::styled(format!("{who:<7}"), dim()),
                Span::styled(find.title.clone(), text()),
            ];
            out.push(row(
                left,
                vec![Span::styled(format!("{} min ", find.minutes), dim())],
                area.width,
            ));
            pos += 1;
        }
        pos += idx.len() - shown[gi];
    }
    if ex.queue.is_empty() {
        out.push(Line::from(Span::styled(
            "  nothing waiting; about one find a day",
            dim(),
        )));
    }
    lines(f, area, out);
}

fn area_colour(app: &App, area: &str) -> &'static str {
    match app
        .fixtures
        .taxonomy
        .areas
        .iter()
        .find(|a| a.id == area)
        .map(|a| a.colour.as_str())
    {
        Some("blue") => "blue",
        Some("maroon") => "maroon",
        Some("peach") => "peach",
        Some("lavender") => "lavender",
        _ => "text",
    }
}

/// Two lines: what is running now (pulsing), and the budget with both limits named in words
/// (round 1: "window 34% · 4%" was "too obscure").
fn draw_system(f: &mut Frame, area: Rect, app: &App) {
    let fx = &app.fixtures;
    let focus = app.focus == Some('s');
    let mut out = vec![rule("system", "", Some('s'), focus, area.width)];
    let mut jobs: Vec<Span> = vec![Span::styled(" running  ", dim())];
    for (i, j) in fx.system.running.iter().enumerate() {
        if i > 0 {
            jobs.push(Span::styled("   │   ", st("surface2")));
        }
        jobs.push(Span::styled(format!("{} ", spin(app.frame + i as u64)), st("blue")));
        jobs.push(Span::styled(format!("{} {}", j.kind, j.subject), text()));
    }
    if fx.system.running.is_empty() {
        jobs.push(Span::styled("nothing", dim()));
    }
    let failed = fx.system.failed_today;
    let counts = vec![
        Span::styled(
            format!("{} {} done today", glyph::DONE, fx.system.done_today),
            st("green"),
        ),
        Span::styled("   ", dim()),
        Span::styled(
            format!("{} {failed} failed ", glyph::FAILED),
            if failed > 0 { st("red") } else { dim() },
        ),
    ];
    out.push(row(jobs, counts, area.width));
    let u = &fx.usage;
    let mut b = vec![Span::styled(" budget   ", dim())];
    b.extend(bar(u.week_pct / (u.cap_pct * 1.3), 24, Some(1.0 / 1.3), c("mauve")));
    b.push(Span::styled(
        format!("  {:.1}% of the {:.0}% weekly cap", u.week_pct, u.cap_pct),
        text(),
    ));
    out.push(row(
        b,
        vec![Span::styled(
            format!(
                "5-hour window: you {:.0}%{SEP}background {:.0}% ",
                u.window.you_pct, u.window.locus_pct
            ),
            dim(),
        )],
        area.width,
    ));
    lines(f, area, out);
}

pub fn key(app: &mut App, k: KeyEvent) -> bool {
    match (app.focus, k.code) {
        // Careers and learning sit side by side: ← and → cross between them (round 1c).
        (Some('c'), KeyCode::Right) => {
            app.focus = Some('g');
            true
        }
        (Some('g'), KeyCode::Left) => {
            app.focus = Some('c');
            true
        }
        (Some('p'), KeyCode::Enter) => {
            if let Some(id) = project_rows(app).get(app.sel('p')).cloned() {
                app.open_project(id);
            }
            true
        }
        (Some('c'), KeyCode::Enter) => {
            if let Some(id) = career_rows(app).get(app.sel('c')).cloned() {
                app.open_project(id);
            }
            true
        }
        (Some('e'), KeyCode::Enter) => {
            // Open Explore on the find that was selected here.
            let order: Vec<usize> = find_groups(app).into_iter().flat_map(|(_, v)| v).collect();
            let qi = order.get(app.sel('e')).copied().unwrap_or(0);
            app.open(ScreenId::Explore);
            app.sel.insert((ScreenId::Explore, 'q'), qi);
            app.focus = Some('q');
            true
        }
        (_, KeyCode::Char('W')) => {
            if app.fixtures.home.review_ready {
                app.open(ScreenId::Review);
            } else {
                app.note = Some("the weekly review opens in its weekend slot".into());
            }
            true
        }
        (_, KeyCode::Char('L')) => {
            app.open(ScreenId::Learning);
            true
        }
        _ => false,
    }
}

pub fn tick(app: &mut App, find: &locus_contract::Find) {
    let note = match find.kind {
        FindKind::Reading => format!("ticked: would push \"{}\" to /Reading on the tablet", find.title),
        _ => format!("ticked: \"{}\" would go on the board", find.title),
    };
    app.mark(find_key(find), crate::app::Mark::Ticked, note);
}

pub fn approve_one(app: &mut App, key: String, text: &str, to: Option<&str>, yes: bool) {
    let what = match to {
        Some(to) => format!("{text} → {to}"),
        None => text.to_string(),
    };
    if yes {
        app.mark(key, crate::app::Mark::Approved, format!("approved: would do {what}"));
    } else {
        app.mark(key, crate::app::Mark::Rejected, format!("refused: {what}"));
    }
}
