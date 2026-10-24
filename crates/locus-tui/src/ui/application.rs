//! The Application screen (round 5, 23 Oct): an application is not a project, so it has a page of
//! its own ("other than the now, new and next section, I do not want this page looking like the
//! projects page as that could be a source of confusion").
//!
//! Top to bottom: where you are (now, new, next); the timeline, one track through the stages with
//! every mail, interview, assessment and submission on it ("would need to look different than the
//! git branches as it is a different concept, but I do still want it to be visually appealing");
//! the prep overview; the questions you were asked; the ask line. The stage's icon sits at the
//! far right of the top border.

use crate::app::{App, Mode};
use crate::screen::ScreenId;
use crate::ui::project::{draw_ask, where_you_are};
use crate::ui::widgets::*;
use crate::ui::{follow_badge, lines};
use chrono::NaiveDate;
use locus_contract::{AppEventKind, Application, ProjectView, QuestionState, StageState, Turn};
use locus_theme::{SEP, c, glyph};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

pub const KEYS: &[(&str, &str)] = &[
    ("← →", "walk the timeline"),
    ("p", "the full prep plan"),
    ("r", "recall a question you were asked: a chat helps you pin it down"),
    ("␣", "ask about this application"),
];

pub fn view(app: &App) -> Option<&ProjectView> {
    let id = app.project_id()?;
    app.fixtures.projects.get(&id).map(|p| &p.view)
}

/// The icon for a stage, at the far right of the top border and on Home's careers rows.
pub fn stage_glyph(stage: &str) -> (&'static str, &'static str) {
    let s = stage.to_lowercase();
    if s.contains("not offered") || s.contains("reject") || s.contains("withdrawn") {
        (glyph::FAILED, "red")
    } else if s.contains("accepted") || s.contains("complete") {
        (glyph::DONE, "green")
    } else if s.contains("offer") {
        (glyph::APPROVE, "green")
    } else if s.contains("final") || s.contains("interview") {
        (glyph::CALENDAR, "peach")
    } else if s.contains("assessment") || s.contains("test") {
        (glyph::DOSSIER, "yellow")
    } else {
        (glyph::MAIL, "sky")
    }
}

fn event_glyph(k: AppEventKind) -> (&'static str, &'static str) {
    match k {
        AppEventKind::Mail => (glyph::MAIL, "sky"),
        AppEventKind::Interview => (glyph::CALENDAR, "peach"),
        AppEventKind::Assessment => (glyph::DOSSIER, "yellow"),
        AppEventKind::Submission => (glyph::PROJECT, "teal"),
        AppEventKind::Call => (glyph::CHAT, "lavender"),
        AppEventKind::Outcome => (glyph::APPROVE, "green"),
    }
}

pub fn section_len(app: &App, sec: char) -> usize {
    let Some(v) = view(app) else { return 0 };
    let a = v.application.as_ref();
    match sec {
        'y' => crate::ui::project::section_len_where(app, ScreenId::Application, v),
        't' => a.map_or(0, |a| a.events.len()),
        'p' => 1,
        'q' => a.map_or(0, |a| a.questions.len()).max(1),
        _ => 1,
    }
}

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let now = app.fixtures.now();
    let v = view(app);
    let name = crate::ui::project_title(app);
    let mut t = vec![Span::styled(format!(" {name} "), bold(text()))];
    let mut right = vec![];
    if let Some(v) = v {
        if let Some(stage) = v.stage.as_deref() {
            t.push(Span::styled(format!("· {stage} "), dim()));
        }
        // Whose move, then the stage's icon at the far right (round 5: "a stage icon could be
        // good, on the far right").
        if let Some(st_) = &v.status {
            match st_.turn {
                Some(Turn::You) => right.push(Span::styled(" your move ", bold(st("peach")))),
                Some(Turn::Them) => {
                    let days = (now.date_naive() - st_.since).num_days();
                    right.push(Span::styled(format!(" their move · {days} days "), dim()))
                }
                None => {}
            }
            let (g, gc) = stage_glyph(&st_.stage);
            right.push(Span::styled(format!(" {g} "), st(gc)));
        }
    }
    if app.stack.first() == Some(&ScreenId::Project) {
        right.extend(follow_badge(app).spans);
    } else {
        right.push(Span::styled(" esc back ", dim()));
    }
    let block = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(st("overlay0"))
        .title(Line::from(t).left_aligned())
        .title(Line::from(right).right_aligned());
    let inner = block.inner(area);
    f.render_widget(block, area);
    let Some(v) = v else {
        lines(
            f,
            inner,
            vec![
                Line::from(""),
                Line::from(Span::styled("   no application chosen", dim())),
            ],
        );
        return;
    };
    let w = inner.width;
    let a = v.application.as_ref();
    // Sizes: where you are as on the Project view; the timeline's strip and its events; prep and
    // questions as tall as they need; the ask line at the foot.
    let (where_probe, _) = where_you_are(app, v, w, 0, false);
    let new_rows = v.new.len().clamp(1, 3);
    let where_h = where_probe.len() + new_rows;
    let prep_lines = prep(app, v, w);
    let q_lines = asked(app, a, w);
    let strip_h = 6u16;
    let events = a.map_or(0, |a| a.events.len()) as u16;
    let fixed = where_h as u16 + 1 + 1 + strip_h + 1 + prep_lines.len() as u16 + 1 + q_lines.len() as u16 + 1 + 2;
    let ev_h = events
        .min(inner.height.saturating_sub(fixed))
        .max(u16::from(events > 0));
    let (where_lines, shown) = where_you_are(app, v, w, new_rows, false);
    app.shown.borrow_mut().insert((ScreenId::Application, 'y'), shown);
    let gap = Constraint::Length(1);
    let [where_r, _, tl_r, _, prep_r, _, q_r, _, ask_r] = Layout::vertical([
        Constraint::Length(where_lines.len() as u16),
        gap,
        Constraint::Length(1 + strip_h + ev_h),
        gap,
        Constraint::Length(prep_lines.len() as u16),
        gap,
        Constraint::Length(q_lines.len() as u16),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .areas(inner);
    lines(f, where_r, where_lines);
    if let Some(a) = a {
        draw_timeline(f, tl_r, app, a, strip_h);
    }
    lines(f, prep_r, prep_lines);
    lines(f, q_r, q_lines);
    draw_ask(f, ask_r, app, v);
    let focused = match app.focus {
        Some('y') => Some(where_r),
        Some('t') => Some(tl_r),
        Some('p') => Some(prep_r),
        Some('q') => Some(q_r),
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

/// The timeline: one track through the stages, dated, with every event placed on it by date
/// and today marked; the selected event explained under it, then the events listed.
fn draw_timeline(f: &mut Frame, area: Rect, app: &App, a: &Application, strip_h: u16) {
    let now = app.fixtures.now().date_naive();
    let focus = app.focus == Some('t');
    let w = usize::from(area.width);
    let mut out = vec![rule(
        "timeline",
        // A key for the marks on the track (round 5c: "what are the green dots").
        &format!("{} events · ● stage done · ◉ now · ○ to come · ▲ today", a.events.len()),
        Some('t'),
        focus,
        area.width,
    )];
    // The span: the first dated thing to the last, or today and the next date if later.
    let mut dates: Vec<NaiveDate> = a.events.iter().map(|e| e.at.date_naive()).collect();
    dates.extend(a.stages.iter().filter_map(|s| s.date));
    dates.push(now);
    if let Some(n) = &a.next {
        dates.push(n.at.date_naive());
    }
    let start = dates.iter().min().copied().unwrap_or(now);
    let mut end = dates.iter().max().copied().unwrap_or(now);
    // Stages still to come with no date sit evenly after the last date.
    let undated = a.stages.iter().filter(|s| s.date.is_none()).count() as i64;
    let span0 = (end - start).num_days().max(1);
    let step = (span0 / 6).max(2);
    end += chrono::Duration::days(step * undated);
    let span = (end - start).num_days().max(1) as f64;
    let (left, right_pad) = (4usize, 6usize);
    let track_w = w.saturating_sub(left + right_pad).max(10);
    let x = |d: NaiveDate| left + (((d - start).num_days() as f64 / span) * (track_w - 1) as f64).round() as usize;
    // Where each stage sits: its date; an undated stage between its dated neighbours, or, if
    // nothing dated follows it, evenly after the last date (one still to come) or just after its
    // predecessor (one already passed).
    let last_dated = dates.iter().max().copied().unwrap_or(now);
    let dated: Vec<Option<NaiveDate>> = a.stages.iter().map(|s| s.date).collect();
    let mut stage_d = Vec::new();
    let mut ahead = 0;
    for (i, s) in a.stages.iter().enumerate() {
        let d = match s.date {
            Some(d) => d,
            None => {
                let prev = (0..i).rev().find_map(|j| dated[j].map(|d| (j, d)));
                let next = (i + 1..dated.len()).find_map(|j| dated[j].map(|d| (j, d)));
                match (prev, next) {
                    (Some((j0, d0)), Some((j1, d1))) => {
                        let f = (i - j0) as i64;
                        let n = (j1 - j0) as i64;
                        d0 + chrono::Duration::days((d1 - d0).num_days() * f / n)
                    }
                    _ if s.state == StageState::Upcoming => {
                        ahead += 1;
                        last_dated + chrono::Duration::days(step * ahead)
                    }
                    (Some((_, d0)), None) => d0 + chrono::Duration::days(1),
                    (None, _) => start,
                }
            }
        };
        stage_d.push(d);
    }
    let stage_x: Vec<usize> = stage_d.iter().map(|&d| x(d)).collect();
    // The selection is a place in the list below, newest first, so ↓ goes older and down the
    // list, and ← (older) and → (newer) follow the track (round 5a: "when you press the down
    // arrow [the highlight] moves up which is counterintuitive"). `sel` is the event it names.
    let n_ev = a.events.len();
    let pos = focus.then(|| app.sel('t').min(n_ev.saturating_sub(1)));
    let sel = pos.map(|k| n_ev - 1 - k);
    let blank = || vec![(' ', Style::default()); w];
    // Rows: stage names (two rows, alternating so they never collide), the track, the events,
    // the dates.
    let mut names = [blank(), blank()];
    let mut track = blank();
    let mut marks = blank();
    let mut when = blank();
    let put = |row: &mut Vec<(char, Style)>, at: usize, s: &str, style: Style| {
        let mut i = at;
        for ch in s.chars() {
            if i < row.len() {
                row[i] = (ch, style);
            }
            i += 1;
        }
    };
    let now_x = x(now);
    // The track: done stages solid, the current stage lit, what is still to come dashed.
    for (col, cell) in track.iter_mut().enumerate().skip(left).take(track_w) {
        *cell = if col <= now_x {
            ('━', st("overlay1"))
        } else {
            ('╌', st("surface2"))
        };
    }
    if let Some(cur) = a.stages.iter().position(|s| s.state == StageState::Current) {
        let from = stage_x[cur];
        let to = stage_x.get(cur + 1).copied().unwrap_or(now_x).min(now_x.max(from));
        for cell in track.iter_mut().take(to.max(from) + 1).skip(from) {
            *cell = ('━', st("blue"));
        }
    }
    // Stage names on two rows above the track, alternating, each cut short of the next one on its
    // row rather than written over it; the part in brackets is dropped.
    let short = |n: &str| {
        n.split(" (")
            .next()
            .unwrap_or(n)
            .split(" / ")
            .next()
            .unwrap_or(n)
            .to_string()
    };
    let mut date_end = 0usize;
    let mut last_end = [0usize, 0usize];
    let mut placed: Vec<(usize, usize, String)> = Vec::new();
    for (i, s) in a.stages.iter().enumerate() {
        let sx = stage_x[i];
        let label = short(&s.name);
        let lx = sx
            .saturating_sub(label.width() / 2)
            .min(w.saturating_sub(label.width()));
        let row = if lx >= last_end[0] {
            0
        } else if lx >= last_end[1] {
            1
        } else if last_end[0] <= last_end[1] {
            0
        } else {
            1
        };
        let lx = lx.max(last_end[row]);
        placed.push((row, lx, label));
        last_end[row] = lx + placed.last().map_or(0, |p| p.2.width()) + 1;
    }
    // Cut each label short of the next one on its row.
    for k in 0..placed.len() {
        let (row, lx) = (placed[k].0, placed[k].1);
        if let Some(nx) = placed[k + 1..].iter().find(|p| p.0 == row).map(|p| p.1) {
            let room = nx.saturating_sub(lx + 1);
            if placed[k].2.width() > room {
                placed[k].2 = crate::ui::truncate(&placed[k].2, room);
            }
        }
    }
    for (i, s) in a.stages.iter().enumerate() {
        let sx = stage_x[i];
        let (ch, style) = match s.state {
            StageState::Done => ('●', st("green")),
            StageState::Current => ('◉', bold(st("blue"))),
            StageState::Upcoming => ('○', st("overlay1")),
        };
        if sx < w {
            track[sx] = (ch, style);
        }
        let ls = match s.state {
            StageState::Current => bold(st("blue")),
            StageState::Done => text(),
            StageState::Upcoming => dim(),
        };
        let (row, lx, label) = &placed[i];
        put(&mut names[*row], *lx, label, ls);
        // A date that would run into the one before it is left out (round 5).
        if let Some(d) = s.date {
            let dl = date(d);
            let dx = sx.saturating_sub(dl.width() / 2).min(w.saturating_sub(dl.width()));
            if dx >= date_end {
                put(&mut when, dx, &dl, dim());
                date_end = dx + dl.width() + 1;
            }
        }
    }
    // Today, on the track.
    if now_x < w {
        put(&mut marks, now_x, "▲", st("peach"));
        let tx = now_x.saturating_sub(1).min(w.saturating_sub(5));
        if tx >= date_end {
            put(&mut when, tx, "today", st("peach"));
        }
    }
    // Every event on its day, two cells apart (an icon draws wider than one cell, so packed tight
    // they ran into each other); events on the same day stack to the right. The selected one is
    // drawn bright, and a pointer under it names it (round 5a: "the icons do not highlight very
    // clearly and look slightly offset wrongly when they are").
    let mut used = vec![false; w + 2];
    let mut sel_x = None;
    for (i, e) in a.events.iter().enumerate() {
        let (g, gc) = event_glyph(e.kind);
        let mut ex = x(e.at.date_naive());
        while ex < w && (used[ex] || used[ex + 1]) {
            ex += 1;
        }
        if ex >= w {
            continue;
        }
        used[ex] = true;
        used[ex + 1] = true;
        let style = if sel == Some(i) {
            sel_x = Some(ex);
            bold(st("blue"))
        } else {
            st(gc)
        };
        if let Some(ch) = g.chars().next() {
            marks[ex] = (ch, style);
        }
    }
    let mut cursor = blank();
    if let (Some(sx), Some(i)) = (sel_x, sel) {
        let e = &a.events[i];
        let kind = match e.kind {
            AppEventKind::Mail => "mail",
            AppEventKind::Interview => "interview",
            AppEventKind::Assessment => "assessment",
            AppEventKind::Submission => "sent",
            AppEventKind::Call => "call",
            AppEventKind::Outcome => "outcome",
        };
        // The arrow sits under the icon; the label runs right of it, or, near the right edge,
        // ends at it.
        let what = format!("{} · {kind}", date(e.at.date_naive()));
        if sx + 2 + what.width() <= w {
            put(&mut cursor, sx, &format!("▲ {what}"), bold(st("blue")));
        } else {
            let lx = (sx + 1).saturating_sub(what.width() + 2);
            put(&mut cursor, lx, &format!("{what} ▲"), bold(st("blue")));
        }
    }
    let to_line = |row: Vec<(char, Style)>| {
        Line::from(
            row.into_iter()
                .map(|(ch, s)| Span::styled(ch.to_string(), s))
                .collect::<Vec<_>>(),
        )
    };
    out.push(to_line(names[1].clone()));
    out.push(to_line(names[0].clone()));
    out.push(to_line(track));
    out.push(to_line(marks));
    // The pointer row sits between the icons and the stage dates, blank until you walk the
    // timeline, so the dates stay put.
    out.push(to_line(cursor));
    out.push(to_line(when));
    out.push(Line::from(""));
    let _ = strip_h;
    // The events, newest first, the selected one lit; its detail on the same line.
    let mut ix: Vec<usize> = (0..a.events.len()).collect();
    ix.reverse();
    let room = usize::from(area.height).saturating_sub(out.len());
    let pos = pos.unwrap_or(0);
    let first = pos.saturating_sub(room.saturating_sub(1));
    for (k, &i) in ix.iter().enumerate().skip(first).take(room) {
        let e = &a.events[i];
        let (g, gc) = event_glyph(e.kind);
        let mut left_ = vec![
            pointer(k, sel.map(|_| pos)),
            Span::styled(format!(" {:<8}", date(e.at.date_naive())), dim()),
            Span::styled(format!("{g} "), st(gc)),
            Span::styled(e.text.clone(), text()),
        ];
        if let Some(d) = e.detail.first() {
            left_.push(Span::styled(format!("{SEP}{d}"), dim()));
        }
        out.push(row(left_, vec![], area.width));
    }
    lines(f, area, out);
}

/// The prep overview: the plan for the round ahead, how much of its time is used, the tracks it
/// draws on with your confidence in each, and the next session; `p` opens the full plan (round 5:
/// "move the centered prep headline to be in that section instead").
fn prep(app: &App, v: &ProjectView, w: u16) -> Vec<Line<'static>> {
    let focus = app.focus == Some('p');
    let plan = app.fixtures.prep.get(&v.project);
    let extra = match plan {
        Some(p) => format!("{} · {}{SEP}full plan [p]", p.round, date(p.date)),
        None => "no plan yet".into(),
    };
    let mut out = vec![rule("prep", &extra, None, focus, w)];
    let Some(p) = plan else {
        out.push(Line::from(Span::styled(
            "   no plan for this one · give an interview's details and your time, and one is drawn up",
            dim(),
        )));
        return out;
    };
    let frac = if p.budget_hours > 0.0 {
        p.used_hours / p.budget_hours
    } else {
        0.0
    };
    let mut spans = vec![Span::styled(format!("   {:<14}", "time"), dim())];
    spans.extend(bar(frac.clamp(0.0, 1.0), 20, None, c("blue")));
    spans.push(Span::styled(
        format!("  {:.1} of {:.0} hours", p.used_hours, p.budget_hours),
        text(),
    ));
    spans.push(Span::styled(format!("{SEP}{} sessions logged", p.log.len()), dim()));
    out.push(Line::from(spans));
    // What it covered last: the most recent sessions, each with its score (round 5b: "last
    // topics covered").
    let recent: Vec<String> = p
        .log
        .iter()
        .take(3)
        .map(|x| format!("{} {} {}/{}", date(x.date), x.track, x.right, x.problems))
        .collect();
    if !recent.is_empty() {
        out.push(Line::from(vec![
            Span::styled(format!("   {:<14}", "last covered"), dim()),
            Span::styled(recent.join(SEP), text()),
        ]));
    }
    // The tracks it drew on, weakest first, with your confidence in each now.
    let mut tracks: Vec<(String, f32)> = p
        .topics
        .iter()
        .filter_map(|t| {
            app.fixtures
                .learning
                .tracks
                .iter()
                .find(|x| x.id == t.track)
                .map(|x| (x.name.clone(), x.confidence))
        })
        .collect();
    tracks.sort_by(|a, b| a.1.total_cmp(&b.1));
    let cells: Vec<String> = tracks
        .iter()
        .take(4)
        .map(|(n, cf)| format!("{n} {}%", (cf * 100.0).round()))
        .collect();
    if !cells.is_empty() {
        out.push(Line::from(vec![
            Span::styled(format!("   {:<14}", "weakest"), dim()),
            Span::styled(cells.join(SEP), text()),
        ]));
    }
    let last = match (&p.outcome, &p.next) {
        (Some(o), _) => (format!("   {:<14}", "outcome"), o.clone()),
        (None, Some(n)) => (
            format!("   {:<14}", "next session"),
            format!("{} · {} min", n.track, n.minutes),
        ),
        (None, None) => (format!("   {:<14}", "next session"), "none planned".into()),
    };
    out.push(Line::from(vec![
        Span::styled(last.0, dim()),
        Span::styled(last.1, text()),
    ]));
    out
}

/// The questions you were asked, by round: a draft until you have pinned it down, with `r` to
/// recall one in a chat (round 5: "perhaps with a chat to help refine my memory").
fn asked(app: &App, a: Option<&Application>, w: u16) -> Vec<Line<'static>> {
    let focus = app.focus == Some('q');
    let qs = a.map(|a| a.questions.as_slice()).unwrap_or(&[]);
    let drafts = qs.iter().filter(|q| q.state == QuestionState::Draft).count();
    let mut extra = format!("{}", qs.len());
    if drafts > 0 {
        extra += &format!(" · {drafts} to pin down");
    }
    extra += " · recall one [r]";
    let mut out = vec![rule("questions you were asked", &extra, Some('q'), focus, w)];
    if qs.is_empty() {
        out.push(Line::from(Span::styled(
            "   none yet · after an interview, r starts a short chat to pin down what you were asked",
            dim(),
        )));
        return out;
    }
    let sel = focus.then(|| app.sel('q'));
    let mut last_round = "";
    for (i, q) in qs.iter().enumerate() {
        let round = if q.round == last_round {
            String::new()
        } else {
            q.round.clone()
        };
        last_round = &q.round;
        let mut right = vec![];
        if q.state == QuestionState::Draft {
            right.push(Span::styled("draft ", st("yellow")));
        }
        if let Some(t) = &q.track {
            right.push(Span::styled(format!("{t} "), dim()));
        }
        out.push(row(
            vec![
                pointer(i, sel),
                Span::styled(format!(" {round:<18}"), dim()),
                Span::styled(q.text.clone(), text()),
            ],
            right,
            w,
        ));
    }
    out
}

pub fn key(app: &mut App, k: KeyEvent) -> bool {
    let Some(v) = view(app).cloned() else { return false };
    let n = v.application.as_ref().map_or(0, |a| a.events.len());
    match (app.focus, k.code) {
        // ← and → walk the timeline from anywhere on the page, as they walk the branches.
        (_, KeyCode::Left | KeyCode::Right) if n > 0 => {
            app.focus = Some('t');
            // The list is newest first: ← is older, further down it; → is newer.
            let d = if k.code == KeyCode::Left { 1 } else { -1 };
            app.move_sel('t', n, d);
        }
        (_, KeyCode::Char('p')) => {
            if app.fixtures.prep.contains_key(&v.project) {
                app.open(ScreenId::Prep);
            } else {
                app.note = Some("no prep plan yet: when an interview is offered, one is drawn up".into());
            }
        }
        (_, KeyCode::Char('r')) => {
            app.note = Some(
                "would start a short chat here to pin down a question you were asked: the round, the wording, what you answered"
                    .into(),
            )
        }
        (_, KeyCode::Char(' ' | 'i')) => app.start_ask(),
        (Some('a'), KeyCode::Enter) => app.open(ScreenId::Answered),
        (Some('y'), KeyCode::Enter) => app.note = Some("would open it where it lives".into()),
        _ => return false,
    }
    let _ = Mode::Normal;
    true
}
