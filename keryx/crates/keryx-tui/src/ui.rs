//! Drawing (keryx-joins-locus X5, your round of 26 Oct): one grey frame with every inbox named in
//! its top edge (the one shown highlighted, `Tab` moving it), a quiet list of two lines a thread
//! with days as headings and a rule at what came since you last looked, the conversation on the
//! right in the order it was written, and locus's footer. `b` hides the list.

use crate::app::{App, Mode, Open};
use crate::theme::{c, centred, fade, fit, popup_block, rgb, selected_style, wrap};
use keryx_core::display::{date_full, date_label};
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

pub fn draw(f: &mut Frame, app: &App) {
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
    } else {
        mail(f, app, main);
    }
    footer(f, app, foot);
    match &app.mode {
        Mode::Help => help(f, area),
        Mode::GoTo => goto(f, app, area),
        Mode::Attachments => attachments(f, app, area),
        Mode::Snooze(input) => crate::ui_write::snooze(f, app, area, input.as_deref()),
        Mode::File => crate::ui_write::file(f, app, area),
        Mode::ConfirmUnsubscribe { how, .. } => crate::ui_send::confirm_unsubscribe(f, area, how),
        Mode::KairosForm(form) => crate::ui_kairos::form(f, app, area, form),
        _ => {}
    }
}

/// The top edge: ` mail ` and every inbox, the one shown highlighted, the others greyed.
fn header(app: &App) -> Line<'static> {
    let mut spans = vec![Span::styled(" mail ", locus_theme::bold(locus_theme::st("text")))];
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
    let names: Vec<String> = std::iter::once("all".to_string())
        .chain(app.cfg.accounts.iter().map(|a| a.name.to_lowercase()))
        .collect();
    spans.push(Span::raw(" "));
    for (i, n) in names.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", locus_theme::st("overlay0")));
        }
        spans.push(if i == app.account {
            Span::styled(n, locus_theme::bold(locus_theme::st("blue")))
        } else {
            Span::styled(n, locus_theme::st("overlay0"))
        });
    }
    spans.push(Span::raw(" "));
    Line::from(spans)
}

/// The list and the reading pane in one frame.
fn mail(f: &mut Frame, app: &App, area: Rect) {
    let new = app.new_count();
    let right = (new > 0).then(|| Line::from(Span::styled(format!(" {new} new "), locus_theme::st("peach"))));
    let block = locus_theme::screen(header(app), right);
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
    if app.list_hidden || (!app.split() && reading) {
        thread(f, app, inner, open);
        return;
    }
    if !app.split() {
        list(f, app, inner);
        return;
    }
    let lw = (inner.width / 3).clamp(30, 46);
    let left = Rect::new(inner.x, inner.y, lw, inner.height);
    let rule = Rect::new(inner.x + lw, inner.y, 1, inner.height);
    let right = Rect::new(inner.x + lw + 1, inner.y, inner.width - lw - 1, inner.height);
    list(f, app, left);
    for y in rule.y..rule.y + rule.height {
        f.buffer_mut().set_string(rule.x, y, "│", locus_theme::st("surface1"));
    }
    thread(f, app, right, open);
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
    for (i, r) in app.rows.iter().enumerate() {
        let d = day_heading(r.date, app.now);
        if since.is_some_and(|s| r.date <= s) && !since_drawn && i > 0 {
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
    let w = area.width as usize;
    let focus = !matches!(app.mode, Mode::Thread | Mode::Attachments);
    for (k, row) in rows.iter().enumerate().skip(first).take(h) {
        let y = area.y + (k - first) as u16;
        let line = match row {
            Row::Day(d) => Line::from(Span::styled(
                format!(" {d}"),
                locus_theme::bold(locus_theme::st("overlay1")),
            )),
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
                    let st = if focus {
                        selected_style(c("blue"))
                    } else {
                        Style::default().bg(c("surface0"))
                    };
                    l.patch_style(st)
                } else {
                    l
                }
            }
        };
        f.buffer_mut().set_line(area.x, y, &line, area.width);
    }
}

/// A thread's first line: unread dot, who (and how many), what it carries, when.
fn row_who(app: &App, r: &ThreadRow, width: usize) -> Line<'static> {
    let date = date_label(r.date, app.now).to_lowercase();
    let mut right: Vec<Span<'static>> = Vec::new();
    if r.suggested {
        right.push(Span::styled(format!("{ICON_TASK}? "), Style::default().fg(c("mauve"))));
    } else if r.linked {
        right.push(Span::styled(format!("{ICON_TASK} "), Style::default().fg(c("green"))));
    }
    if r.flagged {
        right.push(Span::styled(format!("{ICON_FLAG} "), Style::default().fg(c("red"))));
    }
    if r.has_attachment {
        right.push(Span::styled(
            format!("{ICON_ATTACH} "),
            Style::default().fg(c("overlay1")),
        ));
    }
    right.push(Span::styled(format!("{date} "), locus_theme::dim()));
    let rw: usize = right.iter().map(|s| s.content.width()).sum();
    let marked = app.marked.contains(&r.r#ref);
    let lead = if marked {
        Span::styled(" ▸", locus_theme::st("blue"))
    } else if r.unread {
        Span::styled(" ●", locus_theme::st("peach"))
    } else {
        Span::raw("  ")
    };
    let who = if r.count > 1 {
        format!("{} ({})", r.who, r.count)
    } else {
        r.who.clone()
    };
    let room = width.saturating_sub(3 + rw);
    let who = fit(&who, room);
    let gap = room.saturating_sub(who.width());
    let who_style = if r.unread {
        Style::default().fg(c("text"))
    } else {
        Style::default().fg(c("subtext0"))
    };
    let mut spans = vec![
        lead,
        Span::raw(" "),
        Span::styled(who, who_style),
        Span::raw(" ".repeat(gap)),
    ];
    spans.extend(right);
    Line::from(spans)
}

/// A thread's second line: the subject, quieter, and its account in the all view.
fn row_subject(app: &App, r: &ThreadRow, width: usize) -> Line<'static> {
    let mut right: Vec<Span<'static>> = Vec::new();
    if r.needs_reply.is_some() {
        right.push(Span::styled("↩ ", Style::default().fg(c("mauve"))));
    }
    if app.account == 0
        && let Some(a) = app.cfg.account(&r.account)
    {
        right.push(Span::styled(
            format!("{} ", a.name.to_lowercase()),
            Style::default().fg(fade(rgb(a.colour), 0.35)),
        ));
    }
    let rw: usize = right.iter().map(|s| s.content.width()).sum();
    let room = width.saturating_sub(5 + rw);
    let subject = fit(&r.subject, room);
    let gap = room.saturating_sub(subject.width());
    let mut spans = vec![
        Span::raw("    "),
        Span::styled(subject, locus_theme::st("overlay1")),
        Span::raw(" ".repeat(gap + 1)),
    ];
    spans.extend(right);
    Line::from(spans)
}

/// Kept for tests and the runs view: one thread as a single line.
pub fn row_line(app: &App, r: &ThreadRow, width: usize, _show_account: bool, selected: bool) -> Line<'static> {
    let l = row_who(app, r, width);
    if selected {
        l.patch_style(selected_style(c("blue")))
    } else {
        l
    }
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
fn is_me(app: &App, a: &Address) -> bool {
    let e = a.email.to_lowercase();
    app.cfg
        .accounts
        .iter()
        .any(|x| x.address.eq_ignore_ascii_case(&e) || x.aliases.iter().any(|y| y.eq_ignore_ascii_case(&e)))
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
pub fn thread_lines(app: &App, open: &Open, w: usize) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let subject = open.msgs.last().map(|m| m.subject.clone()).unwrap_or_default();
    let (acc, col) = account_tag(app, &open.account);
    let acc = acc.to_lowercase();
    out.push(Line::from(vec![
        Span::styled(
            fit(&subject, w.saturating_sub(acc.width() + 3)),
            Style::default().fg(c("text")).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(acc, Style::default().fg(fade(col, 0.2))),
    ]));
    if open.msgs.len() > 1 {
        let mut people: Vec<String> = Vec::new();
        for m in &open.msgs {
            if let Some(a) = &m.from {
                let n = if is_me(app, a) {
                    "you".to_string()
                } else {
                    a.short().to_string()
                };
                if !people.contains(&n) {
                    people.push(n);
                }
            }
        }
        out.push(Line::from(Span::styled(
            format!("{} messages · {}", open.msgs.len(), people.join(", ")),
            locus_theme::dim(),
        )));
    }
    out.extend(crate::ui_kairos::thread_lines(app, open, w));
    let last = open.msgs.len().saturating_sub(1);
    for (i, m) in open.msgs.iter().enumerate() {
        out.push(Line::raw(""));
        message_lines(app, open, i, m, w, i == last || open.expand_all || !m.read, &mut out);
    }
    out
}

fn message_lines(app: &App, open: &Open, i: usize, m: &Message, w: usize, full: bool, out: &mut Vec<Line<'static>>) {
    let tz = app.cfg.tz();
    let col = voice(app, m.from.as_ref());
    let bar = || Span::styled("▎ ", Style::default().fg(col));
    let body_w = w.saturating_sub(2);
    let who = match &m.from {
        Some(a) if is_me(app, a) => "you".to_string(),
        Some(a) => a.display(),
        None => "(no sender)".into(),
    };
    let date = date_full(m.date, tz).to_lowercase();
    out.push(Line::from(vec![
        bar(),
        Span::styled(
            fit(&who, body_w.saturating_sub(date.width() + 3)),
            Style::default().fg(col).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" · {date}"), locus_theme::dim()),
    ]));
    // Who it went to, when that is not just you.
    let others: Vec<&Address> = m.to.iter().chain(&m.cc).filter(|a| !is_me(app, a)).collect();
    if !others.is_empty() && m.from.as_ref().is_some_and(|a| is_me(app, a)) || others.len() > 1 {
        let to = others
            .iter()
            .map(|a| a.short().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        out.push(Line::from(vec![
            bar(),
            Span::styled(fit(&format!("to {to}"), body_w), locus_theme::dim()),
        ]));
    }
    if let Some(Some(inv)) = open.invites.get(i) {
        for l in invite_lines(app, inv, body_w) {
            let mut spans = vec![bar()];
            spans.extend(l.spans);
            out.push(Line::from(spans));
        }
    }
    if i == open.msgs.len() - 1
        && let Some(why) = app
            .rows
            .iter()
            .find(|r| r.account == open.account && r.thread_id == open.thread_id)
            .and_then(|r| r.needs_reply.as_ref())
    {
        out.push(Line::from(vec![
            bar(),
            Span::styled(format!("needs reply: {why}"), Style::default().fg(c("mauve"))),
        ]));
    }
    if m.hidden_text_removed {
        out.push(Line::from(vec![
            bar(),
            Span::styled("⚠ hidden text was removed", Style::default().fg(c("yellow"))),
        ]));
    }
    out.push(Line::from(vec![bar()]));
    if m.body_text.trim().is_empty() {
        let why = if m.attachments.is_empty() && open.invites.get(i).is_none_or(Option::is_none) {
            "(Mail hasn't downloaded this message yet, or it has no text)"
        } else {
            "(no text)"
        };
        out.push(Line::from(vec![bar(), Span::styled(why, locus_theme::dim())]));
    } else {
        let automated = !m.list_headers.is_empty();
        let r = keryx_core::text::readable(&m.body_text, automated);
        let body = Style::default().fg(c("text"));
        let mut shown: Vec<Line<'static>> = Vec::new();
        if open.show_quotes {
            for l in wrap(&r.lines.join("\n"), body_w) {
                let st = if l.trim_start().starts_with('>') {
                    locus_theme::dim()
                } else {
                    body
                };
                shown.push(Line::from(Span::styled(l, st)));
            }
        } else {
            for part in fold_quotes(&r.lines.join("\n")) {
                match part {
                    Ok(text) => shown.extend(
                        wrap(&text, body_w)
                            .into_iter()
                            .map(|l| Line::from(Span::styled(l, body))),
                    ),
                    Err(n) => shown.push(Line::from(Span::styled(
                        format!("… what it quoted ({n} line{}) · z shows", if n == 1 { "" } else { "s" }),
                        Style::default().fg(c("overlay0")).add_modifier(Modifier::ITALIC),
                    ))),
                }
            }
        }
        while shown.last().is_some_and(|l| l.width() == 0) {
            shown.pop();
        }
        // An earlier message keeps its first lines; z shows it all.
        let keep = if full { shown.len() } else { 3 };
        let more = shown.len().saturating_sub(keep);
        for l in shown.into_iter().take(keep) {
            let mut spans = vec![bar()];
            spans.extend(l.spans);
            out.push(Line::from(spans));
        }
        if more > 0 {
            out.push(Line::from(vec![
                bar(),
                Span::styled(
                    format!("… {more} more line{} · z shows", if more == 1 { "" } else { "s" }),
                    Style::default().fg(c("overlay0")).add_modifier(Modifier::ITALIC),
                ),
            ]));
        }
        if full && r.footer > 0 {
            out.push(Line::from(vec![
                bar(),
                Span::styled(
                    format!("… its footer folded ({} lines: unsubscribe, address)", r.footer),
                    Style::default().fg(c("overlay0")).add_modifier(Modifier::ITALIC),
                ),
            ]));
        }
        if full && !r.links.is_empty() {
            let items: Vec<String> = r.links.iter().map(|(n, _, s)| format!("[{n}] {s}")).collect();
            for l in wrap(&items.join("  "), body_w) {
                out.push(Line::from(vec![bar(), Span::styled(l, locus_theme::dim())]));
            }
        }
    }
    if full && !m.attachments.is_empty() {
        for a in &m.attachments {
            out.push(Line::from(vec![
                bar(),
                Span::styled(format!("{ICON_ATTACH} "), Style::default().fg(c("overlay2"))),
                Span::styled(
                    fit(&a.name, body_w.saturating_sub(14)),
                    Style::default().fg(c("subtext1")),
                ),
                Span::styled(format!(" · {}", size(a.size)), locus_theme::dim()),
            ]));
        }
    }
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
    let max_scroll = (lines.len() as u16).saturating_sub(inner.height);
    f.render_widget(Paragraph::new(lines).scroll((open.scroll.min(max_scroll), 0)), inner);
}

/// locus's footer: the keys that work here on the left; on the right the note, a send in its
/// hold, what waits in Approve, a stale sync.
fn footer(f: &mut Frame, app: &App, area: Rect) {
    let pairs: Vec<(&str, &str)> = match &app.mode {
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
                ("m", "file"),
                ("esc", "back"),
            ]
        }
        Mode::Thread => vec![
            ("r", "reply"),
            ("R", "all"),
            ("F", "forward"),
            ("d", "done"),
            ("m", "file"),
            ("z", "everything"),
            ("o", "attachment"),
            ("esc", "back"),
        ],
        _ if !app.marked.is_empty() => vec![("d", "done"), ("m", "file"), ("f", "flag"), ("esc", "unmark")],
        _ => vec![
            ("⏎", "open"),
            ("r", "reply"),
            ("d", "done"),
            ("m", "file"),
            ("b", "list"),
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
        let waiting = app.pending_sends + app.open_proposals;
        if waiting > 0 {
            notes.push(format!("{waiting} to approve · A"));
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
        ("⇥ ⇧⇥", "the next or previous inbox (all, then each account)"),
        ("b", "hide or show the list"),
        ("g", "go to a list: inbox, needs reply, flagged, sent, runs"),
        ("r R F +", "reply · reply all · forward · write a new mail"),
        ("d m", "done (archive) · file it"),
        ("f U N L", "flag · unread · needs-reply mark · unsubscribe"),
        ("t i w", "a task · an invite to the calendar · reply with free times"),
        ("v u", "mark for a batch · undo the last change"),
        ("z o", "the whole conversation · open an attachment"),
        ("A ^R", "approve, in locus · sync now"),
    ];
    let everywhere: &[(&str, &str)] = &[
        ("/", "search"),
        ("?", "keys"),
        ("q", "quit"),
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

fn goto(f: &mut Frame, app: &App, area: Rect) {
    let mut rows: Vec<(String, String, Option<ratatui::style::Color>)> = vec![
        ("i".into(), "Inbox".into(), None),
        ("r".into(), "Needs reply".into(), None),
        ("f".into(), "Flagged".into(), None),
        ("t".into(), "Sent".into(), None),
        ("v".into(), "Archive".into(), None),
        ("a".into(), "All mail".into(), None),
        ("c".into(), "Claude runs".into(), None),
    ];
    for (i, k) in app.cfg.keys.iter().enumerate().take(9) {
        rows.push(((i + 1).to_string(), k.name.clone(), Some(rgb(k.colour))));
    }
    let w = 34.min(area.width);
    let h = (rows.len() as u16 + 2).min(area.height);
    let r = centred(area, w, h);
    f.render_widget(Clear, r);
    let block = popup_block("go to");
    let inner = block.inner(r);
    f.render_widget(block, r);
    let lines: Vec<Line> = rows
        .into_iter()
        .map(|(k, name, col)| {
            Line::from(vec![
                Span::styled(
                    format!(" {k}  "),
                    Style::default().fg(c("blue")).add_modifier(Modifier::BOLD),
                ),
                Span::styled(name, Style::default().fg(col.unwrap_or(c("text")))),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

fn attachments(f: &mut Frame, app: &App, area: Rect) {
    let Some(open) = &app.open else { return };
    let list = open.attachments();
    let w = 60.min(area.width);
    let h = (list.len().min(9) as u16 + 2).min(area.height);
    let r = centred(area, w, h);
    f.render_widget(Clear, r);
    let block = popup_block("open which attachment?");
    let inner = block.inner(r);
    f.render_widget(block, r);
    let lines: Vec<Line> = list
        .iter()
        .take(9)
        .enumerate()
        .map(|(n, &(mi, ai))| {
            let a = &open.msgs[mi].attachments[ai];
            Line::from(vec![
                Span::styled(
                    format!(" {}  ", n + 1),
                    Style::default().fg(c("blue")).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    fit(&a.name, inner.width.saturating_sub(16) as usize),
                    Style::default().fg(c("text")),
                ),
                Span::styled(
                    format!(" · {}", size(a.size)),
                    Style::default().fg(fade(c("text"), 0.4)),
                ),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn sizes() {
        assert_eq!(size(500), "500 B");
        assert_eq!(size(2048), "2 KB");
        assert_eq!(size(3 * 1024 * 1024), "3.0 MB");
    }
}
