//! `g` then `d`: Claude's drafts, then the drafts you kept (`:q` on a changed mail, X6), newest
//! first. `⏎` opens one in Neovim; `x` twice here throws it away.
//!
//! **Claude's drafts live here, not in locus's Approve** (10 Nov: "drafts do not need approval in
//! the approve tab, they are in drafts which is a form of approval anyways"). A draft of Claude's
//! is a pending send changeset in keryxd; `⏎` opens its text as an edit of that draft
//! (`Purpose::EditDraft`), so `:wq` then `y` sends YOUR version through the confirm screen (with
//! its new-recipient and thread-moved warnings) and the hold, and keryxd marks Claude's replaced.
//! `:cq` leaves it waiting, unchanged. `x` twice rejects it: nothing is sent. Claude still cannot
//! send; nothing leaves until you do.
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

/// A row of the drafts list.
#[derive(Debug, Clone)]
pub enum Entry {
    /// A pending send of Claude's (a changeset in keryxd).
    Claude(keryx_core::changeset::Changeset),
    Kept(Kept),
}

impl Entry {
    fn key(&self) -> String {
        match self {
            Entry::Claude(c) => format!("claude:{}", c.id),
            Entry::Kept(k) => format!("file:{}", k.path.display()),
        }
    }
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
            to: None,
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
                let mut purpose = purpose_of(&name)?;
                // What a reply answers is kept beside it (`<draft>.to`).
                if let Purpose::Reply { to, .. } = &mut purpose {
                    *to = crate::send::read_reply_to(&e.path());
                }
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

    /// Claude's drafts first (they wait on you), then yours.
    pub fn draft_entries(&self) -> Vec<Entry> {
        let mut v: Vec<Entry> = self.pending_sends().into_iter().map(Entry::Claude).collect();
        v.sort_by_key(|e| match e {
            Entry::Claude(c) => -c.created_at,
            Entry::Kept(_) => 0,
        });
        v.extend(self.kept_drafts().into_iter().map(Entry::Kept));
        v
    }

    /// The editor text for one of Claude's drafts: From, To, Cc, Subject and its body.
    pub(crate) fn claude_draft_text(&self, c: &keryx_core::changeset::Changeset) -> Option<(String, Purpose)> {
        let o = c.outgoing()?;
        let text = keryx_core::compose::file_with_body(&o.to, &o.cc, &o.subject, o.body.trim_end(), None, "");
        let text = keryx_core::compose::with_from(&text, &self.from_line(&o.account));
        let purpose = Purpose::EditDraft {
            id: c.id.clone(),
            account: o.account.clone(),
            thread: o
                .thread_id
                .as_deref()
                .map(|t| keryx_core::model::thread_ref(&o.account, t)),
        };
        Some((text, purpose))
    }

    pub fn drafts_key(&mut self, k: Key, sel: usize) {
        let list = self.draft_entries();
        let asked = self.drop_draft.take();
        let last = list.len().saturating_sub(1);
        self.mode = match k {
            Key::Char('j') | Key::Down => Mode::Drafts {
                sel: (sel + 1).min(last),
            },
            Key::Char('k') | Key::Up => Mode::Drafts {
                sel: sel.saturating_sub(1),
            },
            Key::Top => Mode::Drafts { sel: 0 },
            Key::Bottom => Mode::Drafts { sel: last },
            // x asks, x again throws it away: your own text, gone for good (locus's drop).
            Key::Char('x') => {
                if let Some(e) = list.get(sel) {
                    let subject = match e {
                        Entry::Claude(c) => c.outgoing().map(|o| o.subject.clone()).unwrap_or_default(),
                        Entry::Kept(d) => d.subject.clone(),
                    };
                    let what = if subject.is_empty() {
                        "this draft".to_string()
                    } else {
                        subject
                    };
                    if asked.as_deref() == Some(e.key().as_str()) {
                        match e {
                            Entry::Claude(c) => {
                                self.deferred.push(crate::write::Job::Reject(c.id.clone()));
                                self.message = Some((format!("threw away Claude's “{what}”: nothing sent"), false));
                            }
                            Entry::Kept(d) => {
                                self.message = Some(match std::fs::remove_file(&d.path) {
                                    Ok(()) => {
                                        let _ = std::fs::remove_file(crate::send::reply_to_path(&d.path));
                                        (format!("threw away “{what}”"), false)
                                    }
                                    Err(e) => (format!("couldn't throw it away: {e}"), true),
                                });
                            }
                        }
                    } else {
                        self.message = Some((format!("throw away “{what}”? x again throws it away"), false));
                        self.drop_draft = Some(e.key());
                    }
                }
                Mode::Drafts {
                    sel: sel.min(list.len().saturating_sub(1)),
                }
            }
            Key::Enter | Key::Char('e') => match list.get(sel) {
                Some(Entry::Claude(c)) => match self.claude_draft_text(c) {
                    Some((text, purpose)) => {
                        self.editor_request = Some(EditorRequest { text, purpose });
                        self.message = Some((
                            "Claude's draft · :wq then y sends your version · :cq leaves it waiting".into(),
                            false,
                        ));
                        Mode::List
                    }
                    None => Mode::Drafts { sel },
                },
                Some(Entry::Kept(d)) => match std::fs::read_to_string(&d.path) {
                    Ok(text) => {
                        self.editor_request = Some(EditorRequest {
                            text,
                            purpose: d.purpose.clone(),
                        });
                        self.message = Some(("your kept draft · :cq throws it away".into(), false));
                        Mode::List
                    }
                    Err(_) => Mode::Drafts { sel },
                },
                None => Mode::Drafts { sel },
            },
            Key::Esc | Key::Char('q') => Mode::List,
            _ => Mode::Drafts { sel },
        };
    }
}

pub fn draw(f: &mut Frame, app: &App, area: Rect, sel: usize) {
    let title = Line::from(vec![
        Span::styled(" keryx ", locus_theme::bold(locus_theme::st("text"))),
        Span::styled("· drafts ", locus_theme::dim()),
    ]);
    let block = locus_theme::screen(title, None);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let list = app.draft_entries();
    if list.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled(
                " no drafts: Claude's replies wait here, and :q on a mail you changed keeps one here",
                locus_theme::dim(),
            )),
            inner,
        );
        return;
    }
    let w = usize::from(inner.width);
    let now = SystemTime::now();
    let mut lines = Vec::new();
    let rows = usize::from(inner.height);
    let first = sel.saturating_sub(rows.saturating_sub(1));
    let name = |id: &str| app.cfg.account(id).map_or_else(|| id.to_string(), |a| a.name.clone());
    let ago = |secs: u64| match secs {
        s if s < 3600 => format!("{} min", s / 60),
        s if s < 86_400 => format!("{} h", s / 3600),
        s => format!("{} d", s / 86_400),
    };
    for (i, e) in list.iter().enumerate().skip(first).take(rows) {
        let (claude, kind, age, subject, to) = match e {
            Entry::Claude(c) => {
                let o = c.outgoing();
                let account = o.map(|o| o.account.as_str()).unwrap_or_default();
                let secs = (chrono::Utc::now().timestamp() - c.created_at).max(0) as u64;
                (
                    true,
                    format!("Claude · {}", name(account)),
                    ago(secs),
                    o.map(|o| o.subject.clone()).unwrap_or_default(),
                    o.map(|o| o.to.iter().map(|a| a.display()).collect::<Vec<_>>().join(", "))
                        .unwrap_or_default(),
                )
            }
            Entry::Kept(d) => (
                false,
                match &d.purpose {
                    Purpose::Reply { account, .. } => format!("reply · {}", name(account)),
                    Purpose::Forward { account } => format!("forward · {}", name(account)),
                    Purpose::Compose { account } => format!("new · {}", name(account)),
                    Purpose::EditDraft { .. } => "draft".to_string(),
                },
                now.duration_since(d.saved)
                    .map(|x| ago(x.as_secs()))
                    .unwrap_or_default(),
                d.subject.clone(),
                d.to.clone(),
            ),
        };
        let subject = if subject.is_empty() {
            "(no subject)".to_string()
        } else {
            subject
        };
        let to = if to.is_empty() {
            String::new()
        } else {
            format!("  to {to}")
        };
        let head = format!(" {age:>6}  {:<18} ", locus_theme::fit(&kind, 18));
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
        let head_style = match (chosen, claude) {
            (true, _) => style,
            (false, true) => locus_theme::st("mauve"),
            (false, false) => locus_theme::dim(),
        };
        lines.push(Line::from(vec![
            Span::styled(head, head_style),
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
                account: "oxford".into(),
                to: None,
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
