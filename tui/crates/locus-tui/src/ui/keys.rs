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
            Span::styled(format!("  {}  ", s.letter), key),
            Span::styled(s.title, text),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(" everywhere", dim)));
    for (k, what) in [
        ("⏎", "open"),
        ("esc", "back"),
        ("/", "search"),
        ("?", "keys"),
        ("R", "reset fixtures"),
        ("alt-1…4", "home · project · calendar · mail"),
    ] {
        lines.push(Line::from(vec![
            Span::styled(format!("  {k:<8}"), key),
            Span::styled(what, text),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(" symbols", dim)));
    for pair in glyph::ALL.chunks(2) {
        let mut spans = Vec::new();
        for (name, g) in pair {
            spans.push(Span::styled(format!("  {g} "), key));
            spans.push(Span::styled(format!("{name:<22}"), text));
        }
        lines.push(Line::from(spans));
    }
    let h = u16::try_from(lines.len() + 2).unwrap_or(u16::MAX).min(area.height);
    let w = 56.min(area.width);
    let rect = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).block(popup("keys")), rect);
}
