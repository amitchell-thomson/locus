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
use crate::ui::{frame, lines, work};
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
/// project, then the rest as registered. Only kinds you build: code and courses.
pub fn project_rows(app: &App) -> Vec<String> {
    rows_of(app, |k| matches!(k, ProjectKind::Code | ProjectKind::Course))
}

/// Applications and the CV.
pub fn career_rows(app: &App) -> Vec<String> {
    rows_of(app, |k| matches!(k, ProjectKind::Opportunity | ProjectKind::Light))
}

fn rows_of(app: &App, kind: impl Fn(ProjectKind) -> bool) -> Vec<String> {
    let fx = &app.fixtures;
    let mut rows: Vec<(u8, usize, String)> = Vec::new();
    for (i, p) in fx
        .registry
        .projects
        .iter()
        .enumerate()
        .filter(|(_, p)| p.active && kind(p.kind))
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
    let inner = frame(f, area, crate::ui::title("locus", "home"), None);
    let n_proj = project_rows(app).len().max(1) as u16;
    let n_side = career_rows(app).len().max(goal_rows(app).len() + 1).max(1) as u16;
    let [top, wait_r, work_r, proj_r, side_r, exp_r, sys_r] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(2),
        Constraint::Length(9),
        Constraint::Length(n_proj + 1),
        Constraint::Length(n_side + 1),
        Constraint::Min(3),
        Constraint::Length(3),
    ])
    .areas(inner);

    draw_top(f, top, app);
    draw_waiting(f, wait_r, app);
    work::draw_week(f, work_r, app, app.focus == Some('w'));
    draw_projects(f, proj_r, app);
    let [c_r, g_r] = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(side_r);
    draw_careers(f, c_r, app);
    draw_learning(f, g_r, app);
    draw_explore(f, exp_r, app);
    draw_system(f, sys_r, app);
}

/// The moment, centred so it is read (round 1: the corner it sat in "becomes kind of hidden");
/// then the next calendar item and mail.
fn draw_top(f: &mut Frame, area: Rect, app: &App) {
    let fx = &app.fixtures;
    let now = fx.now();
    let w = usize::from(area.width);
    let mut mid = vec![Span::styled(
        now.format("%A %-d %B · %H:%M").to_string().to_lowercase(),
        bold(text()),
    )];
    let running = fx.system.running.len();
    if running > 0 {
        mid.push(Span::styled("    ", text()));
        mid.push(Span::styled(
            format!("{} {running} jobs running", glyph::WORKING),
            pulse(app.frame),
        ));
    }
    if fx.home.review_ready {
        mid.push(Span::styled("    ", text()));
        mid.push(Span::styled("weekly review ready · W", bold(st("peach"))));
    }
    let mw: usize = mid.iter().map(|s| s.content.width()).sum();
    let mut line = vec![Span::raw(" ".repeat(w.saturating_sub(mw) / 2))];
    line.extend(mid);
    let next = match &fx.home.next {
        Some(n) => format!(
            "{}, {}{SEP}{} chores fit today",
            n.title,
            n.at.format("%a %H:%M").to_string().to_lowercase(),
            fx.home.chores_fit
        ),
        None => format!("nothing else today{SEP}{} chores fit", fx.home.chores_fit),
    };
    let mail = &fx.home.mail;
    let mail_s = if mail.need_reply + mail.deadlines_proposed == 0 {
        "nothing needs you".to_string()
    } else {
        format!(
            "{} need a reply{SEP}{} deadline proposed",
            mail.need_reply, mail.deadlines_proposed
        )
    };
    lines(
        f,
        area,
        vec![
            Line::from(line),
            Line::from(vec![
                Span::styled(format!(" {} next  ", glyph::CALENDAR), st("peach")),
                Span::styled(next, text()),
            ]),
            Line::from(vec![
                Span::styled(format!(" {} mail  ", glyph::MAIL), st("sapphire")),
                Span::styled(mail_s, text()),
            ]),
        ],
    );
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
        out.push(row(left, vec![Span::styled("⏎ go through them ", dim())], area.width));
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
                let gs = if s.state == SessionState::Working {
                    pulse(app.frame)
                } else {
                    st(sc)
                };
                left.push(Span::styled(format!("{sg} "), gs));
                left.push(Span::styled(format!("{word:<9}"), st(sc)));
                left.push(Span::styled(format!("{:>4}  ", ago(now, s.since)), dim()));
                let what = s.summary.clone().or_else(|| s.doing.clone()).unwrap_or_default();
                left.push(Span::styled(what, text()));
            }
            None => {
                left.push(Span::styled(format!("{} ", glyph::IDLE), dim()));
                left.push(Span::styled(format!("{:<15}", "no sessions"), dim()));
                left.push(Span::styled(
                    pf.and_then(|pf| pf.view.stage.clone()).unwrap_or_default(),
                    dim(),
                ));
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
        let stage = pf.and_then(|pf| pf.view.stage.clone()).unwrap_or_default();
        let g = if p.kind == ProjectKind::Light {
            glyph::DOSSIER
        } else {
            glyph::MAIL
        };
        let left = vec![
            pointer(i, sel),
            Span::styled(format!("{g} "), st("peach")),
            Span::styled(format!("{:<13}", crate::ui::truncate(&p.name, 12)), bold(st("peach"))),
            Span::styled(stage, text()),
        ];
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
    // Blank lines between groups only when everything still fits.
    let needed: usize = groups.iter().map(|(_, v)| v.len() + 1).sum::<usize>() + groups.len().saturating_sub(1);
    let spaced = needed <= room;
    let mut pos = 0;
    for (gi, (heading, idx)) in groups.iter().enumerate() {
        if gi > 0 && spaced {
            out.push(Line::from(""));
        }
        out.push(Line::from(Span::styled(
            format!("   {heading}{SEP}{}", idx.len()),
            bold(st("subtext1")),
        )));
        for &qi in idx {
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
    }
    if ex.queue.is_empty() {
        out.push(Line::from(Span::styled(
            "  nothing waiting; about one find a day",
            dim(),
        )));
    }
    if out.len() > room + 1 {
        let hidden = out.len() - room;
        out.truncate(room);
        out.push(Line::from(Span::styled(
            format!("   + {hidden} more on the Explore page"),
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
        jobs.push(Span::styled(
            format!("{} ", glyph::WORKING),
            pulse(app.frame + i as u64 * 3),
        ));
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
