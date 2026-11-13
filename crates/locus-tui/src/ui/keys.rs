//! The keys overlay (`?` anywhere): generated from the one key table (cleanup C4, D1), the
//! shared keys and `g` places on the left and this screen's own keys beside them, so help and
//! keys cannot drift (the hand-written list was wrong in places); then the symbols. Any key
//! closes it.

use crate::app::App;
use locus_theme::{c, glyph, popup};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let key = Style::default().fg(c("blue"));
    let text = Style::default().fg(c("text"));
    let dim = Style::default().fg(c("overlay1"));
    let w = 95.min(area.width);
    let screen = app.screen();
    let mut lines = locus_theme::keys::help_lines(screen.title(), &crate::ui::help_keys(screen), w.saturating_sub(2));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(" symbols", dim)));
    for row in glyph::ALL.chunks(3) {
        let mut spans = Vec::new();
        for (name, g) in row {
            spans.push(Span::styled(format!("  {g} "), key));
            spans.push(Span::styled(format!("{:<26}", name.replace('_', " ")), text));
        }
        lines.push(Line::from(spans));
    }
    let h = u16::try_from(lines.len() + 2).unwrap_or(u16::MAX).min(area.height);
    let rect = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).block(popup("keys")), rect);
}
