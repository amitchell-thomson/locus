//! Tasks (spec E7): every task kairos holds, grouped by what it is for (ADR-009), each with when
//! it is due, what is left, when it is next planned and how often it was put off; what was done
//! this week below. The overview the calendar is not: kairos acts on blocks and flags, this page
//! on the tasks themselves. Home shows it as one line (`d`), and kairos's `T` opens it.

use crate::app::App;
use crate::ui::home::area_colour;
use crate::ui::widgets::*;
use crate::ui::{frame, lines, title};
use locus_contract::{TaskItem, Time};
use locus_theme::SEP;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

pub const KEYS: &[(&str, &str)] = &[
    ("↑ ↓", "walk the open tasks"),
    ("⏎", "open its next block (or its due day) in kairos"),
    ("d", "done (through kairos)"),
    ("e", "edit it in kairos's form: title, what it is for, time left, due"),
    ("n", "start it now: a block from now, the rest planned around it"),
    ("x", "drop it (asks first: x again)"),
    (
        "+",
        "add a task in kairos's form, for the same thing as the selected one",
    ),
];

/// Ask kairos's window for one of its forms (`add-task [for]`, `edit-task <id>`) and bring it
/// forward; `doing` is what the footer says when it worked.
fn ask_kairos(app: &mut App, req: &str, doing: String) {
    if app.in_session {
        let sent = locus_contract::link::OpenFile::kairos().write(req).is_ok();
        let shown = sent && locus_contract::link::switch_to("kairos");
        app.note = Some(if shown {
            doing
        } else {
            "asked kairos (the calendar window isn't open)".into()
        });
    } else {
        app.note = Some(format!("would ask kairos: {req}"));
    }
}

/// Add a task in kairos's form (round 7a: one add form, kairos's), For filled when known: the
/// calendar window comes forward with the form open.
pub fn add_in_kairos(app: &mut App, for_id: Option<String>) {
    let req = match &for_id {
        Some(id) => format!("add-task {id}"),
        None => "add-task".to_string(),
    };
    let what = for_id
        .as_deref()
        .and_then(|id| app.fixtures.registry.get(id).map(|p| p.name.clone()))
        .map_or(String::new(), |n| format!(" for {n}"));
    ask_kairos(app, &req, format!("adding a task{what} in kairos"));
}

/// The open tasks in the order drawn: grouped by what they are for, the group due soonest first,
/// within it by due date.
pub fn open(app: &App) -> Vec<&TaskItem> {
    let mut v: Vec<&TaskItem> = app.fixtures.tasks.tasks.iter().filter(|t| t.done.is_none()).collect();
    let soonest = |label: &str| {
        app.fixtures
            .tasks
            .tasks
            .iter()
            .filter(|t| t.done.is_none() && t.for_label == label)
            .filter_map(|t| t.due)
            .min()
    };
    v.sort_by(|a, b| {
        let (sa, sb) = (soonest(&a.for_label), soonest(&b.for_label));
        // Groups with a due date first, soonest first; then by name.
        (sa.is_none(), sa, &a.for_label, a.due.is_none(), a.due).cmp(&(
            sb.is_none(),
            sb,
            &b.for_label,
            b.due.is_none(),
            b.due,
        ))
    });
    v
}

pub fn section_len(app: &App) -> usize {
    open(app).len()
}

/// "due fri 17:00", "due today", "due 31 oct", red when within two days.
fn due_text(now: Time, t: &TaskItem) -> Option<(String, bool)> {
    let d = t.due?;
    let days = (d.date_naive() - now.date_naive()).num_days();
    let when = match days {
        i64::MIN..=-1 => format!("overdue since {}", day(d.date_naive())),
        0 => "today".into(),
        1 => "tomorrow".into(),
        2..=6 => d.format("%a").to_string().to_lowercase(),
        _ => day(d.date_naive()),
    };
    let time = if t.due_is_date || days < 0 {
        String::new()
    } else {
        format!(" {}", d.format("%H:%M"))
    };
    let text = match &t.before {
        Some(e) => format!("before {e}, {when}{time}"),
        None if days < 0 => when,
        None => format!("due {when}{time}"),
    };
    Some((text, days <= 2))
}

fn mins(m: u32) -> String {
    match (m / 60, m % 60) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h{m:02}"),
    }
}

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let now = app.fixtures.now();
    let tasks = open(app);
    let soon = tasks.iter().filter(|t| due_text(now, t).is_some_and(|d| d.1)).count();
    let extra = if soon > 0 {
        format!("{} open{SEP}{soon} due within two days", tasks.len())
    } else {
        format!("{} open", tasks.len())
    };
    // The add key, said where you look for it (round 7b), beside esc.
    let right = Line::from(vec![
        Span::styled(" +", crate::ui::widgets::keyst()),
        Span::styled(" add a task  ", dim()),
        Span::styled("esc", crate::ui::widgets::keyst()),
        Span::styled(" back ", dim()),
    ]);
    let inner = frame(f, area, title("tasks", &extra), Some(right));
    let w = inner.width;
    let sel = app.sel(' ').min(tasks.len().saturating_sub(1));
    let mut out: Vec<Line<'static>> = vec![Line::from("")];
    if tasks.is_empty() {
        out.push(Line::from(Span::styled(
            "   nothing to do: + adds a task (in kairos's form), saying what it is for",
            dim(),
        )));
    }
    let mut last: Option<&str> = None;
    for (k, t) in tasks.iter().enumerate() {
        if last != Some(t.for_label.as_str()) {
            if last.is_some() {
                out.push(Line::from(""));
            }
            last = Some(t.for_label.as_str());
            out.push(Line::from(vec![
                Span::raw("  "),
                Span::styled("● ", st(area_colour(app, t.stream.as_deref().unwrap_or(&t.area)))),
                Span::styled(
                    t.for_label.clone(),
                    bold(st(area_colour(app, t.stream.as_deref().unwrap_or(&t.area)))),
                ),
            ]));
        }
        let mut right = Vec::new();
        if let Some((d, urgent)) = due_text(now, t) {
            right.push(Span::styled(d, if urgent { st("peach") } else { dim() }));
        }
        right.push(Span::styled(
            format!("{}{} left", if right.is_empty() { "" } else { SEP }, mins(t.left_min)),
            dim(),
        ));
        match t.next_block {
            Some(b) => right.push(Span::styled(
                format!("{SEP}next {}", b.format("%a %H:%M").to_string().to_lowercase()),
                dim(),
            )),
            None if t.left_min > 0 => right.push(Span::styled(format!("{SEP}not planned yet"), st("peach"))),
            None => {}
        }
        if t.put_off > 0 {
            right.push(Span::styled(format!("{SEP}put off {}×", t.put_off), st("peach")));
        }
        right.push(Span::raw(" "));
        out.push(row(
            vec![
                pointer(k, Some(sel)),
                Span::styled("   ☐ ", dim()),
                Span::styled(t.title.clone(), text()),
            ],
            right,
            w,
        ));
    }
    // Done this week, quieter, newest first.
    let mut done: Vec<&TaskItem> = app.fixtures.tasks.tasks.iter().filter(|t| t.done.is_some()).collect();
    done.sort_by_key(|t| std::cmp::Reverse(t.done));
    if !done.is_empty() {
        out.push(Line::from(""));
        out.push(rule("done this week", &format!("{}", done.len()), None, false, w));
        for t in done {
            let d = t.done.map(|d| day(d.date_naive())).unwrap_or_default();
            out.push(row(
                vec![
                    Span::styled(format!("     ✓ {:<8}", d), dim()),
                    Span::styled(t.title.clone(), dim()),
                ],
                vec![Span::styled(format!("{} ", t.for_label), dim())],
                w,
            ));
        }
    }
    lines(f, inner, out);
}

pub fn key(app: &mut App, k: KeyEvent) -> bool {
    let armed = app.armed.take();
    let tasks: Vec<TaskItem> = open(app).into_iter().cloned().collect();
    if let Some(d) = crate::app::nav(&k) {
        app.move_sel(' ', tasks.len(), d);
        return true;
    }
    let Some(t) = tasks.get(app.sel(' ').min(tasks.len().saturating_sub(1))).cloned() else {
        if k.code == KeyCode::Char('+') {
            add_in_kairos(app, None);
            return true;
        }
        return matches!(k.code, KeyCode::Enter | KeyCode::Char('d' | 'e' | 'n' | 'x'));
    };
    match k.code {
        // Its next block, else its due day, in kairos (spec K8's link).
        KeyCode::Enter => {
            let at = t.next_block.or(t.due).unwrap_or_else(|| app.fixtures.now());
            let req = locus_contract::link::kairos_request(at.date_naive(), None);
            if app.in_session {
                let sent = locus_contract::link::OpenFile::kairos().write(&req).is_ok();
                let shown = sent && locus_contract::link::switch_to("kairos");
                app.note = Some(if shown {
                    format!("opened {} in kairos", at.format("%a %-d %b"))
                } else {
                    "asked kairos to open it (the calendar window isn't open)".into()
                });
            } else {
                app.note = Some(format!("would open kairos at {}", at.format("%a %-d %b")));
            }
        }
        KeyCode::Char('+') => add_in_kairos(app, t.for_id.clone()),
        KeyCode::Char('e') => ask_kairos(
            app,
            &format!("edit-task {}", t.id),
            format!("editing “{}” in kairos", t.title),
        ),
        KeyCode::Char('n') => {
            app.note = Some(match app.calendar.clone() {
                Some(live) => match live.start(&t.id) {
                    Ok(()) => format!("started “{}”: a block from now", t.title),
                    Err(e) => format!("kairos said no: {e}"),
                },
                None => format!("would start “{}” now in kairos", t.title),
            });
        }
        KeyCode::Char('x') if armed.as_deref() != Some(t.id.as_str()) => {
            app.armed = Some(t.id.clone());
            app.note = Some(format!("drop “{}” and its planned time? x again drops it", t.title));
        }
        KeyCode::Char('x') => {
            app.note = Some(match app.calendar.clone() {
                Some(live) => match live.drop_task(&t.id) {
                    Ok(()) => {
                        app.fixtures.tasks.tasks.retain(|x| x.id != t.id);
                        format!("dropped “{}”", t.title)
                    }
                    Err(e) => format!("kairos said no: {e}"),
                },
                None => {
                    app.fixtures.tasks.tasks.retain(|x| x.id != t.id);
                    format!("would drop “{}” in kairos", t.title)
                }
            });
        }
        KeyCode::Char('d') => {
            app.note = Some(match app.calendar.clone() {
                Some(live) => match live.complete(&t.id) {
                    Ok(()) => {
                        let now = app.fixtures.now();
                        if let Some(x) = app.fixtures.tasks.tasks.iter_mut().find(|x| x.id == t.id) {
                            x.done = Some(now);
                        }
                        format!("✓ {}", t.title)
                    }
                    Err(e) => format!("kairos said no: {e}"),
                },
                None => format!("would mark “{}” done in kairos", t.title),
            });
        }
        _ => return false,
    }
    true
}
