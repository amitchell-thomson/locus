//! Drawing (SPEC §8.1). Rows say only what matters: unread is bold and
//! nothing else; icons appear only when they carry information.

use crate::app::{App, Mode, Open};
use crate::theme::{c, centred, fade, fit, pad, panel, popup_block, rgb, selected_style, wrap};
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

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    if area.height < 3 {
        return;
    }
    let main = Rect::new(area.x, area.y, area.width, area.height - 1);
    let status = Rect::new(area.x, area.y + area.height - 1, area.width, 1);
    let reading = matches!(app.mode, Mode::Thread | Mode::Attachments);
    let reviewing = matches!(
        app.mode,
        Mode::Review { .. } | Mode::ReviewChange { .. } | Mode::ConfirmGroup { .. }
    );
    if reviewing {
        if app.split() {
            let lw = (main.width * 3 / 5).max(60);
            let left = Rect::new(main.x, main.y, lw, main.height);
            let right = Rect::new(main.x + lw, main.y, main.width - lw, main.height);
            crate::ui_write::review(f, app, left);
            thread(f, app, right, app.preview.as_ref(), false);
        } else {
            crate::ui_write::review(f, app, main);
        }
    } else if let Mode::ConfirmSend(d) = &app.mode {
        crate::ui_send::confirm_send(f, app, main, &d.req, &d.warnings);
    } else if let Mode::ApproveSend { id } = &app.mode {
        crate::ui_send::approve_send(f, app, main, id);
    } else if matches!(app.mode, Mode::Approvals { .. }) {
        crate::ui_write::approvals(f, app, main);
    } else if app.split() {
        let lw = (main.width * 2 / 5).max(48);
        let left = Rect::new(main.x, main.y, lw, main.height);
        let right = Rect::new(main.x + lw, main.y, main.width - lw, main.height);
        list(f, app, left, !reading);
        let open = if reading {
            app.open.as_ref()
        } else {
            app.preview.as_ref()
        };
        thread(f, app, right, open, reading);
    } else if reading {
        thread(f, app, main, app.open.as_ref(), true);
    } else {
        list(f, app, main, true);
    }
    status_bar(f, app, status);
    match &app.mode {
        Mode::Help => help(f, area),
        Mode::GoTo => goto(f, app, area),
        Mode::Attachments => attachments(f, app, area),
        Mode::Snooze(input) => crate::ui_write::snooze(f, app, area, input.as_deref()),
        Mode::File => crate::ui_write::file(f, app, area),
        Mode::ReviewChange { .. } => crate::ui_write::change(f, app, area),
        Mode::ConfirmGroup { group, count, .. } => crate::ui_write::confirm_group(f, area, group, *count),
        Mode::ConfirmUnsubscribe { how, .. } => crate::ui_send::confirm_unsubscribe(f, area, how),
        _ => {}
    }
}

fn hints_title(app: &App, width: u16) -> Option<Line<'static>> {
    let h = app.key_hints();
    if h.is_empty() || (width as usize) < h.width() + 4 {
        return None;
    }
    Some(
        Line::from(vec![
            Span::raw(" "),
            Span::styled(h, Style::default().fg(c("overlay1"))),
            Span::raw(" "),
        ])
        .right_aligned(),
    )
}

fn account_tag(app: &App, account: &str) -> (String, ratatui::style::Color) {
    match app.cfg.account(account) {
        Some(a) => (a.name.clone(), rgb(a.colour)),
        None => (account.to_string(), c("overlay1")),
    }
}

fn list(f: &mut Frame, app: &App, area: Rect, focus: bool) {
    let title = format!("{} · {}", app.title(), app.account_label());
    let mut block = panel(&title, focus);
    if focus && let Some(h) = hints_title(app, area.width) {
        block = block.title_bottom(h);
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 {
        return;
    }
    if app.rows.is_empty() {
        let msg = match (&app.search_error, &app.search) {
            (Some(e), _) => e.clone(),
            (None, Some(_)) => "No matches.".into(),
            (None, None) if app.view == keryx_store::view::View::Inbox => "Inbox zero.".into(),
            _ => "Nothing here.".into(),
        };
        let y = inner.y + inner.height / 2;
        let x = inner.x + inner.width.saturating_sub(msg.width() as u16) / 2;
        f.buffer_mut()
            .set_stringn(x, y, &msg, inner.width as usize, Style::default().fg(c("overlay1")));
        return;
    }
    let h = inner.height as usize;
    // Keep the selection near the middle, clamped at both ends.
    let first = app.sel.saturating_sub(h / 2).min(app.rows.len().saturating_sub(h));
    let show_account = app.account == 0;
    for (i, r) in app.rows.iter().enumerate().skip(first).take(h) {
        let y = inner.y + (i - first) as u16;
        let line = row_line(app, r, inner.width as usize, show_account, i == app.sel && focus);
        f.buffer_mut().set_line(inner.x, y, &line, inner.width);
    }
}

/// `who (3) · subject · snippet ……… icons date`
pub fn row_line(app: &App, r: &ThreadRow, width: usize, show_account: bool, selected: bool) -> Line<'static> {
    let date = date_label(r.date, app.now);
    let mut right: Vec<(String, Style)> = Vec::new();
    if let Some(until) = r.snoozed_until {
        let label = if app.view == keryx_store::view::View::Snoozed {
            format!("{ICON_SNOOZE} {}", crate::write::when_label(app, until))
        } else {
            ICON_SNOOZE.to_string()
        };
        right.push((label, Style::default().fg(c("sapphire"))));
    }
    if r.flagged {
        right.push((ICON_FLAG.into(), Style::default().fg(c("red"))));
    }
    if r.has_attachment {
        right.push((ICON_ATTACH.into(), Style::default().fg(c("overlay2"))));
    }
    right.push((format!("{date:>6}"), Style::default().fg(c("overlay1"))));
    let right_w: usize = right.iter().map(|(s, _)| s.width() + 1).sum();

    let base = if r.unread {
        Style::default().fg(c("text")).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(c("subtext1"))
    };
    let marked = app.marked.contains(&r.r#ref);
    let mut spans: Vec<Span<'static>> = vec![if marked {
        Span::styled("•", Style::default().fg(c("blue")).add_modifier(Modifier::BOLD))
    } else {
        Span::raw(" ")
    }];
    let mut used = 1;
    if show_account {
        let (name, col) = account_tag(app, &r.account);
        let tag = pad(&name, 8);
        used += tag.width() + 1;
        spans.push(Span::styled(tag, Style::default().fg(col)));
        spans.push(Span::raw(" "));
    }
    let who_w = if width >= 90 { 20 } else { 14 };
    let who = if r.count > 1 {
        format!("{} ({})", r.who, r.count)
    } else {
        r.who.clone()
    };
    let who = pad(&who, who_w);
    used += who.width() + 1;
    spans.push(Span::styled(who, base));
    spans.push(Span::raw(" "));
    let avail = width.saturating_sub(used + right_w + 1);
    let subject = fit(&r.subject, avail);
    let mut rest = avail.saturating_sub(subject.width());
    spans.push(Span::styled(subject, base));
    if r.needs_reply.is_some() && rest > 12 {
        spans.push(Span::styled(" needs reply", Style::default().fg(c("mauve"))));
        rest -= 12;
    }
    for k in &r.keys {
        // In a key's own view its name says nothing.
        if app.view == keryx_store::view::View::Key(k.clone()) {
            continue;
        }
        if rest < k.width() + 2 {
            break;
        }
        let col = app.cfg.key(k).map(|d| rgb(d.colour)).unwrap_or(c("overlay1"));
        spans.push(Span::styled(format!(" {k}"), Style::default().fg(col)));
        rest -= k.width() + 1;
    }
    if rest > 4 && !r.snippet.is_empty() {
        let s = fit(&format!(" · {}", r.snippet), rest);
        rest -= s.width();
        spans.push(Span::styled(s, Style::default().fg(c("overlay0"))));
    }
    spans.push(Span::raw(" ".repeat(rest + 1)));
    for (s, st) in right {
        spans.push(Span::styled(s, st));
        spans.push(Span::raw(" "));
    }
    if selected {
        let sel = selected_style(c("blue"));
        for s in &mut spans {
            s.style = sel;
        }
    }
    Line::from(spans)
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

fn addr_list(v: &[Address]) -> String {
    v.iter().map(Address::display).collect::<Vec<_>>().join(", ")
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

/// The lines of a thread at width `w`.
pub fn thread_lines(app: &App, open: &Open, w: usize) -> Vec<Line<'static>> {
    let tz = app.cfg.tz();
    let mut out: Vec<Line<'static>> = Vec::new();
    let subject = open.msgs.last().map(|m| m.subject.clone()).unwrap_or_default();
    let (acc, col) = account_tag(app, &open.account);
    out.push(Line::from(vec![
        Span::styled(
            fit(&subject, w.saturating_sub(acc.width() + 3)),
            Style::default().fg(c("text")).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(acc, Style::default().fg(col)),
    ]));
    let last = open.msgs.len().saturating_sub(1);
    for (i, m) in open.msgs.iter().enumerate() {
        out.push(Line::raw(""));
        let expanded = i == last || open.expand_all || !m.read;
        if !expanded {
            out.push(collapsed(app, m, w));
            continue;
        }
        message_lines(app, open, i, m, w, tz, &mut out);
    }
    out
}

fn collapsed(app: &App, m: &Message, w: usize) -> Line<'static> {
    let who = m.from.as_ref().map(|a| a.short().to_string()).unwrap_or_default();
    let date = date_label(m.date, app.now);
    let head = format!("{who} · {date}");
    let rest = w.saturating_sub(head.width() + 3);
    Line::from(vec![
        Span::styled(head, Style::default().fg(c("subtext0"))),
        Span::styled(
            fit(&format!(" · {}", m.snippet), rest),
            Style::default().fg(c("overlay0")),
        ),
    ])
}

fn message_lines(
    app: &App,
    open: &Open,
    i: usize,
    m: &Message,
    w: usize,
    tz: chrono_tz::Tz,
    out: &mut Vec<Line<'static>>,
) {
    let from = m
        .from
        .as_ref()
        .map(Address::display)
        .unwrap_or_else(|| "(no sender)".into());
    let date = date_full(m.date, tz);
    let gap = w.saturating_sub(from.width() + date.width());
    out.push(Line::from(vec![
        Span::styled(
            fit(&from, w.saturating_sub(date.width() + 1)),
            Style::default().fg(c("blue")).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" ".repeat(gap.max(1))),
        Span::styled(date, Style::default().fg(c("overlay1"))),
    ]));
    for (label, list) in [("to", &m.to), ("cc", &m.cc)] {
        if list.is_empty() {
            continue;
        }
        for (j, l) in wrap(&addr_list(list), w.saturating_sub(3)).into_iter().enumerate() {
            let prefix = if j == 0 { format!("{label} ") } else { "   ".into() };
            out.push(Line::from(vec![
                Span::styled(prefix, Style::default().fg(c("overlay1"))),
                Span::styled(l, Style::default().fg(c("subtext0"))),
            ]));
        }
    }
    if let Some(Some(inv)) = open.invites.get(i) {
        out.push(Line::raw(""));
        out.extend(invite_lines(app, inv, w));
    }
    if i == open.msgs.len() - 1
        && let Some(why) = app
            .rows
            .iter()
            .find(|r| r.account == open.account && r.thread_id == open.thread_id)
            .and_then(|r| r.needs_reply.as_ref())
    {
        out.push(Line::from(Span::styled(
            format!("needs reply: {why}"),
            Style::default().fg(c("mauve")),
        )));
    }
    if m.hidden_text_removed {
        out.push(Line::from(Span::styled(
            "⚠ hidden text was removed from this message",
            Style::default().fg(c("yellow")),
        )));
    }
    out.push(Line::raw(""));
    if m.body_text.trim().is_empty() {
        let why = if m.attachments.is_empty() && open.invites.get(i).is_none_or(Option::is_none) {
            "(Mail hasn't downloaded this message yet, or it has no text)"
        } else {
            "(no text)"
        };
        out.push(Line::from(Span::styled(why, Style::default().fg(c("overlay1")))));
    } else {
        let body = Style::default().fg(c("text"));
        let quote = Style::default().fg(c("overlay1"));
        if open.show_quotes {
            for l in wrap(&m.body_text, w) {
                let st = if l.trim_start().starts_with('>') { quote } else { body };
                out.push(Line::from(Span::styled(l, st)));
            }
        } else {
            for part in fold_quotes(&m.body_text) {
                match part {
                    Ok(text) => out.extend(wrap(&text, w).into_iter().map(|l| Line::from(Span::styled(l, body)))),
                    Err(n) => out.push(Line::from(Span::styled(
                        format!("… quoted text ({n} line{}) · z show", if n == 1 { "" } else { "s" }),
                        Style::default().fg(c("overlay0")).add_modifier(Modifier::ITALIC),
                    ))),
                }
            }
        }
    }
    if !m.attachments.is_empty() {
        out.push(Line::raw(""));
        for a in &m.attachments {
            out.push(Line::from(vec![
                Span::styled(format!("{ICON_ATTACH} "), Style::default().fg(c("overlay2"))),
                Span::styled(fit(&a.name, w.saturating_sub(14)), Style::default().fg(c("subtext1"))),
                Span::styled(format!(" · {}", size(a.size)), Style::default().fg(c("overlay1"))),
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

fn thread(f: &mut Frame, app: &App, area: Rect, open: Option<&Open>, focus: bool) {
    let mut block = panel("Thread", focus);
    if focus && let Some(h) = hints_title(app, area.width) {
        block = block.title_bottom(h);
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    let Some(open) = open else {
        let msg = "Select a thread";
        let y = inner.y + inner.height / 2;
        let x = inner.x + inner.width.saturating_sub(msg.width() as u16) / 2;
        f.buffer_mut().set_string(x, y, msg, Style::default().fg(c("overlay0")));
        return;
    };
    let w = inner.width.saturating_sub(2) as usize;
    let lines = thread_lines(app, open, w);
    let max_scroll = (lines.len() as u16).saturating_sub(inner.height);
    let p = Paragraph::new(lines).scroll((open.scroll.min(max_scroll), 0));
    f.render_widget(
        p,
        Rect::new(inner.x + 1, inner.y, inner.width.saturating_sub(2), inner.height),
    );
}

fn chip(text: &str, bg: &str) -> Vec<Span<'static>> {
    vec![
        Span::styled(
            format!(" {text} "),
            Style::default().fg(c("base")).bg(c(bg)).add_modifier(Modifier::BOLD),
        ),
        Span::styled("\u{e0b0}", Style::default().fg(c(bg))),
    ]
}

fn status_bar(f: &mut Frame, app: &App, area: Rect) {
    let mut spans: Vec<Span<'static>> = Vec::new();
    match &app.mode {
        Mode::Search(q) => {
            spans.extend(chip("search", "mauve"));
            spans.push(Span::styled(format!(" {q}█"), Style::default().fg(c("text"))));
            if let Some(e) = &app.search_error {
                spans.push(Span::styled(format!("  {e}"), Style::default().fg(c("red"))));
            } else {
                spans.push(Span::styled(
                    format!(
                        "  {} thread{}",
                        app.rows.len(),
                        if app.rows.len() == 1 { "" } else { "s" }
                    ),
                    Style::default().fg(c("overlay1")),
                ));
            }
        }
        _ => {
            if let Some((m, err)) = &app.message {
                spans.push(Span::styled(
                    format!(" {m}"),
                    Style::default().fg(if *err { c("red") } else { c("text") }),
                ));
            }
            if let Some((_, t)) = &app.hold {
                let left = t - app.now.timestamp();
                if left > 0 {
                    spans.push(Span::styled(
                        format!(" Sending in {left} s · u cancel "),
                        Style::default()
                            .fg(c("base"))
                            .bg(c("peach"))
                            .add_modifier(Modifier::BOLD),
                    ));
                }
            }
            if app.pending_sends > 0 && app.message.is_none() {
                spans.push(Span::styled(
                    format!(" {} waiting for approval · A", app.pending_sends),
                    Style::default().fg(c("peach")),
                ));
            }
            if app.open_proposals > 0 && app.message.is_none() {
                spans.push(Span::styled(
                    format!(" Cleanup: {} to review · A", app.open_proposals),
                    Style::default().fg(c("mauve")),
                ));
            }
            // Only what needs attention: stale syncs.
            for (name, mins) in &app.stale {
                let ago = if *mins < 120 {
                    format!("{mins} min")
                } else {
                    format!("{} h", mins / 60)
                };
                spans.push(Span::styled(
                    format!("  {name} last synced {ago} ago"),
                    Style::default().fg(c("peach")),
                ));
            }
        }
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn help(f: &mut Frame, area: Rect) {
    let rows: &[(&str, &str)] = &[
        ("j k  ↑ ↓", "next / previous"),
        ("Enter", "read the thread"),
        ("Esc", "back (clears a search)"),
        ("Tab  Shift-Tab", "account: All · each account"),
        ("g", "go to: Inbox, Needs reply, Flagged, Snoozed, Sent, keys…"),
        (
            "/",
            "search (from: to: subject: account: key: in: is: has: before: after:)",
        ),
        ("z", "thread: show / fold quoted text"),
        ("o", "thread: open an attachment"),
        ("d s m", "archive · snooze · file into a key"),
        ("f U N", "flag · mark unread · needs-reply tag"),
        ("x", "mark threads for a batch (Esc clears)"),
        ("u", "undo the last change"),
        ("A", "cleanups waiting for you"),
        ("J K", "thread: next / previous thread"),
        ("Ctrl-R", "sync now"),
        ("q", "quit"),
    ];
    let w = 78.min(area.width);
    let h = (rows.len() as u16 + 4).min(area.height);
    let r = centred(area, w, h);
    f.render_widget(Clear, r);
    let block = popup_block("keys");
    let inner = block.inner(r);
    f.render_widget(block, r);
    let lines: Vec<Line> = rows
        .iter()
        .map(|(k, d)| {
            Line::from(vec![
                Span::styled(pad(k, 16), Style::default().fg(c("blue")).add_modifier(Modifier::BOLD)),
                Span::styled(
                    fit(d, inner.width.saturating_sub(17) as usize),
                    Style::default().fg(c("text")),
                ),
            ])
        })
        .collect();
    f.render_widget(
        Paragraph::new(lines),
        Rect::new(inner.x + 1, inner.y, inner.width.saturating_sub(1), inner.height),
    );
}

fn goto(f: &mut Frame, app: &App, area: Rect) {
    let mut rows: Vec<(String, String, Option<ratatui::style::Color>)> = vec![
        ("i".into(), "Inbox".into(), None),
        ("r".into(), "Needs reply".into(), None),
        ("f".into(), "Flagged".into(), None),
        ("s".into(), "Snoozed".into(), None),
        ("t".into(), "Sent".into(), None),
        ("v".into(), "Archive".into(), None),
        ("a".into(), "All mail".into(), None),
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
