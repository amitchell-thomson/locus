//! The Decisions page (`d` on the Project view): the log down the left in a section per state,
//! the selected decision in full on the right (round 2q, 22 Oct: "the different states of
//! decisions should be clearly marked out sections ... list on the left and full detailed entry
//! on the right hand side").
//!
//! What it is for, in his words: "checking why we made a certain decision; checking what
//! decisions are affected by a new one; learning about the current state and why we are where
//! we are; making sure I did not miss any assumptions/ decisions/ trade-offs". So the entry
//! leads with its links (what it replaces, what builds on it), then every labelled field in
//! full, and Enter on a link goes to that decision.

use crate::app::{App, Mark};
use crate::screen::ScreenId;
use crate::ui::widgets::*;
use crate::ui::{esc_back, frame, hints, lines, title};
use locus_contract::{Decision, DecisionPoint, DecisionStatus, Decisions};
use locus_theme::{SEP, c, glyph};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

pub const KEYS: &[(&str, &str)] = &[
    ("→ or ⏎", "read it on the right; on a link, ⏎ goes to that decision"),
    ("←", "back to the list"),
    ("o", "reopen: under review, with a chat about it"),
    (
        "␣",
        "ask about this decision (the answer comes back on it, and in Answered)",
    ),
    ("a", "the ask line; ⏎ there shows what you asked about it"),
    ("c", "the chat where it was decided"),
    ("f", "the full entry in the repository"),
];

/// The list's sections, in the order drawn and walked: (letter, heading).
// Holding first, then new, then the rest (round 2z: "holding should be at the top, then new").
pub const SECTIONS: &[(char, &str)] = &[
    ('h', "holding"),
    ('n', "new since you last looked"),
    ('r', "to re-check"),
    ('v', "under review"),
    ('x', "replaced"),
];

fn log(app: &App) -> Option<&Decisions> {
    let id = app.project_id()?;
    app.fixtures.projects.get(&id)?.decisions.as_ref()
}

fn reopen_key(project: &str, id: &str) -> String {
    format!("reopen:{project}:{id}")
}

/// A decision's state as you see it now: reopening one here puts it under review.
pub fn status_of(app: &App, project: &str, d: &Decision) -> DecisionStatus {
    if app.marked(&reopen_key(project, &d.id)) == Some(&Mark::Reopened) {
        DecisionStatus::Review
    } else {
        d.status.clone()
    }
}

/// Each section's decisions, newest first. A new decision to re-check stays in re-check,
/// marked new; "new" holds the new ones that hold.
fn groups(app: &App, ds: &Decisions) -> Vec<(char, &'static str, Vec<usize>)> {
    SECTIONS
        .iter()
        .map(|&(k, name)| {
            let mut idx: Vec<usize> = (0..ds.items.len())
                .filter(|&i| {
                    let d = &ds.items[i];
                    match (status_of(app, &ds.project, d), k) {
                        (DecisionStatus::Recheck, 'r') | (DecisionStatus::Review, 'v') => true,
                        (DecisionStatus::Holding, 'n') => d.new,
                        (DecisionStatus::Holding, 'h') => !d.new,
                        (DecisionStatus::Redecided { .. }, 'x') => true,
                        _ => false,
                    }
                })
                .collect();
            idx.sort_by_key(|&i| std::cmp::Reverse(ds.items[i].date));
            (k, name, idx)
        })
        .collect()
}

fn find(ds: &Decisions, id: &str) -> Option<usize> {
    ds.items.iter().position(|d| d.id == id)
}

/// The decision on the right: the one selected in the focused list section, else the last one
/// a list section had, else the first drawn.
fn current(app: &App, ds: &Decisions) -> Option<usize> {
    let gs = groups(app, ds);
    if let Some((_, _, idx)) = gs.iter().find(|g| Some(g.0) == app.focus) {
        return idx.get(app.sel(app.focus?)).copied();
    }
    let kept = app
        .shown
        .borrow()
        .get(&(ScreenId::Decisions, '*'))
        .and_then(|v| v.first().copied())
        .filter(|&i| i < ds.items.len());
    kept.or_else(|| gs.iter().find_map(|g| g.2.first().copied()))
}

fn asked_key(project: &str, id: &str) -> String {
    format!("{project}:{id}")
}

/// One section of the entry: its name and colour, the one line shown while folded, and its
/// items.
struct Fold {
    name: &'static str,
    summary: String,
    /// (label, detail) per item.
    lines: Vec<(String, String)>,
}

/// The entry's sections after the connections, in reading order (round 2y): the ADR template's
/// sections, which the repos write, plus your words and alternatives. Reasons first, background
/// last. Their titles are plain (round 2zc: one colour per section, round 2t, said nothing the
/// rule did not). A section with nothing in it is left out.
fn folds(d: &Decision) -> Vec<Fold> {
    // Folded, a section is its labels side by side; open, each label with its short detail
    // (round 2za: "each section is still too long and hard to parse").
    let labels = |v: &[DecisionPoint]| v.iter().map(|p| p.label.as_str()).collect::<Vec<_>>().join(" · ");
    let points = |v: &[DecisionPoint]| {
        v.iter()
            .map(|p| (p.label.clone(), p.detail.clone()))
            .collect::<Vec<_>>()
    };
    let mut out = vec![];
    let mut add = |name, summary: String, lines: Vec<(String, String)>| {
        if !lines.is_empty() {
            out.push(Fold { name, summary, lines });
        }
    };
    add("decision", labels(&d.decision), points(&d.decision));
    // Your own words, whenever there are any ("definitely quote me").
    add(
        "you said",
        d.you.first().map(|q| format!("“{}”", q.text)).unwrap_or_default(),
        d.you
            .iter()
            .map(|q| (format!("“{}”", q.text), q.source.clone()))
            .collect(),
    );
    add("alternatives", labels(&d.alternatives), points(&d.alternatives));
    add("assumes", labels(&d.assumes), points(&d.assumes));
    add("consequences", labels(&d.consequences), points(&d.consequences));
    add("context", labels(&d.context), points(&d.context));
    out
}

/// A question typed on the ask line, about the decision on the right.
pub fn ask(app: &mut App, q: String) {
    let Some(ds) = log(app).cloned() else { return };
    let Some(cur) = current(app, &ds) else { return };
    let d = &ds.items[cur];
    app.asked.push((asked_key(&ds.project, &d.id), q));
    app.note = Some(format!(
        "would ask heuris with \"{}\" as context; the answer comes back on it and in Answered",
        crate::ui::truncate(&d.title, 40)
    ));
}

/// The ask line at the foot of the entry: scoped to the decision on the right, its short title
/// on the rule; `a` reaches it and Enter there shows what you asked about it.
fn draw_ask(f: &mut Frame, area: Rect, app: &App, ds: &Decisions, i: usize) {
    if area.height == 0 {
        return;
    }
    let d = &ds.items[i];
    let focus = app.focus == Some('a');
    let asking = matches!(app.mode, crate::app::Mode::Ask(_));
    let lit = focus || asking;
    let col = st(if lit { "blue" } else { "surface2" });
    let n = app
        .asked
        .iter()
        .filter(|(k, _)| *k == asked_key(&ds.project, &d.id))
        .count();
    let mut head = vec![
        Span::styled("─ ", col),
        Span::styled("ask about this decision", bold(if lit { st("blue") } else { text() })),
    ];
    let right = if n > 0 {
        format!("{n} asked [a] ─")
    } else {
        "[a] ─".into()
    };
    let used: usize = head.iter().map(|s| s.content.width()).sum::<usize>() + right.width();
    head.push(Span::styled(
        format!(" {} ", "─".repeat(usize::from(area.width).saturating_sub(used + 2))),
        col,
    ));
    head.push(Span::styled(right, col));
    let body = match &app.mode {
        crate::app::Mode::Ask(q) => vec![
            Span::styled("   ask › ", st("mauve")),
            Span::styled(q.clone(), text()),
            Span::styled("▏", st("mauve")),
        ],
        _ if focus && n > 0 => vec![
            Span::styled("   ask › ", dim()),
            Span::styled("␣", keyst()),
            Span::styled(" to ask · ", dim()),
            Span::styled("⏎", keyst()),
            Span::styled(" for what you asked about it", dim()),
        ],
        _ => vec![
            Span::styled("   ask › ", dim()),
            Span::styled("␣", keyst()),
            Span::styled(" to ask about this decision · the answer comes back on it", dim()),
        ],
    };
    lines(f, area, vec![Line::from(head), Line::from(body)]);
}

/// Open the page on decision `i` (Enter on the Project view's decisions section).
pub fn focus_on(app: &mut App, i: usize) {
    let Some(ds) = log(app).cloned() else { return };
    for (k, _, idx) in groups(app, &ds) {
        if let Some(pos) = idx.iter().position(|&x| x == i) {
            app.focus = Some(k);
            app.sel.insert((ScreenId::Decisions, k), pos);
            app.shown.borrow_mut().insert((ScreenId::Decisions, '*'), vec![i]);
            return;
        }
    }
}

/// The links an entry leads with, in order: (label, target index). What it replaces and what
/// replaced it; what it builds on and what builds on it, so a new decision shows what it bears
/// on ("checking what decisions are affected by a new one").
fn links(ds: &Decisions, i: usize) -> Vec<(&'static str, usize)> {
    let d = &ds.items[i];
    let mut out = Vec::new();
    for (j, o) in ds.items.iter().enumerate() {
        if matches!(&o.status, DecisionStatus::Redecided { by } if *by == d.id) {
            out.push(("replaces", j));
        }
    }
    if let DecisionStatus::Redecided { by } = &d.status
        && let Some(j) = find(ds, by)
    {
        out.push(("replaced by", j));
    }
    for r in &d.rests_on {
        if let Some(j) = find(ds, r) {
            out.push(("builds on", j));
        }
    }
    for (j, o) in ds.items.iter().enumerate() {
        if o.rests_on.contains(&d.id) {
            out.push(("built on by", j));
        }
    }
    out
}

pub fn section_len(app: &App, sec: char) -> usize {
    let Some(ds) = log(app) else { return 0 };
    if sec == 'e' {
        // The links, then each folded section.
        let m = app
            .shown
            .borrow()
            .get(&(ScreenId::Decisions, 'e'))
            .cloned()
            .unwrap_or_default();
        let (nl, extra) = (m.first().copied().unwrap_or(0), m.get(1).copied().unwrap_or(0));
        return (nl + extra).max(1);
    }
    groups(app, ds)
        .into_iter()
        .find(|g| g.0 == sec)
        .map_or(0, |g| g.2.len())
}

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let name = app
        .project_id()
        .and_then(|id| app.fixtures.registry.get(&id).map(|p| p.name.clone()))
        .unwrap_or_else(|| "no project".into());
    let ds = log(app);
    let counts = ds
        .map(|ds| {
            let gs = groups(app, ds);
            let n = |k: char| gs.iter().find(|g| g.0 == k).map_or(0, |g| g.2.len());
            let mut parts = vec![];
            // In the list's order.
            for (k, word) in [
                ('h', "holding"),
                ('n', "new"),
                ('r', "to re-check"),
                ('v', "under review"),
                ('x', "replaced"),
            ] {
                if n(k) > 0 {
                    parts.push(format!("{} {word}", n(k)));
                }
            }
            parts.join(SEP)
        })
        .unwrap_or_default();
    let inner = frame(
        f,
        area,
        title(&format!("{name} · decisions"), &counts),
        Some(esc_back()),
    );
    let Some(ds) = ds else {
        empty(f, inner, "this project keeps no decision log");
        return;
    };
    if ds.items.is_empty() {
        empty(f, inner, &format!("no decisions in {} yet", ds.log));
        return;
    }
    let Some(cur) = current(app, ds) else { return };
    if SECTIONS.iter().any(|s| Some(s.0) == app.focus) {
        app.shown.borrow_mut().insert((ScreenId::Decisions, '*'), vec![cur]);
    }
    let body = Rect {
        height: inner.height.saturating_sub(1),
        ..inner
    };
    let foot = Rect {
        y: inner.y + inner.height.saturating_sub(1),
        height: 1,
        ..inner
    };
    // Side by side when there is room; on a narrow window the entry goes under the list.
    let (list_r, div_r, entry_r) = if inner.width >= 120 {
        let lw = (inner.width * 2 / 5).clamp(36, 64);
        (
            Rect { width: lw, ..body },
            Rect {
                x: body.x + lw,
                width: 3,
                ..body
            },
            Rect {
                x: body.x + lw + 3,
                width: body.width.saturating_sub(lw + 3),
                ..body
            },
        )
    } else {
        // Stacked: the list as tall as it needs, up to half, a rule, then the entry.
        let gs = groups(app, ds);
        let need = gs
            .iter()
            .map(|g| 1 + (g.2.len() + g.2.iter().filter(|&&i| ds.items[i].recheck.is_some()).count()).max(1))
            .sum::<usize>()
            + gs.len().saturating_sub(1);
        let lh = (need as u16).min(body.height / 2).max(4);
        (
            Rect { height: lh, ..body },
            Rect {
                y: body.y + lh,
                height: 1,
                ..body
            },
            Rect {
                y: body.y + lh + 1,
                height: body.height.saturating_sub(lh + 1),
                ..body
            },
        )
    };
    draw_list(f, list_r, app, ds, cur);
    // The divider is the entry's edge, and lights when the entry is focused, as on Home.
    let col = st(if app.focus == Some('e') { "blue" } else { "surface2" });
    if div_r.width == 3 {
        let side = if app.focus == Some('e') { " ┃ " } else { " │ " };
        lines(
            f,
            div_r,
            (0..div_r.height).map(|_| Line::from(Span::styled(side, col))).collect(),
        );
    } else {
        let bar = if app.focus == Some('e') { "━" } else { "─" };
        lines(
            f,
            div_r,
            vec![Line::from(Span::styled(bar.repeat(usize::from(div_r.width)), col))],
        );
    }
    // The ask line is pinned to the foot of the entry, as on the Project view (round 2v).
    let ask_h = 2.min(entry_r.height);
    let (entry_r, ask_r) = (
        Rect {
            height: entry_r.height - ask_h,
            ..entry_r
        },
        Rect {
            y: entry_r.y + entry_r.height - ask_h,
            height: ask_h,
            ..entry_r
        },
    );
    draw_entry(f, entry_r, app, ds, cur);
    draw_ask(f, ask_r, app, ds, cur);
    let h: Vec<(&str, &str)> = if app.focus == Some('e') {
        let on_link = app.sel('e') < links(ds, cur).len();
        let mut h = vec![];
        if on_link {
            h.push(("⏎", "go to it"));
        }
        h.extend([("↑ ↓", "read on"), ("o", "reopen"), ("←", "the list")]);
        h
    } else {
        vec![
            ("→", "read it"),
            ("o", "reopen"),
            ("c", "its chat"),
            ("f", "full entry"),
        ]
    };
    lines(f, foot, vec![hints(&h).right_aligned()]);
}

fn empty(f: &mut Frame, area: Rect, what: &str) {
    lines(
        f,
        area,
        vec![Line::from(""), Line::from(Span::styled(format!("   {what}"), dim()))],
    );
}

/// The list: a section per state, each sized to its decisions, the blank space between them
/// spread evenly ("spread out sections per decision state"). Short of room, the holding
/// section gives way first and scrolls to its selection.
fn draw_list(f: &mut Frame, area: Rect, app: &App, ds: &Decisions, cur: usize) {
    let now = app.fixtures.now().date_naive();
    // Every state shows, an empty one saying so in a line (round 2y: "the other sections other
    // than holding should still show, just with a note that says nothing here").
    let gs: Vec<_> = groups(app, ds);
    let w = usize::from(area.width);
    // A title wraps to two lines when every section still fits that way (spare rows go to
    // fuller rows first, round 2f); otherwise each title keeps one line.
    let tw = w.saturating_sub(14);
    let title_lines = |i: usize| wrap(&ds.items[i].title, tw).len().clamp(1, 2);
    let reason = |i: usize| {
        let d = &ds.items[i];
        usize::from(status_of(app, &ds.project, d) == DecisionStatus::Recheck && d.recheck.is_some())
    };
    let full: usize = gs
        .iter()
        .map(|g| 1 + g.2.iter().map(|&i| title_lines(i) + reason(i)).sum::<usize>().max(1))
        .sum::<usize>()
        + gs.len().saturating_sub(1);
    let two = full <= usize::from(area.height);
    // Rows a decision takes: its title, and for a re-check its reason.
    let rows_of = |i: usize| if two { title_lines(i) } else { 1 } + reason(i);
    let need: Vec<usize> = gs
        .iter()
        .map(|g| 1 + g.2.iter().map(|&i| rows_of(i)).sum::<usize>().max(1))
        .collect();
    let avail = usize::from(area.height);
    let gaps = gs.len().saturating_sub(1);
    let mut give = need.clone();
    // Too many: the holding section first, then the rest, down to a heading and two rows.
    let mut over = (need.iter().sum::<usize>() + gaps).saturating_sub(avail);
    let shrink: Vec<usize> = gs
        .iter()
        .enumerate()
        .filter(|(_, g)| g.0 == 'h')
        .chain(gs.iter().enumerate().filter(|(_, g)| g.0 != 'h'))
        .map(|(i, _)| i)
        .collect();
    for gi in shrink {
        let cut = give[gi].saturating_sub(3).min(over);
        give[gi] -= cut;
        over -= cut;
    }
    // Spare rows: one blank between sections at least, more while there is room, so the
    // states read as separate blocks rather than one list.
    let spare = avail.saturating_sub(give.iter().sum::<usize>());
    let gap = if gaps == 0 { 0 } else { (spare / gaps).clamp(1, 3) };
    let mut out: Vec<Line<'static>> = Vec::new();
    for (gi, (k, name, idx)) in gs.iter().enumerate() {
        if gi > 0 {
            for _ in 0..gap {
                out.push(Line::from(""));
            }
        }
        let focus = app.focus == Some(*k);
        let head_style = match k {
            'r' => "yellow",
            'v' => "sky",
            'n' => "peach",
            _ => "text",
        };
        let count = if idx.is_empty() {
            String::new()
        } else {
            idx.len().to_string()
        };
        let mut head = rule(name, &count, None, focus, area.width);
        // The state's colour on its heading, so the blocks tell apart at a glance.
        if !focus && let Some(s) = head.spans.get_mut(1) {
            s.style = bold(st(head_style));
        }
        out.push(head);
        // A new decision filed under another state is still new: say where it is.
        let new_elsewhere = gs
            .iter()
            .filter(|g| g.0 != 'n')
            .flat_map(|g| g.2.iter())
            .filter(|&&i| ds.items[i].new)
            .count();
        if idx.is_empty() && *k == 'n' && new_elsewhere > 0 {
            out.push(Line::from(Span::styled(
                format!(
                    "   nothing else new · {new_elsewhere} newly flagged, marked {} below",
                    glyph::UNREAD
                ),
                dim(),
            )));
            continue;
        }
        if idx.is_empty() {
            let note = match k {
                'r' => "nothing needs re-checking",
                'v' => "nothing is under review",
                'n' => "nothing new since you last looked",
                'h' => "nothing holds yet",
                _ => "nothing has been replaced",
            };
            out.push(Line::from(Span::styled(format!("   {note}"), dim())));
            continue;
        }
        let sel = focus.then(|| app.sel(*k));
        let room = give[gi].saturating_sub(1);
        // Scroll so the selection stays in view.
        let mut first = 0;
        if let Some(s) = sel {
            while first < s && idx[first..=s].iter().map(|&i| rows_of(i)).sum::<usize>() > room {
                first += 1;
            }
        }
        let mut used = 0;
        let mut drawn = 0;
        for (pos, &i) in idx.iter().enumerate().skip(first) {
            let extra = rows_of(i);
            let more_after = idx.len() - pos - 1;
            let reserve = usize::from(more_after > 0 && used + extra + 1 > room && pos + 1 < idx.len());
            if used + extra + reserve > room && drawn > 0 {
                break;
            }
            let d = &ds.items[i];
            let status = status_of(app, &ds.project, d);
            let (g, gc, _) = decision_glyph(&status);
            let replaced = matches!(status, DecisionStatus::Redecided { .. });
            let mut right = vec![];
            // The newest stand out: a marker and how long ago, not only their place at the top.
            // In "new" the age says how recent; elsewhere (a decision newly flagged to re-check)
            // the marker alone, since its age is the decision's, not the flag's.
            if d.new && *k == 'n' {
                right.push(Span::styled(format!("{} ", glyph::UNREAD), st("peach")));
                right.push(Span::styled(format!("{:>5} ", days_ago(now, d.date)), st("peach")));
            } else {
                if d.new {
                    right.push(Span::styled(format!("{} ", glyph::UNREAD), st("peach")));
                }
                right.push(Span::styled(format!("{:>7} ", date(d.date)), dim()));
            }
            // A decision made on a branch, not yet on the trunk (round 2y).
            if d.branch.is_some() {
                right.insert(0, Span::styled(format!("{} ", glyph::COMMITS), st("teal")));
            }
            // The decision the entry shows keeps its band and marker while you read the entry, so
            // the list always says which one it is (round 2za).
            let showing = sel.is_none() && i == cur;
            let lead = if showing {
                Span::styled("▌ ", st("blue"))
            } else {
                pointer(pos, sel)
            };
            let first_line = row(
                vec![
                    lead,
                    Span::styled(format!(" {g} "), st(gc)),
                    Span::styled(
                        if two && title_lines(i) > 1 {
                            wrap(&d.title, tw)[0].clone()
                        } else {
                            d.title.clone()
                        },
                        if replaced {
                            dim()
                        } else if d.new {
                            bold(text())
                        } else {
                            text()
                        },
                    ),
                ],
                right,
                area.width,
            );
            out.push(if showing {
                first_line.patch_style(Style::default().bg(c("surface0")))
            } else {
                first_line
            });
            let band = |line: Line<'static>| {
                if sel == Some(pos) || showing {
                    line.patch_style(Style::default().bg(c("surface0")))
                } else {
                    line
                }
            };
            if two && title_lines(i) > 1 {
                let parts = wrap(&d.title, tw);
                let mut rest = parts[1].clone();
                if parts.len() > 2 {
                    rest = crate::ui::truncate(&format!("{rest} {}", parts[2]), tw);
                }
                let style = if replaced {
                    dim()
                } else if d.new {
                    bold(text())
                } else {
                    text()
                };
                out.push(band(row(
                    vec![Span::raw("     "), Span::styled(rest, style)],
                    vec![],
                    area.width,
                )));
            }
            if reason(i) > 0
                && let Some(why) = &d.recheck
            {
                let line = Line::from(vec![
                    Span::raw("     "),
                    Span::styled(crate::ui::truncate(&why.label, w.saturating_sub(6)), st("yellow")),
                ]);
                out.push(band(line));
            }
            used += extra;
            drawn += 1;
        }
        let hidden = idx.len() - first - drawn;
        if hidden > 0 {
            out.push(Line::from(Span::styled(format!("     + {hidden} more"), dim())));
        }
    }
    lines(f, area, out);
}

fn days_ago(now: chrono::NaiveDate, d: chrono::NaiveDate) -> String {
    match (now - d).num_days() {
        0 => "today".into(),
        n => format!("{n}d"),
    }
}

/// The entry: its title and state, the brief, then a section per field, each under a rule with
/// a blank line between (round 2t). Titles are plain white, blue when open (round 2zc); colour
/// is kept for meaning: the states, the re-check, your words, a branch. The connections lead,
/// each a row Enter follows.
fn draw_entry(f: &mut Frame, area: Rect, app: &App, ds: &Decisions, i: usize) {
    let d = &ds.items[i];
    let status = status_of(app, &ds.project, d);
    let (g, gc, word) = decision_glyph(&status);
    let w = usize::from(area.width);
    // Content starts at column 3, under the rule's title, as in every section (round 2h).
    let body = w.saturating_sub(4);
    let focus = app.focus == Some('e');
    let ls = links(ds, i);
    let sel = app.sel('e');
    let mut out: Vec<Line<'static>> = wrap(&d.title, w.saturating_sub(3))
        .into_iter()
        .map(|l| {
            Line::from(vec![
                Span::raw("  "),
                Span::styled(l, bold(st(if focus { "blue" } else { "text" }))),
            ])
        })
        .collect();
    let mut state = vec![
        Span::raw("  "),
        Span::styled(format!("{g} {word}"), st(gc)),
        Span::styled(format!("{SEP}agreed {}", date(d.date)), dim()),
    ];
    if d.new {
        state.push(Span::styled(
            format!("{SEP}{} new since you last looked", glyph::UNREAD),
            st("peach"),
        ));
    }
    if let Some(b) = &d.branch {
        state.push(Span::styled(
            format!("{SEP}{} on branch {b}", glyph::COMMITS),
            st("teal"),
        ));
    }
    if let Some(chat) = &d.chat {
        state.push(Span::styled(format!("{SEP}decided in {chat}"), dim()));
    }
    out.push(Line::from(state));
    // The brief leads: what was chosen and the one biggest reason, in two lines at most, so
    // most visits end here (round 2w).
    out.push(Line::from(""));
    for l in wrap(&d.brief, w.saturating_sub(4)) {
        out.push(Line::from(vec![Span::raw("   "), Span::styled(l, bold(text()))]));
    }
    let head = |out: &mut Vec<Line<'static>>, title: &str, colour: &str| {
        out.push(Line::from(""));
        let mut r = rule(title, "", None, false, area.width);
        if let Some(s) = r.spans.get_mut(1) {
            s.style = bold(st(colour));
        }
        out.push(r);
    };
    let para = |out: &mut Vec<Line<'static>>, value: &str, style: Style| {
        for l in wrap(value, body) {
            out.push(Line::from(vec![Span::raw("   "), Span::styled(l, style)]));
        }
    };
    if let Some(r) = d.recheck.as_ref().filter(|_| status == DecisionStatus::Recheck) {
        // What changed, in a line, and its numbers under it (round 2za).
        head(&mut out, "why re-check", "yellow");
        para(&mut out, &r.label, bold(st("yellow")));
        if !r.detail.is_empty() {
            para(&mut out, &r.detail, dim());
        }
    }
    if status == DecisionStatus::Review {
        head(&mut out, "under review", "sky");
        para(
            &mut out,
            "Reopened by you. A chat in locus has it as context, and moves to the project window if the project itself needs to change.",
            text(),
        );
    }
    // What you asked about it, newest first: the answers that came back, then any still on
    // their way. It builds a record of what you questioned (round 2v).
    let answered: Vec<&locus_contract::Answer> = app
        .fixtures
        .projects
        .get(&ds.project)
        .and_then(|p| p.answered.as_ref())
        .map(|a| {
            a.items
                .iter()
                .filter(|x| x.decision.as_deref() == Some(d.id.as_str()))
                .collect()
        })
        .unwrap_or_default();
    let pending: Vec<&String> = app
        .asked
        .iter()
        .rev()
        .filter(|(k, _)| *k == asked_key(&ds.project, &d.id))
        .map(|(_, q)| q)
        .collect();
    if !answered.is_empty() || !pending.is_empty() {
        head(&mut out, "asked", "text");
        for q in &pending {
            para(&mut out, &format!("“{q}”"), text());
            out.push(Line::from(Span::styled(
                "     on its way · it lands here and in Answered",
                dim(),
            )));
        }
        for a in &answered {
            para(&mut out, &format!("“{}”", a.title), text());
            for b in &a.body {
                para(&mut out, b, st("subtext0"));
            }
        }
        out.push(Line::from(vec![
            Span::styled("   if an answer says it no longer holds, ", dim()),
            Span::styled("o", keyst()),
            Span::styled(" reopens it", dim()),
        ]));
    }
    let link_at = out.len();
    if !ls.is_empty() {
        head(&mut out, "connections", "text");
        let mut last = "";
        for (k, &(label, j)) in ls.iter().enumerate() {
            let o = &ds.items[j];
            let (og, oc, _) = decision_glyph(&status_of(app, &ds.project, o));
            out.push(row(
                vec![
                    if focus && sel == k {
                        pointer(0, Some(0))
                    } else {
                        Span::raw("  ")
                    },
                    Span::styled(
                        format!(" {:<12}", if label == last { "" } else { label }),
                        st("subtext1"),
                    ),
                    Span::styled(format!("{og} "), st(oc)),
                    Span::styled(
                        o.title.clone(),
                        if matches!(o.status, DecisionStatus::Redecided { .. }) {
                            dim()
                        } else {
                            text()
                        },
                    ),
                ],
                vec![],
                area.width,
            ));
            last = label;
        }
    }
    // Then the record's sections, each like connections: a rule with its title and count, one
    // summary line while folded, and everything when the selection reaches it (round 2y: "lets
    // have them all as sections like connections that expand when you hover over them").
    let fs = folds(d);
    let open = if focus { sel.checked_sub(ls.len()) } else { None };
    let mut fold_at = Vec::new();
    for (k, fd) in fs.iter().enumerate() {
        let is_open = open == Some(k);
        out.push(Line::from(""));
        fold_at.push(out.len());
        let n = fd.lines.len();
        let extra = if n > 1 { n.to_string() } else { String::new() };
        let r = rule(fd.name, &extra, None, is_open, area.width);
        // Titles are plain white, blue when open: colour is kept for what means something
        // (states, the re-check, your words, a branch), not to tell sections apart (round 2zc).
        out.push(r);
        if is_open && fd.name == "you said" {
            // Your words at full width, where they came from under each, a blank line between
            // (round 2zb: "my statements are compressed, the date should be below each").
            for (k, (quote, source)) in fd.lines.iter().enumerate() {
                if k > 0 {
                    out.push(Line::from(""));
                }
                for l in wrap(quote, body) {
                    out.push(Line::from(vec![Span::raw("   "), Span::styled(l, st("pink"))]));
                }
                out.push(Line::from(Span::styled(format!("   {source}"), dim())));
            }
        } else if is_open {
            // Each item: its label, then its detail beside it, wrapping under the detail. Single
            // lines read as a table; if any item wraps, a blank line between items keeps them
            // apart (round 2zc).
            let lw = fd.lines.iter().map(|(l, _)| l.width()).max().unwrap_or(0).min(body / 3) + 2;
            let dw = body.saturating_sub(lw + 1).max(10);
            let wraps = fd
                .lines
                .iter()
                .any(|(l, d)| wrap(l, lw - 2).len() > 1 || wrap(d, dw).len() > 1);
            for (k, (label, detail)) in fd.lines.iter().enumerate() {
                if wraps && k > 0 {
                    out.push(Line::from(""));
                }
                let label_lines = wrap(label, lw - 2);
                let detail_lines = wrap(detail, dw);
                for j in 0..label_lines.len().max(detail_lines.len()) {
                    out.push(Line::from(vec![
                        Span::raw("   "),
                        Span::styled(
                            format!("{:<lw$}", label_lines.get(j).cloned().unwrap_or_default()),
                            bold(text()),
                        ),
                        Span::styled(detail_lines.get(j).cloned().unwrap_or_default(), st("subtext0")),
                    ]));
                }
            }
        } else {
            out.push(row(
                vec![Span::raw("   "), Span::styled(fd.summary.clone(), st("subtext0"))],
                vec![],
                area.width,
            ));
        }
    }
    while out.last().is_some_and(|l| l.width() == 0) {
        out.pop();
    }
    // Scrolling follows the selection: the selected row in view, with as much of its open
    // section under it as fits.
    let h = usize::from(area.height);
    let max_scroll = out.len().saturating_sub(h);
    app.shown
        .borrow_mut()
        .insert((ScreenId::Decisions, 'e'), vec![ls.len(), fs.len()]);
    let sel_line = if !focus {
        None
    } else if sel < ls.len() {
        Some(link_at + 2 + sel)
    } else {
        fold_at.get(sel - ls.len()).copied()
    };
    let scroll = match sel_line {
        Some(l) => {
            let end = match open {
                Some(k) if fold_at.get(k) == Some(&l) => fold_at.get(k + 1).copied().unwrap_or(out.len()),
                _ => l + 1,
            };
            end.saturating_sub(h).min(l.saturating_sub(1)).min(max_scroll)
        }
        None => 0,
    };
    let mut view: Vec<Line<'static>> = out.into_iter().skip(scroll).take(h).collect();
    let hidden = (max_scroll - scroll.min(max_scroll)) > 0;
    if hidden && let Some(l) = view.last_mut() {
        *l = Line::from(Span::styled(
            if focus {
                "   ↓ more"
            } else {
                "   ↓ more · → to read on"
            },
            dim(),
        ));
    }
    lines(f, area, view);
}

pub fn key(app: &mut App, k: KeyEvent) -> bool {
    let Some(ds) = log(app).cloned() else { return false };
    let Some(cur) = current(app, &ds) else { return false };
    let d = &ds.items[cur];
    let in_list = SECTIONS.iter().any(|s| Some(s.0) == app.focus);
    match k.code {
        // → crosses to the entry, as Enter does; ← comes back to the decision in the list
        // (round 2r: "right arrow should take you to the description section").
        KeyCode::Left if app.focus == Some('e') => focus_on(app, cur),
        // A section opens as you move onto it, so Enter and → have nothing more to do there.
        KeyCode::Enter | KeyCode::Right if app.focus == Some('e') && app.sel('e') >= links(&ds, cur).len() => {}
        KeyCode::Enter | KeyCode::Right if in_list || app.focus.is_none() => {
            app.shown.borrow_mut().insert((ScreenId::Decisions, '*'), vec![cur]);
            app.focus = Some('e');
            app.sel.insert((ScreenId::Decisions, 'e'), 0);
        }
        KeyCode::Enter if app.focus == Some('e') => {
            if let Some(&(_, j)) = links(&ds, cur).get(app.sel('e')) {
                focus_on(app, j);
            }
        }
        KeyCode::Char('o') => {
            let key = reopen_key(&ds.project, &d.id);
            if app.marked(&key) == Some(&Mark::Reopened) {
                app.marks.remove(&key);
                app.note = Some("no longer under review".into());
            } else {
                app.mark(
                    key,
                    Mark::Reopened,
                    format!(
                        "under review: would start a chat in locus with \"{}\" as context; if the project needs to change, move it to the project window as a working session",
                        crate::ui::truncate(&d.title, 40)
                    ),
                );
            }
            // Keep the reopened decision selected wherever it moved to.
            if in_list {
                focus_on(app, cur);
            }
        }
        // Space asks about this decision from anywhere on the page (round 2v).
        KeyCode::Char(' ') => app.mode = crate::app::Mode::Ask(String::new()),
        KeyCode::Enter if app.focus == Some('a') => {
            if app.asked.iter().any(|(k, _)| *k == asked_key(&ds.project, &d.id)) {
                // Into the entry, which shows what you asked near its top.
                app.focus = Some('e');
                app.sel.insert((ScreenId::Decisions, 'e'), 0);
            } else {
                app.note = Some("nothing asked about this decision yet · ␣ asks".into());
            }
        }
        KeyCode::Char('c') => {
            app.note = Some(match &d.chat {
                Some(chat) => format!("would open the chat where it was decided: {chat}"),
                None => "no chat is recorded for this decision".into(),
            })
        }
        KeyCode::Char('f') => app.note = Some(format!("would open {} at this entry", ds.log)),
        _ => return false,
    }
    true
}
