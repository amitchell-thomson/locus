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

pub const KEYS: &[(&str, &str)] = &[
    ("y", "agree a proposed syllabus (its sessions go to kairos)"),
    ("e", "change it"),
    ("n", "a new goal"),
    ("m", "make a practice sheet for the selected track"),
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
    let goals_h = l.goals.len().max(1) as u16 + 1;
    let sel_goal = l.goals.get(app.sel('g').min(l.goals.len().saturating_sub(1)));
    let syl_h = sel_goal.map_or(0, |g| g.syllabus.len() as u16 + 2);
    let [goals_r, syl_r, tracks_r, sheets_r, foot] = Layout::vertical([
        Constraint::Length(goals_h),
        Constraint::Length(syl_h),
        Constraint::Min(4),
        Constraint::Length(5),
        Constraint::Length(1),
    ])
    .areas(inner);

    let focus = app.focus;
    let mut out = vec![rule("goals", "", Some('g'), focus == Some('g'), w)];
    for (i, g) in l.goals.iter().enumerate() {
        let (state, col) = match g.state {
            GoalState::Proposed => ("syllabus to agree", "peach"),
            GoalState::Active => ("active", "blue"),
            GoalState::Done => ("done", "green"),
        };
        let agreed = app.marked(&format!("goal:{}", g.id)) == Some(&Mark::Ticked);
        let hours: f32 = g.syllabus.iter().map(|s| s.hours).sum();
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
        out.push(Line::from(Span::styled("  no goals yet · n sets one", dim())));
    }
    lines(f, goals_r, out);

    if let Some(g) = sel_goal {
        let mut out = vec![rule(
            "syllabus",
            if g.state == GoalState::Proposed {
                "y agrees · e changes"
            } else {
                ""
            },
            None,
            false,
            w,
        )];
        for (i, s) in g.syllabus.iter().enumerate() {
            let conf = track(l, &s.track).map_or(0.0, |t| t.confidence);
            let name = track(l, &s.track).map_or(s.track.clone(), |t| t.name.clone());
            let mut spans = vec![
                Span::styled(format!(" {}. ", i + 1), dim()),
                Span::styled(format!("{:<22}", crate::ui::truncate(&name, 21)), text()),
            ];
            spans.extend(confidence(conf, 8));
            spans.push(Span::styled(format!(" {:>4.0} h  ", s.hours), text()));
            spans.push(Span::styled(
                crate::ui::truncate(&format!("{}{SEP}{}", s.what, s.from), usize::from(w).saturating_sub(45)),
                dim(),
            ));
            out.push(Line::from(spans));
        }
        lines(f, syl_r, out);
    }

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
        vec![
            hints(&[
                ("y", "agree"),
                ("e", "change"),
                ("n", "new goal"),
                ("m", "make a sheet"),
            ])
            .right_aligned(),
        ],
    );
}

pub fn key(app: &mut App, k: KeyEvent) -> bool {
    let l = app.fixtures.learning.clone();
    if let Some(d) = nav(&k) {
        let sec = app.focus.unwrap_or('g');
        let n = match sec {
            't' => l.tracks.len(),
            's' => l.sheets.len().min(4),
            _ => l.goals.len(),
        };
        app.move_sel(sec, n, d);
        return true;
    }
    let goal = l.goals.get(app.sel('g')).cloned();
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
        KeyCode::Char('n') => app.note = Some("would ask: what do you want to learn, and by when?".into()),
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
