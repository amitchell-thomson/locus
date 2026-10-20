//! The work section: Home's rhythm grid (when you worked this week, coloured by project) and the
//! area chart behind `w` (projects rising and fading over eight weeks). Still to prove which is
//! the right picture (plan G); both are here so the rounds can compare them.

use crate::app::App;
use crate::ui::widgets::*;
use crate::ui::{esc_back, frame, lines, title};
use chrono::{Datelike, Duration, NaiveTime, Timelike};
use locus_theme::{SEP, c};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// The grid runs 06:00 to 24:00, one cell per half hour.
const START_H: u32 = 6;
const CELLS: usize = 36;

fn cell_of(t: NaiveTime) -> usize {
    let mins = (t.hour() * 60 + t.minute()).saturating_sub(START_H * 60);
    (mins / 30) as usize
}

const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

pub fn draw_week(f: &mut Frame, area: Rect, app: &App, focus: bool) {
    let fx = &app.fixtures;
    let w = &fx.work;
    let total: f32 = w.totals().iter().map(|(_, h)| h).sum::<f32>() + 0.0;
    let mut out = vec![rule("work", "this week", Some('w'), focus, area.width)];
    let [grid, side] = Layout::horizontal([Constraint::Length(46), Constraint::Min(0)]).areas(Rect {
        y: area.y + 1,
        height: area.height.saturating_sub(1),
        ..area
    });
    lines(f, Rect { height: 1, ..area }, std::mem::take(&mut out));

    let mut g = vec![Line::from(Span::styled(
        "      06    09    12    15    18    21    24",
        dim(),
    ))];
    let first = w.week.first().map(|d| d.date);
    for (i, name) in DAYS.iter().enumerate() {
        let date = first.map(|d| d + Duration::days(i as i64));
        let day = date.and_then(|d| w.week.iter().find(|x| x.date == d));
        let mut spans = vec![Span::styled(format!(" {name}  "), dim())];
        let mut cells: Vec<Option<(String, u8)>> = vec![None; CELLS];
        if let Some(day) = day {
            for s in &day.spans {
                let (a, b) = (cell_of(s.start), cell_of(s.end).max(cell_of(s.start) + 1));
                for c in cells.iter_mut().take(b.min(CELLS)).skip(a) {
                    *c = Some((s.project.clone(), s.intensity));
                }
            }
        }
        for (x, cell) in cells.iter().enumerate() {
            spans.push(match cell {
                Some((p, n)) => {
                    let ch = match n {
                        1 => "░",
                        2 => "▒",
                        3 => "▓",
                        _ => "█",
                    };
                    Span::styled(ch, Style::default().fg(project_colour(&fx.registry, p)))
                }
                None if day.is_some() && x % 2 == 1 => Span::styled("·", st("surface1")),
                None => Span::raw(" "),
            });
        }
        g.push(Line::from(spans));
    }
    lines(f, grid, g);

    let mut s = vec![row(
        vec![Span::styled("this week", text())],
        vec![Span::styled(format!("{total:.1} h "), bold(text()))],
        side.width,
    )];
    let totals = w.totals();
    let max = totals.first().map(|t| t.1).unwrap_or(1.0).max(1.0);
    for (p, h) in totals.iter().take(4) {
        let col = project_colour(&fx.registry, p);
        let n = ((h / max) * 12.0).round() as usize;
        s.push(Line::from(vec![
            Span::styled(format!("{:<8}", crate::ui::truncate(p, 8)), Style::default().fg(col)),
            Span::styled(format!("{h:>5.1} h "), text()),
            Span::styled("━".repeat(n), Style::default().fg(col)),
        ]));
    }
    if totals.is_empty() {
        s.push(Line::from(Span::styled("no work logged yet", dim())));
    }
    s.push(row(
        vec![Span::styled("blocks kept", dim())],
        vec![Span::styled(
            format!("{} of {} ", w.blocks_kept, w.blocks_planned),
            text(),
        )],
        side.width,
    ));
    s.push(row(
        vec![Span::styled("streak", dim())],
        vec![Span::styled(format!("{} days ", w.streak_days), text())],
        side.width,
    ));
    lines(f, side, s);
}

const BLOCKS: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

/// The area chart: one row per project, one column per day, height by hours.
pub fn draw_chart(f: &mut Frame, area: Rect, app: &App) {
    let fx = &app.fixtures;
    let chart = &fx.work.chart;
    let inner = frame(f, area, title("work", "8 weeks"), Some(esc_back()));
    let mut out = Vec::new();
    let days = chart.series.first().map(|s| s.hours.len()).unwrap_or(0);
    let label_w = 10;
    let cols = usize::from(inner.width).saturating_sub(label_w + 8);
    // Days per column, so eight weeks fit whatever the width.
    let per = days.div_ceil(cols.max(1)).max(1);
    let max = chart
        .series
        .iter()
        .flat_map(|s| s.hours.chunks(per).map(|c| c.iter().sum::<f32>() / c.len() as f32))
        .fold(0.0_f32, f32::max)
        .max(1.0);
    out.push(Line::from(""));
    for s in &chart.series {
        let col = project_colour(&fx.registry, &s.project);
        let mut spans = vec![Span::styled(
            format!(" {:<w$}", s.project, w = label_w - 1),
            Style::default().fg(col),
        )];
        let mut cells = String::new();
        for chunk in s.hours.chunks(per) {
            let v = chunk.iter().sum::<f32>() / chunk.len() as f32;
            cells.push_str(BLOCKS[((v / max) * 8.0).round().clamp(0.0, 8.0) as usize]);
        }
        spans.push(Span::styled(cells, Style::default().fg(col)));
        let total: f32 = s.hours.iter().sum();
        spans.push(Span::styled(format!(" {total:>4.0} h"), dim()));
        out.push(Line::from(spans));
        out.push(Line::from(""));
    }
    if chart.series.is_empty() {
        out.push(Line::from(Span::styled("  nothing logged in eight weeks", dim())));
    } else {
        // Month labels where a month starts.
        let mut axis = " ".repeat(label_w);
        let mut x = label_w;
        for i in (0..days).step_by(per) {
            let d = chart.from + Duration::days(i as i64);
            if d.day() <= per as u32 && axis.chars().count() <= x {
                let m = d.format("%b").to_string().to_lowercase();
                while axis.chars().count() < x {
                    axis.push(' ');
                }
                axis.push_str(&m);
            }
            x += 1;
        }
        out.push(Line::from(Span::styled(axis, dim())));
        out.push(Line::from(""));
        out.push(Line::from(Span::styled(
            format!(
                " from commits, half an hour each{SEP}transcripts replace this in Slice 4{SEP}{per} day{} a column",
                if per == 1 { "" } else { "s" }
            ),
            Style::default().fg(c("overlay0")),
        )));
    }
    lines(f, inner, out);
}
