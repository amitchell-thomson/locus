//! `g` then `d`: the drafts you kept (`:q` on a changed mail, X6), newest first. `⏎` opens one
//! in Neovim where you left it; `:cq` there throws it away. Claude's drafts are not here: they
//! wait in locus's Approve (`A`).
//!
//! A kept draft is a file in locus's state directory named for what it is (`reply-<thread>`,
//! `new-<account>`, `forward-<account>`, see [`Purpose::draft_name`]), so the list is read from
//! the names and the files' own headers: nothing else records them.

use crate::app::{App, Key, Mode};
use crate::send::{EditorRequest, Purpose};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::path::PathBuf;
use std::time::SystemTime;

/// One kept draft.
#[derive(Debug, Clone, PartialEq)]
pub struct Kept {
    pub path: PathBuf,
    pub purpose: Purpose,
    pub to: String,
    pub subject: String,
    pub saved: SystemTime,
}

/// What a draft's file name says it is for.
fn purpose_of(name: &str) -> Option<Purpose> {
    let stem = name.strip_suffix(".eml")?;
    if let Some(rest) = stem.strip_prefix("reply-") {
        // `oxford_12345` was `oxford:12345`: account ids have no `_`.
        let (account, thread) = rest.split_once('_')?;
        return Some(Purpose::Reply {
            r#ref: keryx_core::model::thread_ref(account, thread),
            account: account.to_string(),
        });
    }
    if let Some(account) = stem.strip_prefix("new-") {
        return Some(Purpose::Compose {
            account: account.to_string(),
        });
    }
    stem.strip_prefix("forward-").map(|account| Purpose::Forward {
        account: account.to_string(),
    })
}

/// A header's value from a draft's text.
fn header(text: &str, key: &str) -> String {
    text.lines()
        .take_while(|l| !l.trim().is_empty())
        .find_map(|l| l.strip_prefix(key).map(|v| v.trim().to_string()))
        .unwrap_or_default()
}

impl App {
    /// The drafts you kept, newest first.
    pub fn kept_drafts(&self) -> Vec<Kept> {
        let Some(dir) = &self.drafts_dir else { return vec![] };
        let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
        let mut v: Vec<Kept> = rd
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let purpose = purpose_of(&name)?;
                let text = std::fs::read_to_string(e.path()).ok()?;
                Some(Kept {
                    to: header(&text, "To:"),
                    subject: header(&text, "Subject:"),
                    saved: e
                        .metadata()
                        .and_then(|m| m.modified())
                        .unwrap_or(SystemTime::UNIX_EPOCH),
                    path: e.path(),
                    purpose,
                })
            })
            .collect();
        v.sort_by(|a, b| b.saved.cmp(&a.saved));
        v
    }

    pub fn drafts_key(&mut self, k: Key, sel: usize) {
        let list = self.kept_drafts();
        self.mode = match k {
            Key::Char('j') | Key::Down => Mode::Drafts {
                sel: (sel + 1).min(list.len().saturating_sub(1)),
            },
            Key::Char('k') | Key::Up => Mode::Drafts {
                sel: sel.saturating_sub(1),
            },
            Key::Enter => match list
                .get(sel)
                .and_then(|d| Some((std::fs::read_to_string(&d.path).ok()?, d)))
            {
                Some((text, d)) => {
                    self.editor_request = Some(EditorRequest {
                        text,
                        purpose: d.purpose.clone(),
                    });
                    self.message = Some(("your kept draft · :cq throws it away".into(), false));
                    Mode::List
                }
                None => Mode::Drafts { sel },
            },
            Key::Esc | Key::Char('q') => Mode::List,
            _ => Mode::Drafts { sel },
        };
    }
}

pub fn draw(f: &mut Frame, app: &App, area: Rect, sel: usize) {
    let title = Line::from(vec![
        Span::styled(" mail ", locus_theme::bold(locus_theme::st("text"))),
        Span::styled("· drafts ", locus_theme::dim()),
    ]);
    let block = locus_theme::screen(title, None);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let list = app.kept_drafts();
    if list.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled(
                " no drafts: :q on a mail you changed keeps one here · Claude's wait in Approve (A)",
                locus_theme::dim(),
            )),
            inner,
        );
        return;
    }
    let w = usize::from(inner.width);
    let now = SystemTime::now();
    let mut lines = Vec::new();
    for (i, d) in list.iter().enumerate().take(usize::from(inner.height)) {
        let kind = match &d.purpose {
            Purpose::Reply { .. } => "reply".to_string(),
            Purpose::Forward { account } => format!("forward · {account}"),
            Purpose::Compose { account } => format!("new · {account}"),
            Purpose::EditDraft { .. } => "draft".to_string(),
        };
        let age = now
            .duration_since(d.saved)
            .map(|x| match x.as_secs() {
                s if s < 3600 => format!("{} min", s / 60),
                s if s < 86_400 => format!("{} h", s / 3600),
                s => format!("{} d", s / 86_400),
            })
            .unwrap_or_default();
        let subject = if d.subject.is_empty() {
            "(no subject)".to_string()
        } else {
            d.subject.clone()
        };
        let to = if d.to.is_empty() {
            String::new()
        } else {
            format!("  to {}", d.to)
        };
        let head = format!(" {age:>6}  {:<16} ", locus_theme::fit(&kind, 16));
        let room = w.saturating_sub(head.chars().count() + 1);
        let body = locus_theme::fit(&format!("{subject}{to}"), room);
        let chosen = i == sel;
        let style = if chosen {
            ratatui::style::Style::default()
                .fg(locus_theme::c("text"))
                .bg(locus_theme::fade(locus_theme::c("blue"), 0.55))
                .add_modifier(ratatui::style::Modifier::BOLD)
        } else {
            ratatui::style::Style::default().fg(locus_theme::c("text"))
        };
        let pad = room.saturating_sub(unicode_width::UnicodeWidthStr::width(body.as_str()));
        lines.push(Line::from(vec![
            Span::styled(head, if chosen { style } else { locus_theme::dim() }),
            Span::styled(body, style),
            Span::styled(" ".repeat(pad + 1), style),
        ]));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drafts_name_says_what_it_is_for() {
        assert_eq!(
            purpose_of("reply-oxford_12345.eml"),
            Some(Purpose::Reply {
                r#ref: keryx_core::model::thread_ref("oxford", "12345"),
                account: "oxford".into()
            })
        );
        assert_eq!(
            purpose_of("new-oqts-shared.eml"),
            Some(Purpose::Compose {
                account: "oqts-shared".into()
            })
        );
        assert_eq!(purpose_of("notes.txt"), None);
    }
}
