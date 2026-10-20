//! System (`s` from Home): running jobs live, recent results with failures kept visible, and
//! usage. Budget (`u` there): the week against the cap, the 5-hour window, each day by job type,
//! the most expensive jobs, cost by project (plan E). Text on the left, thin bars on the right,
//! every row aligned (screens v2, 18 Oct).

use crate::app::{App, nav};
use crate::screen::ScreenId;
use crate::ui::widgets::*;
use crate::ui::{esc_back, frame, hints, lines, title};
use locus_contract::{Cost, JobOutcome, Usage};
use locus_theme::{SEP, c, glyph};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};

pub const KEYS: &[(&str, &str)] = &[("⏎", "a failure's details")];
pub const BUDGET_KEYS: &[(&str, &str)] = &[("t", "split each day by job type")];

/// Where the label column ends and the bars start.
const LABEL: usize = 44;

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let fx = &app.fixtures;
    let now = fx.now();
    let inner = frame(f, area, title("system", ""), Some(esc_back()));
    let run_h = fx.system.running.len().max(1) as u16 + 1;
    let rec_h = fx.system.recent.len().clamp(1, 6) as u16 + 1;
    let [run_r, rec_r, use_r] = Layout::vertical([
        Constraint::Length(run_h),
        Constraint::Length(rec_h + 1),
        Constraint::Min(0),
    ])
    .areas(inner);
    let w = inner.width;

    let mut out = vec![rule("running", "live", Some('r'), app.focus == Some('r'), w)];
    for j in &fx.system.running {
        out.push(row(
            vec![
                Span::styled(format!(" {} ", glyph::WORKING), st("blue")),
                Span::styled(format!("{:<26}", format!("{}{SEP}{}", j.kind, j.subject)), text()),
                Span::styled(format!("{:<8}", j.model.as_deref().unwrap_or("")), dim()),
                Span::styled(j.detail.clone(), st("subtext0")),
            ],
            vec![Span::styled(format!("{} ", elapsed(now, j.started)), text())],
            w,
        ));
    }
    if fx.system.running.is_empty() {
        out.push(Line::from(Span::styled("  nothing running", dim())));
    }
    lines(f, run_r, out);

    let focus = app.focus == Some('e');
    let sel = focus.then(|| app.sel('e'));
    let mut out = vec![rule("recent", "", Some('e'), focus, w)];
    for (i, j) in fx.system.recent.iter().enumerate().take(6) {
        let (g, gc) = job_glyph(&j.outcome);
        let detail = match &j.outcome {
            JobOutcome::Failed { cause, next } => format!("{cause}{SEP}{next}"),
            _ => j.detail.clone(),
        };
        let at = j.ended.unwrap_or(j.started);
        out.push(row(
            vec![
                pointer(i, sel),
                Span::styled(format!("{g} "), st(gc)),
                Span::styled(
                    format!(
                        "{:<34}",
                        crate::ui::truncate(&format!("{}{SEP}{}", j.kind, j.subject), 33)
                    ),
                    text(),
                ),
                Span::styled(
                    detail,
                    if matches!(j.outcome, JobOutcome::Failed { .. }) {
                        st("red")
                    } else {
                        dim()
                    },
                ),
            ],
            vec![Span::styled(format!("{} ", hm(at)), dim())],
            w,
        ));
    }
    if fx.system.recent.is_empty() {
        out.push(Line::from(Span::styled("  nothing has run yet", dim())));
    }
    lines(f, rec_r, out);

    let mut out = vec![rule("usage", "u for the budget", Some('u'), app.focus == Some('u'), w)];
    out.extend(summary(&fx.usage, w));
    lines(f, use_r, out);
}

/// The two lines System and Home share: the week, and the window.
fn summary(u: &Usage, w: u16) -> Vec<Line<'static>> {
    let bw = usize::from(w).saturating_sub(LABEL + 2);
    let scale = u.cap_pct * 1.3;
    vec![
        aligned(
            format!("this week     {:>5.1}%   of {:.0}% cap", u.week_pct, u.cap_pct),
            bar(u.week_pct / scale, bw, Some(u.cap_pct / scale), c("mauve")),
        ),
        aligned(
            format!("by sunday     {:>5.1}%   projected", u.projected_pct),
            bar(u.projected_pct / scale, bw, Some(u.cap_pct / scale), c("surface2")),
        ),
        aligned(
            format!("window · you  {:>5.0}%", u.window.you_pct),
            bar(
                u.window.you_pct / 100.0,
                bw,
                Some(u.window.stop_at_pct / 100.0),
                c("blue"),
            ),
        ),
        aligned(
            format!(
                "window · locus{:>5.0}%   stops at {:.0}%",
                u.window.locus_pct, u.window.stop_at_pct
            ),
            bar(
                u.window.locus_pct / 100.0,
                bw,
                Some(u.window.stop_at_pct / 100.0),
                c("mauve"),
            ),
        ),
    ]
}

fn aligned(label: String, bar: Vec<Span<'static>>) -> Line<'static> {
    let mut spans = vec![Span::styled(
        format!(" {:<w$}│ ", crate::ui::truncate(&label, LABEL - 2), w = LABEL - 2),
        text(),
    )];
    spans.extend(bar);
    Line::from(spans)
}

pub fn draw_budget(f: &mut Frame, area: Rect, app: &App) {
    let u = &app.fixtures.usage;
    let inner = frame(
        f,
        area,
        title("budget", &format!("{:.1}% of {:.0}% this week", u.week_pct, u.cap_pct)),
        Some(esc_back()),
    );
    let w = inner.width;
    let bw = usize::from(w).saturating_sub(LABEL + 2);
    let mut out = vec![rule("the week and the window", "", None, false, w)];
    out.extend(summary(u, w));
    if !u.window.held.is_empty() {
        out.push(Line::from(Span::styled(
            format!(" held by the window: {}", u.window.held.join(", ")),
            st("peach"),
        )));
    }
    out.push(Line::from(Span::styled(
        format!(" the window resets at {}", hm(u.window.resets)),
        dim(),
    )));

    out.push(rule(
        "each day",
        if app.split {
            "by job type"
        } else {
            "t splits by job type"
        },
        None,
        false,
        w,
    ));
    let day_max = u.days.iter().map(|d| d.total()).fold(0.0_f32, f32::max).max(0.5);
    let kinds: Vec<String> = {
        let mut k: Vec<String> = u
            .days
            .iter()
            .flat_map(|d| d.by_kind.iter().map(|c| c.label.clone()))
            .collect();
        k.sort();
        k.dedup();
        k
    };
    let kind_col =
        |label: &str| c(PROJECT_COLOURS[kinds.iter().position(|k| k == label).unwrap_or(0) % PROJECT_COLOURS.len()]);
    for d in &u.days {
        let names: Vec<_> = d.by_kind.iter().map(|c| c.label.as_str()).collect();
        let label = format!("{:<7}{:>5.1}%   {}", day(d.date), d.total(), names.join(", "));
        let mut spans = vec![Span::styled(
            format!(" {:<w$}│ ", crate::ui::truncate(&label, LABEL - 2), w = LABEL - 2),
            text(),
        )];
        if app.split {
            for c in &d.by_kind {
                let n = ((c.pct / day_max) * bw as f32).round() as usize;
                spans.push(Span::styled(
                    "━".repeat(n),
                    ratatui::style::Style::default().fg(kind_col(&c.label)),
                ));
            }
        } else {
            spans.extend(bar(d.total() / day_max, bw, None, c("mauve")));
        }
        out.push(Line::from(spans));
    }
    if u.days.is_empty() {
        out.push(Line::from(Span::styled("  nothing spent this week", dim())));
    }
    if app.split && !kinds.is_empty() {
        let mut legend = vec![Span::raw(" ")];
        for k in &kinds {
            legend.push(Span::styled(
                format!("━ {k}   "),
                ratatui::style::Style::default().fg(kind_col(k)),
            ));
        }
        out.push(Line::from(legend));
    }
    let costs = |title: &str, rows: &[Cost], out: &mut Vec<Line<'static>>| {
        out.push(rule(title, "", None, false, w));
        let max = rows.iter().map(|c| c.pct).fold(0.0_f32, f32::max).max(0.5);
        for c in rows {
            let model = c.model.as_deref().map(|m| format!("{SEP}{m}")).unwrap_or_default();
            let label = format!(
                "{:<33}{:>5.1}%",
                crate::ui::truncate(&format!("{}{model}", c.label), 32),
                c.pct
            );
            out.push(aligned(label, bar(c.pct / max, bw, None, locus_theme::c("lavender"))));
        }
        if rows.is_empty() {
            out.push(Line::from(Span::styled("  none", dim())));
        }
    };
    costs("most expensive jobs", &u.top_jobs, &mut out);
    costs("by project", &u.by_project, &mut out);
    lines(f, inner, out);
    let foot = Rect {
        y: inner.y + inner.height.saturating_sub(1),
        height: 1,
        ..inner
    };
    lines(f, foot, vec![hints(&[("t", "split by job type")]).right_aligned()]);
}

pub fn key(app: &mut App, k: KeyEvent) -> bool {
    if app.screen() == ScreenId::Budget {
        if k.code == KeyCode::Char('t') {
            app.split = !app.split;
            return true;
        }
        return false;
    }
    if app.focus == Some('e') {
        if let Some(d) = nav(&k) {
            let n = app.fixtures.system.recent.len().min(6);
            app.move_sel('e', n, d);
            return true;
        }
        if k.code == KeyCode::Enter {
            if let Some(j) = app.fixtures.system.recent.get(app.sel('e')) {
                app.note = Some(match &j.outcome {
                    JobOutcome::Failed { cause, next } => format!("{}: {cause} · {next}", j.kind),
                    _ => format!("{}{SEP}{}{SEP}{}", j.kind, j.subject, j.detail),
                });
            }
            return true;
        }
    }
    false
}
