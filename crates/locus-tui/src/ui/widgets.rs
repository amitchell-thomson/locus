//! Pieces every screen draws with: times, bars, glyphs by kind, section rules, list rows.

use locus_contract::{DecisionStatus, FindKind, ItemKind, JobOutcome, SessionState, Time};
use locus_theme::{SEP, c, glyph};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

// The look's pieces come from locus-theme, shared with kairos (L1).
pub use locus_theme::{bold, dim, keyst, st};

pub fn text() -> Style {
    st("text")
}

pub fn struck(s: Style) -> Style {
    s.add_modifier(Modifier::CROSSED_OUT).fg(c("overlay0"))
}

/// Time since `t`, short: `now`, `12m`, `5h`, `3d`, `2w`.
pub fn ago(now: Time, t: Time) -> String {
    let m = (now - t).num_minutes();
    match m {
        i64::MIN..=0 => "now".into(),
        1..=59 => format!("{m}m"),
        60..=1439 => format!("{}h", m / 60),
        1440..=20159 => format!("{}d", m / 1440),
        _ => format!("{}w", m / 10080),
    }
}

/// `12:41` or `3:07` as a running clock since `t`.
pub fn elapsed(now: Time, t: Time) -> String {
    let s = (now - t).num_seconds().max(0);
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

pub fn hm(t: Time) -> String {
    t.format("%H:%M").to_string()
}

/// `sat 24`
pub fn day(d: chrono::NaiveDate) -> String {
    d.format("%a %-d").to_string().to_lowercase()
}

/// `12 Oct`
pub fn date(d: chrono::NaiveDate) -> String {
    d.format("%-d %b").to_string()
}

/// A thin bar: `frac` of `width` filled (━), the rest (─), with an optional mark (┊) at `mark`.
pub fn bar(frac: f32, width: usize, mark: Option<f32>, fill: Color) -> Vec<Span<'static>> {
    let n = ((frac.clamp(0.0, 1.0) * width as f32).round() as usize).min(width);
    let m = mark.map(|m| ((m.clamp(0.0, 1.0) * width as f32).round() as usize).min(width.saturating_sub(1)));
    let mut spans = Vec::new();
    for i in 0..width {
        let (ch, col) = if Some(i) == m && i >= n {
            ('┊', c("peach"))
        } else if i < n {
            ('━', fill)
        } else {
            ('─', c("surface1"))
        };
        spans.push(Span::styled(ch.to_string(), Style::default().fg(col)));
    }
    spans
}

/// The running glyph, turning: md-sync and md-autorenew are the same two arrows a quarter turn
/// apart, so alternating them reads as rotation (round 1b: "flipping between two rotation
/// glyphs in different degrees of rotation", not a colour change). Frame 0 is md-sync, the
/// D.2 glyph, so `--dump` and the legend agree.
pub fn spin(frame: u64) -> &'static str {
    if (frame / 2).is_multiple_of(2) {
        locus_theme::glyph::WORKING
    } else {
        "\u{f006a}"
    }
}

pub fn item_glyph(k: ItemKind) -> (&'static str, &'static str) {
    match k {
        ItemKind::Recheck => (glyph::RECHECK, "yellow"),
        ItemKind::Redecided => (glyph::REDECIDED, "mauve"),
        ItemKind::Decision => (glyph::HOLDING, "green"),
        ItemKind::Answer => (glyph::ANSWER, "sky"),
        ItemKind::ScoutFind => (glyph::SCOUT_FIND, "mauve"),
        ItemKind::Reading => (glyph::READING, "teal"),
        ItemKind::Idea => (glyph::IDEA, "yellow"),
        ItemKind::Commits => (glyph::COMMITS, "green"),
        ItemKind::Chat => (glyph::CHAT, "lavender"),
        ItemKind::Dossier => (glyph::DOSSIER, "blue"),
        ItemKind::Ink => (glyph::INK, "flamingo"),
        ItemKind::Mail => (glyph::MAIL, "sapphire"),
        ItemKind::Calendar => (glyph::CALENDAR, "peach"),
        ItemKind::AutoAction => (glyph::AUTO_ACTION, "overlay2"),
    }
}

pub fn session_glyph(s: SessionState) -> (&'static str, &'static str, &'static str) {
    match s {
        SessionState::NeedsYou => (glyph::NEEDS_YOU, "peach", "needs you"),
        SessionState::Working => (glyph::WORKING, "blue", "working"),
        SessionState::Failed => (glyph::FAILED, "red", "failed"),
        SessionState::Done => (glyph::DONE, "green", "done"),
        SessionState::Idle => (glyph::IDLE, "overlay1", "idle"),
    }
}

pub fn job_glyph(o: &JobOutcome) -> (&'static str, &'static str) {
    match o {
        JobOutcome::Running => (glyph::WORKING, "blue"),
        JobOutcome::Done => (glyph::DONE, "green"),
        JobOutcome::Failed { .. } => (glyph::FAILED, "red"),
    }
}

pub fn decision_glyph(s: &DecisionStatus) -> (&'static str, &'static str, String) {
    match s {
        DecisionStatus::Holding => (glyph::HOLDING, "green", "holding".into()),
        DecisionStatus::Recheck => (glyph::RECHECK, "yellow", "re-check".into()),
        DecisionStatus::Review => (glyph::REVIEW, "sky", "under review".into()),
        // Never the replacing decision's number (round 2): the screens name it by title.
        DecisionStatus::Redecided { .. } => (glyph::REDECIDED, "mauve", "replaced".into()),
    }
}

pub fn find_glyph(k: FindKind) -> (&'static str, &'static str, &'static str) {
    match k {
        FindKind::Method => (glyph::SCOUT_FIND, "mauve", "method"),
        FindKind::Course => (glyph::SCOUT_FIND, "blue", "course"),
        FindKind::Reading => (glyph::READING, "teal", "read"),
        FindKind::Pattern => (glyph::SCOUT_FIND, "green", "pattern"),
        FindKind::NewProject => (glyph::IDEA, "yellow", "new project"),
    }
}

/// A section's rule: `─ title · extra ──────── [l] ─`, blue when focused.
pub fn rule(title: &str, extra: &str, letter: Option<char>, focus: bool, width: u16) -> Line<'static> {
    let col = if focus { c("blue") } else { c("surface2") };
    if title.is_empty() && extra.is_empty() && letter.is_none() {
        return Line::from(Span::styled("─".repeat(usize::from(width)), Style::default().fg(col)));
    }
    // Titles bright and bold, the line itself quiet, so sections stand apart even unfocused
    // (round 1b: "the grey text hides in between the content of the surrounding sections").
    let ts = if focus {
        bold(Style::default().fg(c("blue")))
    } else {
        bold(st("text"))
    };
    let mut spans = vec![
        Span::styled("─ ", Style::default().fg(col)),
        Span::styled(title.to_string(), ts),
    ];
    let mut used = 2 + title.width();
    if !extra.is_empty() {
        let e = format!("{SEP}{extra}");
        used += e.width();
        spans.push(Span::styled(e, dim()));
    }
    let tail = letter.map(|l| format!(" [{l}] ─")).unwrap_or_else(|| "──".into());
    let fill = usize::from(width).saturating_sub(used + 1 + tail.width());
    spans.push(Span::styled(format!(" {}", "─".repeat(fill)), Style::default().fg(col)));
    spans.push(Span::styled(tail, Style::default().fg(col)));
    Line::from(spans)
}

/// What `pointer` puts at the start of the selected row; `row` sees it and lights the row.
const SELECTED: &str = "▌ ";

/// A row with text on the left and `right` flush right in `width`, the left cut to fit. A row
/// that starts with the selection pointer is drawn on a band across the whole width (round 1:
/// "we need more of an indicator of what is selected than the tiny little arrow").
pub fn row(left: Vec<Span<'static>>, right: Vec<Span<'static>>, width: u16) -> Line<'static> {
    let selected = left.first().is_some_and(|s| s.content == SELECTED);
    let line = row_plain(left, right, width);
    if selected {
        line.patch_style(Style::default().bg(c("surface0")))
    } else {
        line
    }
}

fn row_plain(left: Vec<Span<'static>>, right: Vec<Span<'static>>, width: u16) -> Line<'static> {
    let rw: usize = right.iter().map(|s| s.content.width()).sum();
    let avail = usize::from(width).saturating_sub(rw + 1);
    let mut out = Vec::new();
    let mut used = 0;
    for s in left {
        let w = s.content.width();
        if used + w <= avail {
            used += w;
            out.push(s);
        } else {
            let cut = crate::ui::truncate(&s.content, avail.saturating_sub(used));
            used += cut.width();
            out.push(Span::styled(cut, s.style));
            break;
        }
    }
    out.push(Span::raw(" ".repeat(usize::from(width).saturating_sub(used + rw))));
    out.extend(right);
    Line::from(out)
}

/// The selection marker for row `i` when `sel` is the selected row of a focused list.
pub fn pointer(i: usize, sel: Option<usize>) -> Span<'static> {
    if sel == Some(i) {
        Span::styled(SELECTED, keyst())
    } else {
        Span::raw("  ")
    }
}

/// Colours for projects where a picture needs one per project (the rhythm grid, the chart):
/// stable by position in the registry.
pub const PROJECT_COLOURS: &[&str] = &[
    "blue", "mauve", "green", "peach", "teal", "pink", "yellow", "sky", "lavender",
];

pub fn project_colour(reg: &locus_contract::Registry, id: &str) -> Color {
    let i = reg.projects.iter().position(|p| p.id == id).unwrap_or(0);
    c(PROJECT_COLOURS[i % PROJECT_COLOURS.len()])
}

/// Wrap `s` to `width` columns on spaces.
pub fn wrap(s: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for w in s.split_whitespace() {
        if !line.is_empty() && line.width() + 1 + w.width() > width {
            out.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(w);
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_fills_exactly_the_width() {
        for w in [40u16, 95] {
            let l = rule("new", "5 unread", Some('n'), false, w);
            assert_eq!(l.width(), usize::from(w));
        }
    }

    #[test]
    fn row_puts_right_text_flush_right() {
        let l = row(
            vec![Span::raw("a long left side that will not fit")],
            vec![Span::raw("2h")],
            20,
        );
        assert_eq!(l.width(), 20);
        assert!(l.to_string().ends_with("2h"));
    }

    #[test]
    fn ago_steps() {
        let t: Time = "2026-10-24T21:40:00+01:00".parse().unwrap();
        let back = |m| t - chrono::Duration::minutes(m);
        assert_eq!(
            [0, 12, 300, 3000, 30000].map(|m| ago(t, back(m))),
            ["now", "12m", "5h", "2d", "2w"].map(String::from)
        );
    }
}
