//! Drawing. The look is `locus_theme`'s (kairos's): Mocha on a transparent background, rounded
//! borders, blue focus. Each screen is a module with `draw`, `key` (its own keys, tried before
//! the global ones) and `KEYS` (what the `?` overlay lists); the footer is shared.

pub mod approve;
pub mod chat;
pub mod home;
mod keys;
pub mod learning;
pub mod projects;
pub mod system;
pub mod tasks;
pub mod widgets;
pub mod work;

use crate::app::{App, Mode};
use crate::screen::ScreenId;
use locus_theme::c;
use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use widgets::{dim, keyst, st};

pub fn draw(f: &mut Frame, app: &App) {
    let [body, footer] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(f.area());
    match app.screen() {
        ScreenId::Home => home::draw(f, body, app),
        ScreenId::System => system::draw(f, body, app),
        ScreenId::Budget => system::draw_budget(f, body, app),
        ScreenId::Approve => approve::draw(f, body, app),
        ScreenId::AutoLog => approve::draw_log(f, body, app),
        ScreenId::ApproveItem => approve::draw_item(f, body, app),
        ScreenId::WorkChart => work::draw_chart(f, body, app),
        ScreenId::Learning => learning::draw(f, body, app),
        ScreenId::Tasks => tasks::draw(f, body, app),
        ScreenId::Projects => projects::draw(f, body, app),
        ScreenId::Chat => chat::draw(f, body, app),
    }
    draw_footer(f, footer, app);
    if app.keys_open {
        keys::draw(f, f.area(), app);
    }
}

/// The screen's own keys, tried before the global ones. True when the key was used.
pub fn screen_key(app: &mut App, k: KeyEvent) -> bool {
    // A key that asks first is armed only on the Tasks page; anywhere else it is forgotten.
    if app.screen() != ScreenId::Tasks {
        app.armed = None;
    }
    match app.screen() {
        ScreenId::Home => home::key(app, k),
        ScreenId::System | ScreenId::Budget => system::key(app, k),
        ScreenId::Approve | ScreenId::AutoLog | ScreenId::ApproveItem => approve::key(app, k),
        ScreenId::WorkChart => false,
        ScreenId::Learning => learning::key(app, k),
        ScreenId::Tasks => tasks::key(app, k),
        ScreenId::Projects => projects::key(app, k),
        ScreenId::Chat => chat::key(app, k),
    }
}

/// Screens whose sections are lists that ↑ and ↓ walk through, section to section.
pub fn flows(s: ScreenId) -> bool {
    matches!(s, ScreenId::Home | ScreenId::System | ScreenId::Learning)
}

/// How many rows a section has, for walking through it with the arrows.
pub fn section_len(app: &App, sec: char) -> usize {
    match app.screen() {
        ScreenId::Home => home::section_len(app, sec),
        ScreenId::System => system::section_len(app, sec),
        ScreenId::Learning => match sec {
            'g' => app.fixtures.learning.goals.len(),
            't' => app.fixtures.learning.tracks.len(),
            _ => app.fixtures.learning.sheets.len().min(4),
        },
        _ => 1,
    }
}

/// The keys a screen adds to the global ones, for the overlay.
pub fn screen_keys(s: ScreenId) -> &'static [(&'static str, &'static str)] {
    match s {
        ScreenId::Home => home::KEYS,
        ScreenId::System => system::KEYS,
        ScreenId::Budget => system::BUDGET_KEYS,
        ScreenId::Approve => approve::KEYS,
        ScreenId::AutoLog => approve::LOG_KEYS,
        ScreenId::ApproveItem => approve::ITEM_KEYS,
        ScreenId::WorkChart => &[],
        ScreenId::Learning => learning::KEYS,
        ScreenId::Tasks => tasks::KEYS,
        ScreenId::Projects => projects::KEYS,
        ScreenId::Chat => chat::KEYS,
    }
}

/// The screen's outer frame with its title, and the area inside it (locus-theme's `screen`,
/// the frame kairos draws too).
pub fn frame(f: &mut Frame, area: Rect, title: Line<'static>, right: Option<Line<'static>>) -> Rect {
    let b = locus_theme::screen(title, right);
    let inner = b.inner(area);
    f.render_widget(b, area);
    inner
}

pub use locus_theme::{hints, title};

/// `esc back` in a detail screen's top-right corner.
pub fn esc_back() -> Line<'static> {
    Line::from(vec![Span::styled(" esc", keyst()), Span::styled(" back ", dim())])
}

/// An ask line's rule: its title on the left, the page's keys at the right end (round 3b: "the other hints should be the same format as
/// in the ask window"). Keys that do not fit are dropped from the end; `tail` (a letter, a
/// count) always stays.
pub fn ask_rule(title: &str, lit: bool, keys: &[(&str, &str)], tail: &str, width: u16) -> Line<'static> {
    use unicode_width::UnicodeWidthStr;
    let col = st(if lit { "blue" } else { "surface2" });
    let mut left = vec![
        Span::styled("─ ", col),
        Span::styled(
            title.to_string(),
            widgets::bold(if lit { st("blue") } else { st("text") }),
        ),
    ];
    let tail = Span::styled(
        if tail.is_empty() {
            " ─".to_string()
        } else {
            format!(" {tail} ─")
        },
        col,
    );
    let room = usize::from(width)
        .saturating_sub(left.iter().map(|s| s.content.width()).sum::<usize>() + tail.content.width() + 4);
    let mut n = keys.len();
    let mut right = hints(&keys[..n]).spans;
    while n > 0 && right.iter().map(|s| s.content.width()).sum::<usize>() > room {
        n -= 1;
        right = hints(&keys[..n]).spans;
    }
    let used: usize = left
        .iter()
        .chain(right.iter())
        .map(|s| s.content.width())
        .sum::<usize>()
        + tail.content.width();
    left.push(Span::styled(
        format!(" {} ", "─".repeat(usize::from(width).saturating_sub(used + 2))),
        col,
    ));
    left.extend(right);
    left.push(tail);
    Line::from(left)
}

/// A section's block: title on the left, its letter in the right corner (plan D), blue when
/// focused.
pub fn section_block(title: &str, letter: Option<char>, focus: bool) -> Block<'static> {
    let border = if focus { c("blue") } else { c("overlay0") };
    let title_style = if focus {
        Style::default().fg(c("blue")).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(c("subtext0"))
    };
    let mut b = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border))
        .title(Line::from(Span::styled(format!(" {title} "), title_style)).left_aligned());
    if let Some(l) = letter {
        b = b.title(Line::from(Span::styled(format!("[{l}]"), Style::default().fg(border))).right_aligned());
    }
    b
}

/// The footer: open · log · ask · keys on the left (plan D: Home's key strip keeps only
/// these), the note on the right. While typing, it says how to finish.
fn draw_footer(f: &mut Frame, area: Rect, app: &App) {
    let left = match &app.mode {
        Mode::Chat(_) => hints(&[("⏎", "send"), ("⇧⏎", "new line"), ("esc", "keep it for later")]),
        Mode::ChatName(_) => hints(&[("⏎", "rename"), ("esc", "cancel")]),
        // The fact's value as you correct it, with how to finish.
        Mode::FactEdit(_, _, v) => {
            let mut l = Line::from(vec![
                Span::styled(" value: ", Style::default().fg(locus_theme::c("yellow"))),
                Span::raw(v.clone()),
                Span::styled("▏", Style::default().fg(locus_theme::c("blue"))),
                Span::raw("   "),
            ]);
            l.spans
                .extend(hints(&[("⏎", "approve as written"), ("esc", "cancel")]).spans);
            l
        }
        // Enter's hint says what it does for the focused section, where a screen knows.
        Mode::Normal => {
            let enter = match app.screen() {
                ScreenId::Home => home::enter_hint(app),
                _ => None,
            }
            .unwrap_or("open");
            hints(&[
                ("⏎", enter),
                ("⇥", "sections"),
                ("l", "log"),
                ("/", "ask"),
                ("i", "chat"),
                ("?", "keys"),
            ])
        }
    };
    let used = u16::try_from(left.width()).unwrap_or(u16::MAX);
    let [l, r] = Layout::horizontal([Constraint::Length(used.saturating_add(1)), Constraint::Min(0)]).areas(area);
    f.render_widget(
        Paragraph::new(Line::from([vec![Span::raw(" ")], left.spans].concat())),
        l,
    );
    if let Some(note) = &app.note {
        let text = truncate(note, usize::from(r.width.saturating_sub(1)));
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("{text} "),
                Style::default().fg(c("peach")),
            )))
            .right_aligned(),
            r,
        );
    }
}

/// Cut `s` to `width` display columns, ending with `…` when cut.
pub fn truncate(s: &str, width: usize) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    if s.width() <= width {
        return s.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut w = 0;
    for ch in s.chars() {
        let cw = ch.width().unwrap_or(0);
        if w + cw + 1 > width {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push(locus_theme::ELLIPSIS);
    out
}

/// Whether spare rows show a section's summary box (round 2g: "I may ask to revert if I think I
/// like just normal empty more"): false gives plain space everywhere.
pub const SUMMARY_BOXES: bool = false;

/// A section's summary in a quiet rounded box, centred in rows the section had no use for: the
/// one filler for any section on any page (round 2g: "I want any space filler to be the same
/// sort of thing for any section on any page"). Nothing when fewer than four rows are spare, so
/// a box always has a line of space around it and shrinks away as the section fills.
pub fn summary_box(f: &mut Frame, area: Rect, body: Vec<Line<'static>>) {
    if !SUMMARY_BOXES || area.height < 4 || area.width < 20 || body.is_empty() {
        return;
    }
    let h = (body.len() as u16 + 2).min(area.height - 1);
    let rect = Rect {
        x: area.x + 2,
        y: area.y + (area.height - h) / 2,
        width: area.width.saturating_sub(4),
        height: h,
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(c("surface1")));
    f.render_widget(Paragraph::new(body).block(block), rect);
}

/// Draw `lines` into `area`, top-aligned; anything past the bottom is cut.
pub fn lines(f: &mut Frame, area: Rect, lines: Vec<Line<'static>>) {
    f.render_widget(Paragraph::new(lines), area);
}

#[cfg(test)]
mod tests {
    use super::truncate;

    #[test]
    fn truncates_by_display_width() {
        assert_eq!(truncate("abcdef", 6), "abcdef");
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abcdef", 0), "");
    }
}
