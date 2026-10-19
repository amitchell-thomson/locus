//! A screen not yet built: its title, its sections in their letters' order, and which step
//! builds it. Lets navigation be tried before any screen has content.

use crate::app::App;
use crate::screen::ScreenId;
use crate::ui::section_block;
use locus_theme::c;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::widgets::Paragraph;

pub fn draw(f: &mut Frame, area: Rect, app: &App, screen: ScreenId) {
    let title = match screen {
        ScreenId::Project => crate::ui::project_title(app),
        s => s.title().to_string(),
    };
    let mut outer = section_block(&title, None, false);
    if screen == ScreenId::Project {
        outer = outer.title(crate::ui::follow_badge(app).right_aligned());
    }
    let inner = outer.inner(area);
    f.render_widget(outer, area);
    let sections = screen.sections();
    let dim = Style::default().fg(c("overlay1"));
    if sections.is_empty() {
        f.render_widget(Paragraph::new(" built in step 1.4").style(dim), inner);
        return;
    }
    let rows = Layout::vertical(sections.iter().map(|_| Constraint::Fill(1))).split(inner);
    for (sec, row) in sections.iter().zip(rows.iter()) {
        let block = section_block(sec.title, Some(sec.letter), app.focus == Some(sec.letter));
        let text = match sec.detail {
            Some(d) => format!(" built in step 1.4 · ⏎ opens {}", d.title()),
            None => " built in step 1.4".to_string(),
        };
        f.render_widget(Paragraph::new(text).style(dim).block(block), *row);
    }
}
