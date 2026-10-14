//! Theme: Catppuccin Mocha, transparent background (Color::Reset), rounded
//! borders, blue focus / mauve popups, ` · ` separators, `…` truncation.
//! Copied from kairos-tui `ui.rs` (shared-crate candidate: TUI kit, SPEC §11).

use keryx_core::colour::Rgb;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub fn c(name: &str) -> Color {
    rgb(Rgb::mocha(name))
}

pub fn rgb(r: Rgb) -> Color {
    Color::Rgb(r.0, r.1, r.2)
}

/// Blend a colour towards the Mocha base (0 = unchanged, 1 = base).
pub fn fade(col: Color, t: f32) -> Color {
    let Rgb(br, bg, bb) = Rgb::mocha("base");
    match col {
        Color::Rgb(r, g, b) => {
            let m = |a: u8, z: u8| (f32::from(a) + (f32::from(z) - f32::from(a)) * t).round() as u8;
            Color::Rgb(m(r, br), m(g, bg), m(b, bb))
        }
        c => c,
    }
}

/// Truncate to `w` columns with `…`.
pub fn fit(s: &str, w: usize) -> String {
    if s.width() <= w {
        return s.to_string();
    }
    if w == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in s.chars() {
        let cw = ch.width().unwrap_or(0);
        if used + cw + 1 > w {
            break;
        }
        out.push(ch);
        used += cw;
    }
    out.push('…');
    out
}

/// Pad (or truncate) to exactly `w` columns.
pub fn pad(s: &str, w: usize) -> String {
    let f = fit(s, w);
    let n = w.saturating_sub(f.width());
    format!("{f}{}", " ".repeat(n))
}

/// Word-wrap to `w` columns (long words are broken).
pub fn wrap(s: &str, w: usize) -> Vec<String> {
    let w = w.max(1);
    let mut out = Vec::new();
    for para in s.split('\n') {
        if para.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut line = String::new();
        let mut lw = 0;
        for word in para.split(' ') {
            let ww = word.width();
            if lw > 0 && lw + 1 + ww > w {
                out.push(std::mem::take(&mut line));
                lw = 0;
            }
            if ww > w {
                // Hard-break very long words (URLs).
                for ch in word.chars() {
                    let cw = ch.width().unwrap_or(0);
                    if lw + cw > w {
                        out.push(std::mem::take(&mut line));
                        lw = 0;
                    }
                    line.push(ch);
                    lw += cw;
                }
                continue;
            }
            if lw > 0 {
                line.push(' ');
                lw += 1;
            }
            line.push_str(word);
            lw += ww;
        }
        out.push(line);
    }
    out
}

/// The selected row: a solid fill, dark bold text (as kairos).
pub fn selected_style(col: Color) -> Style {
    Style::default()
        .fg(c("base"))
        .bg(fade(col, 0.15))
        .add_modifier(Modifier::BOLD)
}

pub fn panel(title: &str, focus: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if focus { c("blue") } else { c("overlay0") }))
        .title(
            Line::from(Span::styled(
                format!(" {title} "),
                Style::default().fg(c("blue")).add_modifier(Modifier::BOLD),
            ))
            .alignment(Alignment::Center),
        )
}

pub fn popup_block(title: &str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(c("mauve")))
        .title(
            Line::from(Span::styled(
                format!(" {title} "),
                Style::default().fg(c("mauve")).add_modifier(Modifier::BOLD),
            ))
            .alignment(Alignment::Center),
        )
}

pub fn centred(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_and_pad() {
        assert_eq!(fit("hello", 10), "hello");
        assert_eq!(fit("hello world", 6), "hello…");
        assert_eq!(pad("ab", 4), "ab  ");
        assert_eq!(pad("abcdef", 4), "abc…");
    }

    #[test]
    fn wrapping() {
        assert_eq!(wrap("one two three", 7), vec!["one two", "three"]);
        assert_eq!(wrap("a\n\nb", 5), vec!["a", "", "b"]);
        assert_eq!(wrap("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
    }
}
