//! The approve list (grouped by kind; `Y` approves a group), the live log of automatic actions
//! (`l`, with undo), and ink confirm (C.5): the crop of a mark beside the passages it might point
//! at; a number places it, `0` makes it a general note (none of these passages).

use crate::app::{App, Mark, nav};
use crate::screen::ScreenId;
use crate::ui::home::{approve_one, approve_rows};
use crate::ui::widgets::*;
use crate::ui::{esc_back, frame, hints, lines, title};
use locus_contract::{ApproveDetail, ApproveGroup, ApproveItem, ApproveKind};
use locus_theme::{SEP, c, glyph};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui_image::picker::Picker;
use ratatui_image::{FilterType, Resize, StatefulImage};
use unicode_width::UnicodeWidthStr;

pub const KEYS: &[(&str, &str)] = &[
    ("y n", "approve or refuse; the next one comes up"),
    ("Y", "approve the whole group"),
    ("⏎", "the item whole (an ink item: where does it point?)"),
    ("PgUp PgDn", "a page at a time; Home End the ends"),
    (
        "← →",
        "a date from mail: which application it is for, when the mail did not say",
    ),
];
pub const ITEM_KEYS: &[(&str, &str)] = &[("y n", "approve or refuse, then the next"), ("↑ ↓", "scroll")];
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

/// The badge for an item: its detail's action, else a word for its kind.
fn badge_text(kind: ApproveKind, it: &ApproveItem) -> String {
    if let Some(d) = &it.detail {
        return d.action.clone();
    }
    match kind {
        ApproveKind::Filing => "FILE",
        ApproveKind::Mail => "MAIL",
        ApproveKind::Deadline => "DEADLINE",
        ApproveKind::Ink => "INK",
        ApproveKind::StopAsking => "STOP ASKING?",
        ApproveKind::Date => "DATE",
        ApproveKind::Calendar => "CALENDAR",
        ApproveKind::MailSend => "SEND",
        ApproveKind::MailFiling => "MAIL",
    }
    .into()
}

/// A colour per kind of action, so a page of them reads at a glance: archive quiet, filing blue,
/// the calendar peach, ink flamingo, anything else mauve.
fn badge_colour(badge: &str) -> &'static str {
    let b = badge.to_uppercase();
    if b.starts_with("ARCHIVE") {
        "overlay2"
    } else if b.starts_with("FILE") {
        "blue"
    } else if b.starts_with("SNOOZE") {
        "yellow"
    } else if b.contains("EVENT") || b.starts_with("CALENDAR") || b.starts_with("DATE") || b.starts_with("DEADLINE") {
        "peach"
    } else if b.starts_with("INK") {
        "flamingo"
    } else {
        "mauve"
    }
}

fn badge(text: &str, width: usize) -> Span<'static> {
    let t = crate::ui::truncate(text, width.saturating_sub(2));
    let pad = width.saturating_sub(t.width() + 2);
    Span::styled(
        format!(" {t}{} ", " ".repeat(pad)),
        Style::default()
            .fg(c("base"))
            .bg(c(badge_colour(text)))
            .add_modifier(Modifier::BOLD),
    )
}

fn group_label(g: &ApproveGroup) -> String {
    g.label.clone().unwrap_or_else(|| group_name(g.kind).to_string())
}

/// Text wrapped to `width`, keeping its own line breaks (one blank line at most in a row).
fn wrap_keep(s: &str, width: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for para in s.lines() {
        if para.trim().is_empty() {
            if out.last().is_some_and(|l| !l.is_empty()) {
                out.push(String::new());
            }
        } else {
            out.extend(wrap(para.trim_end(), width.max(10)));
        }
    }
    out
}

/// The selected item: (group, item).
fn selected(app: &App, sel: usize) -> Option<(usize, usize)> {
    let mut n = 0;
    for (g, group) in app.fixtures.approve.groups.iter().enumerate() {
        if sel < n + group.items.len() {
            return Some((g, sel - n));
        }
        n += group.items.len();
    }
    None
}

/// The approve list (10 Nov redesign): the action first as a coloured badge, then who and what,
/// in columns; a group per kind of action; the list follows the cursor (it did not scroll at
/// all before, so 750 of 784 proposals could not be reached); beside it, when the window is
/// wide enough, the selected item whole.
pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let ap = &app.fixtures.approve;
    let total = ap.len();
    let inner = frame(f, area, title("approve", &total.to_string()), Some(esc_back()));
    let [body, foot] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    let split = body.width >= 100 && total > 0;
    let (list_r, detail_r) = if split {
        let [l, r] = Layout::horizontal([Constraint::Percentage(56), Constraint::Percentage(44)]).areas(body);
        (l, Some(r))
    } else {
        (body, None)
    };
    let sel = app.sel(' ').min(total.saturating_sub(1));
    let w = usize::from(list_r.width);
    let bw = ap
        .groups
        .iter()
        .flat_map(|g| g.items.iter().map(move |it| badge_text(g.kind, it).width() + 2))
        .max()
        .unwrap_or(8)
        .clamp(8, 18);
    let who_w = (w / 5).clamp(10, 22);
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut at_sel = 0;
    let mut n = 0;
    for (g, group) in ap.groups.iter().enumerate() {
        if g > 0 {
            out.push(Line::from(""));
        }
        let decided_here = group
            .items
            .iter()
            .enumerate()
            .filter(|(i, _)| app.marked(&format!("approve:{g}:{i}")).is_some())
            .count();
        let mut head = vec![Span::styled(
            format!(" {} {}", glyph::APPROVE, group_label(group)),
            bold(st("subtext1")),
        )];
        head.push(Span::styled(
            format!("{SEP}{}", group.items.len() - decided_here),
            dim(),
        ));
        out.push(row(
            head,
            vec![Span::styled("Y ", keyst()), Span::styled("all ", dim())],
            list_r.width,
        ));
        for (i, it) in group.items.iter().enumerate() {
            let k = format!("approve:{g}:{i}");
            let mark = app.marked(&k);
            let is_sel = n == sel;
            if is_sel {
                at_sel = out.len();
            }
            let base = match mark {
                Some(Mark::Approved) => st("green"),
                Some(Mark::Rejected) => struck(text()),
                _ => text(),
            };
            let (who, what) = match &it.detail {
                Some(d) if !d.who.is_empty() => (d.who.clone(), d.subject.clone()),
                _ => (String::new(), it.text.clone()),
            };
            let mut left = vec![
                pointer(n, Some(sel)),
                badge(&badge_text(group.kind, it), bw),
                Span::raw(" "),
            ];
            if !who.is_empty() {
                let wcut = crate::ui::truncate(&who, who_w);
                let pad = who_w.saturating_sub(wcut.width());
                left.push(Span::styled(format!("{wcut}{} ", " ".repeat(pad)), bold(base)));
            }
            left.push(Span::styled(what, base));
            let right = match mark {
                Some(Mark::Approved) => vec![Span::styled("approved ", st("green"))],
                Some(Mark::Rejected) => vec![Span::styled("refused ", st("red"))],
                _ => vec![],
            };
            out.push(row(left, right, list_r.width));
            n += 1;
        }
    }
    if ap.is_empty() {
        out.push(Line::from(Span::styled("  nothing to approve", dim())));
    }
    // Follow the cursor: keep it a few rows from either edge (and show its group's heading
    // when it is the first item).
    let h = usize::from(list_r.height);
    let margin = 3.min(h / 4);
    let scroll = app.approve_scroll.get();
    let mut top = scroll.min(out.len().saturating_sub(h));
    if at_sel < top + margin {
        top = at_sel.saturating_sub(margin);
    } else if at_sel + margin >= top + h {
        top = (at_sel + margin + 1).saturating_sub(h);
    }
    top = top.min(out.len().saturating_sub(h));
    app.approve_scroll.set(top);
    let shown: Vec<Line<'static>> = out.into_iter().skip(top).take(h).collect();
    lines(f, list_r, shown);
    if let Some(r) = detail_r {
        let it = selected(app, sel).and_then(|(g, i)| Some((ap.groups.get(g)?, ap.groups[g].items.get(i)?)));
        if let Some((group, it)) = it {
            draw_detail(f, r, group, it, 0, true);
        }
    }
    let pos = if total > 0 {
        format!("{}/{total}  ", sel + 1)
    } else {
        String::new()
    };
    let mut foot_line = hints(&[("y n", "one"), ("Y", "the group"), ("⏎", "open"), ("l", "log")]);
    foot_line.spans.insert(0, Span::styled(pos, dim()));
    lines(f, foot, vec![foot_line.right_aligned()]);
}

/// One item laid out for reviewing: the action, whom and what, when and where, why, then the
/// thing itself. `scroll` skips lines of the body; `pane` draws it beside the list, with a rule
/// on its left instead of a frame.
fn draw_detail(f: &mut Frame, area: Rect, group: &ApproveGroup, it: &ApproveItem, scroll: usize, pane: bool) {
    let area = if pane {
        let rule: Vec<Line<'static>> = (0..area.height).map(|_| Line::from(Span::styled("│", dim()))).collect();
        lines(f, Rect { width: 1, ..area }, rule);
        Rect {
            x: area.x + 2,
            width: area.width.saturating_sub(3),
            ..area
        }
    } else {
        Rect {
            x: area.x + 1,
            width: area.width.saturating_sub(2),
            ..area
        }
    };
    let w = usize::from(area.width);
    let d = it.detail.clone().unwrap_or_else(|| ApproveDetail {
        action: badge_text(group.kind, it),
        who: String::new(),
        subject: it.text.clone(),
        meta: match &it.to {
            Some(t) => format!("{} · → {t}", it.from),
            None => it.from.clone(),
        },
        reason: String::new(),
        body: String::new(),
    });
    let mut head: Vec<Line<'static>> = vec![
        Line::from(""),
        Line::from({
            let mut v = vec![badge(&d.action, d.action.width() + 2)];
            let g = group_label(group);
            if !g.eq_ignore_ascii_case(&d.action) {
                v.push(Span::styled(format!("  {g}"), dim()));
            }
            v
        }),
        Line::from(""),
    ];
    if !d.who.is_empty() {
        head.push(Line::from(Span::styled(crate::ui::truncate(&d.who, w), bold(text()))));
    }
    for l in wrap(&d.subject, w).into_iter().take(3) {
        head.push(Line::from(Span::styled(l, text())));
    }
    if !d.meta.is_empty() {
        head.push(Line::from(Span::styled(crate::ui::truncate(&d.meta, w), dim())));
    }
    if !d.reason.is_empty() {
        head.push(Line::from(""));
        for (j, l) in wrap(&d.reason, w.saturating_sub(5)).into_iter().enumerate() {
            head.push(Line::from(vec![
                Span::styled(if j == 0 { "why  " } else { "     " }, st("overlay1")),
                Span::styled(l, st("subtext1")),
            ]));
        }
    }
    let body = wrap_keep(&d.body, w);
    if !body.is_empty() {
        head.push(Line::from(""));
        head.push(Line::from(Span::styled("─".repeat(w), st("surface1"))));
    }
    let room = usize::from(area.height).saturating_sub(head.len() + 1);
    let more = body.len().saturating_sub(scroll + room);
    let mut out = head;
    out.extend(
        body.into_iter()
            .skip(scroll)
            .take(room)
            .map(|l| Line::from(Span::styled(l, st("subtext0")))),
    );
    if more > 0 {
        out.push(Line::from(Span::styled(
            format!(
                "… {more} more lines{}",
                if pane { " · ⏎ to read it all" } else { " · ↓" }
            ),
            dim(),
        )));
    }
    lines(f, area, out);
}

/// `⏎` on Approve: the selected item, whole.
pub fn draw_item(f: &mut Frame, area: Rect, app: &App) {
    let ap = &app.fixtures.approve;
    let sel = app.approve_sel();
    let n = ap.len();
    let inner = frame(
        f,
        area,
        title("approve", &format!("{} of {n}", (sel + 1).min(n))),
        Some(esc_back()),
    );
    let [body, foot] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    match selected(app, sel).and_then(|(g, i)| Some((ap.groups.get(g)?, ap.groups[g].items.get(i)?))) {
        Some((group, it)) => draw_detail(f, body, group, it, app.sel(' '), false),
        None => lines(
            f,
            body,
            vec![Line::from(Span::styled("  nothing left to approve", dim()))],
        ),
    }
    lines(
        f,
        foot,
        vec![
            hints(&[
                ("y", "approve"),
                ("n", "refuse"),
                ("↑ ↓", "scroll"),
                ("esc", "the list"),
            ])
            .right_aligned(),
        ],
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
        ScreenId::ApproveItem => {
            if let Some(d) = nav(&k) {
                app.move_sel(' ', 10_000, d);
                return true;
            }
            match k.code {
                KeyCode::PageDown => app.move_sel(' ', 10_000, 15),
                KeyCode::PageUp => app.move_sel(' ', 10_000, -15),
                // Decide it as the list would, then show the next one.
                KeyCode::Char('y' | 'n' | 'Y') => {
                    app.close_top();
                    key(app, k);
                    if !app.fixtures.approve.is_empty() {
                        app.sel.insert((ScreenId::ApproveItem, ' '), 0);
                        app.open(ScreenId::ApproveItem);
                    }
                }
                _ => return false,
            }
            true
        }
        ScreenId::Approve => {
            let rows = approve_rows(app);
            if let Some(d) = nav(&k) {
                app.move_sel(' ', rows.len(), d);
                return true;
            }
            let page = isize::try_from(app.approve_page()).unwrap_or(10);
            match k.code {
                KeyCode::PageDown => {
                    app.move_sel(' ', rows.len(), page);
                    return true;
                }
                KeyCode::PageUp => {
                    app.move_sel(' ', rows.len(), -page);
                    return true;
                }
                KeyCode::Home => {
                    app.move_sel(' ', rows.len(), -(rows.len() as isize));
                    return true;
                }
                KeyCode::End => {
                    app.move_sel(' ', rows.len(), rows.len() as isize);
                    return true;
                }
                _ => {}
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
                KeyCode::Char(c @ ('y' | 'n')) => {
                    approve_one(app, key, &text, to.as_deref(), c == 'y');
                    // Marked, not removed (fixtures): the next one comes up.
                    app.move_sel(' ', rows.len(), 1);
                }
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
                    None => {
                        app.sel.insert((ScreenId::ApproveItem, ' '), 0);
                        app.open(ScreenId::ApproveItem);
                    }
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
