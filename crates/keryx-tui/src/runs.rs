//! The Claude runs view (`g` → `c`, SPEC §8.1): one line per run, and a
//! run's detail with everything it made. A log, not a banner: `Enter` opens
//! an item's thread, `u` undoes a cleared thread, `A` reviews drafts.

use crate::app::{App, Key, Mode};
use crate::write::Job;
use keryx_store::runs::{RunItem, RunView};

/// One line of a run's detail.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemLine {
    pub kind: &'static str,
    pub text: String,
    /// open | applied | undone | pending | sent | …
    pub status: String,
    pub thread: Option<(String, String)>,
    /// The changeset to undo (`u`), for cleared threads.
    pub undo: Option<String>,
}

impl App {
    pub fn runs(&self) -> Vec<RunView> {
        self.db
            .runs(50)
            .unwrap_or_default()
            .iter()
            .filter_map(|r| self.db.run_view(&r.id).ok().flatten())
            .collect()
    }

    fn subject(&self, account: &str, thread_id: &str) -> String {
        self.db
            .thread(account, thread_id)
            .ok()
            .and_then(|m| m.last().map(|m| m.subject.clone()))
            .unwrap_or_else(|| "(gone)".into())
    }

    /// What a run made, newest state of each.
    pub fn run_lines(&self, v: &RunView) -> Vec<ItemLine> {
        let mut out = Vec::new();
        for it in &v.items {
            let thread = match (&it.account, &it.thread_id) {
                (Some(a), Some(t)) => Some((a.clone(), t.clone())),
                _ => None,
            };
            let subj = thread.as_ref().map(|(a, t)| self.subject(a, t)).unwrap_or_default();
            let line = match it.kind.as_str() {
                "changeset" => {
                    let cs_id = it.item_id.split(':').next().unwrap_or("").to_string();
                    let status = self
                        .db
                        .changeset(&cs_id)
                        .ok()
                        .flatten()
                        .map(|c| c.status.as_str().to_string())
                        .unwrap_or_default();
                    ItemLine {
                        kind: "cleared",
                        text: subj,
                        status,
                        thread,
                        undo: Some(cs_id),
                    }
                }
                "tag" => ItemLine {
                    kind: "needs reply",
                    text: subj,
                    status: String::new(),
                    thread,
                    undo: None,
                },
                "draft" => ItemLine {
                    kind: "draft",
                    text: if subj.is_empty() { draft_subject(self, it) } else { subj },
                    status: self
                        .db
                        .changeset(&it.item_id)
                        .ok()
                        .flatten()
                        .map(|c| c.status.as_str().to_string())
                        .unwrap_or_default(),
                    thread,
                    undo: None,
                },
                "suggestion" => {
                    let s = self
                        .db
                        .suggestions(None, thread.as_ref().map(|(a, t)| (a.as_str(), t.as_str())))
                        .unwrap_or_default()
                        .into_iter()
                        .find(|s| s.id == it.item_id);
                    ItemLine {
                        kind: "suggestion",
                        text: s
                            .as_ref()
                            .and_then(|s| s.proposal.get("title").and_then(|t| t.as_str()).map(str::to_string))
                            .unwrap_or(subj),
                        status: s.map(|s| s.status).unwrap_or_default(),
                        thread,
                        undo: None,
                    }
                }
                "review" => {
                    let (title, open) = keryx_store::state::review_view(&self.db, &it.item_id)
                        .ok()
                        .flatten()
                        .map(|r| {
                            let open = r
                                .items
                                .iter()
                                .filter(|i| i.proposal.status == keryx_store::state::ProposalStatus::Open)
                                .count();
                            (r.review.title, open)
                        })
                        .unwrap_or_default();
                    ItemLine {
                        kind: "review",
                        text: title,
                        status: format!("{open} open"),
                        thread: None,
                        undo: None,
                    }
                }
                _ => continue,
            };
            out.push(line);
        }
        out
    }

    pub fn runs_key(&mut self, k: Key, sel: usize) {
        let n = self.runs().len();
        match k {
            Key::Char('j') | Key::Down => {
                self.mode = Mode::Runs {
                    sel: (sel + 1).min(n.saturating_sub(1)),
                }
            }
            Key::Char('k') | Key::Up => {
                self.mode = Mode::Runs {
                    sel: sel.saturating_sub(1),
                }
            }
            Key::Enter => {
                if let Some(v) = self.runs().get(sel) {
                    self.mode = Mode::RunDetail {
                        id: v.run.id.clone(),
                        sel: 0,
                    };
                }
            }
            Key::Esc | Key::Char('q') => self.mode = Mode::List,
            _ => {}
        }
    }

    pub fn run_detail_key(&mut self, k: Key, id: String, sel: usize) {
        let Some(v) = self.db.run_view(&id).ok().flatten() else {
            self.mode = Mode::Runs { sel: 0 };
            return;
        };
        let lines = self.run_lines(&v);
        let here = Mode::RunDetail { id: id.clone(), sel };
        match k {
            Key::Char('j') | Key::Down => {
                self.mode = Mode::RunDetail {
                    id,
                    sel: (sel + 1).min(lines.len().saturating_sub(1)),
                }
            }
            Key::Char('k') | Key::Up => {
                self.mode = Mode::RunDetail {
                    id,
                    sel: sel.saturating_sub(1),
                }
            }
            Key::Enter => match lines.get(sel) {
                Some(l) if l.kind == "review" || l.kind == "draft" => self.mode = Mode::Approvals { sel: 0 },
                Some(ItemLine {
                    thread: Some((a, t)), ..
                }) => {
                    let (a, t) = (a.clone(), t.clone());
                    if self.open_thread(&a, &t) {
                        self.back = Some(here);
                    }
                }
                _ => {}
            },
            Key::Char('u') => {
                if let Some(ItemLine {
                    undo: Some(cs), status, ..
                }) = lines.get(sel)
                {
                    if status == "applied" {
                        self.deferred.push(Job::Undo(cs.clone()));
                        self.message = Some(("undoing…".into(), false));
                    } else {
                        self.message = Some((format!("that change is {status}"), false));
                    }
                }
            }
            Key::Char('A') => self.mode = Mode::Approvals { sel: 0 },
            Key::Esc | Key::Char('q') => self.mode = Mode::Runs { sel: 0 },
            _ => {}
        }
    }
}

fn draft_subject(app: &App, it: &RunItem) -> String {
    app.db
        .changeset(&it.item_id)
        .ok()
        .flatten()
        .and_then(|c| c.outgoing().map(|o| o.subject.clone()))
        .unwrap_or_default()
}
