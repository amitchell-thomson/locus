//! The look, from locus-theme (keryx-joins-locus X2: one theme crate for every app): the
//! palette, `fit`, frames and popups are locus's; what is left here are text helpers (`pad`,
//! `wrap`) and the selection fill.

use keryx_core::colour::Rgb;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Block;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub use locus_theme::{c, fade, fit};

pub fn rgb(r: Rgb) -> Color {
    Color::Rgb(r.0, r.1, r.2)
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

/// The selected row: a solid fill in the full colour, dark bold text. kairos softens its fill
/// (15% towards the background); in the mail list that read as grey and the row was hard to
/// find (your note of 26 Oct), so keryx's is the colour itself.
pub fn selected_style(col: Color) -> Style {
    Style::default().fg(c("base")).bg(col).add_modifier(Modifier::BOLD)
}

/// The list's selection while you read beside it: a medium tint of the colour with light bold
/// text, as kairos marks what is linked to its selection. It was the grey `surface0`, which sat
/// too close to the list's dim text to find (your note of 26 Oct).
pub fn reading_selected_style(col: Color) -> Style {
    Style::default()
        .fg(c("text"))
        .bg(fade(col, 0.55))
        .add_modifier(Modifier::BOLD)
}

/// A pane: locus's grey screen frame with its title on the left (the frame is never blue: the
/// selection says where you are).
pub fn panel(title: &str, _focus: bool) -> Block<'static> {
    locus_theme::screen(locus_theme::title(title, ""), None)
}

pub fn popup_block(title: &str) -> Block<'static> {
    locus_theme::popup(title)
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
