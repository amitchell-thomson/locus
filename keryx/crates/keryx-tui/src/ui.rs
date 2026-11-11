//! Drawing (keryx-joins-locus X5, your round of 26 Oct): one grey frame with every inbox named in
//! its top edge (the one shown highlighted, `Tab` moving it), a quiet list of two lines a thread
//! with days as headings and a rule at what came since you last looked, the conversation on the
//! right in the order it was written, and locus's footer. `b` hides the list.

use crate::app::{App, Mode, Open};
use crate::theme::{c, centred, fade, fit, popup_block, rgb, wrap};
use keryx_core::display::date_label;
use keryx_core::invite::{Invite, When};
use keryx_core::model::{Address, Message};
use keryx_store::view::ThreadRow;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

pub const ICON_FLAG: &str = "󰈻";
pub const ICON_ATTACH: &str = "󰁦";
pub const ICON_INVITE: &str = "󰃭";
pub const ICON_SNOOZE: &str = "󰒲";
pub const ICON_TASK: &str = "󰄱";
pub const ICON_REPLIED: &str = "\u{f045a}";

pub fn draw(f: &mut Frame, app: &App) {
    draw_writing(f, app, None);
}

/// The frame, with the editor's terminal in the right pane while writing (X6).
pub fn draw_writing(f: &mut Frame, app: &App, editor: Option<&crate::pane::Pane>) {
    draw_screen(f, app, editor);
    one_width(f.buffer_mut());
}

/// Drop variation selectors from every cell. `✔️` is `✔` + U+FE0F: ratatui counts it one cell
/// wide and tmux two, so the rest of its line landed a column to the right and a cell ratatui
/// believed unchanged was never repainted (round 11: a stray "l" by the thread's right border,
/// left from an earlier mail). Without the selector each one is a single cell to both.
fn one_width(buf: &mut ratatui::buffer::Buffer) {
    for cell in &mut buf.content {
        if cell.symbol().contains(['\u{FE0E}', '\u{FE0F}']) {
            let plain: String = cell
                .symbol()
                .chars()
                .filter(|c| !matches!(c, '\u{FE0E}' | '\u{FE0F}'))
                .collect();
            cell.set_symbol(if plain.is_empty() { " " } else { &plain });
        }
    }
}

fn draw_screen(f: &mut Frame, app: &App, editor: Option<&crate::pane::Pane>) {
    let area = f.area();
    if area.height < 4 {
        return;
    }
    let main = Rect::new(area.x, area.y, area.width, area.height - 1);
    let foot = Rect::new(area.x, area.y + area.height - 1, area.width, 1);
    if let Mode::ConfirmSend(d) = &app.mode {
        crate::ui_send::confirm_send(f, app, main, &d.req, &d.warnings);
    } else if matches!(app.mode, Mode::Runs { .. }) {
        crate::ui_runs::runs(f, app, main);
    } else if matches!(app.mode, Mode::RunDetail { .. }) {
        crate::ui_runs::detail(f, app, main);
    } else if let Mode::Filed { sel } = app.mode {
        crate::filed::draw(f, app, main, sel);
    } else if let Mode::Drafts { sel } = app.mode {
        crate::drafts::draw(f, app, main, sel);
    } else {
        mail(f, app, main, editor);
    }
    footer(f, app, foot);
    if let Some(p) = app.writing.as_ref().and_then(|w| w.picker.as_ref()) {
        crate::ui_send::picker(f, area, p);
    }
    match &app.mode {
        Mode::Help => help(f, area),
        Mode::GoTo => goto(f, app, area),
        Mode::Attachments => attachments(f, app, area),
        Mode::Snooze(input) => crate::ui_write::snooze(f, app, area, input.as_deref()),
        Mode::File(p) => crate::ui_write::file(f, app, area, p),
        Mode::ConfirmUnsubscribe { how, .. } => crate::ui_send::confirm_unsubscribe(f, area, how),
        Mode::KairosForm(form) => crate::ui_kairos::form(f, app, area, form),
        _ => {}
    }
}

/// The top edge: ` mail ` and every inbox, the one shown highlighted, the others greyed.
/// The frame's top-left title: keryx, then the list you are in or your search.
fn header(app: &App) -> Line<'static> {
    let mut spans = vec![Span::styled(" keryx ", locus_theme::bold(locus_theme::st("text")))];
    if let Some(q) = &app.search {
        spans.push(Span::styled(
            format!("· search: {} ", fit(q, 30)),
            locus_theme::st("yellow"),
        ));
    } else if app.view != keryx_store::view::View::Inbox {
        spans.push(Span::styled(
            format!("· {} ", app.view.title().to_lowercase()),
            locus_theme::dim(),
        ));
    }
    Line::from(spans)
}

/// Every inbox, named in the middle of the top edge: the ones shown highlighted, the rest grey.
fn inbox_tabs(app: &App) -> Line<'static> {
    let mut spans = Vec::new();
    let names: Vec<String> = std::iter::once("all".to_string())
        .chain(app.cfg.accounts.iter().map(|a| a.name.to_lowercase()))
        .collect();
    spans.push(Span::raw(" "));
    for (i, n) in names.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", locus_theme::st("overlay0")));
        }
        spans.push(if i == app.account || app.also.contains(&i) {
            Span::styled(n, locus_theme::bold(locus_theme::st("blue")))
        } else {
            Span::styled(n, locus_theme::st("overlay0"))
        });
    }
    spans.push(Span::raw(" "));
    Line::from(spans)
}

/// The list and the reading pane in one frame.
/// Where the list, its rule and the right-hand pane go inside the frame's border.
struct Panes {
    list: Option<Rect>,
    right: Rect,
}

fn panes(app: &App, inner: Rect) -> Panes {
    let reading = matches!(app.mode, Mode::Thread | Mode::Attachments) || app.writing.is_some();
    if app.list_hidden || (!app.split() && reading) {
        return Panes {
            list: None,
            right: inner,
        };
    }
    if !app.split() {
        return Panes {
            list: Some(inner),
            right: Rect::new(inner.x + inner.width, inner.y, 0, inner.height),
        };
    }
    // A little over a third (round 11: "slightly wider"; b hides it).
    let lw = (inner.width * 3 / 8).clamp(32, 52);
    Panes {
        list: Some(Rect::new(inner.x, inner.y, lw, inner.height)),
        right: Rect::new(inner.x + lw + 1, inner.y, inner.width - lw - 1, inner.height),
    }
}

/// Where the editor's terminal goes in a frame of `size` (the run loop sizes the pty to it).
pub fn writing_rect(app: &App, size: Rect) -> Rect {
    let main = Rect::new(size.x, size.y, size.width, size.height.saturating_sub(1));
    let inner = Rect::new(
        main.x + 1,
        main.y + 1,
        main.width.saturating_sub(2),
        main.height.saturating_sub(2),
    );
    let r = panes(app, inner).right;
    Rect::new(r.x + 1, r.y, r.width.saturating_sub(1), r.height)
}

fn mail(f: &mut Frame, app: &App, area: Rect, editor: Option<&crate::pane::Pane>) {
    let new = app.new_count();
    let right = match &app.writing {
        Some(w) => Some(Line::from(Span::styled(
            format!(" {} ", w.purpose.label()),
            locus_theme::st("peach"),
        ))),
        None if matches!(app.mode, Mode::Thread | Mode::Attachments) => Some(Line::from(Span::styled(
            " reading ",
            locus_theme::bold(locus_theme::st("mauve")),
        ))),
        None => (new > 0).then(|| Line::from(Span::styled(format!(" {new} new "), locus_theme::st("peach")))),
    };
    let block = locus_theme::screen(header(app), right).title(inbox_tabs(app).centered());
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width < 10 || inner.height == 0 {
        return;
    }
    let reading = matches!(app.mode, Mode::Thread | Mode::Attachments);
    let open = if reading {
        app.open.as_ref()
    } else {
        app.preview.as_ref()
    };
    let p = panes(app, inner);
    if let Some(l) = p.list {
        list(f, app, l);
        if p.right.width > 0 {
            for y in p.right.y..p.right.y + p.right.height {
                let rule = if reading { "mauve" } else { "surface1" };
                f.buffer_mut().set_string(p.right.x - 1, y, "│", locus_theme::st(rule));
            }
        }
    }
    if p.right.width == 0 {
        return;
    }
    if app.writing.is_some() {
        let r = Rect::new(
            p.right.x + 1,
            p.right.y,
            p.right.width.saturating_sub(1),
            p.right.height,
        );
        if let Some(pane) = editor {
            pane.with_screen(|screen| {
                let term = tui_term::widget::PseudoTerminal::new(screen)
                    .cursor(tui_term::widget::Cursor::default().visibility(false));
                f.render_widget(term, r);
                if !screen.hide_cursor() && app.writing.as_ref().is_some_and(|w| w.picker.is_none()) {
                    let (y, x) = screen.cursor_position();
                    f.set_cursor_position((r.x + x, r.y + y));
                }
            });
        }
        return;
    }
    thread(f, app, p.right, open);
}

/// A day as a list heading: `today`, `yesterday`, `friday`, `12 oct`.
fn day_heading(ts: i64, now: chrono::DateTime<chrono_tz::Tz>) -> String {
    use chrono::TimeZone;
    let Some(t) = now.timezone().timestamp_opt(ts, 0).single() else {
        return String::new();
    };
    let (d, today) = (t.date_naive(), now.date_naive());
    let days = (today - d).num_days();
    match days {
        0 => "today".into(),
        1 => "yesterday".into(),
        2..=6 => t.format("%A").to_string().to_lowercase(),
        _ if chrono::Datelike::year(&d) == chrono::Datelike::year(&today) => {
            t.format("%-d %b").to_string().to_lowercase()
        }
        _ => t.format("%-d %b %Y").to_string().to_lowercase(),
    }
}

/// What the list draws, top to bottom.
enum Row {
    /// The inbox's needs-reply section (round 5).
    Reply,
    Day(String),
    Since(String),
    Blank,
    /// A thread: its index in `app.rows`, and which of its two lines.
    Thread(usize, bool),
}

fn list_rows(app: &App) -> Vec<Row> {
    let mut out = Vec::new();
    let mut day = String::new();
    let mut since_drawn = false;
    let since = app.seen_since;
    // The inbox's needs-reply threads lead, under their own heading, then a gap.
    let replies = if app.reply_first() {
        app.rows.iter().take_while(|r| r.needs_reply.is_some()).count()
    } else {
        0
    };
    for i in 0..replies {
        if i == 0 {
            out.push(Row::Reply);
        }
        out.push(Row::Thread(i, true));
        out.push(Row::Thread(i, false));
    }
    for (i, r) in app.rows.iter().enumerate().skip(replies) {
        let d = day_heading(r.date, app.now);
        if since.is_some_and(|s| r.date <= s) && !since_drawn && i > replies {
            let at = since.and_then(|s| {
                use chrono::TimeZone;
                app.now.timezone().timestamp_opt(s, 0).single()
            });
            out.push(Row::Since(
                at.map(|t: chrono::DateTime<chrono_tz::Tz>| t.format("%a %H:%M").to_string().to_lowercase())
                    .unwrap_or_default(),
            ));
            since_drawn = true;
        }
        if since.is_some_and(|s| r.date <= s) {
            since_drawn = true;
        }
        if d != day {
            if !out.is_empty() {
                out.push(Row::Blank);
            }
            out.push(Row::Day(d.clone()));
            day = d;
        }
        out.push(Row::Thread(i, true));
        out.push(Row::Thread(i, false));
    }
    out
}

fn list(f: &mut Frame, app: &App, area: Rect) {
    if app.rows.is_empty() {
        let msg = match (&app.search_error, &app.search) {
            (Some(e), _) => e.clone(),
            (None, Some(_)) => "nothing matches".into(),
            (None, None) if app.view == keryx_store::view::View::Inbox => "nothing new: the inbox is clear".into(),
            _ => "nothing here".into(),
        };
        f.buffer_mut().set_stringn(
            area.x + 1,
            area.y + 1,
            &msg,
            area.width.saturating_sub(2) as usize,
            locus_theme::dim(),
        );
        return;
    }
    let rows = list_rows(app);
    let h = area.height as usize;
    let at = rows
        .iter()
        .position(|r| matches!(r, Row::Thread(i, true) if *i == app.sel))
        .unwrap_or(0);
    // The selected thread kept near the middle, clamped at both ends.
    let first = at.saturating_sub(h / 2).min(rows.len().saturating_sub(h));
    // How far through (10 Nov: "a similar scrollbar for my inbox", as locus's Approve has): a
    // mauve thumb on the right edge, its size what fits on screen, when the list is longer.
    let area = if rows.len() > h && h > 0 && area.width > 8 {
        let size = (h * h / rows.len()).max(1);
        let top = (first * h / rows.len()).min(h - size);
        let x = area.x + area.width - 1;
        for y in 0..h {
            let (ch, st) = if (top..top + size).contains(&y) {
                ("┃", Style::default().fg(c("mauve")))
            } else {
                ("│", Style::default().fg(c("surface1")))
            };
            f.buffer_mut().set_string(x, area.y + y as u16, ch, st);
        }
        Rect::new(area.x, area.y, area.width - 1, area.height)
    } else {
        area
    };
    let w = area.width as usize;
    let focus = !matches!(app.mode, Mode::Thread | Mode::Attachments);
    for (k, row) in rows.iter().enumerate().skip(first).take(h) {
        let y = area.y + (k - first) as u16;
        let line = match row {
            Row::Reply => {
                let label = " needs reply ".to_string();
                let peach = c("peach");
                Line::from(vec![
                    Span::styled(label.clone(), locus_theme::bold(Style::default().fg(peach))),
                    Span::styled(
                        "─".repeat(w.saturating_sub(label.width() + 1)),
                        Style::default().fg(fade(peach, 0.6)),
                    ),
                ])
            }
            // A day: its name in lavender and a rule across the column (your round of 26 Oct).
            Row::Day(d) => {
                let label = format!(" {d} ");
                let lav = c("lavender");
                Line::from(vec![
                    Span::styled(label.clone(), locus_theme::bold(Style::default().fg(lav))),
                    Span::styled(
                        "─".repeat(w.saturating_sub(label.width() + 1)),
                        Style::default().fg(fade(lav, 0.6)),
                    ),
                ])
            }
            Row::Since(t) => {
                let label = format!(" new since {t} ");
                Line::from(vec![
                    Span::styled(" ─", locus_theme::st("peach")),
                    Span::styled(label.clone(), locus_theme::st("peach")),
                    Span::styled(
                        "─".repeat(w.saturating_sub(label.width() + 3)),
                        locus_theme::st("peach"),
                    ),
                ])
            }
            Row::Blank => Line::raw(""),
            Row::Thread(i, first_line) => {
                let r = &app.rows[*i];
                let sel = *i == app.sel;
                let l = if *first_line {
                    row_who(app, r, w)
                } else {
                    row_subject(app, r, w)
                };
                if sel {
                    crate::theme::lit_row(l, !focus)
                } else if !focus {
                    // Reading a thread: the rest of the list steps back.
                    crate::theme::dimmed(l)
                } else {
                    l
                }
            }
        };
        f.buffer_mut().set_line(area.x, y, &line, area.width);
    }
}

/// A thread's first line: unread dot, who (and how many), what it carries, when.
/// A thread's first line: who, how many messages, what it is for, what it carries, when.
///
/// Laid out so each thing has one place (11 Nov: the list "reads messy and hard to figure out
/// what corresponds to what… long labels cut each other off"): the account is a dot in its
/// colour at the start, not a word competing with the subject; the thread count is a quiet
/// `· 11`; the label it is filed for sits on this line, short, before the icons; the subject
/// line below has the whole width to itself.
/// Width of the details block on the right of every row: the area and the date on line 1, the
/// icons on line 2 (11 Nov: "two sections in the box", sender and subject on the left, details
/// on the right). The left never runs into it, so the details read as one column down the list.
const DETAILS: usize = 24;
/// The longest area label: an item name past this is cut.
const AREA_MAX: usize = DETAILS - 9;

/// Split one row into its left text and its right details, each padded to its own block.
fn two_blocks(
    lead: Vec<Span<'static>>,
    left: Vec<Span<'static>>,
    details: Vec<Span<'static>>,
    width: usize,
) -> Line<'static> {
    let lw: usize = lead.iter().chain(&left).map(|s| s.content.width()).sum();
    let dw: usize = details.iter().map(|s| s.content.width()).sum();
    let left_room = width.saturating_sub(DETAILS);
    let gap = left_room.saturating_sub(lw) + DETAILS.saturating_sub(dw);
    let mut spans = lead;
    spans.extend(left);
    spans.push(Span::raw(" ".repeat(gap)));
    spans.extend(details);
    Line::from(spans)
}

/// Line 1: who and how many on the left; the area it is filed under (B14, Millennium…) right
/// against the date on the right. An unfiled thread has only its date there, so filed and
/// unfiled read down the details column at a glance.
fn row_who(app: &App, r: &ThreadRow, width: usize) -> Line<'static> {
    let date = date_label(r.date, app.now).to_lowercase();
    let mut details: Vec<Span<'static>> = Vec::new();
    // Unfiled says so (11 Nov: a blank there was too much empty space), dim beside a filed
    // thread's coloured area, so the two still read apart down the column.
    details.push(
        mark_tag(app, r).unwrap_or_else(|| Span::styled("not assigned  ", Style::default().fg(c("surface2")))),
    );
    details.push(Span::styled(format!("{date:>6} "), locus_theme::dim()));
    let marked = app.marked.contains(&r.r#ref);
    let mark = if marked {
        Span::styled("▸", locus_theme::st("blue"))
    } else if app.is_unread(r) {
        unread_bar(app, r)
    } else {
        Span::raw(" ")
    };
    let count = if r.count > 1 {
        format!(" · {}", r.count)
    } else {
        String::new()
    };
    let room = width.saturating_sub(DETAILS + 3 + 2);
    let who = fit(&r.who, room.saturating_sub(count.width()));
    let who_style = if app.is_unread(r) {
        Style::default().fg(c("text"))
    } else {
        Style::default().fg(c("subtext0"))
    };
    two_blocks(
        vec![reply_bar(r), mark, Span::raw(" ")],
        vec![Span::styled(who, who_style), Span::styled(count, locus_theme::dim())],
        details,
        width,
    )
}

/// Line 2: the subject on the left, cut before the details block; the icons (a suggested task,
/// replied, attachment) right-aligned under the date.
fn row_subject(app: &App, r: &ThreadRow, width: usize) -> Line<'static> {
    let mut icons: Vec<Span<'static>> = Vec::new();
    if r.suggested {
        icons.push(Span::styled(format!("{ICON_TASK}? "), Style::default().fg(c("mauve"))));
    }
    // You wrote last in a thread someone else is in: replied (round 11).
    if r.replied {
        icons.push(Span::styled(format!("{ICON_REPLIED} "), Style::default().fg(c("blue"))));
    }
    if r.has_attachment {
        icons.push(Span::styled(format!("{ICON_ATTACH} "), Style::default().fg(c("overlay1"))));
    }
    let room = width.saturating_sub(DETAILS + 3 + 2);
    let subject = fit(&r.subject, room);
    two_blocks(
        vec![reply_bar(r), unread_bar(app, r), Span::raw(" ")],
        vec![Span::styled(subject, locus_theme::st("overlay1"))],
        icons,
        width,
    )
}

/// How an area or item is named in the details: a course code in capitals (B14), anything else
/// as the registry or taxonomy spells it, first letter up (Millennium, Social).
fn area_label(target: &str) -> String {
    let t = target.trim();
    let code = t.len() <= 5
        && t.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && t.chars().skip(1).all(|c| c.is_ascii_digit())
        && t.len() > 1;
    if code {
        return t.to_uppercase();
    }
    let mut cs = t.chars();
    cs.next().map_or_else(String::new, |f| f.to_uppercase().chain(cs).collect())
}

fn mark_tag(app: &App, r: &ThreadRow) -> Option<Span<'static>> {
    let (kind, target) = app.marks.get(&(r.account.clone(), r.thread_id.clone()))?;
    let shared = app.shared.as_ref();
    let colour_name = match kind.as_str() {
        "item" => shared.and_then(|s| s.registry.get(target)).and_then(|p| {
            let area = p.stream.as_deref().unwrap_or(&p.area);
            shared.and_then(|s| s.taxonomy.colour_of(area))
        }),
        _ => shared.and_then(|s| s.taxonomy.colour_of(target)),
    };
    let col = colour_name.map_or(c("text"), c);
    // Short, so a long item name never pushes the date and icons off (11 Nov).
    Some(Span::styled(
        format!("{}  ", fit(&area_label(target), AREA_MAX)),
        Style::default().fg(col),
    ))
}

fn unread_bar(app: &App, r: &ThreadRow) -> Span<'static> {
    if app.is_unread(r) {
        Span::styled("▎", locus_theme::st("blue"))
    } else {
        Span::raw(" ")
    }
}

fn reply_bar(r: &ThreadRow) -> Span<'static> {
    if r.needs_reply.is_some() {
        Span::styled("▌", locus_theme::st("peach"))
    } else {
        Span::raw(" ")
    }
}

/// Kept for tests and the runs view: one thread as a single line.
pub fn row_line(app: &App, r: &ThreadRow, width: usize, _show_account: bool, selected: bool) -> Line<'static> {
    let l = row_who(app, r, width);
    if selected { crate::theme::lit_row(l, false) } else { l }
}

fn account_tag(app: &App, account: &str) -> (String, ratatui::style::Color) {
    match app.cfg.account(account) {
        Some(a) => (a.name.clone(), rgb(a.colour)),
        None => (account.to_string(), c("overlay1")),
    }
}

/// Split a body into shown lines and folded quote runs.
pub fn fold_quotes(body: &str) -> Vec<Result<String, usize>> {
    let lines: Vec<&str> = body.lines().collect();
    let mut out: Vec<Result<String, usize>> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim();
        let starts_quote = t.starts_with('>')
            || (t.starts_with("On ") && t.ends_with("wrote:"))
            || (t.len() >= 10 && t.chars().all(|c| c == '_'))
            || t == "-----Original Message-----";
        if starts_quote {
            let outlook = !t.starts_with('>') && !t.starts_with("On ");
            let mut j = i + 1;
            while j < lines.len() && (outlook || lines[j].trim().starts_with('>') || lines[j].trim().is_empty()) {
                j += 1;
            }
            // Trailing blank lines aren't part of the quote.
            while j > i + 1 && lines[j - 1].trim().is_empty() {
                j -= 1;
            }
            out.push(Err(j - i));
            i = j;
        } else {
            out.push(Ok(lines[i].to_string()));
            i += 1;
        }
    }
    out
}

fn invite_lines(app: &App, inv: &Invite, w: usize) -> Vec<Line<'static>> {
    let tz = app.cfg.tz();
    let when = |x: &When| match x {
        When::Timed(t) => t.with_timezone(&tz).format("%a %-d %b, %H:%M").to_string(),
        When::AllDay(d) => d.format("%a %-d %b (all day)").to_string(),
    };
    let mut s = format!("{ICON_INVITE} ");
    if inv.cancelled {
        s.push_str("Cancelled: ");
    }
    s.push_str(&inv.summary);
    s.push_str(&format!(" · {}", when(&inv.start)));
    if let (When::Timed(a), Some(When::Timed(b))) = (&inv.start, &inv.end)
        && a.date_naive() == b.date_naive()
    {
        s.push_str(&format!("–{}", b.with_timezone(&tz).format("%H:%M")));
    }
    if let Some(l) = &inv.location {
        s.push_str(&format!(" · {}", l.replace('\n', ", ")));
    }
    if let Some(o) = &inv.organizer {
        s.push_str(&format!(" · from {}", o.short()));
    }
    let col = if inv.cancelled { c("red") } else { c("mauve") };
    let mut out: Vec<Line<'static>> = wrap(&s, w)
        .into_iter()
        .map(|l| Line::from(Span::styled(l, Style::default().fg(col).add_modifier(Modifier::BOLD))))
        .collect();
    if let Some(n) = &inv.tz_note {
        out.push(Line::from(Span::styled(
            format!("  ⚠ {n}"),
            Style::default().fg(c("yellow")),
        )));
    }
    out
}

/// Is this address yours (any account's address or alias)?
/// One of your addresses: an account's, its aliases or the one its From: line shows (your
/// college address was missing, so your own mail read as from someone else).
fn is_me(app: &App, a: &Address) -> bool {
    app.cfg.own_addresses().contains(&a.key())
}

/// Each person in a conversation keeps one colour; you are blue.
fn voice(app: &App, a: Option<&Address>) -> ratatui::style::Color {
    const VOICES: [&str; 6] = ["peach", "mauve", "green", "teal", "pink", "yellow"];
    match a {
        Some(a) if is_me(app, a) => c("blue"),
        Some(a) => {
            let h = a
                .email
                .to_lowercase()
                .bytes()
                .fold(0usize, |h, b| h.wrapping_mul(31).wrapping_add(b as usize));
            c(VOICES[h % VOICES.len()])
        }
        None => c("overlay1"),
    }
}

/// The lines of a thread at width `w`: the conversation oldest first, each message under a
/// coloured bar in its writer's colour (you in blue), what it quoted folded away since it is
/// just above, earlier messages kept to their first lines until `z`.
/// The thread, read for coming back to it (spec keryx-thread-view, 11 Nov): a header that holds
/// the whole thread in a few lines (who, how long, a timeline, whose turn, what they asked that
/// you have not answered), then the messages NEWEST FIRST ("most recent should always be at the
/// top"), each only in its own words, theirs at the left and yours indented, under day bands that
/// say how long the silence before them was.
pub fn thread_lines(app: &App, open: &Open, w: usize) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let subject = open.msgs.last().map(|m| m.subject.clone()).unwrap_or_default();
    let (acc, col) = account_tag(app, &open.account);
    let acc = acc.to_lowercase();
    // The subject is magenta while you read the thread, as its row in the list is.
    let reading = matches!(app.mode, Mode::Thread | Mode::Attachments);
    out.push(Line::from(vec![
        Span::styled(
            fit(&subject, w.saturating_sub(acc.width() + 3)),
            Style::default()
                .fg(if reading { c("mauve") } else { c("text") })
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(acc, Style::default().fg(fade(col, 0.2))),
    ]));
    let order = newest_first(open);
    let new = |i: usize| is_new(app, open, i);
    out.extend(header_lines(app, open, w, &new));
    out.extend(crate::ui_kairos::thread_lines(app, open, w));
    out.extend(draft_lines(app, open, w));
    let any_new = (0..open.msgs.len()).any(&new);
    if any_new {
        let since = open.last_read.map_or("new to you".to_string(), |t| {
            format!("new since you last read · {}", day_of(app, t))
        });
        out.push(Line::raw(""));
        out.push(band(&since, w, Style::default().fg(c("mauve"))));
    }
    let mut seen_people: std::collections::HashSet<String> = std::collections::HashSet::new();
    // Who each message brought in: an address on its To or Cc that no earlier message had.
    let mut adds: Vec<Vec<String>> = vec![Vec::new(); open.msgs.len()];
    let first_of: Vec<bool> = {
        let mut v = vec![false; open.msgs.len()];
        let mut by_date: Vec<usize> = (0..open.msgs.len()).collect();
        by_date.sort_by_key(|&i| open.msgs[i].date);
        // Someone joins when their first message comes from outside every earlier message's
        // From, To and Cc: Milly, written to in the first mail, does not "join" by answering it.
        for &i in &by_date {
            let m = &open.msgs[i];
            let who = m.from.as_ref().map(Address::key).unwrap_or_default();
            v[i] = !seen_people.is_empty() && !seen_people.contains(&who);
            if !seen_people.is_empty() {
                for a in m.to.iter().chain(&m.cc) {
                    if !seen_people.contains(&a.key()) && !is_me(app, a) && !adds[i].contains(&a.short().to_string()) {
                        adds[i].push(a.short().to_string());
                    }
                }
            }
            seen_people.extend(m.from.iter().chain(&m.to).chain(&m.cc).map(Address::key));
        }
        v
    };
    let newest = order.first().copied();
    let mut last_day: Option<chrono::NaiveDate> = None;
    let mut starts: Vec<usize> = Vec::with_capacity(order.len());
    for (k, &i) in order.iter().enumerate() {
        let m = &open.msgs[i];
        let d = local(app, m.date).date_naive();
        let block_start = out.len() + 1;
        if last_day != Some(d) {
            // The silence before this day: back to the next older message.
            let older = order.get(k..).and_then(|rest| {
                rest.iter().map(|&j| &open.msgs[j]).find(|o| local(app, o.date).date_naive() != d)
            });
            let gap = older.map(|o| (d - local(app, o.date).date_naive()).num_days()).unwrap_or(0);
            let mut label = day_of(app, m.date);
            let mut style = locus_theme::dim();
            if gap >= 2 {
                label.push_str(&format!(" · {} later", span_words(gap)));
                if gap >= 7 {
                    style = Style::default().fg(c("peach"));
                }
            }
            out.push(Line::raw(""));
            out.push(band(&label, w, style));
            last_day = Some(d);
        } else {
            out.push(Line::raw(""));
        }
        let open_by_default = Some(i) == newest || (new(i) && !mine(app, m));
        let full = open_by_default != open.toggled.contains(&i) || open.raw.contains(&i);
        let selected = k == open.sel.min(order.len().saturating_sub(1));
        starts.push(block_start);
        let notes = Notes { new: new(i), joins: first_of[i], adds: &adds[i], selected };
        if full {
            out.extend(open_block(app, open, i, m, w, &notes));
        } else {
            out.extend(message_block(app, open, i, m, w, &notes));
        }
    }
    *open.starts.borrow_mut() = starts;
    out
}

/// What the name column (folded) or the header (open) says about a message beyond who and when.
struct Notes<'a> {
    new: bool,
    joins: bool,
    adds: &'a [String],
    selected: bool,
}

/// The selection mark at the left edge: a mauve bar on the selected message's lines.
fn sel_mark(selected: bool) -> Span<'static> {
    if selected {
        Span::styled("▌", Style::default().fg(c("mauve")))
    } else {
        Span::raw(" ")
    }
}

/// An open message, at the pane's width: a header (who, then when on the right; who it went to
/// and who it brought in), a blank line, then its own words in paragraphs, list items hanging,
/// behind a bar in the sender's colour. Yours step in like their folded rows.
fn open_block(app: &App, open: &Open, i: usize, m: &Message, w: usize, n: &Notes) -> Vec<Line<'static>> {
    let me = mine(app, m);
    let indent = if me { MINE } else { 0 };
    let col = voice(app, m.from.as_ref());
    let text_w = w.saturating_sub(indent + 4).clamp(10, TEXT_MAX + META);
    let pre = |out: &mut Vec<Span<'static>>| {
        out.push(sel_mark(n.selected));
        out.push(Span::raw(" ".repeat(indent)));
        out.push(Span::styled("▎ ", Style::default().fg(col)));
    };
    let mut out = Vec::new();
    // Who, and when on the right.
    let who = match &m.from {
        Some(a) if is_me(app, a) => "you".to_string(),
        Some(a) => a.display(),
        None => "(no sender)".into(),
    };
    let when = local(app, m.date).format("%a %-d %b · %H:%M").to_string().to_lowercase();
    let tag = if n.new { " new" } else { "" };
    let who = fit(&who, text_w.saturating_sub(when.width() + tag.width() + 2));
    let gap = text_w.saturating_sub(who.width() + when.width() + tag.width());
    let mut l = Vec::new();
    pre(&mut l);
    l.push(Span::styled(who, Style::default().fg(col).add_modifier(Modifier::BOLD)));
    l.push(Span::raw(" ".repeat(gap)));
    l.push(Span::styled(when, locus_theme::dim()));
    l.push(Span::styled(tag, Style::default().fg(c("mauve"))));
    out.push(Line::from(l));
    // Who it went to, and who joined or was brought in.
    let names = |v: &[Address]| {
        v.iter()
            .map(|a| if is_me(app, a) { "you".to_string() } else { a.short().to_string() })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut to = String::new();
    if !m.to.is_empty() {
        to.push_str(&format!("to {}", names(&m.to)));
    }
    if !m.cc.is_empty() {
        to.push_str(&format!(" · cc {}", names(&m.cc)));
    }
    let mut l = Vec::new();
    pre(&mut l);
    l.push(Span::styled(fit(to.trim_start_matches(" · "), text_w), locus_theme::dim()));
    if n.joins {
        l.push(Span::styled(" · joins", Style::default().fg(c("teal"))));
    }
    for a in n.adds {
        l.push(Span::styled(format!(" · adds {a}"), Style::default().fg(c("teal"))));
    }
    out.push(Line::from(l));
    let mut l = Vec::new();
    pre(&mut l);
    out.push(Line::from(l));
    for body in message_text(app, open, i, m, text_w, true) {
        let mut l = Vec::new();
        pre(&mut l);
        l.extend(body.spans);
        out.push(Line::from(l));
    }
    out
}

/// Wrap paragraph by paragraph, a list item's later lines hanging under its text ("1. It's
/// completely up to you, …" / "   an intern if you prefer"), blank runs kept to one.
fn wrap_hanging(text: &str, w: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for para in text.split('\n') {
        if para.trim().is_empty() {
            if out.last().is_some_and(|l| !l.trim().is_empty()) {
                out.push(String::new());
            }
            continue;
        }
        let lead = para.len() - para.trim_start().len();
        let t = para.trim_start();
        let marker = t
            .split_once(' ')
            .map(|(m, _)| m)
            .filter(|m| {
                matches!(*m, "-" | "*" | "•")
                    || (m.len() <= 3
                        && m.ends_with(['.', ')'])
                        && m[..m.len() - 1].chars().all(|c| c.is_ascii_digit())
                        && m.len() > 1)
            });
        let hang = lead + marker.map_or(0, |m| m.chars().count() + 1);
        let first = wrap(para, w);
        let mut lines = first.into_iter();
        if let Some(l) = lines.next() {
            out.push(l);
        }
        let rest: Vec<String> = lines.collect();
        if !rest.is_empty() {
            let again = wrap(&rest.join(" "), w.saturating_sub(hang).max(10));
            out.extend(again.into_iter().map(|l| format!("{}{l}", " ".repeat(hang))));
        }
    }
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    out
}

/// Message indices, newest first.
fn newest_first(open: &Open) -> Vec<usize> {
    let mut v: Vec<usize> = (0..open.msgs.len()).collect();
    v.sort_by_key(|&i| std::cmp::Reverse(open.msgs[i].date));
    v
}

fn mine(app: &App, m: &Message) -> bool {
    m.from.as_ref().is_some_and(|a| is_me(app, a))
}

/// New to you: theirs, and after the last time you opened this thread here; with no record of
/// that yet, unread in the mirror when the thread was opened.
fn is_new(app: &App, open: &Open, i: usize) -> bool {
    let m = &open.msgs[i];
    !mine(app, m)
        && match open.last_read {
            Some(t) => m.date > t,
            None => !m.read,
        }
}

fn local(app: &App, t: i64) -> chrono::DateTime<chrono_tz::Tz> {
    use chrono::TimeZone;
    app.cfg.tz().timestamp_opt(t, 0).single().unwrap_or(app.now)
}

/// "2 nov", with the year when it is not this year's.
fn day_of(app: &App, t: i64) -> String {
    use chrono::Datelike;
    let d = local(app, t);
    if d.year() == app.now.year() {
        d.format("%-d %b").to_string().to_lowercase()
    } else {
        d.format("%-d %b %Y").to_string().to_lowercase()
    }
}

fn span_words(days: i64) -> String {
    match days {
        d if d < 14 => format!("{d} days"),
        d if d < 60 => format!("{} weeks", d / 7),
        d => format!("{} months", d / 30),
    }
}

/// "── 2 nov · 3 days later ─────…"
fn band(label: &str, w: usize, st: Style) -> Line<'static> {
    let label = fit(label, w.saturating_sub(6));
    Line::from(vec![
        Span::styled("── ", locus_theme::st("surface2")),
        Span::styled(label.clone(), st),
        Span::styled(
            format!(" {}", "─".repeat(w.saturating_sub(label.width() + 5))),
            locus_theme::st("surface2"),
        ),
    ])
}

/// The person as the thread names them: "you", else their short name.
fn person(app: &App, a: Option<&Address>) -> String {
    match a {
        Some(a) if is_me(app, a) => "you".to_string(),
        Some(a) => a.short().to_string(),
        None => "(no sender)".into(),
    }
}

/// The header under the subject: the people (in their colours, which the timeline and the
/// messages use), the count, the span and when you last read it; the timeline; whose turn it
/// is; what they asked since your last reply.
fn header_lines(app: &App, open: &Open, w: usize, new: &dyn Fn(usize) -> bool) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let msgs = &open.msgs;
    let Some(first) = msgs.iter().map(|m| m.date).min() else { return out };
    let last = msgs.iter().map(|m| m.date).max().unwrap_or(first);
    // People, you last.
    let mut people: Vec<(String, ratatui::style::Color)> = Vec::new();
    let mut me = None;
    let mut by_date: Vec<&Message> = msgs.iter().collect();
    by_date.sort_by_key(|m| m.date);
    for m in &by_date {
        let n = person(app, m.from.as_ref());
        let col = voice(app, m.from.as_ref());
        if n == "you" {
            me = Some((n, col));
        } else if !people.iter().any(|(p, _)| *p == n) {
            people.push((n, col));
        }
    }
    people.extend(me);
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (k, (n, col)) in people.iter().enumerate() {
        if k > 0 {
            spans.push(Span::styled(", ", locus_theme::dim()));
        }
        spans.push(Span::styled(n.clone(), Style::default().fg(*col)));
    }
    let n = msgs.len();
    let mut facts = format!(" · {n} message{}", if n == 1 { "" } else { "s" });
    if day_of(app, first) != day_of(app, last) {
        facts.push_str(&format!(" · {} – {}", day_of(app, first), day_of(app, last)));
    } else {
        facts.push_str(&format!(" · {}", day_of(app, last)));
    }
    if let Some(t) = open.last_read {
        facts.push_str(&format!(" · last read {}", day_of(app, t)));
    }
    spans.push(Span::styled(facts, locus_theme::dim()));
    out.push(Line::from(spans));
    if n > 1 {
        out.push(timeline(app, open, w, first, last, new));
    }
    if let Some(l) = turn_line(app, open, w) {
        out.push(l);
    }
    out.extend(question_lines(app, open, w));
    out
}

/// One mark per message on a date axis from the first to the last, in each sender's colour,
/// bright when new to you; ◆ where you last read.
fn timeline(app: &App, open: &Open, w: usize, first: i64, last: i64, new: &dyn Fn(usize) -> bool) -> Line<'static> {
    let left = format!("{} ", day_of(app, first));
    let right = format!(" {}", day_of(app, last));
    let cells = w.saturating_sub(left.width() + right.width() + 1).clamp(4, 80);
    let span = (last - first).max(1) as f64;
    let at = |t: i64| ((((t - first) as f64) / span) * (cells - 1) as f64).round() as usize;
    let mut row: Vec<Option<Span<'static>>> = vec![None; cells];
    let mut by_date: Vec<usize> = (0..open.msgs.len()).collect();
    by_date.sort_by_key(|&i| open.msgs[i].date);
    for &i in &by_date {
        let m = &open.msgs[i];
        let col = voice(app, m.from.as_ref());
        let col = if new(i) || open.last_read.is_none() { col } else { fade(col, 0.35) };
        let mut x = at(m.date).min(cells - 1);
        // A burst of messages spreads to the right rather than hiding under one mark.
        while x + 1 < cells && row[x].is_some() && at(m.date) + 3 > x {
            x += 1;
        }
        row[x] = Some(Span::styled("●", Style::default().fg(col)));
    }
    if let Some(t) = open.last_read.filter(|&t| t >= first && t < last) {
        let mut x = at(t).min(cells - 1);
        while x + 1 < cells && row[x].is_some() {
            x += 1;
        }
        if row[x].is_none() {
            row[x] = Some(Span::styled("◆", Style::default().fg(c("mauve"))));
        }
    }
    let mut spans = vec![Span::styled(left, locus_theme::dim())];
    for cell in row {
        spans.push(cell.unwrap_or_else(|| Span::styled("─", locus_theme::st("surface1"))));
    }
    spans.push(Span::styled(right, locus_theme::dim()));
    Line::from(spans)
}

/// "your turn · Milly wrote last, 9 days ago" or "waiting on them · 4 days", with the needs-reply
/// reason when there is one.
fn turn_line(app: &App, open: &Open, w: usize) -> Option<Line<'static>> {
    let m = open.msgs.iter().max_by_key(|m| m.date)?;
    let days = (app.now.date_naive() - local(app, m.date).date_naive()).num_days();
    let ago = match days {
        0 => "today".to_string(),
        1 => "yesterday".to_string(),
        d => format!("{} ago", span_words(d)),
    };
    let reason = app
        .rows
        .iter()
        .find(|r| r.account == open.account && r.thread_id == open.thread_id)
        .and_then(|r| r.needs_reply.as_ref())
        .filter(|why| !why.trim().is_empty() && *why != "tagged by Alec")
        .map(|why| format!(" · needs reply: {why}"))
        .unwrap_or_default();
    let (text, st) = if mine(app, m) {
        (format!("waiting on them · you wrote last, {ago}{reason}"), locus_theme::dim())
    } else {
        (
            format!("your turn · {} wrote last, {ago}{reason}", person(app, m.from.as_ref())),
            Style::default().fg(c("peach")),
        )
    };
    Some(Line::from(Span::styled(fit(&text, w), st)))
}

/// Their questions since your last message, word for word (deterministic: a quoted sentence can
/// be checked, and a missing one is better than an invented one). At most three, newest first.
fn question_lines(app: &App, open: &Open, w: usize) -> Vec<Line<'static>> {
    let my_last = open.msgs.iter().filter(|m| mine(app, m)).map(|m| m.date).max().unwrap_or(i64::MIN);
    let mut qs: Vec<(String, ratatui::style::Color, String)> = Vec::new();
    for &i in &newest_first(open) {
        let m = &open.msgs[i];
        if mine(app, m) || m.date <= my_last || !m.list_headers.is_empty() {
            continue;
        }
        for q in questions(&own_text(m)) {
            if qs.len() < 3 {
                qs.push((person(app, m.from.as_ref()), voice(app, m.from.as_ref()), q));
            }
        }
    }
    qs.into_iter()
        .map(|(who, col, q)| {
            let head = format!("? {who}: ");
            Line::from(vec![
                Span::styled("? ", Style::default().fg(c("mauve")).add_modifier(Modifier::BOLD)),
                Span::styled(format!("{who}: "), Style::default().fg(col)),
                Span::styled(fit(&q, w.saturating_sub(head.width())), Style::default().fg(c("text"))),
            ])
        })
        .collect()
}

/// The sentences of `text` that end in a question mark.
pub fn questions(text: &str) -> Vec<String> {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = Vec::new();
    let mut start = 0;
    for (k, ch) in flat.char_indices() {
        if matches!(ch, '.' | '!' | '?') {
            let end = k + ch.len_utf8();
            // "e.g." and "3.5" are not sentence ends: the next character must be a space or end.
            if ch != '?' && !flat[end..].starts_with(' ') && end < flat.len() {
                continue;
            }
            let sent = flat[start..end].trim();
            if ch == '?' && sent.chars().filter(|c| c.is_alphabetic()).count() >= 6 && !sent.contains("://") {
                out.push(sent.to_string());
            }
            start = end;
        }
    }
    out
}

/// What the message itself says: its text without the history it quoted or its signature.
fn own_text(m: &Message) -> String {
    let own = keryx_core::text::compact(&m.body_text);
    let own = own.strip_suffix("…[truncated]").unwrap_or(&own).trim().to_string();
    if own.is_empty() { m.body_text.trim().to_string() } else { own }
}

/// Rejoin lines a mail client wrapped at a fixed width (Gmail's plain text breaks at ~76), so the
/// pane wraps them once at its own width; wrapped twice they read "touch / for next cycle". A
/// line joins the one before when that one ran long and this one is not a list item or a quote.
fn reflow(lines: &[String]) -> String {
    let mut out: Vec<String> = Vec::new();
    for l in lines {
        let t = l.trim_start();
        let item = t.starts_with(['-', '*', '•', '>'])
            || t.split_once(['.', ')']).is_some_and(|(n, _)| !n.is_empty() && n.len() <= 2 && n.chars().all(|c| c.is_ascii_digit()));
        match out.last_mut() {
            Some(prev) if !t.is_empty() && !item && prev.chars().count() >= 60 && !prev.trim_start().starts_with('>') => {
                prev.push(' ');
                prev.push_str(t);
            }
            _ => out.push(l.clone()),
        }
    }
    out.join("\n")
}

/// A folded message's preview: its words run together with the greeting left off ("Hi Milly,"),
/// so its two lines say something.
fn preview(own: &str) -> String {
    let flat = own.split_whitespace().collect::<Vec<_>>().join(" ");
    let first = flat.split_once(',').map(|(g, _)| g).unwrap_or("");
    let greeting = first.split_whitespace().count() <= 3
        && ["hi", "hey", "hello", "dear", "morning", "good", "evening", "afternoon", "thanks"]
            .iter()
            .any(|g| first.to_lowercase().starts_with(g));
    match flat.split_once(',') {
        Some((_, rest)) if greeting => rest.trim().to_string(),
        _ => flat,
    }
}

/// Where the quoted history starts in a message's readable lines (the rules of `text::compact`):
/// an "On … wrote:" line, an Outlook From:/Sent: block, an "Original Message" rule.
fn history_start(lines: &[String]) -> Option<usize> {
    lines.iter().enumerate().position(|(i, l)| {
        let t = l.trim();
        // "On Fri, Oct 30, 2026 at 11:30 AM Alec <" + "alec@…> wrote:": Gmail breaks it anywhere.
        (t.starts_with("On ") && lines[i..].iter().take(3).any(|n| n.trim_end().ends_with("wrote:")))
            || t == "-----Original Message-----"
            || (t.chars().all(|c| c == '_') && t.len() >= 10)
            || (t.starts_with("From:") && lines[i + 1..].iter().take(4).any(|n| n.trim_start().starts_with("Sent:")))
    })
}

/// Your drafts for this thread, at the top: Claude's waiting ones and any you kept.
fn draft_lines(app: &App, open: &Open, w: usize) -> Vec<Line<'static>> {
    let thread = keryx_core::model::thread_ref(&open.account, &open.thread_id);
    let mut bodies: Vec<String> = Vec::new();
    for c in app.pending_sends() {
        if let Some(o) = c.outgoing()
            && o.account == open.account
            && o.thread_id.as_deref() == Some(open.thread_id.as_str())
        {
            bodies.push(o.body.clone());
        }
    }
    for k in app.kept_drafts() {
        if matches!(&k.purpose, crate::send::Purpose::Reply { r#ref, .. } if *r#ref == thread)
            && let Ok(text) = std::fs::read_to_string(&k.path)
        {
            // The editor file: headers, a blank line, then the body.
            let body = text.split_once("\n\n").map_or(text.as_str(), |(_, b)| b);
            bodies.push(body.to_string());
        }
    }
    let mut out = Vec::new();
    for b in bodies {
        out.push(Line::raw(""));
        out.push(band("draft, not sent · g d opens it", w, Style::default().fg(c("blue"))));
        let text_w = (w.saturating_sub(MINE + META)).min(TEXT_MAX);
        let own = keryx_core::text::compact(&b);
        let lines: Vec<Line<'static>> = wrap(own.trim(), text_w)
            .into_iter()
            .take(2)
            .map(|l| Line::from(Span::styled(l, locus_theme::st("subtext0"))))
            .collect();
        out.extend(lane(vec![Span::styled("you", Style::default().fg(c("blue")))], vec![], lines, MINE, false));
    }
    out
}

/// The name column, and how far your own messages step in.
const META: usize = 18;
const MINE: usize = 6;
/// Text past this width is hard to read; wide panes keep a margin instead.
const TEXT_MAX: usize = 96;

/// Lay a message out in its lane: the name column (name, then time, then notes, one per line)
/// beside the text.
fn lane(
    name: Vec<Span<'static>>,
    notes: Vec<Vec<Span<'static>>>,
    text: Vec<Line<'static>>,
    indent: usize,
    selected: bool,
) -> Vec<Line<'static>> {
    let mut meta: Vec<Vec<Span<'static>>> = vec![name];
    meta.extend(notes);
    let rows = meta.len().max(text.len());
    let mut out = Vec::new();
    let mut text = text.into_iter();
    for r in 0..rows {
        let mut spans = vec![sel_mark(selected), Span::raw(" ".repeat(indent))];
        let m = meta.get(r).cloned().unwrap_or_default();
        let mw: usize = m.iter().map(|s| s.content.width()).sum();
        spans.extend(m);
        spans.push(Span::raw(" ".repeat(META.saturating_sub(mw).max(1))));
        if let Some(l) = text.next() {
            spans.extend(l.spans);
        }
        out.push(Line::from(spans));
    }
    out
}

/// A folded message in its lane: the name column (name, time, notes) beside its first lines.
fn message_block(app: &App, open: &Open, i: usize, m: &Message, w: usize, n: &Notes) -> Vec<Line<'static>> {
    let me = mine(app, m);
    let indent = if me { MINE } else { 0 };
    let col = voice(app, m.from.as_ref());
    let text_w = w.saturating_sub(indent + META + 2).clamp(10, TEXT_MAX);
    let name = Span::styled(
        fit(&person(app, m.from.as_ref()), META - 2),
        Style::default().fg(col).add_modifier(Modifier::BOLD),
    );
    let mut notes: Vec<Vec<Span<'static>>> = Vec::new();
    let mut when = vec![Span::styled(local(app, m.date).format("%H:%M").to_string(), locus_theme::dim())];
    if n.new {
        when.push(Span::styled(" new", Style::default().fg(c("mauve"))));
    }
    notes.push(when);
    if n.joins {
        notes.push(vec![Span::styled("joins", Style::default().fg(c("teal")))]);
    }
    for a in n.adds {
        let first = a.split_whitespace().next().unwrap_or(a);
        notes.push(vec![Span::styled(fit(&format!("adds {first}"), META - 2), Style::default().fg(c("teal")))]);
    }
    let text = message_text(app, open, i, m, text_w, false);
    lane(vec![name], notes, text, indent, n.selected)
}

/// A message's text in its lane: the whole of it (without quoted history) when `full`, else its
/// first two lines. Recipients when they matter, invites, attachments and links when full.
fn message_text(app: &App, open: &Open, i: usize, m: &Message, w: usize, full: bool) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let body = Style::default().fg(c("text"));
    let note = Style::default().fg(c("overlay0")).add_modifier(Modifier::ITALIC);
    // An invite is its own summary line, folded or not.
    if let Some(Some(inv)) = open.invites.get(i) {
        out.extend(invite_lines(app, inv, w));
    }
    if m.hidden_text_removed {
        out.push(Line::from(Span::styled("⚠ hidden text was removed", Style::default().fg(c("yellow")))));
    }
    if m.body_text.trim().is_empty() {
        let why = if m.attachments.is_empty() && open.invites.get(i).is_none_or(Option::is_none) {
            "(Mail hasn't downloaded this message yet, or it has no text)"
        } else {
            "(no text)"
        };
        out.push(Line::from(Span::styled(why, locus_theme::dim())));
    } else if !full {
        let lines = wrap(&preview(&own_text(m)), w);
        let more = lines.len().saturating_sub(2);
        for l in lines.into_iter().take(2) {
            out.push(Line::from(Span::styled(l, Style::default().fg(c("subtext1")))));
        }
        if more > 0 {
            out.push(Line::from(Span::styled(format!("… {more} more · ⏎ opens"), note)));
        }
    } else {
        let automated = !m.list_headers.is_empty();
        let r = keryx_core::text::readable(&m.body_text, automated);
        let mut lines = r.lines.clone();
        // Its own words: the history it quoted is the messages below (z shows it as received).
        if !open.raw.contains(&i) {
            if let Some(h) = history_start(&lines) {
                lines.truncate(h);
            }
            lines.retain(|l| !l.trim_start().starts_with('>'));
        }
        let mut shown: Vec<Line<'static>> = Vec::new();
        // Mail's placeholder for an inline attachment (U+FFFC) says nothing: the attachment is
        // listed below.
        let lines: Vec<String> = lines.iter().map(|l| l.replace('\u{fffc}', "")).collect();
        let text = reflow(&lines);
        if text.trim().is_empty() {
            shown.push(Line::from(Span::styled("(nothing of its own: only what it quoted · z shows it)", note)));
        }
        for l in wrap_hanging(text.trim_end(), w) {
            let st = if l.trim_start().starts_with('>') { locus_theme::dim() } else { body };
            shown.push(Line::from(Span::styled(l, st)));
        }
        while shown.last().is_some_and(|l| l.width() == 0) {
            shown.pop();
        }
        out.extend(shown);
        if r.footer > 0 {
            out.push(Line::from(Span::styled(
                format!("… its footer folded ({} lines: unsubscribe, address)", r.footer),
                note,
            )));
        }
        if !r.links.is_empty() {
            out.push(Line::raw(""));
            let items: Vec<String> = r.links.iter().map(|(n, _, s)| format!("[{n}] {s}")).collect();
            for l in wrap(&items.join("  "), w) {
                out.push(Line::from(Span::styled(l, locus_theme::dim())));
            }
        }
    }
    if full {
        for a in &m.attachments {
            out.push(Line::from(vec![
                Span::styled(format!("{ICON_ATTACH} "), Style::default().fg(c("overlay2"))),
                Span::styled(fit(&a.name, w.saturating_sub(14)), Style::default().fg(c("subtext1"))),
                Span::styled(format!(" · {}", size(a.size)), locus_theme::dim()),
            ]));
        }
    } else if !m.attachments.is_empty() {
        out.push(Line::from(Span::styled(
            format!("{ICON_ATTACH} {}", m.attachments.len()),
            Style::default().fg(c("overlay2")),
        )));
    }
    out
}

pub fn size(n: u64) -> String {
    match n {
        n if n < 1024 => format!("{n} B"),
        n if n < 1024 * 1024 => format!("{} KB", n.div_ceil(1024)),
        n => format!("{:.1} MB", n as f64 / 1024.0 / 1024.0),
    }
}

fn thread(f: &mut Frame, app: &App, area: Rect, open: Option<&Open>) {
    let Some(open) = open else {
        f.buffer_mut()
            .set_string(area.x + 2, area.y + 1, "nothing selected", locus_theme::dim());
        return;
    };
    let inner = Rect::new(area.x + 1, area.y, area.width.saturating_sub(2), area.height);
    let w = inner.width as usize;
    let lines = thread_lines(app, open, w);
    open.height.set(inner.height);
    let max_scroll = (lines.len() as u16).saturating_sub(inner.height);
    open.max_scroll.set(max_scroll);
    f.render_widget(Paragraph::new(lines).scroll((open.scroll.min(max_scroll), 0)), inner);
}

/// locus's footer: the keys that work here on the left; on the right the note, a send in its
/// hold, what waits in Approve, a stale sync.
fn footer(f: &mut Frame, app: &App, area: Rect) {
    let pairs: Vec<(&str, &str)> = match &app.mode {
        _ if app.writing.as_ref().is_some_and(|w| w.picker.is_some()) => {
            vec![
                ("⏎", "attach"),
                ("^o", "open to check"),
                ("↑ ↓", "move"),
                ("esc", "back"),
            ]
        }
        _ if app.writing.is_some() => {
            vec![
                (":w", "send"),
                (":q", "keep as a draft"),
                (":cq", "throw away"),
                ("^a", "attach"),
            ]
        }
        Mode::Search(_) => vec![("⏎", "keep"), ("esc", "clear"), ("↑ ↓", "move")],
        Mode::Thread
            if app
                .open
                .as_ref()
                .is_some_and(|o| app.suggestion_for(&o.account, &o.thread_id).is_some()) =>
        {
            vec![
                ("y", "create"),
                ("n", "dismiss"),
                ("r", "reply"),
                ("d", "done"),
                ("esc", "back"),
            ]
        }
        // A thread opened from a run's detail: only back works (round 11 audit).
        Mode::Thread if app.back.is_some() => vec![("↑↓", "email"), ("⏎", "open"), ("z", "as received"), ("esc", "back")],
        Mode::Thread => {
            let mut v = vec![
                ("r", "reply"),
                ("d", "done"),
                ("a", "archive"),
                ("m", "for"),
                ("↑↓", "email"),
                ("⏎", "open"),
                ("z", "as received"),
            ];
            if app.open.as_ref().is_some_and(|o| !o.attachments().is_empty()) {
                v.push(("o", "attachment"));
            }
            v.push(("esc", "back"));
            v
        }
        Mode::Help | Mode::GoTo => vec![("esc", "close")],
        Mode::Filed { .. } => vec![("↑ ↓", "move"), ("u", "put back in the inbox"), ("esc", "back")],
        Mode::Runs { .. } => vec![("↑ ↓", "move"), ("⏎", "details"), ("esc", "back")],
        Mode::RunDetail { .. } => vec![("↑ ↓", "move"), ("⏎", "open"), ("u", "undo"), ("esc", "back")],
        Mode::Snooze(_) => vec![("1…4", "choose"), ("esc", "cancel")],
        Mode::KairosForm(_) => vec![("⇥", "next"), ("⏎", "save"), ("esc", "cancel")],
        Mode::ConfirmSend(_) => vec![("y", "send"), ("e", "edit"), ("n", "not now")],
        Mode::ConfirmUnsubscribe { .. } => vec![("y", "unsubscribe"), ("esc", "no")],
        Mode::File(_) => vec![("⏎", "file"), ("↑ ↓", "move"), ("esc", "back")],
        Mode::Attachments => vec![("↑ ↓", "move"), ("⏎", "open"), ("esc", "back")],
        Mode::Drafts { .. } => vec![("↑ ↓", "move"), ("⏎", "open"), ("x", "throw away"), ("esc", "back")],
        _ if !app.marked.is_empty() => vec![
            ("d", "done"),
            ("a", "archive"),
            ("m", "for"),
            ("f", "needs reply"),
            ("esc", "unmark"),
        ],
        _ => vec![
            ("⏎", "open"),
            ("r", "reply"),
            ("d", "done"),
            ("a", "archive"),
            ("m", "for"),
            ("f", "needs reply"),
            ("?", "keys"),
        ],
    };
    let mut left = vec![Span::raw(" ")];
    if let Mode::Search(q) = &app.mode {
        left.push(Span::styled("search: ", locus_theme::dim()));
        left.push(Span::styled(format!("{q}█   "), Style::default().fg(c("text"))));
    } else if !app.marked.is_empty() {
        left.push(Span::styled(
            format!("{} marked   ", app.marked.len()),
            locus_theme::st("blue"),
        ));
    }
    let lead = left;
    let mut notes: Vec<String> = Vec::new();
    if let Mode::Search(_) = &app.mode
        && let Some(e) = &app.search_error
    {
        notes.push(e.clone());
    }
    if let Some((m, _)) = &app.message {
        notes.push(m.clone());
    }
    if let Some((_, t)) = &app.hold {
        let left = t - app.now.timestamp();
        if left > 0 {
            notes.push(format!("sending in {left} s · u cancels"));
        }
    }
    if app.message.is_none() {
        if app.pending_sends > 0 {
            notes.push(format!("{} of Claude's drafts · g d", app.pending_sends));
        }
        if app.open_proposals > 0 {
            notes.push(format!("{} to approve · A", app.open_proposals));
        }
        for (name, mins) in &app.stale {
            let ago = if *mins < 120 {
                format!("{mins} min")
            } else {
                format!("{} h", mins / 60)
            };
            notes.push(format!("{} last synced {ago} ago", name.to_lowercase()));
        }
    }
    let text = notes.join("   ");
    // The note wins: a send in its hold must show its cancel key, so hints drop from the end.
    let want = text.width() as u16 + if text.is_empty() { 0 } else { 3 };
    let mut pairs = pairs;
    let left = loop {
        let mut spans = lead.clone();
        spans.extend(locus_theme::hints(&pairs).spans);
        let line = Line::from(spans);
        if pairs.len() <= 2 || line.width() as u16 + want <= area.width {
            break line;
        }
        pairs.pop();
    };
    let used = left.width() as u16;
    f.render_widget(Paragraph::new(left), area);
    let room = area.width.saturating_sub(used + 2);
    if room > 4 && !text.is_empty() {
        let t = fit(&text, room as usize);
        let w = t.width() as u16 + 1;
        f.render_widget(
            Paragraph::new(Span::styled(format!("{t} "), locus_theme::st("peach"))),
            Rect::new(area.x + area.width - w, area.y, w, 1),
        );
    }
}

/// The keys overlay, as locus's: this screen, everywhere.
fn help(f: &mut Frame, area: Rect) {
    let screen: &[(&str, &str)] = &[
        ("↑ ↓  ⏎ esc", "move · read a thread · back"),
        ("⇧↑ ⇧↓", "the top or bottom of the list, or of the thread"),
        ("J K", "the next or previous thread, while reading"),
        ("⇥ ⇧⇥", "the next or previous inbox (all, then each account)"),
        ("⌥⇥", "add the next inbox to the ones shown (⇥ goes back to one)"),
        ("b", "hide or show the list"),
        (
            "g",
            "go to: inbox, needs reply, sent, archive, drafts, filed by itself, an area",
        ),
        ("r R F +", "reply · reply all · forward · write a new mail"),
        ("d", "done: filed for its mark, else for what you choose"),
        ("a I", "archive straight away · back to the inbox"),
        ("u", "undo (a move comes back at once)"),
        ("m", "mark what it is for; it stays in your inbox"),
        ("f U L", "needs reply on or off · unread · unsubscribe"),
        ("t i w", "a task · an invite to the calendar · reply with free times"),
        ("v", "select several for one change"),
        ("z o 1…9", "the whole conversation · an attachment · open link n"),
        ("A ^R", "approve, in locus · sync now"),
    ];
    let everywhere: &[(&str, &str)] = &[
        ("/", "search"),
        ("?", "keys"),
        ("q ^C", "quit"),
        ("alt-1…4", "home · project · calendar · mail"),
    ];
    let mut lines = vec![Line::from(Span::styled(" this screen", locus_theme::dim()))];
    for (k, d) in screen {
        lines.push(Line::from(vec![
            Span::styled(format!("  {k:<12}"), locus_theme::keyst()),
            Span::styled(*d, Style::default().fg(c("text"))),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(" everywhere", locus_theme::dim())));
    for (k, d) in everywhere {
        lines.push(Line::from(vec![
            Span::styled(format!("  {k:<12}"), locus_theme::keyst()),
            Span::styled(*d, Style::default().fg(c("text"))),
        ]));
    }
    let w = 80.min(area.width);
    let h = (lines.len() as u16 + 2).min(area.height);
    let r = centred(area, w, h);
    f.render_widget(Clear, r);
    f.render_widget(Paragraph::new(lines).block(popup_block("keys")), r);
}

/// `g`: where to go, in two columns (round 7): your lists on the left in small groups, the
/// areas on the right in their colours, Personal's streams under their own heading.
fn goto(f: &mut Frame, app: &App, area: Rect) {
    let key = |k: &str| Span::styled(format!(" {k}  "), locus_theme::bold(locus_theme::st("blue")));
    let head = |t: &str| Line::from(Span::styled(format!(" {t}"), locus_theme::dim()));
    // The two columns' titles, bold, so they stand apart from Personal's subheading.
    let title = |t: &str| {
        Line::from(Span::styled(
            format!(" {t}"),
            locus_theme::bold(locus_theme::st("text")),
        ))
    };
    let entry = |k: &str, name: &str, col: ratatui::style::Color| {
        Line::from(vec![key(k), Span::styled(name.to_string(), Style::default().fg(col))])
    };
    let text = c("text");
    let mut left = vec![title("mail")];
    for group in [
        &[("i", "Inbox"), ("r", "Needs reply"), ("d", "Drafts")][..],
        &[("s", "Sent"), ("v", "Archive"), ("a", "All mail")][..],
        // Claude runs (g c) stays reachable but unlisted until the locus job replaces it (X8).
        &[("l", "Filed by itself")][..],
    ] {
        left.push(Line::raw(""));
        left.extend(group.iter().map(|(k, n)| entry(k, n, text)));
    }

    let folders = app.goto_folders();
    let streams: Vec<String> = app
        .shared
        .as_ref()
        .map(|s| {
            s.taxonomy
                .areas
                .iter()
                .filter(|a| a.filter_by_stream)
                .flat_map(|a| a.streams.clone())
                .collect()
        })
        .unwrap_or_default();
    let parent = app.shared.as_ref().and_then(|s| {
        s.taxonomy
            .areas
            .iter()
            .find(|a| a.filter_by_stream)
            .map(|a| a.id.to_lowercase())
    });
    let mut right = vec![
        title(if app.shared.is_some() { "areas" } else { "keys" }),
        Line::raw(""),
    ];
    let mut in_streams = false;
    for (i, (name, col)) in folders.iter().enumerate() {
        if streams.contains(name) && !in_streams {
            in_streams = true;
            right.push(Line::raw(""));
            right.push(head(parent.as_deref().unwrap_or("personal")));
        }
        right.push(entry(&(i + 1).to_string(), name, *col));
    }

    let (lw, rw) = (24u16, 22u16);
    let w = (lw + rw + 3).min(area.width);
    let h = (left.len().max(right.len()) as u16 + 3).min(area.height);
    let r = centred(area, w, h);
    f.render_widget(Clear, r);
    let block = popup_block("go to");
    let inner = block.inner(r);
    f.render_widget(block, r);
    let inner = Rect::new(inner.x, inner.y + 1, inner.width, inner.height.saturating_sub(1));
    f.render_widget(
        Paragraph::new(left),
        Rect::new(inner.x, inner.y, lw.min(inner.width), inner.height),
    );
    if inner.width > lw + 1 {
        for y in inner.y..inner.y + inner.height {
            f.buffer_mut()
                .set_string(inner.x + lw, y, "│", locus_theme::st("surface1"));
        }
        f.render_widget(
            Paragraph::new(right),
            Rect::new(inner.x + lw + 2, inner.y, inner.width - lw - 2, inner.height),
        );
    }
}

/// `o`: the thread's attachments, the highlighted one lit; `⏎` opens it, the list stays.
fn attachments(f: &mut Frame, app: &App, area: Rect) {
    let Some(open) = &app.open else { return };
    let list = open.attachments();
    let w = 64.min(area.width);
    let rows = usize::from(area.height.saturating_sub(6)).clamp(3, 14);
    let h = (list.len().min(rows) as u16 + 4).min(area.height);
    let r = centred(area, w, h);
    f.render_widget(Clear, r);
    let block = popup_block("attachments · ⏎ open · esc back");
    let inner = block.inner(r);
    f.render_widget(block, r);
    let iw = usize::from(inner.width);
    let first = app.att_sel.saturating_sub(rows.saturating_sub(1));
    let mut lines: Vec<Line> = Vec::new();
    for (n, &(mi, ai)) in list.iter().enumerate().skip(first).take(rows) {
        let a = &open.msgs[mi].attachments[ai];
        let chosen = n == app.att_sel;
        let lit = if chosen {
            Style::default()
                .fg(c("text"))
                .bg(fade(c("mauve"), 0.55))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let num = if n < 9 { format!(" {} ", n + 1) } else { "   ".into() };
        let sz = format!("{:>8} ", size(a.size));
        let name = fit(&a.name, iw.saturating_sub(num.len() + sz.len() + 2));
        let gap = iw.saturating_sub(num.len() + name.width() + sz.len());
        lines.push(Line::from(vec![
            Span::styled(num, lit.fg(c("blue")).add_modifier(Modifier::BOLD)),
            Span::styled(name, lit.fg(c("text"))),
            Span::styled(" ".repeat(gap), lit),
            Span::styled(sz, lit.fg(if chosen { c("text") } else { fade(c("text"), 0.4) })),
        ]));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn questions_are_their_sentences_ending_in_a_question_mark() {
        let t = "Thanks for the CV! Could you do a call on Thursday? See https://x.com/a?b=1 and e.g. this. Why?";
        assert_eq!(questions(t), vec!["Could you do a call on Thursday?".to_string()]);
    }

    #[test]
    fn reflow_rejoins_a_client_wrapped_paragraph_but_not_a_list() {
        let l = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let lines = l(&[
            "I completely understand. While I'm disappointed we haven't had the chance",
            "to interview you this time around.",
            "",
            "1. It's completely up to you, you can apply straight as a graduate or as an",
            "intern if you prefer",
            "2. Yes",
        ]);
        assert_eq!(
            reflow(&lines),
            "I completely understand. While I'm disappointed we haven't had the chance to interview you this time around.\n\n1. It's completely up to you, you can apply straight as a graduate or as an intern if you prefer\n2. Yes"
        );
    }

    #[test]
    fn preview_leaves_the_greeting_off() {
        assert_eq!(preview("Hi Milly,\n\nThank you so much."), "Thank you so much.");
        assert_eq!(preview("No, it wouldn't."), "No, it wouldn't.");
    }

    #[test]
    fn history_starts_at_an_attribution_gmail_broke_over_two_lines() {
        let l = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let lines = l(&["Thanks", "", "On Fri, Oct 30, 2026 at 11:30 AM Alec <", "a@b.com> wrote:", "> old"]);
        assert_eq!(history_start(&lines), Some(2));
    }

    #[test]
    fn quotes_fold_into_one_marker() {
        let body =
            "I'll take part two.\n\nOn Mon 12 Oct 2026 at 09:00, Jane Doe wrote:\n> Let's split.\n> Ok?\n\nThanks";
        let parts = fold_quotes(body);
        assert_eq!(
            parts,
            vec![
                Ok("I'll take part two.".into()),
                Ok("".into()),
                Err(3),
                Ok("".into()),
                Ok("Thanks".into())
            ]
        );
        // Outlook: everything from the rule down is the quote.
        let o = fold_quotes("Approved.\n________________________________\nFrom: A\nSent: B\n\nold text");
        assert_eq!(o, vec![Ok("Approved.".into()), Err(5)]);
    }

    #[test]
    fn emoji_variation_selectors_are_dropped_so_every_cell_is_one_width() {
        let mut buf = ratatui::buffer::Buffer::empty(Rect::new(0, 0, 12, 1));
        buf.set_string(0, 0, "ok \u{2714}\u{FE0F} done", ratatui::style::Style::default());
        one_width(&mut buf);
        let text: String = buf.content.iter().map(|c| c.symbol()).collect();
        assert!(!text.contains('\u{FE0F}'), "{text:?}");
        assert!(text.starts_with("ok \u{2714}"), "{text:?}");
    }

    #[test]
    fn sizes() {
        assert_eq!(size(500), "500 B");
        assert_eq!(size(2048), "2 KB");
        assert_eq!(size(3 * 1024 * 1024), "3.0 MB");
    }
}
