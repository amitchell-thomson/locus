//! The learning system (C.11): goals with their syllabus shown before they start, topic tracks
//! with a confidence bar and its history, practice sheets. Not live yet: slice L builds it.

use crate::app::{App, Mark, nav};
use crate::ui::widgets::*;
use crate::ui::{esc_back, frame, hints, lines, title};
use locus_contract::{GoalState, Learning, SheetState, Track};
use locus_theme::{SEP, c, glyph};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};

/// Learning's own keys, after the shared table (cleanup C4): there y agrees a proposed syllabus,
/// e changes it and + is a new goal (it was n).
pub const KEYS: &[(&str, &str)] = &[
    ("⇥ ↑ ↓ j k", "sections, and through them"),
    ("m", "a practice sheet for the track"),
];

const SPARK: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

fn spark(t: &Track) -> String {
    t.history
        .iter()
        .map(|p| SPARK[((p.confidence * 7.0).round() as usize).min(7)])
        .collect()
}

fn confidence(conf: f32, width: usize) -> Vec<Span<'static>> {
    let col = if conf < 0.4 {
        c("red")
    } else if conf < 0.6 {
        c("peach")
    } else {
        c("green")
    };
    bar(conf, width, None, col)
}

/// A topic with no subtopics after it: only these count towards a goal's hours (a parent's
/// hours are its children's sum).
fn is_leaf(items: &[locus_contract::SyllabusItem], s: &locus_contract::SyllabusItem) -> bool {
    let i = items.iter().position(|x| std::ptr::eq(x, s)).unwrap_or(0);
    items.get(i + 1).is_none_or(|n| n.depth <= s.depth)
}

/// Words onto lines of at most `width` characters.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = vec![String::new()];
    for word in text.split_whitespace() {
        let cur = out.last_mut().map_or(0, |l| l.chars().count());
        if cur > 0 && cur + 1 + word.chars().count() > width.max(10) {
            out.push(String::new());
        }
        if let Some(l) = out.last_mut() {
            if !l.is_empty() {
                l.push(' ');
            }
            l.push_str(word);
        }
    }
    out
}

fn track<'a>(l: &'a Learning, id: &str) -> Option<&'a Track> {
    l.tracks.iter().find(|t| t.id == id)
}

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let l = &app.fixtures.learning;
    let active = l.goals.iter().filter(|g| g.state != GoalState::Done).count();
    let inner = frame(
        f,
        area,
        title("learning", &format!("{active} goals{SEP}{} tracks", l.tracks.len())),
        Some(esc_back()),
    );
    let w = inner.width;
    let focus = app.focus;
    let goals_h = l.goals.len().max(1) as u16 + 1;
    let sel_goal = l.goals.get(app.sel('g').min(l.goals.len().saturating_sub(1)));
    // Tracks and sheets take room only once there is something in them (spec L: until marked
    // practice exists, the curriculum is the screen).
    let practised = l.tracks.iter().any(|t| t.confidence > 0.0 || !t.sessions.is_empty());
    let tracks_h = if practised {
        (l.tracks.len() as u16 + 1).min(6)
    } else {
        0
    };
    let sheets_h = if l.sheets.is_empty() {
        0
    } else {
        (l.sheets.len() as u16 + 1).min(5)
    };
    let [goals_r, cur_r, detail_r, tracks_r, sheets_r, foot] = Layout::vertical([
        Constraint::Length(goals_h),
        Constraint::Min(4),
        Constraint::Length(4),
        Constraint::Length(tracks_h),
        Constraint::Length(sheets_h),
        Constraint::Length(1),
    ])
    .areas(inner);

    let mut out = vec![rule("goals", "", Some('g'), focus == Some('g'), w)];
    for (i, g) in l.goals.iter().enumerate() {
        let (state, col) = match g.state {
            GoalState::Proposed => ("curriculum to agree", "peach"),
            GoalState::Active => ("active", "blue"),
            GoalState::Done => ("done", "green"),
        };
        let agreed = app.marked(&format!("goal:{}", g.id)) == Some(&Mark::Ticked);
        let hours: f32 = g
            .syllabus
            .iter()
            .filter(|s| is_leaf(&g.syllabus, s))
            .map(|s| s.hours)
            .sum::<f32>()
            + 0.0;
        out.push(row(
            vec![
                pointer(i, (focus == Some('g') || focus.is_none()).then(|| app.sel('g'))),
                Span::styled(format!("{} ", glyph::IDEA), st(col)),
                Span::styled(g.title.clone(), if g.state == GoalState::Done { dim() } else { text() }),
            ],
            vec![Span::styled(
                format!("{hours:.0} h{SEP}{} ", if agreed { "agreed" } else { state }),
                st(if agreed { "green" } else { col }),
            )],
            w,
        ));
    }
    if l.goals.is_empty() {
        let msg = match &app.learning_error {
            Some(e) => format!("  could not read the goals: {e}"),
            None if app.home_live => "  reading the goals from the server…".to_string(),
            None => "  no goals yet · + sets one".to_string(),
        };
        out.push(Line::from(Span::styled(msg, dim())));
    }
    lines(f, goals_r, out);

    // The curriculum: the selected goal's topics, depth first, scrolled to the cursor.
    let fo = focus == Some('c');
    let sel = app.sel('c');
    let mut out = Vec::new();
    if let Some(g) = sel_goal {
        let right = g.pace.clone().unwrap_or_default();
        out.push(rule(
            "curriculum",
            &crate::ui::truncate(&right, usize::from(w) / 2),
            Some('c'),
            fo,
            w,
        ));
        let room = usize::from(cur_r.height.saturating_sub(1));
        let start = if fo && sel >= room { sel + 1 - room } else { 0 };
        for (i, s) in g.syllabus.iter().enumerate().skip(start).take(room) {
            let indent = "  ".repeat(usize::from(s.depth));
            let conf = track(l, &s.track).map_or(0.0, |t| t.confidence);
            let spans = vec![
                pointer(i, fo.then_some(sel)),
                Span::styled(
                    format!(
                        "{indent}{}",
                        crate::ui::truncate(&s.what, usize::from(w).saturating_sub(40 + indent.len()))
                    ),
                    if s.depth == 0 { text() } else { dim() },
                ),
            ];
            let right = vec![
                Span::styled(format!("{} ", s.track.replace('-', " ")), dim()),
                Span::styled(format!("{:>4.0} h ", s.hours), text()),
            ];
            let mut r = row(spans, right, w);
            if conf > 0.0 {
                r.spans.extend(confidence(conf, 6));
            }
            out.push(r);
        }
        if g.syllabus.is_empty() {
            out.push(Line::from(Span::styled(
                "  no curriculum yet: `locus learn propose` writes one",
                dim(),
            )));
        }
    }
    lines(f, cur_r, out);

    // Why the selected topic is there, and the line of his material it rests on; the curriculum's
    // summary when the cursor is elsewhere.
    let mut out = Vec::new();
    if let Some(g) = sel_goal {
        let width = usize::from(w).saturating_sub(4);
        match g.syllabus.get(sel).filter(|_| fo) {
            Some(s) => {
                if let Some(why) = &s.why {
                    out.push(Line::from(Span::styled(
                        format!("  {}", crate::ui::truncate(why, width)),
                        text(),
                    )));
                }
                if let Some(q) = &s.quote {
                    out.push(Line::from(vec![
                        Span::styled(
                            format!(
                                "  \u{201c}{}\u{201d} ",
                                crate::ui::truncate(q, width.saturating_sub(16))
                            ),
                            st("lavender"),
                        ),
                        Span::styled(format!("[{}]", s.from), dim()),
                    ]));
                }
            }
            None => {
                if let Some(sm) = &g.summary {
                    for chunk in wrap(sm, width).into_iter().take(3) {
                        out.push(Line::from(Span::styled(format!("  {chunk}"), dim())));
                    }
                }
            }
        }
    }
    lines(f, detail_r, out);

    let fo = focus == Some('t');
    let mut out = vec![rule("tracks", "confidence · history · misses open", Some('t'), fo, w)];
    let mut ts: Vec<&Track> = l.tracks.iter().collect();
    ts.sort_by(|a, b| a.confidence.total_cmp(&b.confidence));
    for (i, t) in ts.iter().enumerate() {
        let mut spans = vec![
            pointer(i, fo.then(|| app.sel('t'))),
            Span::styled(format!("{:<24}", crate::ui::truncate(&t.name, 23)), text()),
        ];
        spans.extend(confidence(t.confidence, 16));
        spans.push(Span::styled(format!(" {:>3.0}%  ", t.confidence * 100.0), text()));
        spans.push(Span::styled(format!("{:<8}", spark(t)), st("lavender")));
        let sessions = if t.sessions.is_empty() {
            "no sessions".to_string()
        } else {
            format!(
                "{} session{}",
                t.sessions.len(),
                if t.sessions.len() == 1 { "" } else { "s" }
            )
        };
        spans.push(Span::styled(format!(" {sessions}"), dim()));
        if t.open_misses > 0 {
            spans.push(Span::styled(format!("{SEP}{} misses open", t.open_misses), st("peach")));
        }
        out.push(Line::from(spans));
    }
    if l.tracks.is_empty() {
        out.push(Line::from(Span::styled(
            "  no tracks yet: a goal's syllabus makes them",
            dim(),
        )));
    }
    lines(f, tracks_r, out);

    let fo = focus == Some('s');
    let mut out = vec![rule("practice sheets", "", Some('s'), fo, w)];
    for (i, s) in l.sheets.iter().enumerate().take(4) {
        let state = match (s.state, s.right) {
            (SheetState::Draft, _) => "draft · m sends it".to_string(),
            (SheetState::OnTablet, _) => "on the tablet".into(),
            (SheetState::Answered, _) => "answered · waiting for the readback".into(),
            (SheetState::Marked, Some(r)) => format!("{r} of {} right", s.questions),
            (SheetState::Marked, None) => "marked".into(),
        };
        out.push(row(
            vec![
                pointer(i, fo.then(|| app.sel('s'))),
                Span::styled(format!("{} ", glyph::INK), st("flamingo")),
                Span::styled(s.title.clone(), text()),
            ],
            vec![Span::styled(format!("{}{SEP}{state} ", date(s.sent)), dim())],
            w,
        ));
    }
    if l.sheets.len() > 4 {
        // The list keeps its newest four; the rest are one key away.
        out.truncate(5);
    }
    if l.sheets.is_empty() {
        out.push(Line::from(Span::styled("  no sheets yet", dim())));
    }
    lines(f, sheets_r, out);
    lines(
        f,
        foot,
        vec![hints(&[("c", "curriculum"), ("y", "agree"), ("e", "change"), ("+", "new goal")]).right_aligned()],
    );
}

/// Agree (`learn approve`: topics current, a kairos task per topic at the pace) or refuse
/// (`learn reject`: the current curriculum stays) a goal's proposed curriculum, on the server.
pub fn decide(goal: &str, yes: bool) -> Result<String, String> {
    let verb = if yes { "approve" } else { "reject" };
    let said = crate::live::server_locus(&["learn".into(), verb.into(), goal.into()])?;
    let first = said.lines().next().unwrap_or("done").to_string();
    let last = said.lines().last().unwrap_or("").to_string();
    Ok(if yes && last != first {
        format!("{first} {last}")
    } else {
        first
    })
}

pub fn key(app: &mut App, k: KeyEvent) -> bool {
    let l = app.fixtures.learning.clone();
    if let Some(d) = nav(&k) {
        let sec = app.focus.unwrap_or('g');
        let n = match sec {
            'c' => l.goals.get(app.sel('g')).map_or(0, |g| g.syllabus.len()),
            't' => l.tracks.len(),
            's' => l.sheets.len().min(4),
            _ => l.goals.len(),
        };
        app.move_sel(sec, n, d);
        return true;
    }
    let goal = l.goals.get(app.sel('g')).cloned();
    // Live (spec L), nothing is pretended: agreeing and changing go through the server's steps.
    if app.home_live {
        if k.code == KeyCode::Char('y') {
            app.note = Some(match goal {
                Some(g) if g.state == GoalState::Proposed => match decide(&g.id, true) {
                    Ok(m) => {
                        if let Some(x) = app.fixtures.learning.goals.iter_mut().find(|x| x.id == g.id) {
                            x.state = GoalState::Active;
                        }
                        app.fixtures
                            .approve
                            .groups
                            .iter_mut()
                            .for_each(|grp| grp.items.retain(|it| it.goal.as_deref() != Some(&g.id)));
                        app.fixtures.approve.groups.retain(|grp| !grp.items.is_empty());
                        m
                    }
                    Err(e) => format!("heuris said no: {e}"),
                },
                _ => "only a proposed curriculum needs agreeing".into(),
            });
            return true;
        }
        let msg = match k.code {
            KeyCode::Char('e') => {
                "to change it, tell any Claude what to change; it re-proposes (`locus learn note`, then `propose`)"
            }
            KeyCode::Char('+') => "a new goal: tell any Claude what you want to learn and by when (`locus learn add`)",
            KeyCode::Char('m') => "practice sheets come with step L4",
            _ => return false,
        };
        app.note = Some(msg.into());
        return true;
    }
    match k.code {
        KeyCode::Char('y') => match goal {
            Some(g) if g.state == GoalState::Proposed => {
                let hours: f32 = g.syllabus.iter().map(|s| s.hours).sum();
                app.mark(
                    format!("goal:{}", g.id),
                    Mark::Ticked,
                    format!("agreed: would ask kairos for {hours:.0} h of study blocks"),
                );
            }
            _ => app.note = Some("only a proposed syllabus needs agreeing".into()),
        },
        KeyCode::Char('e') => app.note = Some("would open the syllabus to change in a chat".into()),
        KeyCode::Char('+') => app.note = Some("would ask: what do you want to learn, and by when?".into()),
        KeyCode::Char('m') => {
            let mut ts: Vec<&Track> = l.tracks.iter().collect();
            ts.sort_by(|a, b| a.confidence.total_cmp(&b.confidence));
            let t = if app.focus == Some('t') {
                ts.get(app.sel('t')).copied()
            } else {
                ts.first().copied()
            };
            app.note = Some(match t {
                Some(t) => format!("would make a practice sheet on {} for the tablet", t.name),
                None => "no tracks to make a sheet from".into(),
            });
        }
        _ => return false,
    }
    true
}
