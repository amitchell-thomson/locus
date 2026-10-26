//! The approve list (grouped by kind; `Y` approves a group), the live log of automatic actions
//! (`l`, with undo), and ink confirm (C.5): the crop of a mark beside the passages it might point
//! at; a number places it, `0` makes it a general note (none of these passages).

use crate::app::{App, Mark, nav};
use crate::screen::ScreenId;
use crate::ui::home::{approve_one, approve_rows};
use crate::ui::widgets::*;
use crate::ui::{esc_back, frame, hints, lines, title};
use locus_contract::ApproveKind;
use locus_theme::{SEP, glyph};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui_image::picker::Picker;
use ratatui_image::{FilterType, Resize, StatefulImage};

pub const KEYS: &[(&str, &str)] = &[
    ("y n", "approve or refuse"),
    ("Y", "approve the whole group"),
    (
        "← →",
        "a date from mail: which application it is for, when the mail did not say",
    ),
    ("⏎", "an ink item: where does it point?"),
];
pub const LOG_KEYS: &[(&str, &str)] = &[("u", "undo")];
pub const INK_KEYS: &[(&str, &str)] = &[("1 2 3", "it points here"), ("0", "a general note: none of these")];

fn group_name(k: ApproveKind) -> &'static str {
    match k {
        ApproveKind::Filing => "filing",
        ApproveKind::Mail => "mail",
        ApproveKind::Deadline => "deadlines",
        ApproveKind::Ink => "ink",
        ApproveKind::StopAsking => "stop asking?",
        ApproveKind::Date => "dates from mail",
        ApproveKind::Calendar => "calendar changes",
        ApproveKind::MailSend => "mail to send",
        ApproveKind::MailFiling => "mail to file",
    }
}

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let ap = &app.fixtures.approve;
    let inner = frame(f, area, title("approve", &ap.len().to_string()), Some(esc_back()));
    let [body, foot] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    let sel = app.sel(' ');
    let mut out = Vec::new();
    let mut n = 0;
    for (g, group) in ap.groups.iter().enumerate() {
        out.push(Line::from(Span::styled(
            format!(
                " {} {}{SEP}{}",
                glyph::APPROVE,
                group_name(group.kind),
                group.items.len()
            ),
            bold(st("subtext1")),
        )));
        for (i, it) in group.items.iter().enumerate() {
            let k = format!("approve:{g}:{i}");
            let style = match app.marked(&k) {
                Some(Mark::Approved) => st("green"),
                Some(Mark::Rejected) => struck(text()),
                _ => text(),
            };
            let mut left = vec![pointer(n, Some(sel)), Span::styled(it.text.clone(), style)];
            if let Some(to) = &it.to {
                left.push(Span::styled(format!(" → {to}"), st("subtext0")));
            }
            left.push(Span::styled(format!("{SEP}{}", it.from), dim()));
            let right = if it.opens.is_some() { "⏎ confirm " } else { "y n " };
            out.push(row(left, vec![Span::styled(right, keyst())], body.width));
            n += 1;
        }
        out.push(Line::from(""));
    }
    if ap.is_empty() {
        out.push(Line::from(Span::styled("  nothing to approve", dim())));
    }
    lines(f, body, out);
    lines(
        f,
        foot,
        vec![hints(&[("y n", "one"), ("Y", "the group"), ("l", "log")]).right_aligned()],
    );
}

pub fn draw_log(f: &mut Frame, area: Rect, app: &App) {
    let log = &app.fixtures.autolog;
    let today = app.fixtures.now().date_naive();
    let inner = frame(f, area, title("automatic actions", "live"), Some(esc_back()));
    let sel = app.sel(' ');
    let mut out = Vec::new();
    let mut last = None;
    for (i, e) in log.entries.iter().enumerate() {
        let d = e.at.date_naive();
        if last != Some(d) && d != today {
            let label = if (today - d).num_days() == 1 {
                "yesterday".to_string()
            } else {
                day(d)
            };
            out.push(rule(&label, "", None, false, inner.width));
        }
        last = Some(d);
        let undone = app.marked(&format!("log:{i}")) == Some(&Mark::Undone);
        let mut left = vec![
            pointer(i, Some(sel)),
            Span::styled(format!("{} ", hm(e.at)), dim()),
            Span::styled(format!("{} ", glyph::AUTO_ACTION), st("overlay2")),
            Span::styled(e.text.clone(), if undone { struck(text()) } else { text() }),
        ];
        if let Some(since) = e.automatic_since {
            left.push(Span::styled(format!("  (automatic since {})", date(since)), dim()));
        }
        let right = if undone {
            vec![Span::styled("undone ", st("peach"))]
        } else if e.undo {
            vec![Span::styled("u", keyst()), Span::styled(" undo ", dim())]
        } else {
            vec![]
        };
        out.push(row(left, right, inner.width));
    }
    if log.entries.is_empty() {
        out.push(Line::from(Span::styled("  nothing done automatically yet", dim())));
    }
    lines(f, inner, out);
}

/// The ink fixture on screen: the one an approve item opened, else the first.
fn ink_name(app: &App) -> Option<String> {
    app.ink.clone().or_else(|| app.fixtures.ink.keys().next().cloned())
}

pub fn draw_ink(f: &mut Frame, area: Rect, app: &App) {
    let Some(name) = ink_name(app) else {
        let inner = frame(f, area, title("where does this mark point?", ""), Some(esc_back()));
        lines(f, inner, vec![Line::from(Span::styled("  no marks waiting", dim()))]);
        return;
    };
    let Some(ink) = app.fixtures.ink.get(&name) else { return };
    let more = if ink.remaining > 0 {
        format!("{SEP}{} more after this", ink.remaining)
    } else {
        String::new()
    };
    let inner = frame(
        f,
        area,
        title(
            "where does this mark point?",
            &format!("{} p.{}{more}", ink.document, ink.page),
        ),
        Some(esc_back()),
    );
    let n = ink.candidates.len() as u16;
    let [img_r, note_r, cand_r, foot] = Layout::vertical([
        Constraint::Min(8),
        Constraint::Length(2),
        Constraint::Length(n * 3 + 1),
        Constraint::Length(1),
    ])
    .areas(inner);
    match &ink.image {
        Some(file) => draw_image(f, img_r, app, &app.fixtures.dir.join("ink").join(file)),
        None => lines(
            f,
            img_r,
            vec![Line::from(Span::styled("  (no crop for this mark)", dim()))],
        ),
    }
    lines(
        f,
        note_r,
        vec![Line::from(vec![
            Span::styled(format!(" {} the mark ", glyph::INK), st("flamingo")),
            Span::styled(ink.note.clone(), text()),
        ])],
    );
    let chosen = app.marked(&format!("ink:{name}"));
    let mut out = vec![rule("it might point at", "", None, false, cand_r.width)];
    let w = usize::from(cand_r.width).saturating_sub(16);
    for (i, p) in ink.candidates.iter().enumerate() {
        let picked = chosen == Some(&Mark::Ticked) && app.sel(' ') == i;
        for (j, l) in wrap(&p.text, w).into_iter().take(2).enumerate() {
            let lead = if j == 0 {
                vec![
                    Span::styled(format!(" {} ", i + 1), bold(keyst())),
                    Span::styled(format!("{:<10}", p.at), st("subtext1")),
                ]
            } else {
                vec![Span::raw(format!("{:13}", ""))]
            };
            let mut spans = lead;
            spans.push(Span::styled(l, if picked { st("green") } else { text() }));
            out.push(Line::from(spans));
        }
        out.push(Line::from(""));
    }
    lines(f, cand_r, out);
    let n = format!("1-{}", ink.candidates.len().max(1));
    let pairs = [(n.as_str(), "it points here"), ("0", "a general note")];
    lines(f, foot, vec![hints(&pairs).right_aligned()]);
}

/// The crop, through the terminal's image protocol (kitty in Ghostty, passed through tmux), or
/// half blocks when the terminal has none and in `--dump` (spec D6).
fn draw_image(f: &mut Frame, area: Rect, app: &App, path: &std::path::Path) {
    let key = path.display().to_string();
    let mut slot = app.image.borrow_mut();
    if slot.as_ref().map(|(k, _)| k) != Some(&key) {
        match image::ImageReader::open(path)
            .map_err(|e| e.to_string())
            .and_then(|r| r.decode().map_err(|e| e.to_string()))
        {
            Ok(img) => {
                let picker = app.picker.clone().unwrap_or_else(Picker::halfblocks);
                *slot = Some((key.clone(), picker.new_resize_protocol(img)));
            }
            Err(e) => {
                drop(slot);
                lines(
                    f,
                    area,
                    vec![Line::from(Span::styled(
                        format!("  the crop did not load: {e}"),
                        st("red"),
                    ))],
                );
                return;
            }
        }
    }
    if let Some((_, proto)) = slot.as_mut() {
        // As large as the section allows, centred in it (round 0, 20 Oct: "could be larger and
        // centered to fill that section").
        let resize = Resize::Scale(Some(FilterType::CatmullRom));
        let size = proto.size_for(resize.clone(), area.as_size());
        let at = Rect {
            x: area.x + area.width.saturating_sub(size.width) / 2,
            y: area.y + area.height.saturating_sub(size.height) / 2,
            width: size.width.min(area.width),
            height: size.height.min(area.height),
        };
        f.render_stateful_widget(StatefulImage::default().resize(resize), at, proto);
    }
}

pub fn key(app: &mut App, k: KeyEvent) -> bool {
    match app.screen() {
        ScreenId::Approve => {
            let rows = approve_rows(app);
            if let Some(d) = nav(&k) {
                app.move_sel(' ', rows.len(), d);
                return true;
            }
            let Some((key, text, to, opens)) = rows.get(app.sel(' ')).cloned() else {
                return false;
            };
            // A date from mail (spec E6): y puts it on the calendar (or moves its event) through
            // kairosd, n says no for good; outside the live window both only say what they would do.
            let date = key
                .strip_prefix("approve:")
                .and_then(|r| r.split_once(':'))
                .and_then(|(g, i)| {
                    let g: usize = g.parse().ok()?;
                    let i: usize = i.parse().ok()?;
                    app.fixtures
                        .approve
                        .groups
                        .get(g)?
                        .items
                        .get(i)?
                        .date
                        .clone()
                        .map(|d| (g, i, d))
                });
            // Which application a date is for, when the mail did not say (←/→ through them).
            if let (Some((g, i, d)), KeyCode::Left | KeyCode::Right) = (&date, k.code)
                && d.choices.len() > 1
            {
                let n = d.choices.len();
                let at = d.choices.iter().position(|c| *c == d.for_id).unwrap_or(0);
                let next = if k.code == KeyCode::Right {
                    (at + 1) % n
                } else {
                    (at + n - 1) % n
                };
                let id = d.choices[next].clone();
                let name = app.fixtures.registry.get(&id).map_or(id.clone(), |p| p.name.clone());
                let old = app
                    .fixtures
                    .registry
                    .get(&d.for_id)
                    .map_or(d.for_id.clone(), |p| p.name.clone());
                let it = &mut app.fixtures.approve.groups[*g].items[*i];
                it.text = it.text.replacen(&old, &name, 1);
                if let Some(x) = it.date.as_mut() {
                    x.for_id = id;
                }
                return true;
            }
            // Mail from keryx (keryx-joins-locus X2): a draft to send, or a thread to file.
            let mail = key
                .strip_prefix("approve:")
                .and_then(|r| r.split_once(':'))
                .and_then(|(g, i)| {
                    let g: usize = g.parse().ok()?;
                    let i: usize = i.parse().ok()?;
                    app.fixtures
                        .approve
                        .groups
                        .get(g)?
                        .items
                        .get(i)?
                        .mail
                        .clone()
                        .map(|m| (g, i, m))
                });
            if let (Some((g, i, m)), KeyCode::Char(c @ ('y' | 'n'))) = (mail, k.code)
                && let Some(live) = app.mail.clone()
            {
                match live.decide(&m, c == 'y') {
                    Ok(msg) => {
                        app.fixtures.approve.groups[g].items.remove(i);
                        app.fixtures.approve.groups.retain(|x| !x.items.is_empty());
                        app.note = Some(msg);
                    }
                    Err(e) => app.note = Some(format!("keryx said no: {e}")),
                }
                return true;
            }
            // A calendar change from kairos (round 7c): approved or refused here, the one place.
            let changeset = key
                .strip_prefix("approve:")
                .and_then(|r| r.split_once(':'))
                .and_then(|(g, i)| {
                    let g: usize = g.parse().ok()?;
                    let i: usize = i.parse().ok()?;
                    app.fixtures
                        .approve
                        .groups
                        .get(g)?
                        .items
                        .get(i)?
                        .changeset
                        .clone()
                        .map(|c| (g, i, c))
                });
            if let (Some((g, i, id)), KeyCode::Char(c @ ('y' | 'n'))) = (changeset, k.code)
                && let Some(live) = app.calendar.clone()
            {
                match live.decide(&id, c == 'y') {
                    Ok(msg) => {
                        app.fixtures.approve.groups[g].items.remove(i);
                        app.fixtures.approve.groups.retain(|x| !x.items.is_empty());
                        app.note = Some(format!("{}: {msg}", if c == 'y' { "approved" } else { "refused" }));
                    }
                    Err(e) => app.note = Some(format!("kairos said no: {e}")),
                }
                return true;
            }
            if let (Some((g, i, d)), KeyCode::Char(c @ ('y' | 'n'))) = (date, k.code)
                && let Some(live) = app.calendar.clone()
            {
                let r = if c == 'n' {
                    crate::maildates::dismiss(&d.for_id, d.at)
                        .map(|_| "not putting it on the calendar".to_string())
                        .map_err(|e| e.to_string())
                } else {
                    match &d.moves {
                        Some((source, uid)) => live.move_to(source, uid, d.at),
                        None => live.add_for(&d.title, d.at, d.minutes, "Careers", &d.for_id),
                    }
                };
                match r {
                    Ok(msg) => {
                        app.fixtures.approve.groups[g].items.remove(i);
                        app.fixtures.approve.groups.retain(|x| !x.items.is_empty());
                        app.note = Some(msg);
                    }
                    Err(e) => app.note = Some(format!("kairos said no: {e}")),
                }
                return true;
            }
            match k.code {
                KeyCode::Char(c @ ('y' | 'n')) => approve_one(app, key, &text, to.as_deref(), c == 'y'),
                KeyCode::Char('Y') => {
                    let group = key.split(':').nth(1).unwrap_or_default().to_string();
                    // A mail group (X2) is decided in keryx, every item in it.
                    if let (Some(live), Some(g)) = (app.mail.clone(), group.parse::<usize>().ok())
                        && let Some(grp) = app.fixtures.approve.groups.get(g)
                        && grp.items.iter().any(|i| i.mail.is_some())
                    {
                        let all: Vec<_> = grp.items.iter().filter_map(|i| i.mail.clone()).collect();
                        let ok = all.iter().filter(|m| live.decide(m, true).is_ok()).count();
                        app.fixtures.approve.groups.remove(g);
                        app.note = Some(format!("approved the group: {ok} of {}", all.len()));
                        return true;
                    }
                    let mut n = 0;
                    for (k2, t2, to2, _) in rows.iter().filter(|r| r.0.split(':').nth(1) == Some(group.as_str())) {
                        approve_one(app, k2.clone(), t2, to2.as_deref(), true);
                        n += 1;
                    }
                    app.note = Some(format!("approved the group: {n} items"));
                }
                KeyCode::Enter => match opens {
                    Some(name) => {
                        app.ink = Some(name);
                        app.sel.insert((ScreenId::InkConfirm, ' '), 0);
                        app.open(ScreenId::InkConfirm);
                    }
                    None => app.note = Some("y approves, n refuses".into()),
                },
                _ => return false,
            }
            true
        }
        ScreenId::AutoLog => {
            let n = app.fixtures.autolog.entries.len();
            if let Some(d) = nav(&k) {
                app.move_sel(' ', n, d);
                return true;
            }
            if k.code == KeyCode::Char('u') {
                let i = app.sel(' ');
                if let Some(e) = app.fixtures.autolog.entries.get(i).cloned() {
                    if e.undo {
                        app.mark(format!("log:{i}"), Mark::Undone, format!("would undo: {}", e.text));
                    } else {
                        app.note = Some("that one cannot be undone".into());
                    }
                }
                return true;
            }
            false
        }
        _ => {
            let Some(name) = ink_name(app) else { return false };
            let Some(ink) = app.fixtures.ink.get(&name).cloned() else {
                return false;
            };
            match k.code {
                KeyCode::Char(c @ '1'..='9') => {
                    let i = (c as usize) - ('1' as usize);
                    if let Some(p) = ink.candidates.get(i) {
                        app.sel.insert((ScreenId::InkConfirm, ' '), i);
                        app.mark(
                            format!("ink:{name}"),
                            Mark::Ticked,
                            format!("placed on {}: would file the mark there", p.at),
                        );
                    }
                    true
                }
                // 0, none of the passages (x removes everywhere else: L4, 26 Oct).
                KeyCode::Char('0') => {
                    app.mark(
                        format!("ink:{name}"),
                        Mark::Read,
                        "a general note: would file it with the document, not a passage",
                    );
                    true
                }
                _ => false,
            }
        }
    }
}
