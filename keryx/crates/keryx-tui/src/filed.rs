//! `g` then `l`: what keryx filed by itself as it arrived (keryx-joins-locus X-d: "listed in the
//! automatic log"), newest first, each undoable with `u`. A filing is one changeset per
//! destination per tick, so a row can be several threads, and `u` puts them all back.

use crate::app::{App, Key, Mode};
use crate::write::Job;
use keryx_core::changeset::{Author, Changeset, Op};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

/// The summary every filing on arrival carries (`engine_file.rs`).
const PREFIX: &str = "filed as it arrived, for ";

impl App {
    /// The system's filings, newest first (the last 200 changesets looked at).
    pub fn filed_by_itself(&self) -> Vec<Changeset> {
        self.db
            .changesets(None, 200)
            .unwrap_or_default()
            .into_iter()
            .filter(|c| c.author == Author::System && c.summary.starts_with(PREFIX))
            .collect()
    }

    /// The subjects of the threads a changeset filed.
    pub fn filed_subjects(&self, cs: &Changeset) -> Vec<String> {
        let mut seen = std::collections::BTreeSet::new();
        cs.ops
            .iter()
            .filter_map(|op| match op {
                Op::State { msg, .. } if seen.insert((msg.account.clone(), msg.thread_id.clone())) => self
                    .db
                    .thread(&msg.account, &msg.thread_id)
                    .ok()
                    .and_then(|m| m.first().map(|m| m.subject.clone())),
                _ => None,
            })
            .collect()
    }

    pub fn filed_key(&mut self, k: Key, sel: usize) {
        let n = self.filed_by_itself().len();
        self.mode = match k {
            Key::Char('j') | Key::Down => Mode::Filed {
                sel: (sel + 1).min(n.saturating_sub(1)),
            },
            Key::Char('k') | Key::Up => Mode::Filed {
                sel: sel.saturating_sub(1),
            },
            Key::Char('u') => {
                if let Some(cs) = self.filed_by_itself().get(sel) {
                    self.deferred.push(Job::Undo(cs.id.clone()));
                    self.message = Some(("putting it back in the inbox…".into(), false));
                }
                Mode::Filed { sel }
            }
            Key::Esc | Key::Char('q') => Mode::List,
            _ => Mode::Filed { sel },
        };
    }
}

pub fn draw(f: &mut Frame, app: &App, area: Rect, sel: usize) {
    let title = Line::from(vec![
        Span::styled(" keryx ", locus_theme::bold(locus_theme::st("text"))),
        Span::styled("· filed by itself ", locus_theme::dim()),
    ]);
    let block = locus_theme::screen(title, None);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let list = app.filed_by_itself();
    if list.is_empty() {
        let note = if app.cfg.automated.file_on_arrival {
            " nothing yet: automated mail you can be told the purpose of is filed here as it arrives"
        } else {
            " off: [automated] file_on_arrival = false"
        };
        f.render_widget(Paragraph::new(Span::styled(note, locus_theme::dim())), inner);
        return;
    }
    let w = usize::from(inner.width);
    let mut lines = Vec::new();
    for (i, cs) in list.iter().enumerate().take(usize::from(inner.height)) {
        let when = keryx_core::display::date_label(cs.created_at, app.now);
        // "filed as it arrived, for Errands (2 threads)" → "Errands".
        let what = cs
            .summary
            .trim_start_matches(PREFIX)
            .split(" (")
            .next()
            .unwrap_or_default()
            .to_string();
        let subjects = app.filed_subjects(cs);
        let first = subjects.first().cloned().unwrap_or_default();
        let more = if subjects.len() > 1 {
            format!(" · {} more", subjects.len() - 1)
        } else {
            String::new()
        };
        let status = format!("{:?}", cs.status).to_lowercase();
        let head = format!(" {when:>9}  {:<12} ", locus_theme::fit(&what, 12));
        let room = w.saturating_sub(head.chars().count() + status.len() + 2);
        let body = locus_theme::fit(&format!("{first}{more}"), room);
        let gap = room.saturating_sub(unicode_width::UnicodeWidthStr::width(body.as_str()));
        let style = if i == sel {
            locus_theme::bold(locus_theme::st("blue"))
        } else {
            ratatui::style::Style::default().fg(locus_theme::c("text"))
        };
        lines.push(Line::from(vec![
            Span::styled(head, if i == sel { style } else { locus_theme::dim() }),
            Span::styled(body, style),
            Span::raw(" ".repeat(gap + 1)),
            Span::styled(
                status.clone(),
                if status == "applied" {
                    locus_theme::st("green")
                } else {
                    locus_theme::dim()
                },
            ),
        ]));
    }
    f.render_widget(Paragraph::new(lines), inner);
}
