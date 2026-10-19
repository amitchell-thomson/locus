//! Render a screen to text without a terminal: what every Slice 1 check prints, and what the
//! snapshot tests compare.

use crate::app::App;
use crate::ui;
use anyhow::Result;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

pub fn render(app: &App, width: u16, height: u16) -> Result<String> {
    let mut term = Terminal::new(TestBackend::new(width, height))?;
    term.draw(|f| ui::draw(f, app))?;
    let buf = term.backend().buffer();
    let mut out = String::new();
    for y in 0..height {
        let mut line = String::new();
        for x in 0..width {
            if let Some(cell) = buf.cell((x, y)) {
                line.push_str(cell.symbol());
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    Ok(out)
}

/// `95x38` → (95, 38).
pub fn parse_size(s: &str) -> Option<(u16, u16)> {
    let (w, h) = s.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}
