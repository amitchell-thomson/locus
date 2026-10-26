//! Drawing for Phase 4: pickers, the `A` pane and the cleanup review.

use crate::app::App;
use crate::theme::{c, centred, pad, popup_block, rgb};
use crate::write::{parse_pick, snooze_presets, when_label};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

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
