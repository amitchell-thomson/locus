//! Drawing the Claude runs view and a run's detail.

use crate::app::{App, Mode};
use crate::theme::{c, fit, pad, panel, selected_style, wrap};
use keryx_core::display::date_full;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

fn status_colour(s: &str) -> ratatui::style::Color {
    match s {
        "done" | "applied" | "sent" | "accepted" => c("green"),
        "failed" | "rejected" => c("red"),
        "running" | "pending" | "holding" | "open" => c("peach"),
        _ => c("overlay1"),
    }
}

pub fn runs(f: &mut Frame, app: &App, area: Rect) {
    let Mode::Runs { sel } = app.mode else { return };
    let block = panel("Claude runs", true).title_bottom(
        Line::from(Span::styled(
            " Enter details · Esc back ",
            Style::default().fg(c("overlay1")),
        ))
        .right_aligned(),
    );
    let inner = block.inner(area);
    f.render_widget(block, area);
    let list = app.runs();
    if list.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                " No runs yet. They start at the [runs] times once scripts/install-runs.sh is installed.",
                Style::default().fg(c("overlay1")),
            ))),
            inner,
        );
        return;
    }
    let w = inner.width as usize;
    let tz = app.cfg.tz();
    let lines: Vec<Line> = list
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let when = date_full(v.run.started_at, tz);
            let what = match &v.run.error {
                Some(e) => e.clone(),
                None => v.counts(),
            };
            let head = format!(" {when:<24} {:<8} ", v.run.kind.as_str());
            let st = format!("{:<8}", v.run.status);
            let rest = fit(&what, w.saturating_sub(head.len() + st.len() + 1));
            if i == sel {
                Line::from(Span::styled(
                    pad(&format!("{head}{st}{rest}"), w),
                    selected_style(c("blue")),
                ))
            } else {
                Line::from(vec![
                    Span::styled(head, Style::default().fg(c("subtext1"))),
                    Span::styled(st, Style::default().fg(status_colour(&v.run.status))),
                    Span::styled(rest, Style::default().fg(c("text"))),
                ])
            }
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

pub fn detail(f: &mut Frame, app: &App, area: Rect) {
    let Mode::RunDetail { id, sel } = &app.mode else { return };
    let Some(v) = app.db.run_view(id).ok().flatten() else {
        return;
    };
    let title = format!(
        "{} run · {}",
        v.run.kind.as_str(),
        date_full(v.run.started_at, app.cfg.tz())
    );
    let block = panel(&title, true).title_bottom(
        Line::from(Span::styled(
            " Enter open · u undo a cleared thread · A drafts & reviews · Esc back ",
            Style::default().fg(c("overlay1")),
        ))
        .right_aligned(),
    );
    let inner = block.inner(area);
    f.render_widget(block, area);
    let w = inner.width as usize;
    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!(" {} ", v.run.status),
            Style::default()
                .fg(status_colour(&v.run.status))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(v.counts(), Style::default().fg(c("text"))),
    ])];
    if let Some(e) = &v.run.error {
        lines.push(Line::from(Span::styled(format!(" {e}"), Style::default().fg(c("red")))));
    }
    for l in wrap(&v.run.summary, w.saturating_sub(2)) {
        lines.push(Line::from(Span::styled(
            format!(" {l}"),
            Style::default().fg(c("subtext1")),
        )));
    }
    lines.push(Line::raw(""));
    for (i, it) in app.run_lines(&v).iter().enumerate() {
        let head = format!(" {:<12}", it.kind);
        let st = format!(" {}", it.status);
        let text = fit(&it.text, w.saturating_sub(head.len() + st.len() + 1));
        let gap = w.saturating_sub(head.len() + unicode_width::UnicodeWidthStr::width(text.as_str()) + st.len());
        if i == *sel {
            lines.push(Line::from(Span::styled(
                pad(&format!("{head}{text}{}{st}", " ".repeat(gap)), w),
                selected_style(c("blue")),
            )));
        } else {
            lines.push(Line::from(vec![
                Span::styled(head, Style::default().fg(c("mauve"))),
                Span::styled(text, Style::default().fg(c("text"))),
                Span::raw(" ".repeat(gap)),
                Span::styled(st, Style::default().fg(status_colour(&it.status))),
            ]));
        }
    }
    f.render_widget(Paragraph::new(lines), inner);
}
