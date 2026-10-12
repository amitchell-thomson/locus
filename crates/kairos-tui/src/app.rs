//! TUI state and key handling (no terminal I/O here, so it's testable).

use chrono::{DateTime, Duration, NaiveDate, NaiveTime, Utc};
use chrono_tz::Tz;
use kairos_core::api;
use kairos_core::changeset::{Changeset, Op, Status, WriteResult};
use kairos_core::config::{Config, Overlay};
use kairos_core::dates::{self, When};
use kairos_core::dedup::Cluster;
use kairos_core::model::{EventKind, Instance, Meta, Span};
use kairos_core::normalize::series_from_calendars;
use kairos_core::task::Task;
use kairos_core::time::resolve_local;
use kairos_core::view::{Filter, view_with};
use kairos_store::cache::Index;
use kairos_store::client::{Daemon, Rejected};

use crate::form::{Form, FormAction, FormKind, describe_span, when_prefill};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Week,
    Day,
    Agenda,
    Tasks,
    Approvals,
}

impl View {
    pub fn name(self) -> &'static str {
        match self {
            View::Week => "week",
            View::Day => "day",
            View::Agenda => "agenda",
            View::Tasks => "tasks",
            View::Approvals => "approvals",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptKind {
    Move,
    Search,
    /// Check-in of a coursework block: how much of the task is left?
    Remaining {
        uid: String,
        title: String,
        outcome: String,
    },
    /// Accepting proposed project time: what to work on.
    Project {
        id: String,
        title: String,
    },
    /// More time for a task (its planned time is used up, or it grew).
    MoreTime {
        id: String,
        title: String,
    },
}

impl PromptKind {
    pub fn label(&self) -> &'static str {
        match self {
            PromptKind::Move => "move to (when)",
            PromptKind::Search => "/",
            PromptKind::Remaining { .. } => "how much is left? (e.g. 3h, 0 = finished)",
            PromptKind::Project { .. } => "work on (Enter = Project time)",
            PromptKind::MoreTime { .. } => "how much more time? (e.g. 2h)",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirm {
    Delete,
    Share,
    Undo(String),
    DropTask(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Prompt(PromptKind, String),
    Form(Form),
    Confirm(Confirm),
    Help,
    Checkin(usize),
}

/// A pending change drawn as a ghost block.
#[derive(Debug, Clone, PartialEq)]
pub struct Ghost {
    pub changeset_id: String,
    pub title: String,
    pub span: Span,
}

/// A change kairosd has applied that the local copy (pulled from iCloud)
/// may not show yet. Re-applied over every reload until it expires, so the
/// view doesn't flicker back or lose the event mid-move.
#[derive(Debug, Clone)]
pub struct LocalPatch {
    pub uid: String,
    /// `None` = deleted.
    pub inst: Option<Instance>,
    pub until: DateTime<Utc>,
}

/// Work to run right after the next frame is drawn (server round trips), so
/// the screen shows the change before the network answers.
pub type Deferred = Box<dyn FnOnce(&mut App)>;

pub struct App {
    pub cfg: Config,
    pub patches: Vec<LocalPatch>,
    /// (uid, blocking, until): Family "block my time" flags set from the TUI.
    pub blocking_overrides: Vec<(String, bool, DateTime<Utc>)>,
    pub deferred: Vec<Deferred>,
    pub tz: Tz,
    pub now: DateTime<Utc>,
    pub index: Index,
    pub filter: Filter,
    pub view: View,
    pub cursor: NaiveDate,
    /// Selected item within the cursor day (week/day) or list (agenda/tasks/approvals).
    pub sel: usize,
    pub expanded: bool,
    pub mode: Mode,
    pub search: Option<String>,
    pub message: Option<(String, bool)>,
    pub tasks: Vec<Task>,
    pub pending: Vec<Changeset>,
    pub checkins: Vec<api::CheckinItem>,
    /// Proposed project time (greyed; accept or drop).
    pub proposals: Vec<api::ProposalItem>,
    /// Squeezed work blocks (from a dry-run plan): (task id, start, reasons).
    pub squeezed: Vec<(String, DateTime<Utc>, Vec<String>)>,
    pub daemon: Option<Daemon>,
    pub daemon_error: Option<String>,
    pub quit: bool,
    /// Set after a write so the runner kicks a sync + reload.
    pub wants_sync: bool,
    /// Week view shows Mon–Fri only (wider columns).
    pub work_week: bool,
}

pub fn midnight(d: NaiveDate, tz: Tz) -> DateTime<Utc> {
    resolve_local(d.and_time(NaiveTime::MIN), tz)
}

impl App {
    pub fn new(cfg: Config, index: Index, now: DateTime<Utc>, daemon: Option<Daemon>) -> App {
        let tz = cfg.tz();
        App {
            filter: Filter::from_config(&cfg),
            cursor: now.with_timezone(&tz).date_naive(),
            tz,
            now,
            index,
            view: View::Week,
            sel: 0,
            expanded: false,
            mode: Mode::Normal,
            search: None,
            message: None,
            tasks: Vec::new(),
            pending: Vec::new(),
            checkins: Vec::new(),
            proposals: Vec::new(),
            squeezed: Vec::new(),
            daemon,
            daemon_error: None,
            quit: false,
            wants_sync: false,
            work_week: false,
            patches: Vec::new(),
            blocking_overrides: Vec::new(),
            deferred: Vec::new(),
            cfg,
        }
    }

    pub fn today(&self) -> NaiveDate {
        self.now.with_timezone(&self.tz).date_naive()
    }

    /// Visible clusters in `[from, to)`, honouring filter and search.
    pub fn clusters(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Vec<Cluster> {
        let inst = self.index.between(from, to, self.tz);
        let mut v = view_with(inst, |id| self.index.kind_of(id), &self.filter, self.tz);
        if let Some(q) = self.search.as_deref().filter(|q| !q.is_empty()) {
            let q = q.to_lowercase();
            v.retain(|c| {
                c.all().any(|i| {
                    i.title.to_lowercase().contains(&q)
                        || i.location.as_deref().is_some_and(|l| l.to_lowercase().contains(&q))
                })
            });
        }
        v
    }

    /// Proposed project time in `[from, to)` as (greyed, selectable) events.
    fn proposal_clusters(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Vec<Cluster> {
        if !self.filter.personal {
            return Vec::new();
        }
        let q = self.search.as_deref().map(str::to_lowercase).filter(|q| !q.is_empty());
        self.proposals
            .iter()
            .filter(|p| p.start < to && p.end > from)
            .filter(|p| q.as_ref().is_none_or(|q| p.title.to_lowercase().contains(q)))
            .map(|p| Cluster {
                primary: Instance {
                    uid: p.id.clone(),
                    recurrence_id: None,
                    source: "proposal".into(),
                    overlay: Overlay::Personal,
                    key: None,
                    title: p.title.clone(),
                    location: None,
                    description: Some("proposed project time — y accept · d drop".into()),
                    span: Span::Timed {
                        start: p.start,
                        end: p.end,
                    },
                    recurring: false,
                    transparent: true,
                    tentative: false,
                    meta: Meta {
                        kind: EventKind::Proposal,
                        ..Default::default()
                    },
                    href: None,
                    alarms: Vec::new(),
                },
                others: Vec::new(),
            })
            .collect()
    }

    pub fn day_clusters(&self, d: NaiveDate) -> Vec<Cluster> {
        let (from, to) = (midnight(d, self.tz), midnight(d + Duration::days(1), self.tz));
        let mut v: Vec<Cluster> = self
            .clusters(from, to)
            .into_iter()
            .chain(self.proposal_clusters(from, to))
            .filter(|c| c.primary.span.local_dates(self.tz).contains(&d))
            .collect();
        v.sort_by_key(|c| (!c.primary.span.is_all_day(), c.primary.start(self.tz)));
        v
    }

    pub fn agenda_days(&self) -> (NaiveDate, NaiveDate) {
        (self.cursor, self.cursor + Duration::days(13))
    }

    /// Agenda rows: (date, cluster).
    pub fn agenda_rows(&self) -> Vec<(NaiveDate, Cluster)> {
        let (a, b) = self.agenda_days();
        let mut out = Vec::new();
        let mut d = a;
        while d <= b {
            for c in self.day_clusters(d) {
                out.push((d, c));
            }
            d += Duration::days(1);
        }
        out
    }

    pub fn selected(&self) -> Option<Cluster> {
        match self.view {
            View::Agenda => self.agenda_rows().into_iter().nth(self.sel).map(|(_, c)| c),
            View::Week | View::Day => self.day_clusters(self.cursor).into_iter().nth(self.sel),
            _ => None,
        }
    }

    fn list_len(&self) -> usize {
        match self.view {
            View::Agenda => self.agenda_rows().len(),
            View::Week | View::Day => self.day_clusters(self.cursor).len(),
            View::Tasks => self.tasks.len(),
            View::Approvals => self.pending.len(),
        }
    }

    /// Ghost blocks for pending changesets' new/updated event times.
    pub fn ghosts(&self) -> Vec<Ghost> {
        let mut out = Vec::new();
        for cs in &self.pending {
            for op in &cs.ops {
                let after = match op {
                    Op::CreateEvent { after, .. } | Op::UpdateEvent { after, .. } | Op::MoveEvent { after, .. } => {
                        after
                    }
                    _ => continue,
                };
                let Ok(cals) = kairos_core::ics::parse(after) else {
                    continue;
                };
                let mut w = Vec::new();
                for s in series_from_calendars(&cals, "pending", None, &mut w) {
                    if let Some(m) = &s.master {
                        let span = if m.start.is_date() {
                            Span::AllDay {
                                start: m.start.naive().date(),
                                end: m.end.naive().date(),
                            }
                        } else {
                            Span::Timed {
                                start: m.start.instant(self.tz),
                                end: m.end.instant(self.tz),
                            }
                        };
                        out.push(Ghost {
                            changeset_id: cs.id.clone(),
                            title: m.summary.clone(),
                            span,
                        });
                    }
                }
            }
        }
        out
    }

    pub fn set_message(&mut self, s: impl Into<String>, error: bool) {
        self.message = Some((s.into(), error));
    }

    // ------------------------------------------------------------ daemon

    pub fn refresh_remote(&mut self) {
        let Some(d) = &self.daemon else {
            self.daemon_error = Some("daemon not configured".into());
            return;
        };
        match d.changesets(Some("pending"), 100) {
            Ok(Ok(v)) => {
                self.pending = v;
                self.daemon_error = None;
            }
            Ok(Err(e)) => self.daemon_error = Some(e.to_string()),
            Err(e) => {
                self.daemon_error = Some(e.to_string());
                return;
            }
        }
        if let Ok(Ok(t)) = d.tasks("open") {
            self.tasks = t;
        }
        if let Ok(Ok(c)) = d.checkin_items() {
            self.checkins = c;
        }
        if let Ok(Ok(p)) = d.proposals() {
            self.proposals = p;
        }
        if let Ok(Ok(p)) = d.plan(true) {
            self.squeezed = p
                .squeezed
                .iter()
                .flat_map(|s| s.blocks.iter().map(|b| (s.task_id.clone(), b.start, b.reasons.clone())))
                .collect();
        }
    }

    /// Why a work block is squeezed, if it is.
    pub fn squeeze_of(&self, i: &Instance) -> Option<&[String]> {
        let task = i.meta.task_id.as_deref()?;
        let start = i.start(self.tz);
        self.squeezed
            .iter()
            .find(|(t, s, _)| t == task && *s == start)
            .map(|(_, _, r)| r.as_slice())
    }

    /// Replace the index (after a reload) and re-apply recent local patches.
    pub fn set_index(&mut self, idx: Index) {
        self.index = idx;
        let now = Utc::now();
        self.blocking_overrides.retain(|(_, _, until)| *until > now);
        for (uid, on, _) in &self.blocking_overrides {
            for i in self.index.instances.iter_mut().filter(|i| &i.uid == uid) {
                i.meta.blocking = *on;
            }
        }
        self.patches.retain(|p| p.until > now);
        // An accepted proposal's stand-in goes once the real event has synced.
        let idx = &self.index;
        let tz = self.tz;
        self.patches.retain(|p| {
            !p.uid.starts_with("accepted:")
                || !p.inst.as_ref().is_some_and(|a| {
                    idx.instances
                        .iter()
                        .any(|i| i.title == a.title && i.start(tz) == a.start(tz))
                })
        });
        for p in self.patches.clone() {
            self.index.instances.retain(|i| i.uid != p.uid);
            if let Some(i) = p.inst {
                let at = self
                    .index
                    .instances
                    .partition_point(|x| x.start(self.tz) <= i.start(self.tz));
                self.index.instances.insert(at, i);
            }
        }
    }

    /// Show a Family event's "blocks my time" flag at once (all occurrences),
    /// and keep it over reloads until the server's copy has caught up.
    fn set_blocking_local(&mut self, uid: &str, on: bool) {
        self.blocking_overrides.retain(|(u, _, _)| u != uid);
        self.blocking_overrides
            .push((uid.to_string(), on, Utc::now() + Duration::seconds(90)));
        for i in self.index.instances.iter_mut().filter(|i| i.uid == uid) {
            i.meta.blocking = on;
        }
    }

    pub fn run_deferred(&mut self) {
        for f in std::mem::take(&mut self.deferred) {
            f(self);
        }
    }

    /// Undo an optimistic patch the server refused.
    fn unpatch(&mut self, orig: Option<Instance>, uid: &str) {
        self.patches.retain(|p| p.uid != uid);
        self.index.instances.retain(|i| i.uid != uid);
        if let Some(i) = orig {
            let at = self
                .index
                .instances
                .partition_point(|x| x.start(self.tz) <= i.start(self.tz));
            self.index.instances.insert(at, i);
        }
    }

    /// Show a change straight away (one-off events only; series wait for the
    /// sync). Optimistic: callers roll back with [`App::unpatch`] on failure.
    fn patch_local(&mut self, uid: &str, f: impl Fn(&mut Instance) -> bool) {
        let Some(cur) = self.index.instances.iter().find(|i| i.uid == uid).cloned() else {
            return;
        };
        if cur.recurring {
            return;
        }
        let mut next = cur;
        let keep = f(&mut next);
        self.patches.retain(|p| p.uid != uid);
        self.patches.push(LocalPatch {
            uid: uid.to_string(),
            inst: keep.then_some(next),
            until: Utc::now() + Duration::seconds(90),
        });
        let idx = self.index.clone();
        self.set_index(idx);
    }

    /// Where a key's events live (source id), for local patches.
    fn source_for_key(&self, key: &str) -> Option<String> {
        self.cfg
            .sources
            .iter()
            .find(|s| {
                s.overlay == Overlay::Personal
                    && s.key.as_deref().is_some_and(|k| k.eq_ignore_ascii_case(key))
                    && s.id != "test"
            })
            .map(|s| s.id.clone())
    }

    /// Like `write_result`, but true only if the change was applied (an
    /// optimistic patch must roll back if it was refused or queued).
    fn write_applied(&mut self, r: Result<Result<WriteResult, Rejected>, kairos_store::StoreError>) -> bool {
        let pending = matches!(&r, Ok(Ok(w)) if w.status != Status::Applied);
        self.write_result(r) && !pending
    }

    fn write_result(&mut self, r: Result<Result<WriteResult, Rejected>, kairos_store::StoreError>) -> bool {
        match r {
            Ok(Ok(w)) => {
                let applied = w.status == Status::Applied;
                let msg = match w.status {
                    Status::Applied => format!("✓ {}", w.summary.lines().next().unwrap_or("")),
                    Status::Pending => format!(
                        "◌ needs approval: {} (A to review)",
                        w.summary.lines().next().unwrap_or("")
                    ),
                    s => format!("{} {}", s.as_str(), w.summary),
                };
                self.set_message(msg, false);
                self.wants_sync = true;
                self.refresh_remote();
                applied
            }
            Ok(Err(e)) => {
                self.set_message(e.to_string().replace('\n', " · "), true);
                false
            }
            Err(e) => {
                self.set_message(e.to_string(), true);
                false
            }
        }
    }

    fn with_daemon<T>(&mut self, f: impl FnOnce(&Daemon) -> T) -> Option<T> {
        match &self.daemon {
            Some(d) => Some(f(d)),
            None => {
                self.set_message("kairosd not configured — writes unavailable", true);
                None
            }
        }
    }

    // ------------------------------------------------------------ keys

    pub fn key(&mut self, k: Key) {
        let mode = std::mem::replace(&mut self.mode, Mode::Normal);
        match mode {
            Mode::Help => {}
            Mode::Form(mut form) => match form.key(k) {
                FormAction::Continue => self.mode = Mode::Form(form),
                FormAction::Cancel => self.message = None,
                FormAction::Submit => self.submit_form(form),
            },
            Mode::Prompt(kind, mut buf) => match k {
                Key::Esc => {
                    if kind == PromptKind::Search {
                        self.search = None;
                    }
                }
                Key::Enter => self.submit(kind, buf),
                Key::Backspace => {
                    buf.pop();
                    if kind == PromptKind::Search {
                        self.search = Some(buf.clone());
                    }
                    self.mode = Mode::Prompt(kind, buf);
                }
                Key::Char(c) => {
                    buf.push(c);
                    if kind == PromptKind::Search {
                        self.search = Some(buf.clone());
                        self.sel = 0;
                    }
                    self.mode = Mode::Prompt(kind, buf);
                }
                _ => self.mode = Mode::Prompt(kind, buf),
            },
            Mode::Confirm(c) => {
                if k == Key::Char('y') {
                    self.confirmed(c);
                } else {
                    self.set_message("cancelled", false);
                }
            }
            Mode::Checkin(i) => self.checkin_key(i, k),
            Mode::Normal => self.normal(k),
        }
    }

    fn normal(&mut self, k: Key) {
        let n = self.list_len();
        match k {
            Key::Char('q') | Key::Esc if self.expanded => self.expanded = false,
            Key::Char('q') => self.quit = true,
            Key::Esc => {
                if matches!(self.view, View::Tasks | View::Approvals) {
                    self.view = View::Week;
                    self.sel = 0;
                }
                self.search = None;
            }
            Key::Char('?') => self.mode = Mode::Help,
            Key::Char('h') | Key::Left => self.shift(-1),
            Key::Char('l') | Key::Right => self.shift(1),
            Key::Char('H') => self.shift(-7),
            Key::Char('L') => self.shift(7),
            Key::Char('t') => {
                self.cursor = self.today();
                self.sel = 0;
            }
            Key::Char('j') | Key::Down => {
                if n > 0 {
                    self.sel = (self.sel + 1).min(n - 1);
                }
            }
            Key::Char('k') | Key::Up => self.sel = self.sel.saturating_sub(1),
            Key::Tab => {
                self.view = match self.view {
                    View::Week => View::Day,
                    View::Day => View::Agenda,
                    _ => View::Week,
                };
                self.sel = 0;
            }
            Key::Enter => self.expanded = !self.expanded,
            Key::Char('p') => self.filter.personal = !self.filter.personal,
            Key::Char('f') => self.filter.family = !self.filter.family,
            Key::Char('b') => self.filter.background = !self.filter.background,
            Key::Char('i') => self.filter.imported = !self.filter.imported,
            Key::Char(c @ '1'..='7') => {
                let idx = c as u8 - b'0';
                if let Some(k) = self.cfg.keys.iter().find(|k| k.index == idx) {
                    let name = k.name.clone();
                    self.filter.toggle_key(&name);
                }
            }
            Key::Char('/') => self.mode = Mode::Prompt(PromptKind::Search, self.search.clone().unwrap_or_default()),
            Key::Char('T') => {
                self.view = View::Tasks;
                self.sel = 0;
                self.refresh_remote();
            }
            Key::Char('A') => {
                self.view = View::Approvals;
                self.sel = 0;
                self.refresh_remote();
            }
            Key::Char('C') => {
                self.refresh_remote();
                if self.checkins.iter().any(|c| c.outcome.is_none()) {
                    self.mode = Mode::Checkin(0);
                } else {
                    self.set_message("nothing to check in today", false);
                }
            }
            Key::Char('r') => {
                self.wants_sync = true;
                self.refresh_remote();
                self.set_message("syncing…", false);
            }
            Key::Char('a') => {
                let keys = self.key_names();
                let default = self.cfg.default_key().map(|k| k.name.clone()).unwrap_or_default();
                self.message = None;
                self.mode = Mode::Form(if self.view == View::Tasks {
                    Form::add_task(keys, &default)
                } else {
                    Form::add_event(keys, &default, when_prefill(self.cursor, self.today()))
                });
            }
            Key::Char('w') => {
                self.work_week = !self.work_week;
                self.set_message(
                    if self.work_week {
                        "week: Mon–Fri"
                    } else {
                        "week: Mon–Sun"
                    },
                    false,
                );
            }
            Key::Char('u') => {
                if let Some(d) = &self.daemon
                    && let Ok(Ok(v)) = d.changesets(Some("applied"), 1)
                    && let Some(c) = v.first()
                {
                    self.mode = Mode::Confirm(Confirm::Undo(c.id.clone()));
                    self.set_message(format!("undo “{}”? y/N", c.summary.lines().next().unwrap_or("")), false);
                    return;
                }
                self.set_message("nothing to undo", true);
            }
            _ => self.contextual(k),
        }
    }

    /// Keys whose meaning depends on the view / selection.
    fn contextual(&mut self, k: Key) {
        match self.view {
            View::Approvals => {
                let Some(cs) = self.pending.get(self.sel).cloned() else {
                    return;
                };
                let action = match k {
                    Key::Char('y') => "approve",
                    Key::Char('n') => "reject",
                    _ => return,
                };
                if let Some(r) = self.with_daemon(|d| d.decide(&cs.id, action)) {
                    self.write_result(r);
                    self.sel = self.sel.min(self.pending.len().saturating_sub(1));
                }
            }
            View::Tasks => {
                let Some(t) = self.tasks.get(self.sel).cloned() else {
                    return;
                };
                match k {
                    Key::Char('d') => {
                        if let Some(r) = self.with_daemon(|d| d.complete_task(&t.id, None)) {
                            match r {
                                Ok(Ok(r)) => {
                                    if r.changesets.first().is_some_and(|w| w.status == Status::Applied) {
                                        let now = self.now;
                                        let gone: Vec<String> = self
                                            .index
                                            .instances
                                            .iter()
                                            .filter(|i| {
                                                i.meta.task_id.as_deref() == Some(t.id.as_str())
                                                    && i.start(self.tz) >= now
                                            })
                                            .map(|i| i.uid.clone())
                                            .collect();
                                        for uid in gone {
                                            self.patch_local(&uid, |_| false);
                                        }
                                    }
                                    self.set_message(format!("✓ done: {}", t.title), false);
                                    self.wants_sync = true;
                                    self.refresh_remote();
                                }
                                Ok(Err(e)) => self.set_message(e.to_string(), true),
                                Err(e) => self.set_message(e.to_string(), true),
                            }
                        }
                    }
                    Key::Char('n') => self.start_now(t.id.clone(), t.title.clone()),
                    Key::Char('+') => {
                        self.message = None;
                        self.mode = Mode::Prompt(
                            PromptKind::MoreTime {
                                id: t.id.clone(),
                                title: t.title.clone(),
                            },
                            String::new(),
                        );
                    }
                    Key::Char('x') => {
                        self.mode = Mode::Confirm(Confirm::DropTask(t.id.clone()));
                        self.set_message(format!("drop “{}”? y/N", t.title), false);
                    }
                    Key::Char('P') => self.plan(),
                    _ => {}
                }
            }
            View::Week | View::Day | View::Agenda => {
                if k == Key::Char('P') {
                    self.plan();
                    return;
                }
                let Some(c) = self.selected() else { return };
                let i = c.primary.clone();
                if i.meta.kind == EventKind::Proposal {
                    // Proposed project time: accept (then say what to work on) or drop.
                    match k {
                        Key::Char('y') | Key::Char('B') => {
                            self.message = None;
                            self.mode = Mode::Prompt(
                                PromptKind::Project {
                                    id: i.uid.clone(),
                                    title: i.title.clone(),
                                },
                                String::new(),
                            );
                        }
                        Key::Char('d') | Key::Char('x') => self.reject_proposal(&i.uid),
                        _ => {}
                    }
                    return;
                }
                if k == Key::Char('n') {
                    // Start this block's (or deadline's) task now.
                    match i.meta.task_id.clone() {
                        Some(id) => {
                            let title = i.title.trim_start_matches('⚑').trim().to_string();
                            self.start_now(id, title);
                        }
                        None => self.set_message("n starts a task now: select one of its blocks or its ⚑", true),
                    }
                    return;
                }
                match k {
                    Key::Char('e') | Key::Char('K') => {
                        let mut f = Form::edit_event(self.key_names(), &i, self.tz);
                        if k == Key::Char('K') {
                            f.focus_on("Calendar");
                        }
                        self.message = None;
                        self.mode = Mode::Form(f);
                    }
                    Key::Char('m') => self.mode = Mode::Prompt(PromptKind::Move, String::new()),
                    Key::Char('d') => {
                        self.mode = Mode::Confirm(Confirm::Delete);
                        self.set_message(format!("delete “{}”? y/N", i.title), false);
                    }
                    Key::Char('B') if i.overlay == Overlay::Family => {
                        if self.daemon.is_none() {
                            return self.set_message("kairosd not configured — writes unavailable", true);
                        }
                        let now_blocking = !i.meta.blocking;
                        self.set_blocking_local(&i.uid, now_blocking);
                        self.set_message(
                            if now_blocking {
                                format!("“{}” now blocks your time (B again to undo)", i.title)
                            } else {
                                format!("“{}” no longer blocks your time", i.title)
                            },
                            false,
                        );
                        self.deferred.push(Box::new(move |app: &mut App| {
                            let req = api::UpdateEventReq {
                                blocking: Some(now_blocking),
                                ..Default::default()
                            };
                            let failed = match app.with_daemon(|d| d.update_event(&i.source, &i.uid, &req)) {
                                Some(Ok(Ok(_))) => None,
                                Some(Ok(Err(e))) => Some(e.to_string()),
                                Some(Err(e)) => Some(e.to_string()),
                                None => Some("kairosd not configured".into()),
                            };
                            match failed {
                                None => app.wants_sync = true,
                                Some(e) => {
                                    app.set_blocking_local(&i.uid, !now_blocking);
                                    app.set_message(e, true);
                                }
                            }
                        }));
                    }
                    Key::Char('s') => {
                        self.mode = Mode::Confirm(Confirm::Share);
                        self.set_message(format!("share “{}” to Family (moves it)? y/N", i.title), false);
                    }
                    _ => {}
                }
            }
        }
    }

    fn plan(&mut self) {
        if let Some(r) = self.with_daemon(|d| d.plan(false)) {
            match r {
                Ok(Ok(p)) => {
                    let warn = plan_warning(&p);
                    self.set_message(
                        format!(
                            "plan: +{} −{} ({} moved){}",
                            p.created.len(),
                            p.deleted.len(),
                            p.moved,
                            warn.clone().unwrap_or_default()
                        ),
                        warn.is_some(),
                    );
                    self.wants_sync = true;
                    self.refresh_remote();
                }
                Ok(Err(e)) => self.set_message(e.to_string(), true),
                Err(e) => self.set_message(e.to_string(), true),
            }
        }
    }

    fn shift(&mut self, days: i64) {
        self.cursor += Duration::days(days);
        self.sel = 0;
        self.expanded = false;
    }

    fn confirmed(&mut self, c: Confirm) {
        match c {
            Confirm::Delete => {
                if let Some(cl) = self.selected() {
                    let i = cl.primary;
                    if self.daemon.is_none() {
                        return self.set_message("kairosd not configured — writes unavailable", true);
                    }
                    self.patch_local(&i.uid, |_| false);
                    self.deferred.push(Box::new(move |app: &mut App| {
                        if let Some(r) = app.with_daemon(|d| d.delete_event(&i.source, &i.uid))
                            && !app.write_applied(r)
                        {
                            app.unpatch(Some(i.clone()), &i.uid);
                        }
                    }));
                }
            }
            Confirm::Share => {
                if let Some(cl) = self.selected() {
                    let i = cl.primary;
                    if let Some(r) = self.with_daemon(|d| d.share_family(&i.source, &i.uid)) {
                        self.write_result(r);
                    }
                }
            }
            Confirm::Undo(id) => {
                if let Some(r) = self.with_daemon(|d| d.decide(&id, "undo")) {
                    self.write_result(r);
                }
            }
            Confirm::DropTask(id) => {
                if self.daemon.is_none() {
                    return self.set_message("kairosd not configured — writes unavailable", true);
                }
                // Its blocks leave the grid at once; back they come if the
                // drop is refused or waits for approval.
                let now = self.now;
                let blocks: Vec<Instance> = self
                    .index
                    .instances
                    .iter()
                    .filter(|i| i.meta.task_id.as_deref() == Some(id.as_str()) && i.end(self.tz) > now)
                    .cloned()
                    .collect();
                for b in &blocks {
                    self.patch_local(&b.uid, |_| false);
                }
                self.deferred.push(Box::new(move |app: &mut App| {
                    let Some(r) = app.with_daemon(|d| d.drop_task(&id)) else {
                        return;
                    };
                    let applied = match r {
                        Ok(Ok(t)) => t
                            .changesets
                            .into_iter()
                            .next()
                            .is_some_and(|w| app.write_result(Ok(Ok(w)))),
                        Ok(Err(e)) => {
                            app.set_message(e.to_string(), true);
                            false
                        }
                        Err(e) => {
                            app.set_message(e.to_string(), true);
                            false
                        }
                    };
                    if !applied {
                        for b in blocks {
                            let uid = b.uid.clone();
                            app.unpatch(Some(b), &uid);
                        }
                    }
                }));
            }
        }
    }

    fn when_str(&self, s: &str) -> Result<String, String> {
        match dates::parse_when(s, self.today()).map_err(|e| e.to_string())? {
            When::Date(d) => Ok(d.to_string()),
            When::DateTime(n) => Ok(resolve_local(n, self.tz).to_rfc3339()),
        }
    }

    fn submit(&mut self, kind: PromptKind, buf: String) {
        let buf = buf.trim().to_string();
        if let PromptKind::Remaining { uid, title, outcome } = &kind {
            let left = if buf.is_empty() {
                None
            } else {
                match dates::parse_minutes(&buf) {
                    Ok(m) => Some(m),
                    Err(_) => {
                        self.set_message("how much is left: e.g. 3h, 90m, 0", true);
                        self.mode = Mode::Prompt(kind.clone(), buf);
                        return;
                    }
                }
            };
            return self.send_checkin(uid.clone(), &title.clone(), &outcome.clone(), left);
        }
        if let PromptKind::MoreTime { id, title } = &kind {
            let Ok(more) = dates::parse_minutes(&buf) else {
                self.set_message("more time: e.g. 2h, 90m", true);
                self.mode = Mode::Prompt(kind.clone(), buf);
                return;
            };
            let cur = self.tasks.iter().find(|t| &t.id == id).map_or(0, |t| t.remaining_min);
            let req = api::UpdateTaskReq {
                remaining_min: Some(cur + more),
                ..Default::default()
            };
            let title = title.clone();
            match self.with_daemon(|d| d.update_task(id, &req)) {
                Some(Ok(Ok(r))) => {
                    let warn = r.plan.as_ref().and_then(plan_warning).unwrap_or_default();
                    self.set_message(
                        format!(
                            "✓ {title}: +{} planned{warn}",
                            crate::form::fmt_minutes(i64::from(more))
                        ),
                        !warn.is_empty(),
                    );
                    self.wants_sync = true;
                    self.refresh_remote();
                }
                Some(Ok(Err(e))) => self.set_message(e.to_string(), true),
                Some(Err(e)) => self.set_message(e.to_string(), true),
                None => {}
            }
            return;
        }
        if let PromptKind::Project { id, title } = &kind {
            let title = if buf.is_empty() { title.clone() } else { buf };
            return self.accept_proposal(id.clone(), title);
        }
        if buf.is_empty() && kind != PromptKind::Search {
            return;
        }
        match kind {
            PromptKind::Remaining { .. } | PromptKind::Project { .. } | PromptKind::MoreTime { .. } => {}
            PromptKind::Search => {
                self.search = if buf.is_empty() { None } else { Some(buf) };
            }
            PromptKind::Move => {
                let Some(cl) = self.selected() else { return };
                let i = cl.primary;
                let start = match self.when_str(&buf) {
                    Ok(s) => s,
                    Err(e) => return self.set_message(e, true),
                };
                let req = api::UpdateEventReq {
                    start: Some(start.clone()),
                    ..Default::default()
                };
                if self.daemon.is_none() {
                    return self.set_message("kairosd not configured — writes unavailable", true);
                }
                let tz = self.tz;
                self.patch_local(&i.uid, |x| {
                    retime(x, &start, None, tz);
                    true
                });
                self.deferred.push(Box::new(move |app: &mut App| {
                    if let Some(r) =
                        app.with_daemon(|d| d.update_event(&i.source, &i.uid, &req).map(|r| r.map(to_write)))
                        && !app.write_applied(r)
                    {
                        app.unpatch(Some(i.clone()), &i.uid);
                    }
                }));
            }
        }
    }

    pub fn key_names(&self) -> Vec<String> {
        let mut keys = self.cfg.keys.clone();
        keys.sort_by_key(|k| k.index);
        keys.into_iter().map(|k| k.name).collect()
    }

    /// How the form's time fields were understood, or why they weren't.
    pub fn form_preview(&self, f: &Form) -> Result<String, String> {
        let key = f.get("Calendar");
        match f.kind {
            FormKind::AddTask => {
                let ty = f.get("Type");
                let est = match f.get("Estimate") {
                    "" => match ty {
                        "sheet" => 300,
                        "admin" => 45,
                        _ => return Err("Estimate: coursework needs one (e.g. 6h)".into()),
                    },
                    e => dates::parse_minutes(e).map_err(|_| "Estimate: e.g. 4h or 90m".to_string())?,
                };
                let due = match f.get("Due") {
                    "" => "no deadline".to_string(),
                    d => match dates::parse_when(d, self.today()).map_err(|e| e.to_string())? {
                        When::Date(d) => format!("due {}", d.format("%a %-d %b")),
                        When::DateTime(n) => format!("due {}", n.format("%a %-d %b %H:%M")),
                    },
                };
                Ok(format!(
                    "{} of {ty} · {due} · {key}",
                    crate::form::fmt_minutes(i64::from(est))
                ))
            }
            _ => {
                let when = f.get("When");
                if when.is_empty() {
                    return Err("When: e.g. tue 19:00".into());
                }
                let rep = match f.get("Repeat") {
                    "" | "no" => String::new(),
                    r => format!(" · {r}"),
                };
                match dates::parse_when(when, self.today()).map_err(|e| e.to_string())? {
                    When::Date(d) => Ok(format!("{} · all day{rep} · {key}", d.format("%a %-d %b"))),
                    When::DateTime(n) => {
                        let dur = match f.get("Duration") {
                            "" => 60,
                            d => dates::parse_minutes(d).map_err(|_| "Duration: e.g. 90m, 1h30, 2h".to_string())?,
                        };
                        let s = resolve_local(n, self.tz);
                        let e = s + Duration::minutes(i64::from(dur));
                        Ok(format!("{}{rep} · {key}", describe_span(s, e, self.tz)))
                    }
                }
            }
        }
    }

    fn submit_form(&mut self, f: Form) {
        if f.get("Title").is_empty() {
            self.set_message("Title is empty", true);
            self.mode = Mode::Form(f);
            return;
        }
        if let Err(e) = self.form_preview(&f) {
            self.set_message(e, true);
            self.mode = Mode::Form(f);
            return;
        }
        let opt = |s: &str| (!s.is_empty()).then(|| s.to_string());
        let dur = |s: &str| dates::parse_minutes(s).ok();
        match f.kind.clone() {
            FormKind::AddEvent => {
                let start = match self.when_str(f.get("When")) {
                    Ok(s) => s,
                    Err(e) => return self.set_message(e, true),
                };
                let req = api::CreateEventReq {
                    title: f.get("Title").to_string(),
                    start,
                    duration_min: dur(f.get("Duration")),
                    key: opt(f.get("Calendar")),
                    location: opt(f.get("Location")),
                    description: opt(f.get("Notes")),
                    repeat: crate::form::repeat_of(f.get("Repeat")),
                    ..Default::default()
                };
                if let Some(r) = self.with_daemon(|d| d.create_event(&req)) {
                    self.write_result(r);
                }
            }
            FormKind::EditEvent { source, uid } => {
                let time_changed = f.changed("When") || f.changed("Duration");
                let start = if time_changed {
                    match self.when_str(f.get("When")) {
                        Ok(s) => Some(s),
                        Err(e) => return self.set_message(e, true),
                    }
                } else {
                    None
                };
                let changed = |l: &str| f.changed(l).then(|| f.get(l).to_string());
                let req = api::UpdateEventReq {
                    title: changed("Title"),
                    start,
                    duration_min: if time_changed { dur(f.get("Duration")) } else { None },
                    location: changed("Location"),
                    description: changed("Notes"),
                    ..Default::default()
                };
                let rekey = f.changed("Calendar").then(|| f.get("Calendar").to_string());
                if req == api::UpdateEventReq::default() && rekey.is_none() {
                    return self.set_message("no changes", false);
                }
                if self.daemon.is_none() {
                    return self.set_message("kairosd not configured — writes unavailable", true);
                }
                // Show it now; the server round trip runs after the next frame.
                let orig = self.index.instances.iter().find(|i| i.uid == uid).cloned();
                let tz = self.tz;
                let target = rekey.as_deref().and_then(|k| self.source_for_key(k));
                {
                    let (req, rekey) = (req.clone(), rekey.clone());
                    self.patch_local(&uid, move |x| {
                        if let Some(t) = &req.title {
                            x.title = t.clone();
                        }
                        if let Some(l) = &req.location {
                            x.location = (!l.is_empty()).then(|| l.clone());
                        }
                        if let Some(d) = &req.description {
                            x.description = (!d.is_empty()).then(|| d.clone());
                        }
                        if let Some(st) = &req.start {
                            retime(x, st, req.duration_min, tz);
                        }
                        if let Some(k) = &rekey {
                            x.key = Some(k.clone());
                            if let Some(t) = &target {
                                x.source = t.clone();
                            }
                        }
                        true
                    });
                }
                self.set_message("saving…", false);
                self.deferred.push(Box::new(move |app: &mut App| {
                    let mut ok = true;
                    if req != api::UpdateEventReq::default()
                        && let Some(r) =
                            app.with_daemon(|d| d.update_event(&source, &uid, &req).map(|r| r.map(to_write)))
                    {
                        ok = app.write_applied(r);
                    }
                    if ok
                        && let Some(k) = rekey
                        && let Some(r) = app.with_daemon(|d| d.move_event(&source, &uid, &k))
                    {
                        ok = app.write_applied(r);
                    }
                    if !ok {
                        app.unpatch(orig, &uid);
                    }
                }));
            }
            FormKind::AddTask => {
                let due = match opt(f.get("Due")).map(|d| self.when_str(&d)).transpose() {
                    Ok(d) => d,
                    Err(e) => return self.set_message(e, true),
                };
                let req = api::CreateTaskReq {
                    title: f.get("Title").to_string(),
                    key: f.get("Calendar").to_string(),
                    estimate_min: dur(f.get("Estimate")).unwrap_or(0),
                    kind: Some(f.get("Type").to_string()).filter(|k| !k.is_empty()),
                    due,
                    ..Default::default()
                };
                if let Some(r) = self.with_daemon(|d| d.create_task(&req)) {
                    match r {
                        Ok(Ok(t)) => {
                            let short = t.plan.as_ref().and_then(plan_warning);
                            self.set_message(
                                format!("✓ task {}{}", t.task.title, short.clone().unwrap_or_default()),
                                short.is_some(),
                            );
                            self.wants_sync = true;
                            self.refresh_remote();
                        }
                        Ok(Err(e)) => self.set_message(e.to_string(), true),
                        Err(e) => self.set_message(e.to_string(), true),
                    }
                }
            }
        }
    }

    fn checkin_key(&mut self, i: usize, k: Key) {
        let open: Vec<api::CheckinItem> = self.checkins.iter().filter(|c| c.outcome.is_none()).cloned().collect();
        let Some(item) = open.get(i) else { return };
        let outcome = match k {
            Key::Char('d') => "done",
            Key::Char('p') => "partial",
            Key::Char('m') => "missed",
            Key::Char('s') | Key::Char('j') => {
                if i + 1 < open.len() {
                    self.mode = Mode::Checkin(i + 1);
                }
                return;
            }
            Key::Esc | Key::Char('q') => return,
            _ => {
                self.mode = Mode::Checkin(i);
                return;
            }
        };
        if item.ask_remaining && outcome != "missed" {
            // Coursework: its estimate moves, so ask what's left.
            let guess = if outcome == "done" {
                item.remaining_min.saturating_sub(item.planned_min)
            } else {
                item.remaining_min.saturating_sub(item.planned_min / 2)
            };
            self.mode = Mode::Prompt(
                PromptKind::Remaining {
                    uid: item.block_uid.clone(),
                    title: item.title.clone(),
                    outcome: outcome.into(),
                },
                crate::form::fmt_minutes(i64::from(guess)),
            );
            return;
        }
        let (uid, title) = (item.block_uid.clone(), item.title.clone());
        self.send_checkin(uid, &title, outcome, None);
    }

    /// Start a task now: a block from now, the rest of the plan reflows.
    fn start_now(&mut self, id: String, title: String) {
        if self.daemon.is_none() {
            return self.set_message("kairosd not configured — writes unavailable", true);
        }
        self.set_message(format!("▶ starting {title}…"), false);
        self.deferred.push(Box::new(move |app: &mut App| {
            match app.with_daemon(|d| d.start_task(&id, None)) {
                Some(Ok(Ok(r))) => {
                    let warn = r.plan.as_ref().and_then(plan_warning).unwrap_or_default();
                    // The summary reads "Add "…" Mon 12 Oct 10:05–11:30 to …".
                    let until = r
                        .changesets
                        .last()
                        .and_then(|c| c.summary.split('–').nth(1).and_then(|x| x.split_whitespace().next()))
                        .unwrap_or("")
                        .to_string();
                    app.set_message(format!("▶ {title} now, until {until}{warn}"), !warn.is_empty());
                    app.wants_sync = true;
                    app.refresh_remote();
                }
                Some(Ok(Err(e))) => app.set_message(e.to_string(), true),
                Some(Err(e)) => app.set_message(e.to_string(), true),
                None => {}
            }
        }));
    }

    fn accept_proposal(&mut self, id: String, title: String) {
        if self.daemon.is_none() {
            return self.set_message("kairosd not configured — writes unavailable", true);
        }
        let Some(p) = self.proposals.iter().find(|p| p.id == id).cloned() else {
            return;
        };
        self.proposals.retain(|x| x.id != id);
        // Stand-in until the real event syncs back.
        let key = self.cfg.default_key().map(|k| k.name.clone());
        let source = key.as_deref().and_then(|k| self.source_for_key(k)).unwrap_or_default();
        self.patches.push(LocalPatch {
            uid: format!("accepted:{id}"),
            inst: Some(Instance {
                uid: format!("accepted:{id}"),
                recurrence_id: None,
                source,
                overlay: Overlay::Personal,
                key,
                title: title.clone(),
                location: None,
                description: None,
                span: Span::Timed {
                    start: p.start,
                    end: p.end,
                },
                recurring: false,
                transparent: false,
                tentative: false,
                meta: Meta::default(),
                href: None,
                alarms: Vec::new(),
            }),
            until: Utc::now() + Duration::seconds(120),
        });
        let idx = self.index.clone();
        self.set_index(idx);
        let tz = self.tz;
        self.set_message(
            format!(
                "✓ {title} {}–{}",
                p.start.with_timezone(&tz).format("%a %H:%M"),
                p.end.with_timezone(&tz).format("%H:%M")
            ),
            false,
        );
        self.deferred.push(Box::new(move |app: &mut App| {
            let failed = match app.with_daemon(|d| d.accept_proposal(&id, &title)) {
                Some(Ok(Ok(_))) => None,
                Some(Ok(Err(e))) => Some(e.to_string()),
                Some(Err(e)) => Some(e.to_string()),
                None => Some("kairosd not configured".into()),
            };
            match failed {
                None => app.wants_sync = true,
                Some(e) => {
                    app.patches.retain(|x| x.uid != format!("accepted:{id}"));
                    app.proposals.push(p);
                    app.proposals.sort_by_key(|x| x.start);
                    let idx = app.index.clone();
                    app.set_index(idx);
                    app.set_message(e, true);
                }
            }
        }));
    }

    fn reject_proposal(&mut self, id: &str) {
        if self.daemon.is_none() {
            return self.set_message("kairosd not configured — writes unavailable", true);
        }
        let Some(p) = self.proposals.iter().find(|p| p.id == id).cloned() else {
            return;
        };
        self.proposals.retain(|x| x.id != id);
        self.sel = self.sel.saturating_sub(1);
        self.set_message("dropped proposed project time", false);
        let id = id.to_string();
        self.deferred.push(Box::new(move |app: &mut App| {
            let failed = match app.with_daemon(|d| d.reject_proposal(&id)) {
                Some(Ok(Ok(_))) => None,
                Some(Ok(Err(e))) => Some(e.to_string()),
                Some(Err(e)) => Some(e.to_string()),
                None => Some("kairosd not configured".into()),
            };
            if let Some(e) = failed {
                app.proposals.push(p);
                app.proposals.sort_by_key(|x| x.start);
                app.set_message(e, true);
            }
        }));
    }

    fn send_checkin(&mut self, uid: String, title: &str, outcome: &str, remaining: Option<u32>) {
        let req = api::CheckinReq {
            block_uid: uid,
            outcome: outcome.into(),
            actual_min: None,
            remaining_min: remaining,
        };
        if let Some(r) = self.with_daemon(|d| d.checkin(&req)) {
            match r {
                Ok(Ok(p)) => {
                    let left = remaining.map(|m| format!(" · {} left", crate::form::fmt_minutes(i64::from(m))));
                    let warn = plan_warning(&p).unwrap_or_default();
                    self.set_message(
                        format!("{title}: {outcome}{}{warn}", left.unwrap_or_default()),
                        !warn.is_empty(),
                    );
                    self.wants_sync = true;
                }
                Ok(Err(e)) => self.set_message(e.to_string(), true),
                Err(e) => self.set_message(e.to_string(), true),
            }
        }
        self.refresh_remote();
        if self.checkins.iter().any(|c| c.outcome.is_none()) {
            self.mode = Mode::Checkin(0);
        }
    }
}

/// Apply a new start (RFC 3339 or a date) and optional duration to a local copy.
fn retime(x: &mut Instance, start: &str, dur: Option<u32>, tz: Tz) {
    let (s0, e0) = x.span.bounds(tz);
    let len = dur.map_or(e0 - s0, |m| Duration::minutes(i64::from(m)));
    if let Ok(t) = DateTime::parse_from_rfc3339(start) {
        let s = t.with_timezone(&Utc);
        x.span = Span::Timed { start: s, end: s + len };
    } else if let Ok(d) = NaiveDate::parse_from_str(start, "%Y-%m-%d") {
        x.span = Span::AllDay {
            start: d,
            end: d + Duration::days(1),
        };
    }
}

fn to_write(v: serde_json::Value) -> WriteResult {
    serde_json::from_value(v.clone()).unwrap_or(WriteResult {
        changeset_id: String::new(),
        risk: kairos_core::changeset::Risk::Low,
        status: Status::Applied,
        summary: v["summary"].as_str().unwrap_or("updated").to_string(),
        reasons: vec![],
    })
}

/// `Squash @ tue 19:00 #Sport for 90m` → (title, when, key, minutes)
pub fn parse_add(s: &str) -> Option<(String, String, Option<String>, Option<u32>)> {
    let (title, rest) = s.split_once('@')?;
    let mut when = Vec::new();
    let mut key = None;
    let mut dur = None;
    let mut it = rest.split_whitespace().peekable();
    while let Some(w) = it.next() {
        if let Some(k) = w.strip_prefix('#') {
            key = Some(k.to_string());
        } else if w == "for" {
            dur = it.next().and_then(|d| dates::parse_minutes(d).ok());
        } else {
            when.push(w);
        }
    }
    let title = title.trim();
    (!title.is_empty() && !when.is_empty()).then(|| (title.to_string(), when.join(" "), key, dur))
}

/// `B4 sheet #Engineering ~4h due fri 17:00` → (title, key, minutes, due)
pub fn parse_task(s: &str) -> Option<(String, String, u32, Option<String>)> {
    let (head, due) = match s.split_once(" due ") {
        Some((h, d)) => (h, Some(d.trim().to_string())),
        None => (s, None),
    };
    let mut title = Vec::new();
    let mut key = None;
    let mut est = None;
    for w in head.split_whitespace() {
        if let Some(k) = w.strip_prefix('#') {
            key = Some(k.to_string());
        } else if let Some(e) = w.strip_prefix('~') {
            est = dates::parse_minutes(e).ok();
        } else {
            title.push(w);
        }
    }
    Some((title.join(" "), key?, est?, due)).filter(|t| !t.0.is_empty())
}

/// Terminal-independent key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Esc,
    Tab,
    BackTab,
    Backspace,
    Up,
    Down,
    Left,
    Right,
}

/// Status-bar legend: (toggle key, name, shown?, colour, group).
pub fn overlay_flags(app: &App) -> Vec<(String, String, bool, kairos_core::colour::Rgb, u8)> {
    let name_of = |o: Overlay, fallback: &str| {
        let names: Vec<String> = app
            .cfg
            .sources
            .iter()
            .filter(|s| s.overlay == o)
            .map(|s| s.name.clone().unwrap_or_else(|| s.id.clone()))
            .collect();
        if names.is_empty() {
            fallback.to_string()
        } else {
            names.join("+")
        }
    };
    // Alec's own calendars form group 0, "Private" (drawn as "p Private ▸
    // 1 Personal …"; not "Personal", which is also key 1's name), Family
    // group 1, read-only feeds group 2.
    let mut v = vec![(
        "p".to_string(),
        "Private".to_string(),
        app.filter.personal,
        kairos_core::colour::Rgb::mocha("lavender"),
        0,
    )];
    let mut keys = app.cfg.keys.clone();
    keys.sort_by_key(|k| k.index);
    for k in keys {
        v.push((
            k.index.to_string(),
            k.name.clone(),
            app.filter.personal && !app.filter.hidden_keys.contains(&k.name.to_lowercase()),
            k.colour,
            0,
        ));
    }
    v.push((
        "f".to_string(),
        "Family".to_string(),
        app.filter.family,
        app.cfg.overlays.family_colour,
        1,
    ));
    if app.cfg.sources.iter().any(|s| s.overlay == Overlay::Background) {
        v.push((
            "b".to_string(),
            name_of(Overlay::Background, "holidays"),
            app.filter.background,
            kairos_core::colour::Rgb::mocha("peach"),
            2,
        ));
    }
    if app.cfg.sources.iter().any(|s| s.overlay == Overlay::Imported) {
        v.push((
            "i".to_string(),
            name_of(Overlay::Imported, "imported"),
            app.filter.imported,
            app.cfg.overlays.imported_colour,
            2,
        ));
    }
    v
}

pub fn is_dimmed(i: &Instance) -> bool {
    matches!(i.overlay, Overlay::Imported | Overlay::Background) || i.tentative || i.meta.kind == EventKind::Proposal
}

/// The most important thing a plan has to say: a shortfall, else a squeeze.
fn plan_warning(p: &api::PlanResp) -> Option<String> {
    p.shortfalls
        .first()
        .map(|s| format!(" · ! {}", s.message))
        .or_else(|| p.squeezed.first().map(|s| format!(" · ⚠ {}", s.message)))
}

pub fn is_deadline(i: &Instance) -> bool {
    i.meta.kind == EventKind::Deadline
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_syntax() {
        assert_eq!(
            parse_add("Squash @ tue 19:00 #Sport for 90m"),
            Some(("Squash".into(), "tue 19:00".into(), Some("Sport".into()), Some(90)))
        );
        assert_eq!(
            parse_add("Coffee @ tomorrow 9"),
            Some(("Coffee".into(), "tomorrow 9".into(), None, None))
        );
        assert_eq!(parse_add("no at sign"), None);
    }

    #[test]
    fn task_syntax() {
        assert_eq!(
            parse_task("B4 problem sheet #Engineering ~4h due fri 17:00"),
            Some((
                "B4 problem sheet".into(),
                "Engineering".into(),
                240,
                Some("fri 17:00".into())
            ))
        );
        assert_eq!(
            parse_task("Essay #Careers ~90m"),
            Some(("Essay".into(), "Careers".into(), 90, None))
        );
        assert_eq!(parse_task("Essay ~90m"), None, "key required");
    }
}
