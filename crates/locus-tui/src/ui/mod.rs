//! Drawing. The look is `locus_theme`'s (kairos's): Mocha on a transparent background, rounded
//! borders, blue focus. Each screen fills the body; the footer is shared.

mod keys;
mod placeholder;

use crate::app::App;
use crate::screen::ScreenId;
use locus_theme::{SEP, c};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

pub fn draw(f: &mut Frame, app: &App) {
    let [body, footer] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(f.area());
    match app.screen() {
        // Every screen is a placeholder until step 1.4 builds it.
        s @ (ScreenId::Home
        | ScreenId::Project
        | ScreenId::System
        | ScreenId::Budget
        | ScreenId::Explore
        | ScreenId::Approve
        | ScreenId::AutoLog
        | ScreenId::InkConfirm
        | ScreenId::Decisions
        | ScreenId::Answered
        | ScreenId::Changes
        | ScreenId::Search
        | ScreenId::WorkChart
        | ScreenId::Prep
        | ScreenId::Learning
        | ScreenId::Review) => placeholder::draw(f, body, app, s),
    }
    draw_footer(f, footer, app);
    if app.keys_open {
        keys::draw(f, f.area(), app);
    }
}

/// The Project view's title: the project the view is on, by the registry; a session the
/// registry does not know is named as such rather than guessed at.
pub fn project_title(app: &App) -> String {
    match app.follow.session.as_deref() {
        None => "project · no project window seen yet".into(),
        Some(s) => match app.fixtures.registry.by_session(s) {
            Some(p) => format!("{} {}", locus_theme::glyph::PROJECT, p.name),
            None => format!("{s} · not a registered project"),
        },
    }
}

/// Following or pinned, in the Project view's top-right corner.
pub fn follow_badge(app: &App) -> Line<'static> {
    let (text, col) = if app.follow.pinned {
        (format!(" {} pinned ", locus_theme::glyph::HOLDING), c("peach"))
    } else {
        (format!(" {} following ", locus_theme::glyph::ATTENDED), c("overlay1"))
    };
    Line::from(Span::styled(text, Style::default().fg(col)))
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

/// The footer: open · log · search · keys on the left (plan D: Home's key strip keeps only
/// these), the note on the right.
fn draw_footer(f: &mut Frame, area: Rect, app: &App) {
    let key = Style::default().fg(c("blue"));
    let dim = Style::default().fg(c("overlay1"));
    let mut spans = Vec::new();
    for (i, (k, what)) in [("⏎", "open"), ("l", "log"), ("/", "search"), ("?", "keys")]
        .into_iter()
        .enumerate()
    {
        if i > 0 {
            spans.push(Span::styled(SEP, dim));
        }
        spans.push(Span::styled(k, key));
        spans.push(Span::styled(format!(" {what}"), dim));
    }
    let left = Line::from(spans);
    let used = u16::try_from(left.width()).unwrap_or(u16::MAX);
    f.render_widget(Paragraph::new(Line::from(vec![Span::raw(" ")]).patch_style(dim)), area);
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

#[cfg(test)]
mod tests {
    use super::truncate;

    #[test]
    fn truncates_by_display_width() {
        assert_eq!(truncate("abcdef", 6), "abcdef");
        assert_eq!(truncate("abcdef", 4), "abc…");
    }
}
