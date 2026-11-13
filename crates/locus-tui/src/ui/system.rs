//! System (⏎ on Home's system section): what runs now, every timer with its last result and next
//! run, recent results with failures kept visible, and the credit. Budget (`u` there): the credit
//! this cycle, today's allowance, each of the last seven days by job, and this cycle's jobs.
//!
//! Both read `locus home-system` from the server, the same answer Home's section draws (cleanup
//! C3, 13 Nov 2026: "it should properly show real data as that real data exists"). They were
//! sample data before, laid out for a weekly subscription cap that no longer applies: model
//! calls run on the $100 API credit, paced per day (agent/claude.credit_allows). Times are shown
//! against the moment the server answered, so nothing old is drawn as current.

use crate::app::{App, nav};
use crate::live::{BudgetLive, SystemLive};
use crate::screen::ScreenId;
use crate::ui::widgets::*;
use crate::ui::{esc_back, frame, hints, lines, title};
use chrono::{DateTime, Local, NaiveDate};
use locus_theme::{SEP, c, glyph};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

pub const KEYS: &[(&str, &str)] = &[
    ("⇥ ↑ ↓ j k", "sections, and through them"),
    ("⏎", "a failure's cause; on usage, the budget"),
    ("b", "the budget"),
];
pub const BUDGET_KEYS: &[(&str, &str)] = &[("t", "split each day by job")];

/// Where the label column ends and the bars start.
const LABEL: usize = 44;

fn local(s: &str) -> Option<DateTime<Local>> {
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Local))
}

/// `14:05` today, `thu 14:05` within the week, else `12 nov`.
fn when(s: Option<&str>, now: DateTime<Local>) -> String {
    let Some(t) = s.and_then(local) else {
        return "never".into();
    };
    if t.date_naive() == now.date_naive() {
        t.format("%H:%M").to_string()
    } else if (t - now).num_days().abs() < 6 {
        t.format("%a %H:%M").to_string().to_lowercase()
    } else {
        t.format("%-d %b").to_string().to_lowercase()
    }
}

/// What the server said, or the line saying why there is nothing to show.
fn live(app: &App) -> Result<&SystemLive, String> {
    match &app.system_live {
        None => Err("asking the server…".into()),
        Some(Err(e)) => Err(format!("server not reached: {e}")),
        Some(Ok(s)) => Ok(s),
    }
}

fn at(sys: &SystemLive) -> DateTime<Local> {
    local(&sys.at).unwrap_or_else(Local::now)
}

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let sys = match live(app) {
        Ok(s) => s,
        Err(e) => {
            let inner = frame(f, area, title("system", ""), Some(esc_back()));
            return lines(f, inner, vec![Line::from(Span::styled(format!("  {e}"), dim()))]);
        }
    };
    let now = at(sys);
    let inner = frame(
        f,
        area,
        title("system", &format!("as of {}", now.format("%H:%M"))),
        Some(esc_back()),
    );
    let w = inner.width;
    let mut out = vec![rule("running", "now", Some('r'), app.focus == Some('r'), w)];
    for (i, j) in sys.running.iter().enumerate() {
        let since = j
            .started
            .as_deref()
            .map(|s| format!("since {}", when(Some(s), now)))
            .unwrap_or_default();
        out.push(row(
            vec![
                Span::styled(format!(" {} ", spin(app.frame + i as u64)), st("blue")),
                Span::styled(j.kind.clone(), text()),
            ],
            vec![Span::styled(format!("{since} "), dim())],
            w,
        ));
    }
    if sys.running.is_empty() {
        out.push(Line::from(Span::styled("  nothing running", dim())));
    }

    out.push(Line::from(""));
    out.push(rule("timers", "last run · next", Some('t'), app.focus == Some('t'), w));
    for t in &sys.timers {
        let (g, col) = match t.result.as_str() {
            "success" => (glyph::DONE, "green"),
            "" => ("·", "overlay1"),
            _ => (glyph::FAILED, "red"),
        };
        let res = if t.result.is_empty() || t.result == "success" {
            String::new()
        } else {
            format!("  {}", t.result)
        };
        out.push(row(
            vec![
                Span::styled(format!(" {g} "), st(col)),
                Span::styled(format!("{:<18}", t.name), text()),
                Span::styled(format!("last {}", when(t.last.as_deref(), now)), dim()),
                Span::styled(res, st("red")),
            ],
            vec![Span::styled(format!("next {} ", when(t.next.as_deref(), now)), dim())],
            w,
        ));
    }
    if sys.timers.is_empty() {
        out.push(Line::from(Span::styled("  no timers reported", dim())));
    }

    out.push(Line::from(""));
    let focus = app.focus == Some('h');
    let sel = focus.then(|| app.sel('h'));
    out.push(rule(
        "recent",
        &format!("{} done · {} failed today", sys.done_today, sys.failed_today),
        Some('h'),
        focus,
        w,
    ));
    for (i, j) in sys.recent.iter().enumerate() {
        let (g, gc) = if j.ok {
            (glyph::DONE, "green")
        } else {
            (glyph::FAILED, "red")
        };
        out.push(row(
            vec![
                pointer(i, sel),
                Span::styled(format!("{g} "), st(gc)),
                Span::styled(format!("{:<20}", crate::ui::truncate(&j.kind, 19)), text()),
                Span::styled(j.cause.clone(), if j.ok { dim() } else { st("red") }),
            ],
            vec![Span::styled(
                format!("{} ", when(j.ended.as_deref().or(Some(&j.started)), now)),
                dim(),
            )],
            w,
        ));
    }
    if sys.recent.is_empty() {
        out.push(Line::from(Span::styled("  nothing has run in two days", dim())));
    }

    out.push(Line::from(""));
    out.push(rule("credit", "b for the budget", Some('b'), app.focus == Some('b'), w));
    match &sys.budget {
        Some(b) => out.extend(summary(b, w)),
        None => out.push(Line::from(Span::styled(
            format!("  ${:.2} of $100 since {}", sys.credit_used, sys.credit_since),
            text(),
        ))),
    }
    out.push(Line::from(Span::styled(
        format!("  {} model calls today{SEP}route {}", sys.calls_today, sys.backend),
        dim(),
    )));
    lines(f, inner, out);
}

/// The credit and today's allowance, shared by System and Budget.
fn summary(b: &BudgetLive, w: u16) -> Vec<Line<'static>> {
    let bw = usize::from(w).saturating_sub(LABEL + 2);
    vec![
        aligned(
            format!(
                "this cycle   ${:>6.2} of ${:.0}  since {}",
                b.used,
                b.cap,
                short(&b.since)
            ),
            bar((b.used / b.cap) as f32, bw, None, c("mauve")),
        ),
        aligned(
            format!("today        ${:>6.2} of ${:.2} a day", b.today, b.daily),
            bar(
                (b.today / b.daily.max(0.01)) as f32,
                bw,
                Some((b.bulk_daily / b.daily.max(0.01)) as f32),
                c("blue"),
            ),
        ),
    ]
}

fn short(iso: &str) -> String {
    NaiveDate::parse_from_str(iso, "%Y-%m-%d")
        .map(|d| d.format("%-d %b").to_string().to_lowercase())
        .unwrap_or_else(|_| iso.to_string())
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
    let b = match live(app).and_then(|s| s.budget.as_ref().ok_or_else(|| "the server sent no budget".into())) {
        Ok(b) => b,
        Err(e) => {
            let inner = frame(f, area, title("budget", ""), Some(esc_back()));
            return lines(f, inner, vec![Line::from(Span::styled(format!("  {e}"), dim()))]);
        }
    };
    let inner = frame(
        f,
        area,
        title(
            "budget",
            &format!(
                "${:.2} of ${:.0} until {} · estimated: the console is exact",
                b.used,
                b.cap,
                short(&b.until)
            ),
        ),
        Some(esc_back()),
    );
    let w = inner.width;
    let bw = usize::from(w).saturating_sub(LABEL + 2);
    let mut out = vec![rule(
        "the credit",
        "background work keeps to 70% of the day's share",
        None,
        false,
        w,
    )];
    out.extend(summary(b, w));

    out.push(Line::from(""));
    out.push(rule(
        "each day",
        if app.split { "by job" } else { "t splits by job" },
        None,
        false,
        w,
    ));
    let total = |d: &crate::live::BudgetDay| d.by_job.iter().map(|j| j.usd).sum::<f64>();
    let day_max = b.days.iter().map(total).fold(0.0_f64, f64::max).max(0.5);
    let jobs: Vec<String> = {
        let mut k: Vec<String> = b
            .days
            .iter()
            .flat_map(|d| d.by_job.iter().map(|j| j.job.clone()))
            .collect();
        k.sort();
        k.dedup();
        k
    };
    let col = |job: &str| c(PROJECT_COLOURS[jobs.iter().position(|k| k == job).unwrap_or(0) % PROJECT_COLOURS.len()]);
    for d in &b.days {
        let names: Vec<_> = d.by_job.iter().take(3).map(|j| j.job.as_str()).collect();
        let label = format!("{:<7}${:>5.2}   {}", short_day(&d.date), total(d), names.join(", "));
        let mut spans = vec![Span::styled(
            format!(" {:<w$}│ ", crate::ui::truncate(&label, LABEL - 2), w = LABEL - 2),
            text(),
        )];
        if app.split {
            for j in &d.by_job {
                let n = ((j.usd / day_max) * bw as f64).round() as usize;
                spans.push(Span::styled(
                    "━".repeat(n),
                    ratatui::style::Style::default().fg(col(&j.job)),
                ));
            }
        } else {
            spans.extend(bar(
                (total(d) / day_max) as f32,
                bw,
                Some((b.daily / day_max) as f32),
                c("mauve"),
            ));
        }
        out.push(Line::from(spans));
    }
    if app.split && !jobs.is_empty() {
        let mut legend = vec![Span::raw(" ")];
        for k in &jobs {
            legend.push(Span::styled(
                format!("━ {k}   "),
                ratatui::style::Style::default().fg(col(k)),
            ));
        }
        out.push(Line::from(legend));
    }

    out.push(Line::from(""));
    out.push(rule("this cycle's jobs", "", None, false, w));
    let max = b.jobs.iter().map(|j| j.usd).fold(0.0_f64, f64::max).max(0.01);
    for j in &b.jobs {
        let failed = if j.failed > 0 {
            format!(" · {} failed", j.failed)
        } else {
            String::new()
        };
        let label = format!(
            "{:<22}{:>5} calls  ${:>6.2}{failed}",
            crate::ui::truncate(
                &format!("{}{}", j.job, if j.backend == "api" { "" } else { " (plan)" }),
                21
            ),
            j.calls,
            j.usd
        );
        out.push(aligned(label, bar((j.usd / max) as f32, bw, None, c("lavender"))));
    }
    if b.jobs.is_empty() {
        out.push(Line::from(Span::styled("  nothing this cycle", dim())));
    }
    lines(f, inner, out);
    let foot = Rect {
        y: inner.y + inner.height.saturating_sub(1),
        height: 1,
        ..inner
    };
    lines(f, foot, vec![hints(&[("t", "split by job")]).right_aligned()]);
}

fn short_day(iso: &str) -> String {
    NaiveDate::parse_from_str(iso, "%Y-%m-%d")
        .map(day)
        .unwrap_or_else(|_| iso.to_string())
}

/// Rows in a section, for the arrows.
pub fn section_len(app: &App, sec: char) -> usize {
    let Ok(sys) = live(app) else { return 1 };
    match sec {
        'r' => sys.running.len(),
        't' => sys.timers.len(),
        'h' => sys.recent.len(),
        _ => 1,
    }
}

pub fn key(app: &mut App, k: KeyEvent) -> bool {
    if app.screen() == ScreenId::Budget {
        if k.code == KeyCode::Char('t') {
            app.split = !app.split;
            return true;
        }
        return false;
    }
    // b: the budget, straight from System (it was u, before u became undo; 13 Nov).
    if k.code == KeyCode::Char('b') {
        app.open(ScreenId::Budget);
        return true;
    }
    if app.focus == Some('h') {
        if let Some(d) = nav(&k) {
            let n = section_len(app, 'h');
            app.move_sel('h', n, d);
            return true;
        }
        if k.code == KeyCode::Enter {
            let pick = live(app).ok().and_then(|s| s.recent.get(app.sel('h')).cloned());
            if let Some(j) = pick {
                app.note = Some(if j.cause.is_empty() {
                    format!("{}: {}", j.kind, if j.ok { "ok" } else { "failed, no cause recorded" })
                } else {
                    format!("{}: {}", j.kind, j.cause)
                });
            }
            return true;
        }
    }
    false
}
