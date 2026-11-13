//! The chat (files-and-chat M2): its bar at the foot of Home and its own view on `i`.
//!
//! The bar is a rule and one line: what you are typing, what the running turn is doing, or the
//! last thing it said. The view lists your conversations on the left, newest first, and shows
//! the selected one on the right, every change it made marked (C2). Tab keeps its meaning
//! everywhere (9 Nov: Tab behaving differently here would be wrong), so the view has its own
//! key, `i`, which nothing else used.

use crate::app::{App, Mode};
use crate::chat::Entry;
use crate::screen::ScreenId;
use crate::ui::widgets::*;
use crate::ui::{ask_rule, esc_back, frame, title};
use locus_theme::c;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

/// The chat view's own keys, after the shared table (cleanup C4): there `+` is a new
/// conversation (it was `n`), `e` renames it (it was `r`), and y n answer a fact it offers to
/// remember.
pub const KEYS: &[(&str, &str)] = &[
    ("↑ ↓ j k", "scroll (the mouse wheel too)"),
    ("← →", "your conversations, and back"),
    ("⇟ ⇞ ^d ^u", "a page down or up"),
    ("Home End G", "the top; the end"),
    ("␣ ⏎", "type into it"),
    ("o", "open it in full Claude Code"),
];

/// The fact waiting for your y/n in a conversation, if any (H2.3).
pub fn pending(app: &App, id: Option<&str>) -> Option<crate::chat::Remember> {
    id.and_then(|i| app.chat.pending(i))
}

/// `y` or `n` on a waiting fact: true when the key was the answer.
pub fn answer_key(app: &mut App, id: Option<&str>, k: KeyEvent) -> bool {
    let yes = match k.code {
        KeyCode::Char('y') => true,
        KeyCode::Char('n') => false,
        _ => return false,
    };
    let Some(r) = pending(app, id) else { return false };
    app.chat.answer(&r, yes);
    if !yes {
        app.note = Some(format!("dropped: {}", r.line()));
    }
    true
}

/// The question, as a line: `remember: "jump · outcome = no offer"? y/n`.
fn ask_line(r: &crate::chat::Remember, w: usize) -> Line<'static> {
    Line::from(vec![
        Span::styled("   remember: ", st("yellow")),
        Span::styled(cut(&format!("\"{}\"", r.line()), w.saturating_sub(22)), text()),
        Span::styled("? ", st("yellow")),
        Span::styled("y", keyst()),
        Span::styled("/", dim()),
        Span::styled("n", keyst()),
    ])
}

/// Scroll the open conversation by `d` lines (up is positive), kept inside what there is.
pub fn scroll(app: &mut App, d: isize) {
    let max = app.chat.max_scroll.get();
    app.chat.scroll = (app.chat.scroll as isize + d).clamp(0, max as isize) as usize;
}

/// The conversation the view shows: the selected row; a new one when there are none or you
/// pressed `n`.
pub fn viewed(app: &App) -> Option<String> {
    if app.chat_new {
        return None;
    }
    app.chat.list().get(app.sel(' ')).map(|c| c.id.clone())
}

/// Send what was typed, in the conversation it was typed into.
pub fn send(app: &mut App, text: String, into: Option<String>) {
    app.chat.select(into);
    app.chat_new = false;
    match app.chat.send(&text) {
        Ok(()) => {
            // The conversation it went to heads the list now: keep it selected.
            app.sel.insert((ScreenId::Chat, ' '), 0);
        }
        Err(e) => {
            app.chat_draft = text;
            app.note = Some(format!("not sent: {e} · ␣ brings it back"));
        }
    }
}

/// The chat view's keys. The arrows scroll the open conversation, as reading it is what the
/// view is for (9 Nov: "in the chat there is no way to scroll": they used to choose a
/// conversation, with scrolling on PgUp, which a Mac keyboard does not have, and on Shift-↑);
/// ← moves them to the list of conversations, → back.
pub fn key(app: &mut App, k: KeyEvent) -> bool {
    // A fact waiting for an answer takes y and n.
    let open = viewed(app);
    if answer_key(app, open.as_deref(), k) {
        return true;
    }
    let n = app.chat.list().len();
    let page = 12;
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    match k.code {
        KeyCode::Left => app.chat_list = true,
        KeyCode::Right => app.chat_list = false,
        KeyCode::Up | KeyCode::Char('k') if app.chat_list => {
            if app.chat_new {
                app.chat_new = false;
            } else {
                app.move_sel(' ', n, -1);
            }
            app.chat.scroll = 0;
        }
        KeyCode::Down | KeyCode::Char('j') if app.chat_list => {
            app.chat_new = false;
            app.move_sel(' ', n, 1);
            app.chat.scroll = 0;
        }
        KeyCode::Up | KeyCode::Char('k') => scroll(app, 2),
        KeyCode::Down | KeyCode::Char('j') => scroll(app, -2),
        KeyCode::PageUp | KeyCode::Char('u') if k.code == KeyCode::PageUp || ctrl => scroll(app, page),
        KeyCode::PageDown | KeyCode::Char('d') if k.code == KeyCode::PageDown || ctrl => scroll(app, -page),
        // Home is the top (g was too, until g became "go to", C4).
        KeyCode::Home => scroll(app, isize::MAX / 4),
        KeyCode::Char('G') | KeyCode::End => app.chat.scroll = 0,
        KeyCode::Char(' ') | KeyCode::Enter => {
            let into = viewed(app);
            app.chat.select(into);
            app.mode = Mode::Chat(std::mem::take(&mut app.chat_draft));
        }
        // + a new conversation (it was n, which is "no" in the table, C4).
        KeyCode::Char('+') => {
            app.chat_new = true;
            app.chat.select(None);
            app.mode = Mode::Chat(String::new());
        }
        KeyCode::Char('o') => match viewed(app) {
            Some(id) => {
                app.note = Some(match crate::chat::open_in_claude(&id) {
                    Ok(()) => "opened in full Claude Code: a `chat` window in the locus session".into(),
                    Err(e) => format!("could not open it: {e}"),
                })
            }
            None => app.note = Some("nothing to open yet: say something first".into()),
        },
        // e renames it (it was r, C4).
        KeyCode::Char('e') => match viewed(app).and_then(|id| app.chat.get(&id).map(|c| c.name.clone())) {
            Some(name) => app.mode = Mode::ChatName(name),
            None => app.note = Some("nothing to rename yet".into()),
        },
        _ => return false,
    }
    true
}

/// The bar at the foot of Home: a rule, then one line.
pub fn draw_bar(f: &mut Frame, area: Rect, app: &App) {
    let typing = matches!(app.mode, Mode::Chat(_));
    let name = app
        .chat
        .current()
        .map(|c| c.name.clone())
        .unwrap_or_else(|| "new conversation".into());
    let long = app.chat.index.current.as_deref().is_some_and(|id| app.chat.is_long(id));
    let tail = if long {
        format!("· {name} · long, + in i for a fresh one")
    } else {
        format!("· {name}")
    };
    let rule = ask_rule(
        "chat",
        typing,
        &[("␣", "ask"), ("i", "conversations")],
        &tail,
        area.width,
    );
    let w = usize::from(area.width).saturating_sub(5);
    let body = if let Mode::Chat(q) = &app.mode {
        let shown = tail_cut(q, w.saturating_sub(1));
        vec![
            Span::styled("   ❯ ", st("blue")),
            Span::styled(shown, text()),
            Span::styled("▏", st("blue")),
        ]
    } else if app.chat.busy.is_some() {
        let said = app.chat.live.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("");
        let what = if said.is_empty() {
            app.chat.status.clone()
        } else {
            said.to_string()
        };
        vec![
            Span::styled(format!("   {} ", spin(app.frame)), st("blue")),
            Span::styled(cut(&what, w.saturating_sub(2)), text()),
        ]
    } else if let Some(e) = &app.chat.error {
        vec![Span::styled("   ! ", st("red")), Span::styled(cut(e, w), text())]
    } else if let Some(r) = pending(app, app.chat.index.current.as_deref()) {
        return f.render_widget(Paragraph::new(vec![rule, ask_line(&r, w)]), area);
    } else if let Some(r) = app.chat.index.current.as_deref().and_then(|id| app.chat.last_reply(id)) {
        // Its first line, rendered as the view renders it, then dimmed.
        let first = crate::markdown::render(&r, 400)
            .into_iter()
            .find(|l| l.width() > 0)
            .unwrap_or_default();
        let mut v = vec![Span::styled("   ", dim())];
        v.extend(
            first
                .spans
                .into_iter()
                .map(|s| Span::styled(s.content, s.style.patch(dim()))),
        );
        return f.render_widget(
            Paragraph::new(vec![rule, row(v, vec![], area.width.saturating_sub(1))]),
            area,
        );
    } else {
        vec![
            Span::styled("   ␣", keyst()),
            Span::styled(" to ask anything that needs your whole life as context · ", dim()),
            Span::styled("i", keyst()),
            Span::styled(" for your conversations", dim()),
        ]
    };
    f.render_widget(Paragraph::new(vec![rule, Line::from(body)]), area);
}

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let inner = frame(f, area, title("chat", "your whole life as context"), Some(esc_back()));
    let [list_r, bar_x, conv_r] =
        Layout::horizontal([Constraint::Length(34), Constraint::Length(3), Constraint::Min(20)]).areas(inner);
    draw_list(f, list_r, app);
    // The divider between your conversations and the open one (9 Nov), joined to the rules on
    // either side as Home's careers | studies divider is.
    let mut div = vec![Line::from(Span::styled("─┐ ", st("surface2")))];
    div.extend((1..bar_x.height.saturating_sub(2)).map(|_| Line::from(Span::styled(" │ ", st("surface2")))));
    div.push(Line::from(Span::styled(" ├─", st("surface2"))));
    div.push(Line::from(Span::styled(" │ ", st("surface2"))));
    f.render_widget(Paragraph::new(div), bar_x);
    let [talk_r, bar_r] = Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).areas(conv_r);
    draw_conversation(f, talk_r, app);
    draw_input(f, bar_r, app);
}

fn draw_list(f: &mut Frame, area: Rect, app: &App) {
    // Lit while the arrows move through it (←).
    let mut lines = vec![rule("conversations", "", None, app.chat_list, area.width)];
    let sel = (!app.chat_new).then(|| app.sel(' '));
    if app.chat_new || app.chat.list().is_empty() {
        lines.push(row(
            vec![pointer(0, Some(0)), Span::styled("new conversation", st("blue"))],
            vec![],
            area.width,
        ));
    }
    let now = chrono::Local::now();
    for (i, c) in app.chat.list().into_iter().enumerate() {
        let when = chrono::DateTime::from_timestamp(c.updated, 0)
            .map(|t| t.with_timezone(&chrono::Local))
            .map(|t| {
                if t.date_naive() == now.date_naive() {
                    t.format("%H:%M").to_string()
                } else if (now - t).num_days() < 6 {
                    t.format("%a").to_string()
                } else {
                    t.format("%-d %b").to_string()
                }
            })
            .unwrap_or_default();
        let running = app.chat.busy.as_deref() == Some(c.id.as_str());
        let mut left = vec![pointer(i, sel)];
        if running {
            left.push(Span::styled(format!("{} ", spin(app.frame)), st("blue")));
        }
        left.push(Span::styled(c.name.clone(), text()));
        lines.push(row(left, vec![Span::styled(when, dim())], area.width));
    }
    f.render_widget(Paragraph::new(lines), area);
}

fn draw_conversation(f: &mut Frame, area: Rect, app: &App) {
    let w = usize::from(area.width).saturating_sub(4).max(10);
    let id = viewed(app);
    let mut out: Vec<Line<'static>> = Vec::new();
    let entries = id.as_deref().map(|i| app.chat.entries(i)).unwrap_or_default();
    if entries.is_empty() && app.chat.busy.is_none() {
        let hint = "A new conversation. Ask anything: where a project stands, what you decided in a \
                    chat, what is due; or ask it to add a task, draft a reply, file notes, or \
                    register a project.";
        for l in paragraph_lines(hint, w) {
            out.push(Line::from(Span::styled(format!("  {l}"), dim())));
        }
    }
    let mut looked: Vec<String> = Vec::new();
    let flush = |looked: &mut Vec<String>, out: &mut Vec<Line<'static>>| {
        if looked.is_empty() {
            return;
        }
        let more = looked.len().saturating_sub(2);
        let mut s = looked.iter().take(2).cloned().collect::<Vec<_>>().join(" · ");
        if more > 0 {
            s.push_str(&format!(" · {more} more"));
        }
        // Lookups as Claude Code shows a tool's result: a faint `⎿` under the reply.
        let refused = s.starts_with("refused");
        out.push(Line::from(vec![
            Span::styled("  ⎿  ", dim()),
            Span::styled(cut(&s, w.saturating_sub(5)), if refused { st("red") } else { dim() }),
        ]));
        looked.clear();
    };
    for e in entries {
        match e {
            Entry::Looked(s) => looked.push(s),
            // As Claude Code draws a conversation: your message behind `❯`, its replies behind
            // `⏺` with their markdown rendered.
            // Your message on Claude Code's own band (the theme's userMessageBackground, #313244)
            // across the full width, behind `>`.
            Entry::You(t) => {
                flush(&mut looked, &mut out);
                out.push(Line::default());
                let full = usize::from(area.width);
                for (i, l) in paragraph_lines(&t, full.saturating_sub(3)).into_iter().enumerate() {
                    let lead = if i == 0 { "> " } else { "  " };
                    let pad = full.saturating_sub(2 + l.width());
                    let band = Style::default().bg(c("surface0"));
                    out.push(Line::from(vec![
                        Span::styled(lead, st("overlay2").patch(band)),
                        Span::styled(l, text().patch(band)),
                        Span::styled(" ".repeat(pad), band),
                    ]));
                }
            }
            Entry::Claude(t) => {
                flush(&mut looked, &mut out);
                out.push(Line::default());
                out.extend(reply_lines(&t, w));
            }
            Entry::Remember(r) => {
                flush(&mut looked, &mut out);
                match app.chat.index.answered.get(&r.key) {
                    None => out.push(ask_line(&r, w)),
                    Some(yes) => out.push(Line::from(vec![
                        Span::styled(
                            if *yes { "⏺ " } else { "  ⎿  " },
                            if *yes { st("green") } else { dim() },
                        ),
                        Span::styled(
                            cut(
                                &format!("{} {}", if *yes { "remembered" } else { "not remembered:" }, r.line()),
                                w.saturating_sub(5),
                            ),
                            if *yes { text() } else { dim() },
                        ),
                    ])),
                }
            }
            Entry::Did(s) => {
                flush(&mut looked, &mut out);
                // A change as Claude Code shows a tool call that ran: a green ⏺ (the theme's
                // success colour).
                out.push(Line::from(vec![
                    Span::styled("⏺ ", st("green")),
                    Span::styled(cut(&s, w.saturating_sub(2)), text()),
                ]));
            }
        }
    }
    flush(&mut looked, &mut out);
    if app.chat.busy.is_some() && app.chat.busy == id.or_else(|| app.chat.index.current.clone()) {
        out.push(Line::default());
        out.extend(reply_lines(&app.chat.live, w));
        out.push(Line::from(vec![
            Span::styled(format!("{} ", spin(app.frame)), st("blue")),
            Span::styled(app.chat.status.clone(), dim()),
        ]));
    }
    if let Some(e) = &app.chat.error {
        out.push(Line::from(vec![
            Span::styled("  ! ", st("red")),
            Span::styled(e.clone(), text()),
        ]));
    }
    // Pinned to the bottom; ↑ scrolls back. The draw records how far back there is, so the keys
    // stop at the top instead of piling up scroll nobody can see.
    let h = usize::from(area.height);
    let max_scroll = out.len().saturating_sub(h);
    app.chat.max_scroll.set(max_scroll);
    let scroll = app.chat.scroll.min(max_scroll);
    if scroll > 0 && h > 1 {
        // Say there is more below, on the last row.
        let below = Line::from(Span::styled(format!("  ↓ {scroll} more lines · G the end"), dim()));
        let start = out.len().saturating_sub(h + scroll);
        let mut shown: Vec<Line<'static>> = out.into_iter().skip(start).take(h - 1).collect();
        shown.push(below);
        f.render_widget(Paragraph::new(shown), area);
        return;
    }
    let start = out.len().saturating_sub(h + scroll);
    let shown: Vec<Line<'static>> = out.into_iter().skip(start).take(h).collect();
    f.render_widget(Paragraph::new(shown), area);
}

fn draw_input(f: &mut Frame, area: Rect, app: &App) {
    let w = usize::from(area.width).saturating_sub(6);
    let (lit, body) = match &app.mode {
        Mode::Chat(q) => (
            true,
            vec![
                Span::styled("   ❯ ", st("blue")),
                Span::styled(tail_cut(q, w), text()),
                Span::styled("▏", st("blue")),
            ],
        ),
        Mode::ChatName(n) => (
            true,
            vec![
                Span::styled("   name ❯ ", st("blue")),
                Span::styled(tail_cut(n, w.saturating_sub(5)), text()),
                Span::styled("▏", st("blue")),
            ],
        ),
        _ => (
            false,
            vec![
                Span::styled("   ␣", keyst()),
                Span::styled(" to type · ", dim()),
                Span::styled("o", keyst()),
                Span::styled(" when it needs to do more than it may here", dim()),
            ],
        ),
    };
    let rule = ask_rule(
        "ask",
        lit,
        &[("+", "new"), ("o", "Claude Code"), ("e", "rename")],
        "",
        area.width,
    );
    f.render_widget(Paragraph::new(vec![rule, Line::from(body)]), area);
}

/// A reply as the view draws it: `⏺` before its first line, its markdown rendered, every line
/// indented under the mark.
fn reply_lines(md: &str, w: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for (i, l) in crate::markdown::render(md, w.saturating_sub(2)).into_iter().enumerate() {
        let lead = if i == 0 {
            Span::styled("⏺ ", st("text"))
        } else {
            Span::raw("  ")
        };
        let mut spans = vec![lead];
        spans.extend(l.spans);
        out.push(Line::from(spans));
    }
    out
}

/// Text as lines of at most `w` columns, keeping its own line breaks and blank lines.
fn paragraph_lines(s: &str, w: usize) -> Vec<String> {
    let mut out = Vec::new();
    for raw in s.trim_end().lines() {
        if raw.trim().is_empty() {
            if out.last().is_some_and(|l: &String| !l.is_empty()) {
                out.push(String::new());
            }
            continue;
        }
        let raw = plain(raw);
        let indent: String = raw.chars().take_while(|c| *c == ' ').collect();
        let body = raw.trim_start();
        // A list item's wrapped lines hang under its text; anything else wraps flush.
        let hang = if body.starts_with("- ") || body.starts_with("* ") {
            "  ".to_string()
        } else if let Some(n) = body
            .find(". ")
            .filter(|&n| n <= 3 && body[..n].chars().all(|c| c.is_ascii_digit()))
        {
            " ".repeat(n + 2)
        } else {
            String::new()
        };
        for (i, l) in wrap(body, w.saturating_sub(indent.width() + hang.len()).max(8))
            .into_iter()
            .enumerate()
        {
            out.push(if i == 0 {
                format!("{indent}{l}")
            } else {
                format!("{indent}{hang}{l}")
            });
        }
    }
    out
}

fn cut(s: &str, w: usize) -> String {
    crate::ui::truncate(plain(s).trim(), w)
}

/// Markdown's marks dropped (`**bold**`, `` `code` ``): the terminal shows them as clutter, and
/// the words read the same without them.
fn plain(s: &str) -> String {
    s.replace("**", "").replace('`', "")
}

/// The END of what you are typing, so the cursor stays in view on a long message.
fn tail_cut(s: &str, w: usize) -> String {
    if s.width() <= w {
        return s.to_string();
    }
    let mut out: Vec<char> = Vec::new();
    let mut used = 0;
    for ch in s.chars().rev() {
        let cw = UnicodeWidthStr::width(ch.to_string().as_str());
        if used + cw + 1 > w {
            break;
        }
        used += cw;
        out.push(ch);
    }
    out.reverse();
    format!("…{}", out.into_iter().collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_text_keeps_its_breaks_and_wraps() {
        let l = paragraph_lines("first line\n\n- a point that is long enough to wrap around", 20);
        assert_eq!(l[0], "first line");
        assert_eq!(l[1], "");
        assert!(l.len() > 3 && l.iter().all(|x| x.width() <= 22));
        assert!(l[3].starts_with("  "), "a list item's second line hangs: {l:?}");
        let p = paragraph_lines("Only **locus** had commits in the last few days, on `files`", 24);
        assert!(!p[1].starts_with(' ') && !p.concat().contains('*'), "{p:?}");
    }

    #[test]
    fn typing_shows_the_end() {
        assert!(tail_cut("abcdefghijklmnopqrstuvwxyz", 10).ends_with("xyz"));
    }
}
