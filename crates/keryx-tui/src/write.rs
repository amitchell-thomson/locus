//! Phase 4 in the TUI: triage keys, snooze and file pickers, the `A` pane
//! and the cleanup review screen. Changes go to keryxd as Alec (interactive);
//! the list re-reads the mirror afterwards. Reviews are read from the mirror
//! directly; only decisions go to keryxd.

use crate::app::{App, Key, Mode};
use chrono::{Datelike, Duration, NaiveTime, TimeZone, Weekday};
use keryx_store::api::{Decision, Intent, ReviewView, TriageReq};
use keryx_store::state::{self, Action, Keep, ProposalStatus};

/// Work for keryxd, run after the key is drawn (so "archiving…" shows).
#[derive(Debug, Clone, PartialEq)]
pub enum Job {
    Triage { req: TriageReq, quiet: bool },
    UndoLast,
    Decide { proposal: String, decision: Decision },
    AcceptGroup { review: String, group: String },
    UndoReview(String),
    Tag(String),
    Untag(String),
    Sync,
}

/// A line of the review screen.
#[derive(Debug, Clone, PartialEq)]
pub enum ReviewLine {
    Header { group: String, open: usize, total: usize },
    Item(usize),
}

/// Group order: largest group first, then by name; items keep their order.
pub fn review_lines(v: &ReviewView) -> Vec<ReviewLine> {
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, it) in v.items.iter().enumerate() {
        match groups.iter_mut().find(|(g, _)| *g == it.group) {
            Some((_, v)) => v.push(i),
            None => groups.push((it.group.clone(), vec![i])),
        }
    }
    groups.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));
    let mut out = Vec::new();
    for (g, items) in groups {
        let open = items
            .iter()
            .filter(|&&i| v.items[i].proposal.status == ProposalStatus::Open)
            .count();
        out.push(ReviewLine::Header {
            group: g,
            open,
            total: items.len(),
        });
        out.extend(items.into_iter().map(ReviewLine::Item));
    }
    out
}

/// Snooze presets: (label, unix time).
pub fn snooze_presets(app: &App) -> Vec<(String, i64)> {
    let tz = app.cfg.tz();
    let now = app.now.with_timezone(&tz);
    let morning = keryx_core::config::parse_hhmm(&app.cfg.snooze.morning)
        .unwrap_or_else(|| NaiveTime::from_hms_opt(8, 0, 0).unwrap_or_default());
    let at = |d: chrono::NaiveDate| {
        tz.from_local_datetime(&d.and_time(morning))
            .earliest()
            .map(|t| t.timestamp())
            .unwrap_or(0)
    };
    let today = now.date_naive();
    let tomorrow = today + Duration::days(1);
    let mut monday = tomorrow;
    while monday.weekday() != Weekday::Mon {
        monday += Duration::days(1);
    }
    let later = now + Duration::hours(i64::from(app.cfg.snooze.later_today_hours));
    vec![
        ("Later today".into(), later.timestamp()),
        ("Tomorrow morning".into(), at(tomorrow)),
        ("Monday morning".into(), at(monday)),
    ]
}

/// `Pick…`: natural input ("fri 17:00", "23 oct", "tomorrow 9") → time.
/// A bare date means the morning time. Must be in the future.
pub fn parse_pick(app: &App, input: &str) -> Result<i64, String> {
    parse_when(&app.cfg, app.now, input)
}

/// Natural time input → Unix seconds in the future (`keryx_core::when`).
pub fn parse_when(
    cfg: &keryx_core::config::Config,
    now: chrono::DateTime<chrono_tz::Tz>,
    input: &str,
) -> Result<i64, String> {
    keryx_core::when::parse_future(cfg, now, input)
}

pub fn when_label(app: &App, t: i64) -> String {
    app.cfg
        .tz()
        .timestamp_opt(t, 0)
        .single()
        .map(|d| d.format("%a %-d %b, %H:%M").to_string())
        .unwrap_or_default()
}

impl App {
    /// The threads a triage key acts on: marked rows, else the open thread,
    /// else the selected row.
    pub fn targets(&self) -> Vec<String> {
        if !self.marked.is_empty() {
            return self.marked.iter().cloned().collect();
        }
        if self.mode == Mode::Thread
            && let Some(o) = &self.open
        {
            return vec![keryx_core::model::thread_ref(&o.account, &o.thread_id)];
        }
        self.selected().map(|r| vec![r.r#ref.clone()]).unwrap_or_default()
    }

    fn triage(&mut self, intent: Intent, doing: &str) {
        let refs = self.targets();
        if refs.is_empty() {
            return;
        }
        let n = refs.len();
        let leaves = matches!(intent, Intent::Archive | Intent::File { .. } | Intent::Snooze { .. });
        self.deferred.push(Job::Triage {
            req: TriageReq {
                intent,
                refs,
                summary: None,
                review_id: None,
            },
            quiet: false,
        });
        self.message = Some((format!("{doing} {n} thread{}…", if n == 1 { "" } else { "s" }), false));
        self.marked.clear();
        if leaves && self.mode == Mode::Thread {
            self.open = None;
            self.mode = Mode::List;
        }
    }

    /// Triage keys in the list and thread view. Returns whether `k` was one.
    pub fn write_key(&mut self, k: Key) -> bool {
        match k {
            Key::Char('d') => self.triage(Intent::Archive, "archiving"),
            Key::Char('s') => self.mode = Mode::Snooze(None),
            Key::Char('m') => {
                if self.file_keys().is_empty() {
                    self.message = Some(("no key is mapped for this account (config [[keys]])".into(), true));
                } else {
                    self.mode = Mode::File;
                }
            }
            Key::Char('f') => {
                let on = !self
                    .rows
                    .iter()
                    .filter(|r| self.targets().contains(&r.r#ref))
                    .all(|r| r.flagged);
                self.triage(Intent::Flag { on }, if on { "flagging" } else { "unflagging" });
            }
            Key::Char('U') => self.triage(Intent::Read { on: false }, "marking unread"),
            Key::Char('u') => {
                self.deferred.push(Job::UndoLast);
                self.message = Some(("undoing…".into(), false));
            }
            Key::Char('x') if self.mode == Mode::List => {
                if let Some(r) = self.selected().map(|r| r.r#ref.clone()) {
                    if !self.marked.remove(&r) {
                        self.marked.insert(r);
                    }
                    self.move_sel_by(1);
                }
            }
            Key::Char('N') => {
                for r in self.targets() {
                    let tagged = self.rows.iter().any(|x| x.r#ref == r && x.needs_reply.is_some());
                    self.deferred.push(if tagged { Job::Untag(r) } else { Job::Tag(r) });
                }
            }
            Key::Char('A') => self.mode = Mode::Approvals { sel: 0 },
            Key::Ctrl('r') => {
                self.deferred.push(Job::Sync);
                self.message = Some(("syncing…".into(), false));
            }
            _ => return false,
        }
        true
    }

    /// Keys usable for `m` on the targets: mapped on every target's account.
    pub fn file_keys(&self) -> Vec<(usize, String)> {
        let accounts: Vec<String> = self
            .targets()
            .iter()
            .filter_map(|r| keryx_core::model::parse_thread_ref(r).map(|(a, _)| a.to_string()))
            .collect();
        self.cfg
            .keys
            .iter()
            .enumerate()
            .filter(|(_, k)| !accounts.is_empty() && accounts.iter().all(|a| k.folders.contains_key(a)))
            .map(|(i, k)| (i + 1, k.name.clone()))
            .collect()
    }

    pub fn snooze_key(&mut self, k: Key, input: Option<String>) {
        let back = if self.open.is_some() { Mode::Thread } else { Mode::List };
        match (input, k) {
            (None, Key::Esc) => self.mode = back,
            (None, Key::Char(c)) if ('1'..='3').contains(&c) => {
                let p = snooze_presets(self);
                let (label, t) = p[c as usize - '1' as usize].clone();
                self.mode = back;
                self.triage(Intent::Snooze { until: t }, &format!("snoozing to {label}:"));
            }
            (None, Key::Char('4' | 'p')) => self.mode = Mode::Snooze(Some(String::new())),
            (None, _) => {}
            (Some(_), Key::Esc) => self.mode = Mode::Snooze(None),
            (Some(mut s), Key::Backspace) => {
                s.pop();
                self.mode = Mode::Snooze(Some(s));
            }
            (Some(mut s), Key::Char(c)) => {
                s.push(c);
                self.mode = Mode::Snooze(Some(s));
            }
            (Some(s), Key::Enter) => match parse_pick(self, &s) {
                Ok(t) => {
                    let label = when_label(self, t);
                    self.mode = back;
                    self.triage(Intent::Snooze { until: t }, &format!("snoozing to {label}:"));
                }
                Err(e) => self.message = Some((e, true)),
            },
            (Some(s), _) => self.mode = Mode::Snooze(Some(s)),
        }
    }

    pub fn file_key(&mut self, k: Key) {
        let back = if self.open.is_some() { Mode::Thread } else { Mode::List };
        match k {
            Key::Char(c) if c.is_ascii_digit() && c != '0' => {
                let n = c as usize - '0' as usize;
                if let Some((_, name)) = self.file_keys().into_iter().find(|(i, _)| *i == n) {
                    self.mode = back;
                    self.triage(Intent::File { key: name.clone() }, &format!("filing into {name}:"));
                }
            }
            _ => self.mode = back,
        }
    }

    // ---------------- approvals and reviews ----------------

    /// Open reviews: (id, title, open proposals, total).
    pub fn open_reviews(&self) -> Vec<(String, String, usize, usize)> {
        self.db
            .reviews(Some("open"))
            .unwrap_or_default()
            .into_iter()
            .map(|r| {
                let ps = self.db.proposals(&r.id).unwrap_or_default();
                let open = ps.iter().filter(|p| p.status == ProposalStatus::Open).count();
                (r.id, r.title, open, ps.len())
            })
            .collect()
    }

    pub fn approvals_key(&mut self, k: Key, sel: usize) {
        let list = self.open_reviews();
        match k {
            Key::Esc | Key::Char('q') | Key::Char('A') => self.mode = Mode::List,
            Key::Char('j') | Key::Down => {
                self.mode = Mode::Approvals {
                    sel: (sel + 1).min(list.len().saturating_sub(1)),
                }
            }
            Key::Char('k') | Key::Up => {
                self.mode = Mode::Approvals {
                    sel: sel.saturating_sub(1),
                }
            }
            Key::Enter => {
                if let Some((id, ..)) = list.get(sel) {
                    self.load_review(id);
                    let first = self.first_open_item();
                    self.mode = Mode::Review {
                        id: id.clone(),
                        sel: first,
                    };
                }
            }
            _ => {}
        }
    }

    pub fn load_review(&mut self, id: &str) {
        self.review = state::review_view(&self.db, id).ok().flatten();
    }

    /// Item indices in screen order.
    pub fn review_items(&self) -> Vec<usize> {
        self.review
            .as_ref()
            .map(|v| {
                review_lines(v)
                    .into_iter()
                    .filter_map(|l| match l {
                        ReviewLine::Item(i) => Some(i),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn first_open_item(&self) -> usize {
        let Some(v) = &self.review else { return 0 };
        self.review_items()
            .iter()
            .position(|&i| v.items[i].proposal.status == ProposalStatus::Open)
            .unwrap_or(0)
    }

    /// The selected item (index into `review.items`).
    pub fn review_selected(&self, sel: usize) -> Option<usize> {
        self.review_items().get(sel).copied()
    }

    /// Proposals the keys act on: marked ones, else the selection.
    fn review_targets(&self, sel: usize) -> Vec<String> {
        let Some(v) = &self.review else { return vec![] };
        if !self.review_marked.is_empty() {
            return self.review_marked.iter().cloned().collect();
        }
        self.review_selected(sel)
            .map(|i| vec![v.items[i].proposal.id.clone()])
            .unwrap_or_default()
    }

    fn decide(&mut self, sel: usize, d: Decision) {
        for p in self.review_targets(sel) {
            self.deferred.push(Job::Decide {
                proposal: p,
                decision: d.clone(),
            });
        }
        self.review_marked.clear();
    }

    pub fn review_key(&mut self, k: Key, id: String, sel: usize) {
        let n = self.review_items().len();
        let next = (sel + 1).min(n.saturating_sub(1));
        match k {
            Key::Esc | Key::Char('q') => {
                self.review = None;
                self.review_marked.clear();
                self.mode = Mode::Approvals { sel: 0 };
            }
            Key::Char('j') | Key::Down => self.mode = Mode::Review { id, sel: next },
            Key::Char('k') | Key::Up => {
                self.mode = Mode::Review {
                    id,
                    sel: sel.saturating_sub(1),
                }
            }
            Key::Char('y') => {
                self.decide(sel, Decision::Accept);
                self.mode = Mode::Review { id, sel: next };
            }
            Key::Char('n') => {
                self.decide(sel, Decision::Decline);
                self.mode = Mode::Review { id, sel: next };
            }
            Key::Char('c') => self.mode = Mode::ReviewChange { id, sel },
            Key::Char('x') => {
                if let Some(i) = self.review_selected(sel)
                    && let Some(v) = &self.review
                {
                    let p = v.items[i].proposal.id.clone();
                    if !self.review_marked.remove(&p) {
                        self.review_marked.insert(p);
                    }
                }
                self.mode = Mode::Review { id, sel: next };
            }
            Key::Char('Y') => {
                let Some(v) = &self.review else { return };
                let Some(i) = self.review_selected(sel) else { return };
                let group = v.items[i].group.clone();
                let count = v
                    .items
                    .iter()
                    .filter(|it| it.group == group && it.proposal.status == ProposalStatus::Open && !it.stale)
                    .count();
                if count > 0 {
                    self.mode = Mode::ConfirmGroup { id, sel, group, count };
                }
            }
            Key::Enter => {
                let Some(v) = &self.review else { return };
                let Some(i) = self.review_selected(sel) else { return };
                let it = &v.items[i];
                if let Some((a, t)) = keryx_core::model::parse_thread_ref(&it.r#ref) {
                    // Reading from the review never marks the thread read.
                    self.open = self.load_thread(a, t);
                    if self.open.is_some() {
                        self.back = Some(Mode::Review { id, sel });
                        self.mode = Mode::Thread;
                    }
                }
            }
            Key::Char('u') => {
                self.deferred.push(Job::UndoLast);
                self.message = Some(("undoing the last decision…".into(), false));
            }
            _ => {}
        }
    }

    pub fn review_change_key(&mut self, k: Key, id: String, sel: usize) {
        let action = match k {
            Key::Char('a') => Some(Action::Archive),
            Key::Char('k') => Some(Action::Keep {
                why: Keep::Action,
                needs_reply: false,
            }),
            Key::Char('s') => snooze_presets(self).get(1).map(|(_, t)| Action::Snooze { until: *t }),
            Key::Char(c) if c.is_ascii_digit() && c != '0' => self
                .cfg
                .keys
                .get(c as usize - '1' as usize)
                .map(|k| Action::File { key: k.name.clone() }),
            _ => None,
        };
        if let Some(a) = action {
            self.decide(sel, Decision::Change(a));
            let n = self.review_items().len();
            self.mode = Mode::Review {
                id,
                sel: (sel + 1).min(n.saturating_sub(1)),
            };
        } else {
            self.mode = Mode::Review { id, sel };
        }
    }

    pub fn confirm_group_key(&mut self, k: Key, id: String, sel: usize, group: String) {
        if matches!(k, Key::Char('y') | Key::Enter) {
            self.deferred.push(Job::AcceptGroup {
                review: id.clone(),
                group,
            });
        }
        self.mode = Mode::Review { id, sel };
    }

    /// Run queued work against keryxd, then re-read the mirror.
    pub fn run_deferred(&mut self) {
        let jobs = std::mem::take(&mut self.deferred);
        if jobs.is_empty() {
            return;
        }
        let Some(d) = self.daemon.as_ref() else {
            if jobs.iter().any(|j| !matches!(j, Job::Triage { quiet: true, .. })) {
                self.message = Some(("can't change mail: keryxd isn't running (keryx doctor)".into(), true));
            }
            return;
        };
        let mut last: Option<(String, bool)> = None;
        for job in jobs {
            let res: Result<Option<String>, String> = match &job {
                Job::Triage { req, quiet } => d
                    .triage(req)
                    .map(|o| (!quiet).then_some(o.summary))
                    .map_err(|e| e.to_string()),
                Job::UndoLast => d.undo_last().map(|c| Some(c.summary)).map_err(|e| e.to_string()),
                Job::Decide { proposal, decision } => {
                    d.decide(proposal, decision).map(|_| None).map_err(|e| e.to_string())
                }
                Job::AcceptGroup { review, group } => d
                    .accept_group(review, group)
                    .map(|g| {
                        Some(format!(
                            "accepted {}{}",
                            g.accepted,
                            if g.skipped_stale > 0 {
                                format!(" · {} changed since proposed, left for you", g.skipped_stale)
                            } else {
                                String::new()
                            }
                        ))
                    })
                    .map_err(|e| e.to_string()),
                Job::UndoReview(r) => d
                    .undo_review(r)
                    .map(|u| Some(format!("undid {} change(s)", u.undone)))
                    .map_err(|e| e.to_string()),
                Job::Tag(r) => d
                    .tag(r, "tagged by Alec")
                    .map(|_| Some("tagged: needs reply".into()))
                    .map_err(|e| e.to_string()),
                Job::Untag(r) => d
                    .untag(r)
                    .map(|_| Some("needs-reply tag cleared".into()))
                    .map_err(|e| e.to_string()),
                Job::Sync => d.sync().map(|_| Some("syncing".into())).map_err(|e| e.to_string()),
            };
            match res {
                Ok(Some(m)) => last = Some((m, false)),
                Ok(None) => {}
                Err(e) => last = Some((e, true)),
            }
        }
        self.message = last;
        self.reload();
        if let Mode::Review { id, .. } = &self.mode {
            let id = id.clone();
            self.load_review(&id);
        }
    }
}
