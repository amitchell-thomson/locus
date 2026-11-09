//! Markdown as Claude Code draws it in a terminal (9 Nov: the chat "should use the same rendering
//! as regular claude conversations"): **bold**, *italic*, `code` in its own colour, headings in
//! bold, `•` bullets and numbered lists that hang their wrapped lines, quotes behind a bar, code
//! blocks behind a gutter, and tables in box-drawing rules. Parsed by pulldown-cmark; this module
//! only lays the result out in a width, so a half-written reply (still streaming) renders as far
//! as it goes.

use crate::ui::widgets::{dim, st, text};
use pulldown_cmark::{Alignment, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

fn code_style() -> Style {
    st("peach")
}

/// `md` as lines no wider than `width`.
pub fn render(md: &str, width: usize) -> Vec<Line<'static>> {
    let width = width.max(12);
    let mut r = Renderer { width, out: Vec::new(), buf: Vec::new(), styles: vec![text()], lists: Vec::new(),
        marker: None, quote: 0, code: None, table: None };
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    for ev in Parser::new_ext(md, opts) {
        r.event(ev);
    }
    r.flush();
    while r.out.last().is_some_and(|l| l.width() == 0) {
        r.out.pop();
    }
    r.out
}

struct Table {
    align: Vec<Alignment>,
    rows: Vec<Vec<Vec<Span<'static>>>>,
    head_rows: usize,
    in_head: bool,
}

struct Renderer {
    width: usize,
    out: Vec<Line<'static>>,
    /// The paragraph being built.
    buf: Vec<Span<'static>>,
    styles: Vec<Style>,
    /// Open lists: Some(next number) for numbered, None for bullets.
    lists: Vec<Option<u64>>,
    /// The bullet or number waiting for an item's first line.
    marker: Option<String>,
    quote: usize,
    code: Option<String>,
    table: Option<Table>,
}

impl Renderer {
    fn style(&self) -> Style {
        *self.styles.last().unwrap_or(&text())
    }

    fn push_style(&mut self, f: impl Fn(Style) -> Style) {
        let s = f(self.style());
        self.styles.push(s);
    }

    fn pop_style(&mut self) {
        if self.styles.len() > 1 {
            self.styles.pop();
        }
    }

    fn blank(&mut self) {
        if self.out.last().is_some_and(|l| l.width() > 0) {
            self.out.push(Line::default());
        }
    }

    /// The prefix every line gets: the quote bars, then the list indent.
    fn prefix(&self) -> (Vec<Span<'static>>, Vec<Span<'static>>) {
        let mut first = Vec::new();
        for _ in 0..self.quote {
            first.push(Span::styled("│ ", dim()));
        }
        let depth = self.lists.len();
        let mut rest = first.clone();
        if depth > 0 {
            let pad = "  ".repeat(depth - 1);
            let marker = self.marker.clone().unwrap_or_default();
            let hang = " ".repeat(marker.width().max(2));
            first.push(Span::raw(pad.clone()));
            first.push(Span::styled(if marker.is_empty() { "  ".into() } else { marker }, dim()));
            rest.push(Span::raw(format!("{pad}{hang}")));
        }
        (first, rest)
    }

    fn flush(&mut self) {
        if self.buf.is_empty() {
            return;
        }
        let spans = std::mem::take(&mut self.buf);
        let (first, rest) = self.prefix();
        self.marker = None;
        let lines = wrap_spans(spans, self.width, first, rest);
        self.out.extend(lines);
    }

    fn event(&mut self, ev: Event) {
        if let Some(t) = &mut self.table {
            match ev {
                Event::End(TagEnd::Table) => {
                    if let Some(t) = self.table.take() {
                        self.blank();
                        let lines = draw_table(t, self.width);
                        self.out.extend(lines);
                        self.blank();
                    }
                }
                Event::Start(Tag::TableHead) => t.in_head = true,
                Event::End(TagEnd::TableHead) => {
                    t.in_head = false;
                    t.head_rows = t.rows.len();
                }
                Event::Start(Tag::TableRow) => t.rows.push(Vec::new()),
                Event::Start(Tag::TableCell) => {
                    if t.in_head && t.rows.len() == t.head_rows {
                        t.rows.push(Vec::new());
                    }
                    if let Some(r) = t.rows.last_mut() {
                        r.push(Vec::new());
                    }
                }
                Event::Text(s) | Event::Code(s) => {
                    let style = if t.in_head { text().add_modifier(Modifier::BOLD) } else { text() };
                    if let Some(c) = t.rows.last_mut().and_then(|r| r.last_mut()) {
                        c.push(Span::styled(s.to_string(), style));
                    }
                }
                _ => {}
            }
            return;
        }
        if let Some(code) = &mut self.code {
            match ev {
                Event::Text(s) => code.push_str(&s),
                Event::End(TagEnd::CodeBlock) => {
                    let code = self.code.take().unwrap_or_default();
                    self.blank();
                    let (first, _) = self.prefix();
                    for l in code.trim_end_matches('\n').lines() {
                        let mut spans = first.clone();
                        spans.push(Span::styled("▏ ", dim()));
                        spans.push(Span::styled(crate::ui::truncate(l, self.width.saturating_sub(4)), code_style()));
                        self.out.push(Line::from(spans));
                    }
                    self.blank();
                }
                _ => {}
            }
            return;
        }
        match ev {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {}
                Tag::Heading { level, .. } => {
                    self.flush();
                    self.blank();
                    let h1 = level == HeadingLevel::H1;
                    self.push_style(|s| {
                        let s = s.add_modifier(Modifier::BOLD);
                        if h1 { s.add_modifier(Modifier::UNDERLINED) } else { s }
                    });
                }
                Tag::BlockQuote(_) => {
                    self.flush();
                    self.quote += 1;
                    self.push_style(|s| s.add_modifier(Modifier::ITALIC));
                }
                Tag::CodeBlock(_) => {
                    self.flush();
                    self.code = Some(String::new());
                }
                Tag::List(start) => {
                    self.flush();
                    if self.lists.is_empty() {
                        self.blank();
                    }
                    self.lists.push(start);
                }
                Tag::Item => {
                    self.flush();
                    let m = match self.lists.last_mut() {
                        Some(Some(n)) => {
                            let m = format!("{n}. ");
                            *n += 1;
                            m
                        }
                        _ => "• ".to_string(),
                    };
                    self.marker = Some(m);
                }
                Tag::Emphasis => self.push_style(|s| s.add_modifier(Modifier::ITALIC)),
                Tag::Strong => self.push_style(|s| s.add_modifier(Modifier::BOLD)),
                Tag::Strikethrough => self.push_style(|s| s.add_modifier(Modifier::CROSSED_OUT)),
                Tag::Link { .. } => self.push_style(|s| s.add_modifier(Modifier::UNDERLINED)),
                Tag::Table(align) => {
                    self.flush();
                    self.table = Some(Table { align, rows: Vec::new(), head_rows: 0, in_head: false });
                }
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => {
                    self.flush();
                    if self.lists.is_empty() {
                        self.blank();
                    }
                }
                TagEnd::Heading(_) => {
                    self.flush();
                    self.pop_style();
                    self.blank();
                }
                TagEnd::BlockQuote(_) => {
                    self.flush();
                    self.quote = self.quote.saturating_sub(1);
                    self.pop_style();
                    self.blank();
                }
                TagEnd::List(_) => {
                    self.flush();
                    self.lists.pop();
                    if self.lists.is_empty() {
                        self.blank();
                    }
                }
                TagEnd::Item => self.flush(),
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link => self.pop_style(),
                _ => {}
            },
            Event::Text(s) => {
                let st = self.style();
                self.buf.push(Span::styled(s.to_string(), st));
            }
            Event::Code(s) => self.buf.push(Span::styled(s.to_string(), code_style())),
            Event::SoftBreak => self.buf.push(Span::styled(" ", self.style())),
            Event::HardBreak => self.flush(),
            Event::Rule => {
                self.flush();
                self.blank();
                self.out.push(Line::from(Span::styled("─".repeat(self.width.min(40)), dim())));
                self.blank();
            }
            Event::TaskListMarker(done) => {
                self.marker = Some(if done { "☑ ".into() } else { "☐ ".into() });
            }
            _ => {}
        }
    }
}

/// Lay styled spans into lines of at most `width`, breaking at spaces (a word longer than a line
/// is cut). `first` starts the first line, `rest` every other.
pub fn wrap_spans(
    spans: Vec<Span<'static>>,
    width: usize,
    first: Vec<Span<'static>>,
    rest: Vec<Span<'static>>,
) -> Vec<Line<'static>> {
    // Words, each a run of styled pieces, and whether a space follows.
    let mut words: Vec<(Vec<Span<'static>>, bool)> = Vec::new();
    let mut cur: Vec<Span<'static>> = Vec::new();
    for s in spans {
        let mut piece = String::new();
        for ch in s.content.chars() {
            if ch == ' ' || ch == '\n' {
                if !piece.is_empty() {
                    cur.push(Span::styled(std::mem::take(&mut piece), s.style));
                }
                if !cur.is_empty() {
                    words.push((std::mem::take(&mut cur), true));
                }
            } else {
                piece.push(ch);
            }
        }
        if !piece.is_empty() {
            cur.push(Span::styled(piece, s.style));
        }
    }
    if !cur.is_empty() {
        words.push((cur, false));
    }
    let wid = |v: &[Span]| v.iter().map(|s| s.content.width()).sum::<usize>();
    let mut out = Vec::new();
    let mut line = first.clone();
    let mut lead = wid(&first);
    let mut used = lead;
    let mut empty = true;
    for (w, _) in words {
        let ww = wid(&w);
        if !empty && used + 1 + ww > width {
            out.push(Line::from(std::mem::take(&mut line)));
            line = rest.clone();
            lead = wid(&rest);
            used = lead;
            empty = true;
        }
        if !empty {
            line.push(Span::raw(" "));
            used += 1;
        }
        if lead + ww > width {
            // A word wider than the line (a long path): cut it.
            for s in w {
                let room = width.saturating_sub(used);
                let c = crate::ui::truncate(&s.content, room);
                used += c.width();
                line.push(Span::styled(c, s.style));
            }
        } else {
            used += ww;
            line.extend(w);
        }
        empty = false;
    }
    if !empty || out.is_empty() {
        out.push(Line::from(line));
    }
    out
}

fn draw_table(t: Table, width: usize) -> Vec<Line<'static>> {
    let ncol = t.rows.iter().map(Vec::len).max().unwrap_or(0);
    if ncol == 0 {
        return Vec::new();
    }
    let cell_w = |c: &Vec<Span>| c.iter().map(|s| s.content.width()).sum::<usize>();
    let mut w: Vec<usize> = (0..ncol)
        .map(|i| t.rows.iter().filter_map(|r| r.get(i)).map(cell_w).max().unwrap_or(0).max(1))
        .collect();
    // Fit: shrink the widest column until the table fits (3 per column for " │ ", plus edges).
    let budget = width.saturating_sub(3 * ncol + 1);
    while w.iter().sum::<usize>() > budget {
        match w.iter().copied().enumerate().max_by_key(|&(_, v)| v) {
            Some((i, m)) if m > 3 => w[i] -= 1,
            _ => break,
        }
    }
    let rule = |l: &str, m: &str, r: &str| {
        let mut s = String::from(l);
        for (i, cw) in w.iter().enumerate() {
            s.push_str(&"─".repeat(cw + 2));
            s.push_str(if i + 1 == ncol { r } else { m });
        }
        Line::from(Span::styled(s, dim()))
    };
    let mut out = vec![rule("┌", "┬", "┐")];
    for (ri, row) in t.rows.iter().enumerate() {
        let mut spans = vec![Span::styled("│", dim())];
        for (i, cw) in w.iter().enumerate() {
            let cell = row.get(i).cloned().unwrap_or_default();
            let txt: String = cell.iter().map(|s| s.content.as_ref()).collect();
            let style = cell.first().map(|s| s.style).unwrap_or(text());
            let shown = crate::ui::truncate(&txt, *cw);
            let pad = cw.saturating_sub(shown.width());
            let (l, r) = match t.align.get(i) {
                Some(Alignment::Right) => (pad, 0),
                Some(Alignment::Center) => (pad / 2, pad - pad / 2),
                _ => (0, pad),
            };
            spans.push(Span::raw(format!(" {}", " ".repeat(l))));
            spans.push(Span::styled(shown, style));
            spans.push(Span::raw(format!("{} ", " ".repeat(r))));
            spans.push(Span::styled("│", dim()));
        }
        out.push(Line::from(spans));
        if t.head_rows > 0 && ri + 1 == t.head_rows {
            out.push(rule("├", "┼", "┤"));
        }
    }
    out.push(rule("└", "┴", "┘"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| l.to_string()).collect()
    }

    #[test]
    fn emphasis_and_code_are_styled_not_printed() {
        let l = render("Only **locus** had commits, on `files`.", 60);
        assert_eq!(plain(&l), ["Only locus had commits, on files."]);
        let bold = l[0].spans.iter().find(|s| s.content == "locus").unwrap();
        assert!(bold.style.add_modifier.contains(Modifier::BOLD));
        assert!(l[0].spans.iter().any(|s| s.content == "files" && s.style == code_style()));
    }

    #[test]
    fn lists_get_bullets_and_hang_their_wrapped_lines() {
        let l = plain(&render("- one item that is long enough to wrap onto a second line\n- two", 24));
        assert!(l[0].starts_with("• one"), "{l:?}");
        assert!(l[1].starts_with("  "), "{l:?}");
        assert!(l.iter().any(|x| x.starts_with("• two")));
        let n = plain(&render("1. first\n2. second", 30));
        assert_eq!(n, ["1. first", "2. second"]);
    }

    #[test]
    fn tables_are_drawn_in_rules() {
        let l = plain(&render("| repo | commits |\n|---|--:|\n| locus | 2 |\n| quast | 0 |", 40));
        assert!(l[0].starts_with('┌') && l.last().unwrap().starts_with('└'), "{l:?}");
        assert!(l.iter().any(|x| x.contains("locus") && x.contains('2')));
        assert!(l.iter().any(|x| x.starts_with('├')));
    }

    #[test]
    fn nothing_is_wider_than_asked() {
        let md = "# Heading\n\nA paragraph with a /very/long/path/that/does/not/fit/anywhere/at/all and more.\n\n> a quote\n\n```\nfn main() {}\n```";
        for l in render(md, 30) {
            assert!(l.width() <= 30, "{l}");
        }
    }
}
