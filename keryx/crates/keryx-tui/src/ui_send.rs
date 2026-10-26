//! Drawing for Phase 6: the confirm screen (Alec's sends), the approval
//! screen (Claude's drafts) and the unsubscribe confirm. Recipients come
//! first, one per line, bold; warnings are impossible to miss.

use crate::app::App;
use crate::theme::{c, centred, fit, panel, popup_block, wrap};
use keryx_core::changeset::Outgoing;
use keryx_core::model::Address;
use keryx_store::api::SendReq;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

fn label(s: &str) -> Span<'static> {
    Span::styled(format!("{s:<9}"), Style::default().fg(c("overlay1")))
}

fn recipients(out: &mut Vec<Line<'static>>, name: &str, v: &[Address]) {
    if v.is_empty() {
        return;
    }
    for (i, a) in v.iter().enumerate() {
        out.push(Line::from(vec![
            if i == 0 { label(name) } else { label("") },
            Span::styled(a.display(), Style::default().fg(c("text")).add_modifier(Modifier::BOLD)),
        ]));
    }
}

/// The lines both screens share.
#[allow(clippy::too_many_arguments)]
fn message_lines(
    app: &App,
    account: &str,
    to: &[Address],
    cc: &[Address],
    bcc: &[Address],
    attachments: &[(String, u64)],
    subject: &str,
    body: &str,
    warnings: &[String],
    w: usize,
) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let from = app
        .cfg
        .account(account)
        .map(|a| format!("{} · {}", a.name, a.address))
        .unwrap_or_else(|| account.to_string());
    out.push(Line::from(vec![
        label("From"),
        Span::styled(from, Style::default().fg(c("blue"))),
    ]));
    recipients(&mut out, "To", to);
    recipients(&mut out, "Cc", cc);
    recipients(&mut out, "Bcc", bcc);
    for (i, (name, size)) in attachments.iter().enumerate() {
        out.push(Line::from(vec![
            if i == 0 { label("Attach") } else { label("") },
            Span::styled(
                format!("{name} · {}", crate::ui::size(*size)),
                Style::default().fg(c("subtext1")),
            ),
        ]));
    }
    for wn in warnings {
        out.push(Line::from(Span::styled(
            wn.clone(),
            Style::default().fg(c("yellow")).add_modifier(Modifier::BOLD),
        )));
    }
    out.push(Line::from(vec![
        label("Subject"),
        Span::styled(subject.to_string(), Style::default().fg(c("text"))),
    ]));
    out.push(Line::from(Span::styled(
        "─".repeat(w.min(60)),
        Style::default().fg(c("surface2")),
    )));
    for l in wrap(body, w) {
        let st = if l.trim_start().starts_with('>') {
            c("overlay1")
        } else {
            c("text")
        };
        out.push(Line::from(Span::styled(l, Style::default().fg(st))));
    }
    let urls = keryx_core::outgoing::urls(body);
    if !urls.is_empty() {
        out.push(Line::raw(""));
        out.push(Line::from(Span::styled(
            "Links in this message:",
            Style::default().fg(c("overlay1")),
        )));
        for (i, u) in urls.iter().enumerate() {
            out.push(Line::from(Span::styled(
                fit(&format!("[{}] {u}", i + 1), w),
                Style::default().fg(c("sapphire")),
            )));
        }
    }
    out
}

fn screen(f: &mut Frame, area: Rect, title: &str, hints: &str, lines: Vec<Line<'static>>) {
    let block = panel(title, true).title_bottom(
        Line::from(vec![
            Span::raw(" "),
            Span::styled(
                hints.to_string(),
                Style::default().fg(c("text")).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
        ])
        .right_aligned(),
    );
    let inner = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    f.render_widget(
        Paragraph::new(lines),
        Rect::new(inner.x + 1, inner.y, inner.width.saturating_sub(2), inner.height),
    );
}

pub fn confirm_send(f: &mut Frame, app: &App, area: Rect, r: &SendReq, warnings: &[String]) {
    let w = area.width.saturating_sub(4) as usize;
    let atts: Vec<(String, u64)> = r
        .attachments
        .iter()
        .map(|p| {
            let path = std::path::Path::new(p);
            (
                path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
            )
        })
        .collect();
    let lines = message_lines(
        app, &r.account, &r.to, &r.cc, &r.bcc, &atts, &r.subject, &r.body, warnings, w,
    );
    let hold = app.cfg.send.hold_seconds;
    screen(
        f,
        area,
        "Send?",
        &format!("y send ({hold} s to cancel) · e edit · n don't send"),
        lines,
    );
}

pub fn approve_send(f: &mut Frame, app: &App, area: Rect, id: &str) {
    let Some(cs) = app.db.changeset(id).ok().flatten() else {
        return;
    };
    let Some(o): Option<&Outgoing> = cs.outgoing() else {
        return;
    };
    let w = area.width.saturating_sub(4) as usize;
    // Warnings again now (the thread may have moved on since drafting).
    let sent_to = keryx_store::view::sent_to(&app.db, &app.cfg).unwrap_or_default();
    let thread = match &o.thread_id {
        Some(t) => app.db.thread(&o.account, t).unwrap_or_default(),
        None => vec![],
    };
    let warnings = keryx_core::outgoing::warnings(o, &sent_to, &app.cfg.own_addresses(), &thread, cs.created_at);
    let atts: Vec<(String, u64)> = o.attachments.iter().map(|a| (a.name.clone(), a.size)).collect();
    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!("Drafted by {} · ", cs.author.as_str()),
            Style::default().fg(c("mauve")).add_modifier(Modifier::BOLD),
        ),
        Span::styled(cs.summary.clone(), Style::default().fg(c("subtext1"))),
    ])];
    lines.push(Line::raw(""));
    lines.extend(message_lines(
        app, &o.account, &o.to, &o.cc, &o.bcc, &atts, &o.subject, &o.body, &warnings, w,
    ));
    screen(
        f,
        area,
        "Approve this send?",
        "y approve · n reject · e edit · Esc back",
        lines,
    );
}

pub fn confirm_unsubscribe(f: &mut Frame, area: Rect, how: &str) {
    let r = centred(area, 70.min(area.width), 5);
    f.render_widget(Clear, r);
    let block = popup_block("unsubscribe?");
    let inner = block.inner(r);
    f.render_widget(block, r);
    let lines = vec![
        Line::from(Span::styled(format!(" {how}"), Style::default().fg(c("text")))),
        Line::raw(""),
        Line::from(vec![
            Span::styled(" y ", Style::default().fg(c("blue")).add_modifier(Modifier::BOLD)),
            Span::raw("unsubscribe   "),
            Span::styled("Esc ", Style::default().fg(c("blue")).add_modifier(Modifier::BOLD)),
            Span::raw("no"),
        ]),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}
