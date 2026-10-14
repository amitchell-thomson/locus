//! Drawing for Phase 4: pickers, the `A` pane and the cleanup review.

use crate::app::{App, Mode};
use crate::theme::{c, centred, fit, pad, panel, popup_block, rgb, selected_style};
use crate::write::{ReviewLine, parse_pick, review_lines, snooze_presets, when_label};
use keryx_core::display::date_label;
use keryx_store::state::{Action, ProposalStatus};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

fn key_span(k: &str) -> Span<'static> {
    Span::styled(
        format!(" {k}  "),
        Style::default().fg(c("blue")).add_modifier(Modifier::BOLD),
    )
}

fn popup(f: &mut Frame, area: Rect, title: &str, w: u16, lines: Vec<Line<'static>>) {
    let r = centred(area, w.min(area.width), (lines.len() as u16 + 2).min(area.height));
    f.render_widget(Clear, r);
    let block = popup_block(title);
    let inner = block.inner(r);
    f.render_widget(block, r);
    f.render_widget(Paragraph::new(lines), inner);
}

pub fn snooze(f: &mut Frame, app: &App, area: Rect, input: Option<&str>) {
    let mut lines: Vec<Line<'static>> = snooze_presets(app)
        .into_iter()
        .enumerate()
        .map(|(i, (label, t))| {
            Line::from(vec![
                key_span(&(i + 1).to_string()),
                Span::styled(pad(&label, 22), Style::default().fg(c("text"))),
                Span::styled(when_label(app, t), Style::default().fg(c("overlay1"))),
            ])
        })
        .collect();
    match input {
        None => lines.push(Line::from(vec![
            key_span("4"),
            Span::styled("Pick…", Style::default().fg(c("text"))),
        ])),
        Some(s) => {
            lines.push(Line::from(vec![
                Span::styled(" when: ", Style::default().fg(c("blue")).add_modifier(Modifier::BOLD)),
                Span::styled(format!("{s}█"), Style::default().fg(c("text"))),
            ]));
            let preview = if s.trim().is_empty() {
                Span::styled(
                    "  e.g. fri 17:00 · 23 oct · tomorrow 9",
                    Style::default().fg(c("overlay1")),
                )
            } else {
                match parse_pick(app, s) {
                    Ok(t) => Span::styled(format!("  → {}", when_label(app, t)), Style::default().fg(c("green"))),
                    Err(e) => Span::styled(format!("  {e}"), Style::default().fg(c("peach"))),
                }
            };
            lines.push(Line::from(preview));
        }
    }
    let n = app.targets().len();
    popup(
        f,
        area,
        &format!("snooze {n} thread{} until", if n == 1 { "" } else { "s" }),
        50,
        lines,
    );
}

pub fn file(f: &mut Frame, app: &App, area: Rect) {
    let lines: Vec<Line<'static>> = app
        .file_keys()
        .into_iter()
        .map(|(n, name)| {
            let col = app.cfg.key(&name).map(|k| rgb(k.colour)).unwrap_or(c("text"));
            Line::from(vec![
                key_span(&n.to_string()),
                Span::styled(name, Style::default().fg(col)),
            ])
        })
        .collect();
    popup(f, area, "file into", 34, lines);
}

pub fn change(f: &mut Frame, app: &App, area: Rect) {
    let mut lines = vec![
        Line::from(vec![key_span("a"), Span::raw("archive")]),
        Line::from(vec![key_span("s"), Span::raw("snooze until tomorrow morning")]),
        Line::from(vec![key_span("k"), Span::raw("keep in the inbox")]),
    ];
    for (i, k) in app.cfg.keys.iter().enumerate().take(9) {
        lines.push(Line::from(vec![
            key_span(&(i + 1).to_string()),
            Span::raw("file into "),
            Span::styled(k.name.clone(), Style::default().fg(rgb(k.colour))),
        ]));
    }
    popup(f, area, "instead", 40, lines);
}

pub fn confirm_group(f: &mut Frame, area: Rect, group: &str, count: usize) {
    let lines = vec![
        Line::from(Span::styled(
            format!(" {group}: {count} thread{}?", if count == 1 { "" } else { "s" }),
            Style::default().fg(c("text")).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            " Changed-since-proposed ones are skipped.",
            Style::default().fg(c("overlay1")),
        )),
        Line::from(vec![
            key_span("y"),
            Span::raw("yes, all of them    "),
            key_span("Esc"),
            Span::raw("no"),
        ]),
    ];
    popup(f, area, "accept the group", 50, lines);
}

pub fn approvals(f: &mut Frame, app: &App, area: Rect) {
    let Mode::Approvals { sel } = app.mode else { return };
    let block = panel("Waiting for you", true).title_bottom(
        Line::from(Span::styled(
            " Enter review · Esc back ",
            Style::default().fg(c("overlay1")),
        ))
        .right_aligned(),
    );
    let inner = block.inner(area);
    f.render_widget(block, area);
    let sends = app.pending_sends();
    let reviews = app.open_reviews();
    if reviews.is_empty() && sends.is_empty() {
        let msg = "Nothing waiting.";
        f.buffer_mut().set_string(
            inner.x + inner.width.saturating_sub(msg.width() as u16) / 2,
            inner.y + inner.height / 2,
            msg,
            Style::default().fg(c("overlay1")),
        );
        return;
    }
    let row = |i: usize, text: String| {
        let st = if i == sel {
            selected_style(c("blue"))
        } else {
            Style::default().fg(c("text"))
        };
        Line::from(Span::styled(pad(&text, inner.width as usize), st))
    };
    let mut lines = Vec::new();
    if !sends.is_empty() {
        lines.push(Line::from(Span::styled(
            " Drafts to approve",
            Style::default().fg(c("peach")).add_modifier(Modifier::BOLD),
        )));
        for (i, cs) in sends.iter().enumerate() {
            let warn = if cs.warnings.is_empty() {
                String::new()
            } else {
                " · ⚠".into()
            };
            lines.push(row(i, format!(" {} · {}{warn}", cs.author.as_str(), cs.summary)));
        }
        lines.push(Line::raw(""));
    }
    if !reviews.is_empty() {
        lines.push(Line::from(Span::styled(
            " Cleanups",
            Style::default().fg(c("mauve")).add_modifier(Modifier::BOLD),
        )));
    }
    for (j, (_, title, open, total)) in reviews.iter().enumerate() {
        let text = format!(
            " {} · {} to review · {} of {total} decided",
            fit(title, inner.width.saturating_sub(40) as usize),
            open,
            total - open
        );
        lines.push(row(sends.len() + j, text));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn action_label(app: &App, a: &Action) -> String {
    match a {
        Action::Archive => "archive".into(),
        Action::File { key } => format!("file → {key}"),
        Action::Snooze { until } => format!("snooze → {}", when_label(app, *until)),
        Action::Keep { needs_reply: true, .. } => "keep · needs reply".into(),
        Action::Keep { .. } => "keep".into(),
    }
}

pub fn review(f: &mut Frame, app: &App, area: Rect) {
    let sel = match &app.mode {
        Mode::Review { sel, .. } | Mode::ReviewChange { sel, .. } | Mode::ConfirmGroup { sel, .. } => *sel,
        _ => 0,
    };
    let Some(v) = &app.review else { return };
    let decided = v
        .items
        .iter()
        .filter(|i| i.proposal.status != ProposalStatus::Open)
        .count();
    let title = format!("Cleanup: {} · {decided} of {} decided", v.review.title, v.items.len());
    let mut block = panel(&title, true);
    let hints = app.key_hints();
    if (area.width as usize) > hints.width() + 10 {
        block = block.title_bottom(
            Line::from(vec![
                Span::raw(" "),
                Span::styled(hints, Style::default().fg(c("overlay1"))),
                Span::raw(" "),
            ])
            .right_aligned(),
        );
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    let w = inner.width as usize;
    let items = app.review_items();
    let selected_item = items.get(sel).copied();
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut sel_line = 0;
    for l in review_lines(v) {
        match l {
            ReviewLine::Header { group, open, total } => {
                if !lines.is_empty() {
                    lines.push(Line::raw(""));
                }
                lines.push(Line::from(vec![
                    Span::styled(group, Style::default().fg(c("mauve")).add_modifier(Modifier::BOLD)),
                    Span::styled(format!(" · {open} of {total} open"), Style::default().fg(c("overlay1"))),
                ]));
            }
            ReviewLine::Item(i) => {
                let it = &v.items[i];
                let p = &it.proposal;
                let marked = app.review_marked.contains(&p.id);
                let (mark, mark_col) = match p.status {
                    ProposalStatus::Accepted => ("✓", c("green")),
                    ProposalStatus::Declined => ("✗", c("overlay1")),
                    ProposalStatus::Changed => ("↻", c("peach")),
                    ProposalStatus::Open if marked => ("•", c("blue")),
                    ProposalStatus::Open => (" ", c("text")),
                };
                let date = date_label(it.date, app.now);
                let who = pad(&it.who, 16);
                let reason_w = w.saturating_sub(2 + 17 + 7);
                let subject_w = (reason_w * 2 / 5).max(10);
                let subject = pad(&it.subject, subject_w);
                let tail = if it.stale {
                    "changed since proposed".to_string()
                } else if p.status == ProposalStatus::Changed {
                    format!("instead: {}", action_label(app, &p.action))
                } else if matches!(p.action, Action::Snooze { .. }) {
                    format!("{} · {}", action_label(app, &p.action), p.reason)
                } else {
                    p.reason.clone()
                };
                let tail = fit(&tail, reason_w.saturating_sub(subject_w + 1));
                let dim = p.status != ProposalStatus::Open;
                let text_col = if dim { c("overlay1") } else { c("text") };
                let mut spans = vec![
                    Span::styled(
                        format!("{mark} "),
                        Style::default().fg(mark_col).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("{who} "), Style::default().fg(text_col)),
                    Span::styled(format!("{subject} "), Style::default().fg(text_col)),
                    Span::styled(
                        tail,
                        Style::default().fg(if it.stale { c("peach") } else { c("overlay1") }),
                    ),
                ];
                let used: usize = spans.iter().map(|s| s.content.width()).sum();
                spans.push(Span::raw(" ".repeat(w.saturating_sub(used + 7))));
                spans.push(Span::styled(format!("{date:>6} "), Style::default().fg(c("overlay1"))));
                if Some(i) == selected_item {
                    sel_line = lines.len();
                    let st = selected_style(c("blue"));
                    for s in &mut spans {
                        s.style = st;
                    }
                }
                lines.push(Line::from(spans));
            }
        }
    }
    let h = inner.height as usize;
    let first = sel_line.saturating_sub(h / 2).min(lines.len().saturating_sub(h));
    f.render_widget(Paragraph::new(lines).scroll((first as u16, 0)), inner);
}
