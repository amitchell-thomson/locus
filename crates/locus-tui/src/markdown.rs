//! Markdown drawn exactly as Claude Code draws it for you (9 Nov: "give it the same theme as
//! claude code too, so that everything matches"): a port of your `md-render` plugin
//! (`~/.claude/plugins/cache/alec-mods/md-render`, `hooks/register.tsx`), which draws replies
//! the way your Neovim markdown setup does, in your `catppuccin-mocha` Claude theme.
//!
//! - **Headings:** `#` over a heavy rule; `##` over a thin full rule; `###` over a rule as long
//!   as the title; deeper levels bold, and 5+ italic.
//! - **Tables:** rounded borders, columns fitted to the width (the plugin's water-fill), cells
//!   word-wrapped, and a faint dotted line between body rows once any row wraps.
//! - **Code:** a thin bracket down the left, the language's icon and name on the top corner.
//!   Not syntax-highlighted: the plugin hands that to Claude Code's own highlighter, which has
//!   no Rust equivalent here.
//! - **`---`:** a centred `•   •   •`.
//! - **Lists:** `•` `◦` `▪` by depth, task boxes (done ones dimmed), numbers padded to line up,
//!   hanging indents.
//! - **Quotes:** a `▋` bar down the left; callouts (`[!NOTE]`) titled with their icon.
//! - **Inline:** bold, italic, strikethrough, grey `code`, underlined links.
//!
//! Parsed by pulldown-cmark into blocks, then laid out in a width, so a half-written reply
//! (still streaming) renders as far as it goes.

use crate::ui::widgets::{st, text};
use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

/// The plugin's three colours (Catppuccin Mocha): text, grey (inline code, bullets, the section
/// break, the quote bar) and dim (rules, the code bracket, done tasks).
fn grey() -> Style {
    st("overlay2")
}
fn dimc() -> Style {
    st("overlay0")
}

const BULLETS: [&str; 3] = ["•", "◦", "▪"];

/// Language icons and colours, as the plugin has them (from the Neovim setup's mini.icons).
fn icon(lang: &str) -> Option<(&'static str, &'static str)> {
    let lang = match lang {
        "py" | "python3" => "python",
        "shell" | "shellscript" => "bash",
        "ts" => "typescript",
        "js" => "javascript",
        "yml" => "yaml",
        "md" => "markdown",
        "jsonc" | "json5" => "json",
        "rs" => "rust",
        "golang" => "go",
        "c++" | "hpp" => "cpp",
        "h" => "c",
        "docker" => "dockerfile",
        "tf" => "terraform",
        "hs" => "haskell",
        "kt" => "kotlin",
        "rb" => "ruby",
        "txt" | "plaintext" => "text",
        "makefile" => "make",
        "patch" => "diff",
        "shell_session" | "shell-session" | "sh_session" | "terminal" => "console",
        other => other,
    };
    Some(match lang {
        "python" => ("󰌠", "yellow"),
        "bash" | "sh" | "fish" | "console" => ("", "text"),
        "zsh" => ("", "green"),
        "lua" => ("󰢱", "sapphire"),
        "typescript" => ("󰛦", "sapphire"),
        "tsx" => ("", "blue"),
        "javascript" => ("󰌞", "yellow"),
        "jsx" => ("", "sapphire"),
        "json" => ("󰘦", "yellow"),
        "yaml" => ("", "mauve"),
        "toml" => ("", "peach"),
        "sql" => ("󰆼", "text"),
        "rust" => ("󱘗", "peach"),
        "go" => ("󰟓", "sapphire"),
        "c" => ("󰙱", "blue"),
        "cpp" => ("󰙲", "sapphire"),
        "markdown" => ("󰍔", "text"),
        "html" => ("󰌝", "peach"),
        "css" => ("󰌜", "sapphire"),
        "diff" => ("󰦓", "red"),
        "dockerfile" => ("󰡨", "blue"),
        "tex" | "latex" => ("", "green"),
        "r" => ("󰟔", "blue"),
        "julia" => ("", "mauve"),
        "swift" => ("󰛥", "peach"),
        "java" => ("󰬷", "peach"),
        "nix" => ("󱄅", "sapphire"),
        "vim" => ("", "green"),
        "make" => ("󱁤", "text"),
        "ruby" => ("󰴭", "red"),
        "haskell" => ("󰲒", "mauve"),
        "kotlin" => ("󱈙", "blue"),
        "xml" => ("󰗀", "peach"),
        "csv" => ("", "green"),
        "ini" => ("󰯂", "sapphire"),
        "conf" => ("󰒓", "text"),
        "text" => ("󰦪", "yellow"),
        "matlab" => ("󰿈", "peach"),
        _ => return None,
    })
}

// --- blocks ---------------------------------------------------------------------------------------

type Spans = Vec<Span<'static>>;

enum Item {
    Bullet,
    Number(u64),
    Task(bool),
}

enum Block {
    Heading(u8, Spans),
    /// Lines split at hard breaks.
    Para(Vec<Spans>),
    Code(String, String),
    Rule,
    /// (depth, marker, paragraphs)
    List(Vec<(usize, Item, Vec<Spans>)>),
    /// Rows: an empty row is a gap; a callout's title comes first.
    Quote(Vec<Spans>),
    Table(Vec<Alignment>, Vec<Vec<Spans>>),
}

struct Builder {
    blocks: Vec<Block>,
    spans: Spans,
    lines: Vec<Spans>,
    styles: Vec<Style>,
    lists: Vec<Option<u64>>,
    /// The list block being built; while an item is open its text goes to the last item.
    list: Vec<(usize, Item, Vec<Spans>)>,
    item_open: bool,
    quote: usize,
    quote_rows: Vec<Spans>,
    code: Option<(String, String)>,
    table: Option<(Vec<Alignment>, Vec<Vec<Spans>>, bool)>,
    heading: Option<u8>,
}

impl Builder {
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

    /// The text gathered so far goes where it belongs: a quote, an open list item, a heading or
    /// a paragraph.
    fn end_text(&mut self) {
        let spans = std::mem::take(&mut self.spans);
        let mut lines = std::mem::take(&mut self.lines);
        if !spans.is_empty() {
            lines.push(spans);
        }
        if lines.is_empty() {
            return;
        }
        if self.quote > 0 {
            if !self.quote_rows.is_empty() && !self.item_open {
                self.quote_rows.push(Vec::new());
            }
            self.quote_rows.extend(lines);
        } else if self.item_open {
            if let Some(it) = self.list.last_mut() {
                it.2.extend(lines);
            }
        } else if let Some(level) = self.heading {
            self.blocks.push(Block::Heading(level, lines.concat()));
        } else {
            self.blocks.push(Block::Para(lines));
        }
    }

    fn event(&mut self, ev: Event) {
        if let Some((_, rows, in_head)) = &mut self.table {
            match ev {
                Event::End(TagEnd::Table) => {
                    if let Some((a, r, _)) = self.table.take() {
                        self.blocks.push(Block::Table(a, r));
                    }
                }
                Event::Start(Tag::TableHead) => {
                    *in_head = true;
                    rows.push(Vec::new());
                }
                Event::End(TagEnd::TableHead) => *in_head = false,
                Event::Start(Tag::TableRow) => rows.push(Vec::new()),
                Event::Start(Tag::TableCell) => {
                    if let Some(r) = rows.last_mut() {
                        r.push(Vec::new());
                    }
                }
                Event::Text(s) => {
                    let style = if *in_head { text().add_modifier(Modifier::BOLD) } else { text() };
                    if let Some(c) = rows.last_mut().and_then(|r| r.last_mut()) {
                        c.push(Span::styled(s.to_string(), style));
                    }
                }
                Event::Code(s) => {
                    if let Some(c) = rows.last_mut().and_then(|r| r.last_mut()) {
                        c.push(Span::styled(s.to_string(), grey()));
                    }
                }
                _ => {}
            }
            return;
        }
        if let Some((_, body)) = &mut self.code {
            match ev {
                Event::Text(s) => body.push_str(&s),
                Event::End(TagEnd::CodeBlock) => {
                    if let Some((lang, body)) = self.code.take() {
                        self.blocks.push(Block::Code(lang, body));
                    }
                }
                _ => {}
            }
            return;
        }
        match ev {
            Event::Start(tag) => match tag {
                Tag::Heading { level, .. } => {
                    self.end_text();
                    let n = match level {
                        HeadingLevel::H1 => 1,
                        HeadingLevel::H2 => 2,
                        HeadingLevel::H3 => 3,
                        HeadingLevel::H4 => 4,
                        HeadingLevel::H5 => 5,
                        HeadingLevel::H6 => 6,
                    };
                    self.heading = Some(n);
                    self.push_style(|s| {
                        let s = s.add_modifier(Modifier::BOLD);
                        if n >= 5 { s.add_modifier(Modifier::ITALIC) } else { s }
                    });
                }
                Tag::BlockQuote(_) => {
                    self.end_text();
                    self.quote += 1;
                }
                Tag::CodeBlock(kind) => {
                    self.end_text();
                    let lang = match kind {
                        CodeBlockKind::Fenced(l) => l.split_whitespace().next().unwrap_or("").to_lowercase(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    self.code = Some((lang, String::new()));
                }
                Tag::List(start) => {
                    self.end_text();
                    self.lists.push(start);
                }
                Tag::Item => {
                    self.end_text();
                    let depth = self.lists.len().saturating_sub(1);
                    let marker = match self.lists.last_mut() {
                        Some(Some(n)) => {
                            let m = Item::Number(*n);
                            *n += 1;
                            m
                        }
                        _ => Item::Bullet,
                    };
                    if self.quote > 0 {
                        // Inside a quote, an item is a row with its marker.
                        self.spans.push(match marker {
                            Item::Number(n) => Span::styled(format!("{n}. "), text()),
                            _ => Span::styled("• ", grey()),
                        });
                    } else {
                        self.list.push((depth, marker, Vec::new()));
                    }
                    self.item_open = true;
                }
                Tag::Emphasis => self.push_style(|s| s.add_modifier(Modifier::ITALIC)),
                Tag::Strong => self.push_style(|s| s.add_modifier(Modifier::BOLD)),
                Tag::Strikethrough => self.push_style(|s| s.add_modifier(Modifier::CROSSED_OUT)),
                Tag::Link { .. } => self.push_style(|s| s.add_modifier(Modifier::UNDERLINED)),
                Tag::Table(align) => {
                    self.end_text();
                    self.table = Some((align, Vec::new(), false));
                }
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => self.end_text(),
                TagEnd::Heading(_) => {
                    self.end_text();
                    self.heading = None;
                    self.pop_style();
                }
                TagEnd::BlockQuote(_) => {
                    self.end_text();
                    self.quote = self.quote.saturating_sub(1);
                    if self.quote == 0 {
                        let rows = std::mem::take(&mut self.quote_rows);
                        if !rows.is_empty() {
                            self.blocks.push(Block::Quote(callout(rows)));
                        }
                    }
                }
                TagEnd::Item => {
                    self.end_text();
                    self.item_open = false;
                }
                TagEnd::List(_) => {
                    self.end_text();
                    self.lists.pop();
                    // A nested list's parent item is still open after it.
                    self.item_open = !self.lists.is_empty();
                    if self.lists.is_empty() && self.quote == 0 {
                        let items = std::mem::take(&mut self.list);
                        if !items.is_empty() {
                            self.blocks.push(Block::List(items));
                        }
                    }
                }
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link => self.pop_style(),
                _ => {}
            },
            Event::Text(s) => {
                let st = self.style();
                self.spans.push(Span::styled(s.to_string(), st));
            }
            Event::Code(s) => {
                let s2 = grey().add_modifier(self.style().add_modifier);
                self.spans.push(Span::styled(s.to_string(), s2));
            }
            Event::SoftBreak => {
                // A callout's `[!NOTE] title` is its own line, not the start of the paragraph.
                let callout_line = self.quote > 0
                    && self.quote_rows.is_empty()
                    && self.lines.is_empty()
                    // pulldown-cmark hands `[!NOTE]` over in pieces (`[`, `!NOTE`, `]`).
                    && self.spans.iter().map(|s| s.content.as_ref()).collect::<String>().starts_with("[!");
                if callout_line {
                    let l = std::mem::take(&mut self.spans);
                    self.lines.push(l);
                } else {
                    let st = self.style();
                    self.spans.push(Span::styled(" ", st));
                }
            }
            Event::HardBreak => {
                let l = std::mem::take(&mut self.spans);
                self.lines.push(l);
            }
            Event::Rule => {
                self.end_text();
                self.blocks.push(Block::Rule);
            }
            Event::TaskListMarker(done) => {
                if let Some(it) = self.list.last_mut() {
                    it.1 = Item::Task(done);
                }
            }
            _ => {}
        }
    }
}

/// A quote whose first line is `[!NOTE] title` is a callout: its icon and title, bold.
fn callout(mut rows: Vec<Spans>) -> Vec<Spans> {
    let first: String = rows.first().map(|r| r.iter().map(|s| s.content.as_ref()).collect()).unwrap_or_default();
    let Some(rest) = first.strip_prefix("[!") else { return rows };
    let Some(end) = rest.find(']') else { return rows };
    let kind = rest[..end].to_lowercase();
    let title = rest[end + 1..].trim_start_matches(['+', '-']).trim();
    let icon = match kind.as_str() {
        "tip" | "hint" => "󰌶",
        "important" => "󰅾",
        "warning" => "󰀪",
        "caution" | "danger" => "󰳦",
        "example" => "󰉹",
        "quote" => "󱆨",
        "todo" => "󰗡",
        "success" => "󰄬",
        "question" => "󰘥",
        "bug" => "󰨰",
        _ => "󰋽",
    };
    let title = if title.is_empty() {
        let mut c = kind.chars();
        c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
    } else {
        title.to_string()
    };
    rows[0] = vec![Span::styled(format!("{icon} {title}"), text().add_modifier(Modifier::BOLD))];
    rows
}

// --- drawing --------------------------------------------------------------------------------------

/// `md` as lines no wider than `width` (the reply's ⏺ gutter is the caller's).
pub fn render(md: &str, width: usize) -> Vec<Line<'static>> {
    let w = width.max(20);
    let mut b = Builder {
        blocks: Vec::new(),
        spans: Vec::new(),
        lines: Vec::new(),
        styles: vec![text()],
        lists: Vec::new(),
        list: Vec::new(),
        item_open: false,
        quote: 0,
        quote_rows: Vec::new(),
        code: None,
        table: None,
        heading: None,
    };
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    for ev in Parser::new_ext(md, opts) {
        b.event(ev);
    }
    // A reply still streaming may end inside a block: draw what there is.
    b.end_text();
    if let Some((lang, body)) = b.code.take() {
        b.blocks.push(Block::Code(lang, body));
    }
    if !b.list.is_empty() {
        let items = std::mem::take(&mut b.list);
        b.blocks.push(Block::List(items));
    }
    if let Some((a, r, _)) = b.table.take() {
        b.blocks.push(Block::Table(a, r));
    }
    let mut out: Vec<Line<'static>> = Vec::new();
    for (k, block) in b.blocks.into_iter().enumerate() {
        // One blank line between blocks; two before a `#` or `##` heading (the plugin's gaps).
        let gap = match (&block, k) {
            (_, 0) => 0,
            (Block::Heading(l, _), _) if *l <= 2 => 2,
            _ => 1,
        };
        out.extend((0..gap).map(|_| Line::default()));
        out.extend(match block {
            Block::Heading(level, spans) => heading(level, spans, w),
            Block::Para(lines) => lines.into_iter().flat_map(|l| wrap_spans(l, w, vec![], vec![])).collect(),
            Block::Code(lang, body) => code(&lang, &body, w),
            Block::Rule => {
                let mark = "•   •   •";
                let pad = " ".repeat(w.saturating_sub(mark.width()) / 2);
                vec![Line::from(Span::styled(format!("{pad}{mark}"), grey()))]
            }
            Block::List(items) => list(items, w),
            Block::Quote(rows) => quote(rows, w),
            Block::Table(align, rows) => table(&align, rows, w),
        });
    }
    out
}

fn heading(level: u8, spans: Spans, w: usize) -> Vec<Line<'static>> {
    let title_w: usize = spans.iter().map(|s| s.content.width()).sum();
    let mut out = wrap_spans(spans, w, vec![], vec![]);
    match level {
        1 => out.push(Line::from(Span::styled("━".repeat(w), text()))),
        2 => out.push(Line::from(Span::styled("─".repeat(w), dimc()))),
        3 => out.push(Line::from(Span::styled("─".repeat(title_w.min(w)), dimc()))),
        _ => {}
    }
    out
}

fn code(lang: &str, body: &str, w: usize) -> Vec<Line<'static>> {
    let avail = w.saturating_sub(2).max(8);
    let mut top = vec![Span::styled("╭─", dimc())];
    if !lang.is_empty() {
        top.push(Span::raw(" "));
        if let Some((i, col)) = icon(lang) {
            top.push(Span::styled(format!("{i} "), st(col)));
        }
        top.push(Span::styled(lang.to_string(), dimc()));
    }
    let mut out = vec![Line::from(top)];
    let row = |s: String| Line::from(vec![Span::styled("│ ", dimc()), Span::styled(s, text())]);
    for raw in body.replace('\t', "    ").trim_end_matches('\n').split('\n') {
        // Hard-wrapped, so each row of code is one row under the bracket; a continuation keeps
        // the line's indent.
        let ind: String = raw.chars().take_while(|c| *c == ' ').take(avail / 2).collect();
        let mut cur = String::new();
        let mut used = 0;
        for ch in raw.chars() {
            let cw = ch.to_string().width();
            if used + cw > avail {
                out.push(row(std::mem::take(&mut cur)));
                cur.push_str(&ind);
                used = ind.width();
            }
            cur.push(ch);
            used += cw;
        }
        out.push(row(cur));
    }
    out.push(Line::from(Span::styled("╰─", dimc())));
    out
}

fn list(items: Vec<(usize, Item, Vec<Spans>)>, w: usize) -> Vec<Line<'static>> {
    // Numbers at one depth share a width, so their text lines up.
    let mut num_w = std::collections::HashMap::new();
    for (d, m, _) in &items {
        if let Item::Number(n) = m {
            let e = num_w.entry(*d).or_insert(0usize);
            *e = (*e).max(format!("{n}.").width());
        }
    }
    let mut out = Vec::new();
    for (depth, marker, paras) in items {
        let (mark, done) = match marker {
            Item::Task(true) => (Span::styled("󰱒", dimc()), true),
            Item::Task(false) => (Span::styled("󰄱", text()), false),
            Item::Number(n) => {
                let nw = num_w.get(&depth).copied().unwrap_or(2);
                (Span::styled(format!("{:>nw$}", format!("{n}.")), text()), false)
            }
            Item::Bullet => (Span::styled(BULLETS[depth % BULLETS.len()], grey()), false),
        };
        let indent = " ".repeat(depth * 2);
        let rest = vec![Span::raw(format!("{indent}{}", " ".repeat(mark.content.width() + 1)))];
        let first = vec![Span::raw(indent), mark, Span::raw(" ")];
        if paras.is_empty() {
            out.push(Line::from(first));
            continue;
        }
        for (i, p) in paras.into_iter().enumerate() {
            let p: Spans = if done {
                p.into_iter().map(|s| Span::styled(s.content, s.style.patch(dimc()))).collect()
            } else {
                p
            };
            let lead = if i == 0 { first.clone() } else { rest.clone() };
            out.extend(wrap_spans(p, w, lead, rest.clone()));
        }
    }
    out
}

fn quote(mut rows: Vec<Spans>, w: usize) -> Vec<Line<'static>> {
    while rows.last().is_some_and(Vec::is_empty) {
        rows.pop();
    }
    let bar = || vec![Span::styled("▋ ", grey())];
    let mut out = Vec::new();
    for r in rows {
        if r.is_empty() {
            out.push(Line::from(bar()));
        } else {
            out.extend(wrap_spans(r, w, bar(), bar()));
        }
    }
    out
}

/// The plugin's water-fill: a column gets what it asks for when that fits its share; what is
/// left is split evenly among the rest.
fn waterfill(demand: &[usize], budget: usize) -> Vec<usize> {
    let mut out = vec![0; demand.len()];
    let mut open: Vec<usize> = (0..demand.len()).collect();
    let mut left = budget;
    let mut changed = true;
    while changed && !open.is_empty() {
        changed = false;
        let share = left / open.len();
        let mut keep = Vec::new();
        for k in open {
            if demand[k] <= share {
                out[k] = demand[k];
                left -= demand[k];
                changed = true;
            } else {
                keep.push(k);
            }
        }
        open = keep;
    }
    let n = open.len().max(1);
    for (i, &k) in open.iter().enumerate() {
        out[k] = left / n + usize::from(i < left % n);
    }
    out
}

fn fit(natural: &[usize], longest: &[usize], budget: usize) -> (Vec<usize>, bool) {
    if natural.iter().sum::<usize>() <= budget {
        return (natural.to_vec(), false);
    }
    let cap = (budget / (2 * natural.len())).max(8);
    let floor: Vec<usize> = natural.iter().zip(longest).map(|(n, l)| (*n).min(*l).min(cap)).collect();
    let used: usize = floor.iter().sum();
    if used > budget {
        return (waterfill(natural, budget), true);
    }
    let want: Vec<usize> = natural.iter().zip(&floor).map(|(n, f)| n - f).collect();
    let extra = waterfill(&want, budget - used);
    (floor.iter().zip(extra).map(|(f, e)| f + e).collect(), true)
}

fn table(align: &[Alignment], rows: Vec<Vec<Spans>>, w: usize) -> Vec<Line<'static>> {
    let ncol = rows.iter().map(Vec::len).max().unwrap_or(0).max(align.len());
    if ncol == 0 {
        return Vec::new();
    }
    let word_widths = |c: &Spans| -> Vec<usize> {
        c.iter().flat_map(|s| s.content.split_whitespace().map(|x| x.width()).collect::<Vec<_>>()).collect()
    };
    let mut natural = vec![1; ncol];
    let mut longest = vec![1; ncol];
    for r in &rows {
        for (k, c) in r.iter().enumerate() {
            let ws = word_widths(c);
            natural[k] = natural[k].max(ws.iter().sum::<usize>() + ws.len().saturating_sub(1));
            longest[k] = longest[k].max(ws.iter().copied().max().unwrap_or(1));
        }
    }
    let budget = w.saturating_sub(3 * ncol + 1);
    if budget < 3 * ncol {
        // Too narrow to draw a table: its cells as plain rows.
        return rows.into_iter().map(|r| Line::from(r.into_iter().flatten().collect::<Vec<_>>())).collect();
    }
    let (widths, wrapped) = fit(&natural, &longest, budget);
    let rule = |l: &str, m: &str, r: &str| {
        let mid: Vec<String> = widths.iter().map(|cw| "─".repeat(cw + 2)).collect();
        Line::from(Span::styled(format!("{l}{}{r}", mid.join(m)), dimc()))
    };
    let mut out = vec![rule("╭", "┬", "╮")];
    let nrows = rows.len();
    for (ri, row) in rows.into_iter().enumerate() {
        let cells: Vec<Vec<Line<'static>>> = (0..ncol)
            .map(|k| wrap_spans(row.get(k).cloned().unwrap_or_default(), widths[k].max(1), vec![], vec![]))
            .collect();
        let height = cells.iter().map(Vec::len).max().unwrap_or(1);
        for n in 0..height {
            let mut spans = vec![Span::styled("│", dimc())];
            for (k, c) in cells.iter().enumerate() {
                let seg = c.get(n).cloned().unwrap_or_default();
                let gap = widths[k].saturating_sub(seg.width());
                let left = match align.get(k) {
                    Some(Alignment::Right) => gap,
                    Some(Alignment::Center) => gap / 2,
                    _ => 0,
                };
                spans.push(Span::raw(format!(" {}", " ".repeat(left))));
                spans.extend(seg.spans);
                spans.push(Span::raw(format!("{} ", " ".repeat(gap - left))));
                spans.push(Span::styled("│", dimc()));
            }
            out.push(Line::from(spans));
        }
        if ri == 0 {
            out.push(rule("├", "┼", "┤"));
        } else if ri + 1 < nrows && wrapped {
            // A faint dotted line inside each cell, the column bars unbroken.
            let mut soft = vec![Span::styled("│", dimc())];
            for cw in &widths {
                soft.push(Span::styled(format!(" {} ", "┈".repeat(*cw)), dimc()));
                soft.push(Span::styled("│", dimc()));
            }
            out.push(Line::from(soft));
        }
    }
    out.push(rule("╰", "┴", "╯"));
    out
}

/// Lay styled spans into lines of at most `width`, breaking at spaces (a word longer than a line
/// is split across lines). `first` starts the first line, `rest` every other.
pub fn wrap_spans(spans: Spans, width: usize, first: Spans, rest: Spans) -> Vec<Line<'static>> {
    let mut words: Vec<Spans> = Vec::new();
    let mut cur: Spans = Vec::new();
    for s in spans {
        let mut piece = String::new();
        for ch in s.content.chars() {
            if ch.is_whitespace() {
                if !piece.is_empty() {
                    cur.push(Span::styled(std::mem::take(&mut piece), s.style));
                }
                if !cur.is_empty() {
                    words.push(std::mem::take(&mut cur));
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
        words.push(cur);
    }
    let wid = |v: &[Span]| v.iter().map(|s| s.content.width()).sum::<usize>();
    let rest_w = wid(&rest);
    let mut out = Vec::new();
    let mut line = first.clone();
    let mut used = wid(&first);
    let mut empty = true;
    for word in words {
        let ww = wid(&word);
        if !empty && used + 1 + ww > width {
            out.push(Line::from(std::mem::replace(&mut line, rest.clone())));
            used = rest_w;
            empty = true;
        }
        if !empty {
            line.push(Span::raw(" "));
            used += 1;
        }
        if used + ww <= width {
            used += ww;
            line.extend(word);
        } else {
            // Longer than a whole line (a path, a URL): split across lines.
            for s in word {
                let mut buf = String::new();
                for ch in s.content.chars() {
                    let cw = ch.to_string().width();
                    if used + cw > width && used > rest_w {
                        line.push(Span::styled(std::mem::take(&mut buf), s.style));
                        out.push(Line::from(std::mem::replace(&mut line, rest.clone())));
                        used = rest_w;
                    }
                    buf.push(ch);
                    used += cw;
                }
                line.push(Span::styled(buf, s.style));
            }
        }
        empty = false;
    }
    if !empty || out.is_empty() {
        out.push(Line::from(line));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| l.to_string()).collect()
    }

    #[test]
    fn inline_marks_are_styles_and_code_is_grey() {
        let l = render("Only **locus** had commits, on `files`.", 60);
        assert_eq!(plain(&l), ["Only locus had commits, on files."]);
        assert!(l[0].spans.iter().any(|s| s.content == "locus" && s.style.add_modifier.contains(Modifier::BOLD)));
        assert!(l[0].spans.iter().any(|s| s.content == "files" && s.style.fg == grey().fg));
    }

    #[test]
    fn headings_sit_over_the_plugins_rules() {
        let l = plain(&render("# One\n\n## Two\n\n### Three", 30));
        assert_eq!(l[0], "One");
        assert_eq!(l[1], "━".repeat(30));
        assert_eq!(&l[2..4], ["", ""], "two blank lines before ##");
        assert_eq!(l[5], "─".repeat(30));
        assert_eq!(l[8], "─────", "### is ruled as long as its title");
    }

    #[test]
    fn bullets_go_by_depth_and_wrapped_lines_hang() {
        let l = plain(&render("- one item that is long enough to wrap onto a second line\n  - nested\n- two", 24));
        assert!(l[0].starts_with("• one"), "{l:?}");
        assert!(l[1].starts_with("  "), "{l:?}");
        assert!(l.iter().any(|x| x.starts_with("  ◦ nested")), "{l:?}");
        assert!(l.iter().any(|x| x.starts_with("• two")), "{l:?}");
        let n = plain(&render("9. ninth\n10. tenth", 30));
        assert_eq!(n, [" 9. ninth", "10. tenth"], "numbers line up");
    }

    #[test]
    fn tables_are_rounded_and_wrap() {
        let md = "| repo | note |\n|---|---|\n| locus | a long note that has to wrap in a narrow table |\n| quast | short |";
        let l = plain(&render(md, 34));
        assert!(l[0].starts_with('╭') && l.last().is_some_and(|x| x.starts_with('╰')), "{l:?}");
        assert!(l.iter().any(|x| x.contains('┈')), "a dotted line once a row wraps: {l:?}");
        assert!(l.iter().all(|x| x.width() <= 34), "{l:?}");
    }

    #[test]
    fn code_is_bracketed_with_its_language() {
        let l = plain(&render("```rust\nfn main() {}\n```", 40));
        assert!(l[0].starts_with("╭─") && l[0].ends_with("rust"), "{l:?}");
        assert_eq!(l[1], "│ fn main() {}");
        assert_eq!(l[2], "╰─");
    }

    #[test]
    fn breaks_quotes_and_callouts() {
        let l = plain(&render("a\n\n---\n\nb", 21));
        assert!(l[2].trim() == "•   •   •" && l[2].starts_with(' '), "{l:?}");
        let q = plain(&render("> [!NOTE]\n> remember this", 40));
        assert!(q[0].starts_with("▋ 󰋽 Note"), "{q:?}");
        assert_eq!(q.last().map(String::as_str), Some("▋ remember this"), "{q:?}");
    }

    #[test]
    fn nothing_is_wider_than_asked() {
        let md = "# Heading\n\nA paragraph with a /very/long/path/that/does/not/fit/anywhere/at/all and more.\n\n> a quote\n\n```\nfn main() { let x = 1; let y = 2; let z = 3; }\n```\n\n- [x] done\n- [ ] not";
        for l in render(md, 30) {
            assert!(l.width() <= 30, "{l}");
        }
    }
}
