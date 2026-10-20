//! Home (Alt-1): only what no other view shows (plan D). Next and mail one line each; the week's
//! work; every active project with its sessions; Explore finds beside the approve list; the
//! system's live state.

use crate::app::{App, Mark, Mode, nav};
use crate::screen::ScreenId;
use crate::ui::widgets::*;
use crate::ui::{frame, lines, work};
use locus_contract::{Find, ProjectKind, SessionState};
use locus_theme::{SEP, c, glyph};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};

pub const KEYS: &[(&str, &str)] = &[
    ("y n", "tick or strike a find · approve or refuse"),
    ("W", "the weekly review, when it is ready"),
    ("L", "learning"),
];

/// Active projects in the order Home lists them: a session that needs you first, then the
/// attended project, then the rest as registered.
pub fn project_rows(app: &App) -> Vec<String> {
    let fx = &app.fixtures;
    let mut rows: Vec<(u8, usize, String)> = Vec::new();
    for (i, p) in fx.registry.projects.iter().enumerate().filter(|(_, p)| p.active) {
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

/// The approve list flattened: (item key, text, destination).
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

pub fn find_key(f: &Find) -> String {
    format!("find:{}", f.id)
}

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let fx = &app.fixtures;
    let now = fx.now();
    let running = fx.system.running.len();
    let mut right = vec![Span::styled(
        format!(" {} ", now.format("%a %-d %b %H:%M").to_string().to_lowercase()),
        dim(),
    )];
    if running > 0 {
        right.push(Span::styled(
            format!("{} {running} running ", glyph::WORKING),
            st("blue"),
        ));
    }
    if fx.home.review_ready {
        right.push(Span::styled("· review ready ", st("peach")));
    }
    let inner = frame(f, area, crate::ui::title("locus", ""), Some(Line::from(right)));
    let w = inner.width;
    let n_proj = project_rows(app).len().max(1) as u16;
    let [top, work_r, proj_r, ea_r, sys_r] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(9),
        Constraint::Length(n_proj + 1),
        Constraint::Min(4),
        Constraint::Length(3),
    ])
    .areas(inner);

    // Next and mail.
    let next = match &fx.home.next {
        Some(n) => format!(
            "{} {}{SEP}{} chores fit today",
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
        top,
        vec![
            row(
                vec![
                    Span::styled(format!(" {} next  ", glyph::CALENDAR), st("peach")),
                    Span::styled(next, text()),
                ],
                vec![Span::styled("alt-3 calendar ", dim())],
                w,
            ),
            row(
                vec![
                    Span::styled(format!(" {} mail  ", glyph::MAIL), st("sapphire")),
                    Span::styled(mail_s, text()),
                ],
                vec![Span::styled("alt-4 mail ", dim())],
                w,
            ),
        ],
    );

    work::draw_week(f, work_r, app, app.focus == Some('w'));
    draw_projects(f, proj_r, app);
    let [e_r, a_r] = Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(ea_r);
    draw_explore(f, e_r, app);
    draw_approve(f, a_r, app);
    draw_system(f, sys_r, app);
}

fn draw_projects(f: &mut Frame, area: Rect, app: &App) {
    let fx = &app.fixtures;
    let now = fx.now();
    let focus = app.focus == Some('p');
    let ids = project_rows(app);
    let mut out = vec![rule(
        "projects",
        &format!("{} active", ids.len()),
        Some('p'),
        focus,
        area.width,
    )];
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
                format!("{:<13}", crate::ui::truncate(&p.name, 12)),
                bold(st(area_colour(app, &p.area))),
            ),
        ];
        let mut sessions: Vec<_> = pf.map(|pf| pf.view.sessions.iter().collect()).unwrap_or_default();
        sessions.sort_by_key(|s| s.state);
        match sessions.first() {
            Some(s) => {
                let (sg, sc, word) = session_glyph(s.state);
                left.push(Span::styled(format!("{sg} {word}"), st(sc)));
                left.push(Span::styled(format!("{SEP}{}", ago(now, s.since)), dim()));
                if let Some(d) = &s.doing {
                    left.push(Span::styled(format!("{SEP}{d}"), text()));
                }
            }
            None => {
                let stage = pf.and_then(|pf| pf.view.stage.clone()).unwrap_or_default();
                let kind = match p.kind {
                    ProjectKind::Opportunity => "opportunity",
                    ProjectKind::Course => "course",
                    ProjectKind::Light | ProjectKind::Code => "no sessions",
                };
                left.push(Span::styled(format!("{} {kind}", glyph::IDLE), dim()));
                if !stage.is_empty() {
                    left.push(Span::styled(format!("{SEP}{stage}"), dim()));
                }
            }
        }
        let mut right = Vec::new();
        if sessions.len() > 1 {
            right.push(Span::styled(format!("+{} ", sessions.len() - 1), dim()));
        }
        let unread = pf.map(|pf| pf.view.unread()).unwrap_or(0);
        if unread > 0 {
            right.push(Span::styled(format!("{} {unread} ", glyph::UNREAD), st("peach")));
        } else {
            right.push(Span::raw("    "));
        }
        out.push(row(left, right, area.width));
    }
    if ids.is_empty() {
        out.push(Line::from(Span::styled("  no active projects", dim())));
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

fn draw_explore(f: &mut Frame, area: Rect, app: &App) {
    let ex = &app.fixtures.explore;
    let focus = app.focus == Some('e');
    let full = if ex.queue.len() as u32 >= ex.queue_cap {
        " · full, scout paused"
    } else {
        ""
    };
    let mut out = vec![rule(
        "explore",
        &format!("{} of {}{full}", ex.queue.len(), ex.queue_cap),
        Some('e'),
        focus,
        area.width,
    )];
    let sel = if focus { app.sel('e') } else { 0 };
    let width = usize::from(area.width);
    for (i, find) in ex.queue.iter().enumerate() {
        let (g, gc, kind) = find_glyph(find.kind);
        let mark = app.marked(&find_key(find));
        let style = match mark {
            Some(Mark::Struck(_)) => struck(text()),
            Some(Mark::Ticked) => st("green"),
            _ => text(),
        };
        let who = find.project.as_deref().unwrap_or(kind);
        let ptr = if i == sel {
            Span::styled("▸ ", keyst())
        } else {
            Span::raw("  ")
        };
        out.push(Line::from(vec![
            ptr,
            Span::styled(format!("{g} "), st(gc)),
            Span::styled(format!("{:<8}", crate::ui::truncate(who, 7)), dim()),
            Span::styled(crate::ui::truncate(&find.title, width.saturating_sub(13)), style),
        ]));
        if i == sel {
            out.push(Line::from(Span::styled(
                format!("     why: {}", crate::ui::truncate(&find.why, width.saturating_sub(10))),
                dim(),
            )));
            let mut cost = format!("     {} min", find.minutes);
            if let Some(t) = &find.touches {
                cost = format!("     touches {}{SEP}{} min", crate::ui::truncate(t, 24), find.minutes);
            }
            out.push(row(
                vec![Span::styled(cost, dim())],
                vec![
                    Span::styled("y", keyst()),
                    Span::styled(" ✓ ", dim()),
                    Span::styled("n", keyst()),
                    Span::styled(" ✗ ", dim()),
                ],
                area.width,
            ));
        }
    }
    if ex.queue.is_empty() {
        out.push(Line::from(Span::styled(
            "  the queue is empty; the scout is looking",
            dim(),
        )));
    }
    lines(f, area, out);
}

fn draw_approve(f: &mut Frame, area: Rect, app: &App) {
    let focus = app.focus == Some('a');
    let rows = approve_rows(app);
    let mut out = vec![rule("approve", &rows.len().to_string(), Some('a'), focus, area.width)];
    let sel = focus.then(|| app.sel('a'));
    let room = usize::from(area.height.saturating_sub(1));
    for (i, (k, t, to, _)) in rows.iter().enumerate() {
        if i + 1 == room && rows.len() > room {
            out.push(Line::from(Span::styled(format!("  + {} more", rows.len() - i), dim())));
            break;
        }
        let style = match app.marked(k) {
            Some(Mark::Approved) => st("green"),
            Some(Mark::Rejected) => struck(text()),
            _ => text(),
        };
        let s = match to {
            Some(to) => format!("{t} → {to}"),
            None => t.clone(),
        };
        out.push(Line::from(vec![
            pointer(i, sel),
            Span::styled(
                crate::ui::truncate(&s, usize::from(area.width).saturating_sub(3)),
                style,
            ),
        ]));
    }
    if rows.is_empty() {
        out.push(Line::from(Span::styled("  nothing to approve", dim())));
    }
    lines(f, area, out);
}

fn draw_system(f: &mut Frame, area: Rect, app: &App) {
    let fx = &app.fixtures;
    let now = fx.now();
    let focus = app.focus == Some('s');
    let mut out = vec![rule("system", "", Some('s'), focus, area.width)];
    let mut jobs: Vec<Span> = vec![Span::raw(" ")];
    for j in &fx.system.running {
        let model = j.model.as_deref().map(|m| format!("{SEP}{m}")).unwrap_or_default();
        jobs.push(Span::styled(format!("{} ", glyph::WORKING), st("blue")));
        jobs.push(Span::styled(
            format!("{}{SEP}{}{model}{SEP}{}   ", j.kind, j.subject, ago(now, j.started)),
            text(),
        ));
    }
    if fx.system.running.is_empty() {
        jobs.push(Span::styled("nothing running   ", dim()));
    }
    let counts = vec![
        Span::styled(format!("{} {} today ", glyph::DONE, fx.system.done_today), st("green")),
        Span::styled(
            format!("{} {} ", glyph::FAILED, fx.system.failed_today),
            if fx.system.failed_today > 0 { st("red") } else { dim() },
        ),
    ];
    out.push(row(jobs, counts, area.width));
    let u = &fx.usage;
    let bw = 30;
    let mut b = vec![Span::styled(format!(" {} budget ", glyph::BUDGET), dim())];
    b.extend(bar(u.week_pct / (u.cap_pct * 1.3), bw, Some(1.0 / 1.3), c("mauve")));
    b.push(Span::styled(
        format!("  {:.1}% of {:.0}% this week", u.week_pct, u.cap_pct),
        text(),
    ));
    out.push(row(
        b,
        vec![Span::styled(
            format!("window {:.0}%{SEP}{:.0}% ", u.window.you_pct, u.window.locus_pct),
            dim(),
        )],
        area.width,
    ));
    lines(f, area, out);
}

pub fn key(app: &mut App, k: KeyEvent) -> bool {
    let focus = app.focus;
    if let Some(d) = nav(&k) {
        let len = match focus {
            Some('p') => project_rows(app).len(),
            Some('e') => app.fixtures.explore.queue.len(),
            Some('a') => approve_rows(app).len(),
            _ => return false,
        };
        if let Some(sec) = focus {
            app.move_sel(sec, len, d);
        }
        return true;
    }
    match (focus, k.code) {
        (Some('p'), KeyCode::Enter) => {
            if let Some(id) = project_rows(app).get(app.sel('p')).cloned() {
                app.viewing = Some(id);
                app.open(ScreenId::Project);
            }
            true
        }
        (Some('e'), KeyCode::Char('y')) => {
            if let Some(find) = app.fixtures.explore.queue.get(app.sel('e')).cloned() {
                tick(app, &find);
            }
            true
        }
        (Some('e'), KeyCode::Char('n')) => {
            if let Some(find) = app.fixtures.explore.queue.get(app.sel('e')) {
                app.mode = Mode::Strike(find.id.clone());
                app.note = Some("why? i irrelevant · k knew it · l level · t too long".into());
            }
            true
        }
        (Some('a'), KeyCode::Char(ch @ ('y' | 'n'))) => {
            if let Some((key, t, to, _)) = approve_rows(app).get(app.sel('a')).cloned() {
                approve_one(app, key, &t, to.as_deref(), ch == 'y');
            }
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

pub fn tick(app: &mut App, find: &Find) {
    let note = match find.kind {
        locus_contract::FindKind::Reading => format!("ticked: would push \"{}\" to /Reading on the tablet", find.title),
        _ => format!("ticked: \"{}\" would go on the board", find.title),
    };
    app.mark(find_key(find), Mark::Ticked, note);
}

pub fn approve_one(app: &mut App, key: String, text: &str, to: Option<&str>, yes: bool) {
    let what = match to {
        Some(to) => format!("{text} → {to}"),
        None => text.to_string(),
    };
    if yes {
        app.mark(key, Mark::Approved, format!("approved: would do {what}"));
    } else {
        app.mark(key, Mark::Rejected, format!("refused: {what}"));
    }
}
