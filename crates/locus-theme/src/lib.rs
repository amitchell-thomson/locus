//! The look shared by locus, kairos and keryx.
//!
//! Catppuccin Mocha on a transparent background, rounded borders, blue focus and mauve popups,
//! ` · ` separators and `…` truncation: kairos's conventions, lifted here so the three apps
//! cannot drift apart. kairos and keryx still carry their own copies and move onto this crate
//! when next touched (locus Slice 1 spec, D2). It lived in the `eco` repo, renamed `locus-shared`, until it was
//! folded into the locus repo on 19 Oct 2026 (ADR-004)..

pub mod glyph;

use ratatui::layout::Alignment;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders};

/// Catppuccin Mocha palette, `(name, rgb)`.
pub const MOCHA: &[(&str, (u8, u8, u8))] = &[
    ("rosewater", (0xf5, 0xe0, 0xdc)),
    ("flamingo", (0xf2, 0xcd, 0xcd)),
    ("pink", (0xf5, 0xc2, 0xe7)),
    ("mauve", (0xcb, 0xa6, 0xf7)),
    ("red", (0xf3, 0x8b, 0xa8)),
    ("maroon", (0xeb, 0xa0, 0xac)),
    ("peach", (0xfa, 0xb3, 0x87)),
    ("yellow", (0xf9, 0xe2, 0xaf)),
    ("green", (0xa6, 0xe3, 0xa1)),
    ("teal", (0x94, 0xe2, 0xd5)),
    ("sky", (0x89, 0xdc, 0xeb)),
    ("sapphire", (0x74, 0xc7, 0xec)),
    ("blue", (0x89, 0xb4, 0xfa)),
    ("lavender", (0xb4, 0xbe, 0xfe)),
    ("text", (0xcd, 0xd6, 0xf4)),
    ("subtext1", (0xba, 0xc2, 0xde)),
    ("subtext0", (0xa6, 0xad, 0xc8)),
    ("overlay2", (0x93, 0x99, 0xb2)),
    ("overlay1", (0x7f, 0x84, 0x9c)),
    ("overlay0", (0x6c, 0x70, 0x86)),
    ("surface2", (0x58, 0x5b, 0x70)),
    ("surface1", (0x45, 0x47, 0x5a)),
    ("surface0", (0x31, 0x32, 0x44)),
    ("base", (0x1e, 0x1e, 0x2e)),
    ("mantle", (0x18, 0x18, 0x25)),
    ("crust", (0x11, 0x11, 0x1b)),
];

/// A Mocha colour by name. Unknown names fall back to `text`, never panic: a typo in a
/// config should look wrong, not crash the window.
pub fn c(name: &str) -> Color {
    let (r, g, b) = MOCHA
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map_or((0xcd, 0xd6, 0xf4), |(_, rgb)| *rgb);
    Color::Rgb(r, g, b)
}

/// Blend a colour towards the Mocha base (0 = unchanged, 1 = base).
pub fn fade(col: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let Color::Rgb(br, bg, bb) = c("base") else { return col };
    match col {
        Color::Rgb(r, g, b) => {
            let m = |a: u8, z: u8| (f32::from(a) + (f32::from(z) - f32::from(a)) * t).round() as u8;
            Color::Rgb(m(r, br), m(g, bg), m(b, bb))
        }
        other => other,
    }
}

/// The separator between items on one line.
pub const SEP: &str = " · ";
/// What a truncated string ends with.
pub const ELLIPSIS: char = '…';

/// Text in a Mocha colour.
pub fn st(name: &str) -> Style {
    Style::default().fg(c(name))
}

/// Quiet text: hint words, empty states, secondary facts.
pub fn dim() -> Style {
    st("overlay1")
}

/// A key, in a hint: blue, not bold.
pub fn keyst() -> Style {
    st("blue")
}

pub fn bold(s: Style) -> Style {
    s.add_modifier(Modifier::BOLD)
}

/// Key hints, the one format for every app (L2): `y tick · n strike`, the key blue, the words
/// dim.
pub fn hints(pairs: &[(&str, &str)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, (k, what)) in pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(SEP, dim()));
        }
        spans.push(Span::styled(k.to_string(), keyst()));
        spans.push(Span::styled(format!(" {what}"), dim()));
    }
    Line::from(spans)
}

/// A screen title: ` name · extra `, the name bold, the extra dim.
pub fn title(name: &str, extra: &str) -> Line<'static> {
    if extra.is_empty() {
        return Line::from(Span::styled(format!(" {name} "), bold(st("text"))));
    }
    Line::from(vec![
        Span::styled(format!(" {name}"), bold(st("text"))),
        Span::styled(format!("{SEP}{extra} "), dim()),
    ])
}

/// A screen's frame: rounded, grey (overlay0), its title on the left and an optional corner
/// on the right.
pub fn screen(title: Line<'static>, right: Option<Line<'static>>) -> Block<'static> {
    let mut b = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(st("overlay0"))
        .title(title.left_aligned());
    if let Some(r) = right {
        b = b.title(r.right_aligned());
    }
    b
}

/// Cut `s` to `width` display columns, ending with `…` when cut.
pub fn fit(s: &str, width: usize) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    if s.width() <= width {
        return s.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in s.chars() {
        let cw = ch.width().unwrap_or(0);
        if used + cw + 1 > width {
            break;
        }
        out.push(ch);
        used += cw;
    }
    out.push(ELLIPSIS);
    out
}

/// A panel: rounded border, blue when focused, overlay0 otherwise; the title centred.
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

/// A popup: rounded border and title in mauve.
pub fn popup(title: &str) -> Block<'static> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_name_falls_back_to_text() {
        assert_eq!(c("nope"), c("text"));
        assert_eq!(c("Blue"), Color::Rgb(0x89, 0xb4, 0xfa));
    }

    #[test]
    fn fade_ends_at_base() {
        assert_eq!(fade(c("red"), 1.0), c("base"));
        assert_eq!(fade(c("red"), 0.0), c("red"));
    }
}
