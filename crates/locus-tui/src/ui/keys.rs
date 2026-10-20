//! The keys overlay (`?` anywhere): this screen's sections, the keys that work everywhere, and
//! the symbols. Any key closes it.

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
    let mut lines = vec![Line::from(Span::styled(" this screen", dim))];
    for s in app.screen().sections() {
        lines.push(Line::from(vec![
            Span::styled(format!("  {:<8}", s.letter), key),
            Span::styled(s.title, text),
        ]));
    }
    for (k, what) in crate::ui::screen_keys(app.screen()) {
        lines.push(Line::from(vec![
            Span::styled(format!("  {k:<8}"), key),
            Span::styled(*what, text),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(" everywhere", dim)));
    for (k, what) in [
        ("⏎", "open"),
        ("esc", "back"),
        ("/", "search"),
        ("l", "the automatic-actions log"),
        ("j k ↑ ↓", "move in a list"),
        ("?", "keys"),
        ("R", "clear the prototype's ticks and strikes (fixture data only)"),
        ("alt-1…4", "home · project · calendar · mail"),
    ] {
        lines.push(Line::from(vec![
            Span::styled(format!("  {k:<8}"), key),
            Span::styled(what, text),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(" symbols", dim)));
    for row in glyph::ALL.chunks(3) {
        let mut spans = Vec::new();
        for (name, g) in row {
            spans.push(Span::styled(format!("  {g} "), key));
            spans.push(Span::styled(format!("{:<20}", name.replace('_', " ")), text));
        }
        lines.push(Line::from(spans));
    }
    let h = u16::try_from(lines.len() + 2).unwrap_or(u16::MAX).min(area.height);
    let w = 80.min(area.width);
    let rect = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).block(popup("keys")), rect);
}
