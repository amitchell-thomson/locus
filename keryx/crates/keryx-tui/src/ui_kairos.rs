//! Drawing for the kairos link: the form popup (as kairos-tui's
//! `form_popup`) and the thread view's task / suggestion lines.

use crate::app::{App, Open};
use crate::form::{FieldKind, Form, minutes_label};
use crate::theme::{c, centred, fit, popup_block, wrap};
use crate::ui::ICON_TASK;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use serde_json::Value;
use unicode_width::UnicodeWidthStr;

pub fn form(f: &mut Frame, app: &App, area: Rect, form: &Form) {
    let w = 76.min(area.width.saturating_sub(2));
    let label_w = 9usize;
    let val_w = (w as usize).saturating_sub(label_w + 6);
    let mut lines: Vec<Line> = vec![Line::raw("")];
    if let Some(r) = &form.reason {
        for (i, l) in wrap(r, val_w).into_iter().enumerate() {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{:>label_w$}  ", if i == 0 { "Claude" } else { "" }),
                    Style::default().fg(c("mauve")),
                ),
                Span::styled(l, Style::default().fg(c("subtext1")).add_modifier(Modifier::ITALIC)),
            ]));
        }
        lines.push(Line::raw(""));
    }
    for (k, fld) in form.fields.iter().enumerate() {
        let focus = k == form.focus;
        let mut spans = vec![Span::styled(
            format!("{:>label_w$}  ", fld.label),
            if focus {
                Style::default().fg(c("mauve")).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(c("overlay1"))
            },
        )];
        match &fld.kind {
            FieldKind::Pick(_) => {
                let arrow = Style::default().fg(if focus { c("mauve") } else { c("surface2") });
                spans.push(Span::styled("◀ ", arrow));
                if fld.label == "Calendar" {
                    // The calendar's colour, as kairos and locus draw it: the taxonomy's.
                    let col = keryx_store::view::shared()
                        .and_then(|s| s.taxonomy.colour_of(&fld.value))
                        .map_or(c("text"), c);
                    spans.push(Span::styled("● ", Style::default().fg(col)));
                }
                spans.push(Span::styled(
                    fld.value.clone(),
                    Style::default()
                        .fg(c("text"))
                        .add_modifier(if focus { Modifier::BOLD } else { Modifier::empty() }),
                ));
                spans.push(Span::styled(" ▶", arrow));
            }
            _ => {
                let v = if focus {
                    // Keep the end (where typing happens) visible.
                    let chars: Vec<char> = fld.value.chars().collect();
                    let mut start = 0;
                    while start < chars.len() && chars[start..].iter().collect::<String>().width() + 1 > val_w {
                        start += 1;
                    }
                    chars[start..].iter().collect::<String>()
                } else {
                    fit(&fld.value, val_w)
                };
                spans.push(Span::styled(v, Style::default().fg(c("text"))));
                if focus {
                    spans.push(Span::styled("█", Style::default().fg(c("mauve"))));
                }
            }
        }
        lines.push(Line::from(spans));
        if focus && !fld.hint.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("{:>label_w$}  {}", "", fit(fld.hint, val_w)),
                Style::default().fg(c("overlay0")).add_modifier(Modifier::ITALIC),
            )));
        }
    }
    lines.push(Line::raw(""));
    // The preview wraps: it's the one line Alec checks before saving.
    let (mark, text, col) = match form.build(&app.cfg, app.now) {
        Ok((_, p)) => ("→", p, c("green")),
        Err(e) => ("✗", e, c("peach")),
    };
    for (i, l) in wrap(&text, (w as usize).saturating_sub(8)).into_iter().enumerate() {
        let lead = if i == 0 { format!("  {mark} ") } else { "    ".into() };
        lines.push(Line::from(Span::styled(format!("{lead}{l}"), Style::default().fg(col))));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "  Tab next · ⇧Tab back · Enter save · Esc cancel",
        Style::default().fg(c("overlay0")),
    )));
    let h = (lines.len() as u16 + 2).min(area.height);
    let r = centred(area, w, h);
    f.render_widget(Clear, r);
    f.render_widget(Paragraph::new(lines).block(popup_block(form.title())), r);
}

fn hours_left(m: u32) -> String {
    if m == 0 {
        "planned time used".into()
    } else {
        format!("{} left", minutes_label(m))
    }
}

/// `󰄱 kairos: B4 sheet · due Fri 17:00 · 2 h left`, and Claude's waiting idea.
pub fn thread_lines(app: &App, open: &Open, w: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for l in app.db.links(&open.account, &open.thread_id).unwrap_or_default() {
        let mut parts = vec![format!("{ICON_TASK} kairos: {}", l.title)];
        let mut col = c("green");
        if l.kind == "event" {
            parts[0] = format!("{ICON_TASK} kairos event: {}", l.title);
        } else if let Some(s) = app.task_status.get(&l.target) {
            if let Some(d) = &s.due {
                let label = keryx_core::when::canonical(&app.cfg, app.now, d)
                    .map(|x| x.label)
                    .unwrap_or_else(|_| d.clone());
                parts.push(format!("due {label}"));
            }
            match s.status.as_str() {
                "done" => {
                    parts.push("done".into());
                    col = c("overlay1");
                }
                "dropped" => {
                    parts.push("dropped".into());
                    col = c("overlay1");
                }
                _ => {
                    if let Some(m) = s.remaining_min {
                        parts.push(hours_left(m));
                    }
                }
            }
        }
        out.push(Line::from(Span::styled(
            fit(&parts.join(" · "), w),
            Style::default().fg(col),
        )));
    }
    if let Some(s) = app.suggestion_for(&open.account, &open.thread_id) {
        let p = &s.proposal;
        let get = |k: &str| p.get(k).and_then(Value::as_str).map(str::to_string);
        let mut parts = vec![format!(
            "{ICON_TASK}? kairos {}: {}",
            if s.kind == "event" { "event" } else { "task" },
            get("title").unwrap_or_default()
        )];
        if let Some(d) = get("due").or_else(|| get("start")) {
            let label = keryx_core::when::canonical(&app.cfg, app.now, &d)
                .map(|x| x.label)
                .unwrap_or(d);
            parts.push(if s.kind == "event" {
                label
            } else {
                format!("due {label}")
            });
        }
        if let Some(m) = p.get("estimate_min").and_then(Value::as_u64) {
            parts.push(format!("~{}", minutes_label(m as u32)));
        }
        let text = parts.join(" · ");
        let keys = " · y create · n dismiss";
        out.push(Line::from(vec![
            Span::styled(
                fit(&text, w.saturating_sub(keys.width())),
                Style::default().fg(c("mauve")),
            ),
            Span::styled(keys, Style::default().fg(c("overlay1"))),
        ]));
        out.push(Line::from(Span::styled(
            fit(&format!("  Claude: {}", s.reason), w),
            Style::default().fg(c("overlay1")).add_modifier(Modifier::ITALIC),
        )));
    }
    out
}
