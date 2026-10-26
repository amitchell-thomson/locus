//! Drawing for Phase 4: pickers, the `A` pane and the cleanup review.

use crate::app::App;
use crate::theme::{c, centred, pad, popup_block};
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

/// `d` / `m`: what it is for, the guess first with its reason, the folder beside each.
pub fn file(f: &mut Frame, app: &App, area: Rect, p: &crate::file_for::ForPicker) {
    let w: u16 = 64;
    let inner_w = usize::from(w - 2);
    let mut lines: Vec<Line<'static>> = vec![if p.query.is_empty() {
        Line::from(Span::styled(
            if p.guess.is_some() {
                " ⏎ takes the guess · type to choose another"
            } else {
                " type what it is for · ↓ to choose"
            },
            locus_theme::dim(),
        ))
    } else {
        Line::from(Span::styled(format!(" {}█", p.query), Style::default().fg(c("text"))))
    }];
    lines.push(Line::raw(""));
    let choices = app.for_choices(p);
    let rows = usize::from(area.height.saturating_sub(8)).clamp(3, 14);
    let first = p.sel.unwrap_or(0).saturating_sub(rows.saturating_sub(1));
    for (i, ch) in choices.iter().enumerate().skip(first).take(rows) {
        let chosen = p.sel == Some(i);
        let style = if chosen {
            locus_theme::bold(locus_theme::st("blue"))
        } else {
            Style::default().fg(c("text"))
        };
        let (right, right_style) = match ch {
            crate::file_for::Choice::For(o) => (o.folder.clone(), locus_theme::dim()),
            crate::file_for::Choice::Nothing => (String::new(), locus_theme::dim()),
        };
        let is_guess = p.query.is_empty()
            && i == 0
            && matches!((ch, &p.guess), (crate::file_for::Choice::For(o), Some(g)) if o.id == g.id);
        let mut spans = vec![Span::styled(if chosen { " ▸ " } else { "   " }, style)];
        let label = ch.label();
        let why = if is_guess {
            p.guess.as_ref().map(|g| format!("  {}", g.why)).unwrap_or_default()
        } else {
            String::new()
        };
        let room = inner_w.saturating_sub(4 + right.len());
        let label = crate::theme::fit(&label, room);
        let why = crate::theme::fit(
            &why,
            room.saturating_sub(unicode_width::UnicodeWidthStr::width(label.as_str())),
        );
        let used = 3
            + unicode_width::UnicodeWidthStr::width(label.as_str())
            + unicode_width::UnicodeWidthStr::width(why.as_str());
        spans.push(Span::styled(label, style));
        spans.push(Span::styled(why, locus_theme::st("peach")));
        spans.push(Span::raw(" ".repeat(inner_w.saturating_sub(used + right.len() + 1))));
        spans.push(Span::styled(right, right_style));
        lines.push(Line::from(spans));
    }
    let n = app.targets().len();
    let title = match (p.mark, n) {
        (true, 1) => "what is it for · stays in your inbox".to_string(),
        (true, _) => format!("{n} threads: what are they for · they stay in your inbox"),
        (false, 1) => "done: what is it for".to_string(),
        (false, _) => format!("done, {n} threads: what are they for"),
    };
    popup(f, area, &title, w, lines);
}
