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
    Month,
    Agenda,
    Tasks,
    Approvals,
    Projects,
}

impl View {
    pub fn name(self) -> &'static str {
        match self {
            View::Week => "week",
            View::Day => "day",
            View::Month => "month",
            View::Agenda => "agenda",
            View::Tasks => "tasks",
            View::Approvals => "approvals",
            View::Projects => "projects",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptKind {
    Move,
    Search,
    /// Check-in: how long did you work on it (partly done)?
    Partial {
        t: CheckinTarget,
    },
    /// Check-in (coursework, or a block that uses up the task): how much of
    /// the task is left?
    Remaining {
        t: CheckinTarget,
        outcome: String,
        actual: Option<u32>,
    },
    /// Log work done outside a block: how long (ending now)?
    LogWork {
        task_id: String,
        title: String,
    },
    /// Accepting proposed project time for a new project: its name.
    NewProject {
        proposal: String,
    },
    /// More time for a task (its planned time is used up, or it grew).
    MoreTime {
        id: String,
        title: String,
    },
    /// Move a task's deadline (from its ⚑).
    MoveDeadline {
        task_id: String,
    },
    /// Reopen a finished task: how much is left?
    Reopen {
        id: String,
        title: String,
    },
}

impl PromptKind {
    pub fn label(&self) -> &'static str {
        match self {
            PromptKind::Move => "move to (when)",
            PromptKind::Search => "/",
            PromptKind::Partial { .. } => "partly done — how long did you work? (e.g. 45m)",
            PromptKind::Remaining { .. } => "how much is left? (e.g. 3h, 0 = finished)",
            PromptKind::LogWork { .. } => "log work — how long, ending now? (e.g. 40m)",
            PromptKind::NewProject { .. } => "new project — name (this time goes to it)",
            PromptKind::MoreTime { .. } => "how much more time? (e.g. 2h)",
            PromptKind::MoveDeadline { .. } => "deadline (when, e.g. fri 17:00)",
            PromptKind::Reopen { .. } => "reopen — how much is left? (e.g. 2h)",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirm {
    Delete,
    Share,
    /// Undo a change (and the scheduler's replans after it).
    Undo(String),
    DropTask(String),
    /// Skip the selected work block (work planned elsewhere).
    Skip,
    /// Remove a task's deadline (the task stays, with no deadline).
    ClearDue(String),
    /// Delete a project (its sessions stay on the calendar).
    DeleteProject(String),
}

/// A choice when accepting proposed project time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickOption {
    Project {
        id: String,
        name: String,
    },
    New,
    /// No project: just the proposal's generic title.
    Plain,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Prompt(PromptKind, String),
    Form(Form),
    Confirm(Confirm),
    Help,
    Checkin(usize),
    /// `a` in the calendar: an event or a task?
    AddChoice,
    /// Accepting proposed project time: which project?
    PickProject {
        proposal: String,
        sel: usize,
    },
}

/// A block being checked in (from the C list or the calendar).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckinTarget {
    pub uid: String,
    pub title: String,
    pub planned_min: u32,
    pub ask_remaining: bool,
    pub remaining_min: u32,
    /// Opened from the C list: go back to it afterwards.
    pub from_list: bool,
    /// A project session (no "what's left"; F finishes the project).
    pub project: bool,
}

impl CheckinTarget {
    fn of(item: &api::CheckinItem, from_list: bool) -> CheckinTarget {
        CheckinTarget {
            uid: item.block_uid.clone(),
            title: item.title.clone(),
            planned_min: item.planned_min,
            ask_remaining: item.ask_remaining,
            remaining_min: item.remaining_min,
            from_list,
            project: item.project,
        }
    }
}

/// Finished tasks listed (and selectable) in the tasks view.
pub const DONE_SHOWN: usize = 12;

/// A pending change drawn as a ghost block.
#[derive(Debug, Clone, PartialEq)]
pub struct Ghost {
    pub changeset_id: String,
    pub title: String,
    pub span: Span,
}

/// A change shown before kairosd has answered (optimistic). Re-applied over
/// every reload until an index fetched from kairosd after it was made
/// arrives (that one shows the real result), or it expires (offline).
#[derive(Debug, Clone)]
pub struct LocalPatch {
    pub uid: String,
    /// `None` = deleted.
    pub inst: Option<Instance>,
    pub made: DateTime<Utc>,
    pub until: DateTime<Utc>,
}

/// Work to run right after the next frame is drawn (server round trips), so
/// the screen shows the change before the network answers.
pub type Deferred = Box<dyn FnOnce(&mut App)>;

pub struct App {
    pub cfg: Config,
    pub patches: Vec<LocalPatch>,
    /// (uid, blocking, made, until): Family "block my time" flags set from the TUI.
    pub blocking_overrides: Vec<(String, bool, DateTime<Utc>, DateTime<Utc>)>,
    /// (uid, not going, made, until): "not going" set from the TUI on your own events.
    pub going_overrides: Vec<(String, bool, DateTime<Utc>, DateTime<Utc>)>,
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
    /// Projects (active, next, paused, done) with time spent.
    pub projects: Vec<api::ProjectItem>,
    /// Finished tasks, most recent first (the tasks view's Completed section).
    pub done_tasks: Vec<Task>,
    /// Squeezed work blocks (from a dry-run plan): (task id, start, reasons).
    pub squeezed: Vec<(String, DateTime<Utc>, Vec<String>)>,
    pub daemon: Option<Daemon>,
    pub daemon_error: Option<String>,
    pub quit: bool,
    /// Set after a write so the runner fetches kairosd's index at once.
    pub wants_sync: bool,
    /// Week view shows Mon–Fri only (wider columns).
    pub work_week: bool,
    /// The week/day grid's first shown minute when you scrolled it (↑/↓ past the first or last
    /// item); `None` = placed by itself around the selection or now.
    pub day_top: std::cell::Cell<Option<i64>>,
    /// What the last drawn grid showed: (first minute, end minute, the day's extent from, to).
    pub grid: std::cell::Cell<(i64, i64, i64, i64)>,
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
            projects: Vec::new(),
            done_tasks: Vec::new(),
            squeezed: Vec::new(),
            daemon,
            daemon_error: None,
            quit: false,
            wants_sync: false,
            work_week: false,
            day_top: std::cell::Cell::new(None),
            grid: std::cell::Cell::new((0, 24 * 60, 0, 24 * 60)),
            patches: Vec::new(),
            blocking_overrides: Vec::new(),
            going_overrides: Vec::new(),
            deferred: Vec::new(),
            cfg,
        }
    }

    pub fn today(&self) -> NaiveDate {
        self.now.with_timezone(&self.tz).date_naive()
    }

    /// Visible clusters in `[from, to)`, honouring filter and search.
    pub fn clusters(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Vec<Cluster> {
        let mut inst = self.index.between(from, to, self.tz);
        // Due dates are the tasks' own (spec K5): drawn as ⚑ here, never stored as events. A
        // task whose old ⚑ event is still on the calendar shows that one instead.
        let marks = kairos_core::task::due_marks(&self.tasks, from, to, self.tz)
            .into_iter()
            .filter(|m| {
                let legacy = self
                    .tasks
                    .iter()
                    .find(|t| m.meta.task_id.as_deref() == Some(t.id.as_str()))
                    .and_then(|t| t.deadline_uid.as_deref());
                !legacy.is_some_and(|u| inst.iter().any(|i| i.uid == u)) && !inst.iter().any(|i| i.uid == m.uid)
            })
            .collect::<Vec<_>>();
        inst.extend(marks);
        // Worked time is drawn as bars of its own (`worked_on`), never as events.
        inst.retain(|i| i.meta.kind != EventKind::Worked);
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

    /// Worked-time spans on day `d` (spec K6), from kairosd's index.
    pub fn worked_on(&self, d: NaiveDate) -> Vec<Instance> {
        let (from, to) = (midnight(d, self.tz), midnight(d + Duration::days(1), self.tz));
        self.index
            .between(from, to, self.tz)
            .into_iter()
            .filter(|i| i.meta.kind == EventKind::Worked && (self.filter.personal || self.filter.family))
            .collect()
    }

    pub fn day_clusters(&self, d: NaiveDate) -> Vec<Cluster> {
        let (from, to) = (midnight(d, self.tz), midnight(d + Duration::days(1), self.tz));
        let mut v: Vec<Cluster> = self
            .clusters(from, to)
            .into_iter()
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
            View::Week | View::Day | View::Month => self.day_clusters(self.cursor).into_iter().nth(self.sel),
            _ => None,
        }
    }

    fn list_len(&self) -> usize {
        match self.view {
            View::Agenda => self.agenda_rows().len(),
            View::Week | View::Day | View::Month => self.day_clusters(self.cursor).len(),
            View::Tasks => self.tasks.len() + self.done_tasks.len().min(DONE_SHOWN),
            View::Approvals => self.pending.len(),
            View::Projects => self.projects.len(),
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
        if let Ok(Ok(mut t)) = d.tasks("done") {
            t.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            self.done_tasks = t;
        }
        if let Ok(Ok(c)) = d.checkin_items() {
            self.checkins = c;
        }
        if let Ok(Ok(p)) = d.proposals() {
            self.proposals = p;
        }
        if let Ok(Ok(p)) = d.projects() {
            self.projects = p;
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

    /// An index fetched from kairosd, requested at `requested`: it shows
    /// every change finished before then, so local patches made earlier go.
    pub fn set_server_index(&mut self, idx: Index, requested: DateTime<Utc>) {
        self.patches.retain(|p| p.made >= requested);
        self.blocking_overrides.retain(|o| o.2 >= requested);
        self.going_overrides.retain(|o| o.2 >= requested);
        self.set_index(idx);
    }

    /// Patches or overrides that an index requested at `requested` replaces.
    pub fn has_patches_before(&self, requested: DateTime<Utc>) -> bool {
        self.patches.iter().any(|p| p.made < requested)
            || self.blocking_overrides.iter().any(|o| o.2 < requested)
            || self.going_overrides.iter().any(|o| o.2 < requested)
    }

    /// Replace the index (after a reload) and re-apply recent local patches.
    pub fn set_index(&mut self, idx: Index) {
        self.index = idx;
        let now = Utc::now();
        self.blocking_overrides.retain(|o| o.3 > now);
        for (uid, on, _, _) in &self.blocking_overrides {
            for i in self.index.instances.iter_mut().filter(|i| &i.uid == uid) {
                i.meta.blocking = *on;
            }
        }
        self.going_overrides.retain(|o| o.3 > now);
        for (uid, off, _, _) in &self.going_overrides {
            for i in self.index.instances.iter_mut().filter(|i| &i.uid == uid) {
                i.meta.not_going = *off;
            }
        }
        self.patches.retain(|p| p.until > now);
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
        self.blocking_overrides.retain(|o| o.0 != uid);
        let now = Utc::now();
        self.blocking_overrides
            .push((uid.to_string(), on, now, now + Duration::seconds(90)));
        for i in self.index.instances.iter_mut().filter(|i| i.uid == uid) {
            i.meta.blocking = on;
        }
    }

    /// Show "not going" on one of your events at once, kept over reloads until kairosd agrees.
    fn set_not_going_local(&mut self, uid: &str, off: bool) {
        self.going_overrides.retain(|o| o.0 != uid);
        let now = Utc::now();
        self.going_overrides
            .push((uid.to_string(), off, now, now + Duration::seconds(90)));
        for i in self.index.instances.iter_mut().filter(|i| i.uid == uid) {
            i.meta.not_going = off;
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
            made: Utc::now(),
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
            Mode::PickProject { proposal, sel } => self.pick_project_key(proposal, sel, k),
            Mode::AddChoice => match k {
                Key::Char('e') => self.open_add(false),
                Key::Char('t') => self.open_add(true),
                Key::Char('p') => self.open_add_session(None),
                _ => self.message = None,
            },
            Mode::Normal => self.normal(k),
        }
    }

    fn normal(&mut self, k: Key) {
        let n = self.list_len();
        match k {
            Key::Char('q') | Key::Esc if self.expanded => self.expanded = false,
            Key::Char('q') => self.quit = true,
            Key::Esc => {
                if matches!(self.view, View::Tasks | View::Approvals | View::Projects) {
                    self.view = View::Week;
                    self.sel = 0;
                }
                self.search = None;
            }
            Key::Char('?') => self.mode = Mode::Help,
            // Arrows and Tab move, as everywhere in locus (spec K6): ←/→ days, Shift-←/→ weeks.
            Key::Left => self.shift(-1),
            Key::Right => self.shift(1),
            Key::ShiftLeft => self.shift(-7),
            Key::ShiftRight => self.shift(7),
            Key::Char('t') => {
                self.cursor = self.today();
                self.sel = 0;
            }
            // ↑/↓ walk the items; past the first or last they scroll the day's hours.
            Key::Down => {
                if n > 0 && self.sel + 1 < n {
                    self.sel += 1;
                    self.day_top.set(None);
                } else {
                    self.scroll_day(1);
                }
            }
            Key::Up => {
                if self.sel > 0 {
                    self.sel -= 1;
                    self.day_top.set(None);
                } else {
                    self.scroll_day(-1);
                }
            }
            Key::Tab | Key::BackTab => {
                // week → day → month → agenda → week (Shift-Tab back)
                const ORDER: [View; 4] = [View::Week, View::Day, View::Month, View::Agenda];
                let at = ORDER.iter().position(|v| *v == self.view).unwrap_or(0);
                let step = if k == Key::Tab { 1 } else { ORDER.len() - 1 };
                self.view = ORDER[(at + step) % ORDER.len()];
                self.sel = 0;
                self.day_top.set(None);
            }
            Key::Char('g') => {
                self.view = if self.view == View::Agenda {
                    View::Week
                } else {
                    View::Agenda
                };
                self.sel = 0;
            }
            Key::Enter => self.expanded = !self.expanded,
            Key::Char('p') => self.filter.personal = !self.filter.personal,
            Key::Char('f') => self.filter.family = !self.filter.family,
            Key::Char('b') => self.filter.background = !self.filter.background,
            Key::Char('i') => self.filter.imported = !self.filter.imported,
            Key::Char(c @ '1'..='9') => {
                let idx = c as u8 - b'0';
                if let Some(k) = self.cfg.keys.iter().find(|k| k.index == idx) {
                    let name = k.name.clone();
                    self.filter.toggle_key(&name);
                } else if idx == self.cfg.tui.personal_index {
                    self.filter.toggle_area(&self.cfg, "Personal");
                } else if idx == self.cfg.tui.family_index {
                    self.filter.family = !self.filter.family;
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
            Key::Char('P') => {
                self.view = View::Projects;
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
                if self.view == View::Projects {
                    self.message = None;
                    let k = self.projects_key();
                    self.mode = Mode::Form(Form::add_project(self.key_names(), &k));
                } else if self.view == View::Tasks {
                    self.open_add(true);
                } else {
                    self.mode = Mode::AddChoice;
                    self.set_message("add: e event · t task · p project time (Esc cancels)", false);
                }
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
                // Your (or Claude's) last change — not the scheduler's replan
                // that followed it; undoing it undoes that replan too.
                if let Some(d) = &self.daemon
                    && let Ok(Ok(v)) = d.changesets(Some("applied"), 50)
                    && let Some(c) = v.iter().find(|c| c.author != kairos_core::model::Author::Scheduler)
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

    /// Scroll the week/day grid an hour (spec K6: every hour the same height, more hours rather
    /// than taller ones, so a long day scrolls instead of squashing).
    fn scroll_day(&mut self, dir: i64) {
        if !matches!(self.view, View::Week | View::Day) {
            return;
        }
        let (h0, h1, e0, e1) = self.grid.get();
        let top = (h0 + dir * 60).clamp(e0, (e1 - (h1 - h0)).max(e0));
        if top != h0 {
            self.day_top.set(Some(top));
        }
    }

    /// Keys whose meaning depends on the view / selection.
    fn contextual(&mut self, k: Key) {
        match self.view {
            View::Projects => {
                let Some(p) = self.projects.get(self.sel).map(|p| p.project.clone()) else {
                    return;
                };
                match k {
                    Key::Char('n') => self.open_add_session(Some(p.name.clone())),
                    Key::Char('e') => {
                        self.message = None;
                        let k = self.projects_key();
                        self.mode = Mode::Form(Form::edit_project(self.key_names(), &k, &p));
                    }
                    Key::Char('s') => {
                        let next = p.status.cycle();
                        let req = api::UpdateProjectReq {
                            status: Some(next.as_str().into()),
                            ..Default::default()
                        };
                        self.update_project_and_report(&p.id, &req);
                        // Keep the same project selected after re-sorting.
                        if let Some(k) = self.projects.iter().position(|x| x.project.id == p.id) {
                            self.sel = k;
                        }
                    }
                    Key::Char('d') => {
                        self.mode = Mode::Confirm(Confirm::DeleteProject(p.id.clone()));
                        self.set_message(
                            format!("delete project “{}”? (its sessions stay on the calendar) y/N", p.name),
                            false,
                        );
                    }
                    _ => {}
                }
            }
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
                // Finished tasks (listed after the open ones): o reopens.
                if self.sel >= self.tasks.len() {
                    if let Some(t) = self.done_tasks.get(self.sel - self.tasks.len()).cloned()
                        && k == Key::Char('o')
                    {
                        self.message = None;
                        self.mode = Mode::Prompt(
                            PromptKind::Reopen {
                                id: t.id,
                                title: t.title,
                            },
                            String::new(),
                        );
                    }
                    return;
                }
                let Some(t) = self.tasks.get(self.sel).cloned() else {
                    return;
                };
                match k {
                    Key::Char('D') => {
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
                    Key::Char('W') => {
                        self.message = None;
                        self.mode = Mode::Prompt(
                            PromptKind::LogWork {
                                task_id: t.id.clone(),
                                title: t.title.clone(),
                            },
                            String::new(),
                        );
                    }
                    Key::Char('e') => {
                        self.message = None;
                        let f = self.task_form_projects(Form::edit_task(self.key_names(), &t, self.tz), &t);
                        self.mode = Mode::Form(f);
                    }
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
                    Key::Char('d') => {
                        self.mode = Mode::Confirm(Confirm::DropTask(t.id.clone()));
                        self.set_message(format!("drop “{}”? y/N", t.title), false);
                    }
                    Key::Char('S') => self.plan(),
                    _ => {}
                }
            }
            View::Week | View::Day | View::Month | View::Agenda => {
                if k == Key::Char('S') {
                    self.plan();
                    return;
                }
                let Some(c) = self.selected() else { return };
                let i = c.primary.clone();
                if i.meta.kind == EventKind::Proposal {
                    // Proposed project time: accept (then say what to work on) or drop.
                    match k {
                        Key::Char('y') => {
                            self.message = None;
                            self.mode = Mode::PickProject {
                                proposal: i.uid.clone(),
                                sel: 0,
                            };
                        }
                        Key::Char('d') => self.reject_proposal(&i.uid),
                        _ => {}
                    }
                    return;
                }
                // A work block that has started and isn't checked in: check it
                // in right here (y done · c partly · + took longer · x/d missed · F finished).
                if self.awaits_checkin(&i) && matches!(k, Key::Char('y' | 'c' | 'x' | 'd' | 'F' | '+')) {
                    let t = self.checkin_target(&i);
                    if k == Key::Char('+') {
                        self.message = None;
                        self.mode = Mode::Form(Form::took_longer(t));
                        return;
                    }
                    let outcome = match k {
                        Key::Char('y') => "done",
                        Key::Char('c') => "partial",
                        Key::Char('F') => "finished",
                        _ => "missed",
                    };
                    return self.check_in(t, outcome);
                }
                // W: log work on this block's (or ⚑'s) task, ending now.
                if k == Key::Char('W') {
                    if let Some(id) = i.meta.task_id.clone() {
                        let title =
                            kairos_core::build::bare_block_title(i.title.trim_start_matches('⚑').trim()).to_string();
                        self.message = None;
                        self.mode = Mode::Prompt(PromptKind::LogWork { task_id: id, title }, String::new());
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
                // A task's ⚑ edits the task's deadline, not just the event.
                if i.meta.kind == EventKind::Deadline
                    && let Some(tid) = i.meta.task_id.clone()
                {
                    let title = i.title.trim_start_matches('⚑').trim().to_string();
                    match k {
                        Key::Char('e') | Key::Char('K') => match self.tasks.iter().find(|t| t.id == tid).cloned() {
                            Some(t) => {
                                let mut f = self.task_form_projects(Form::edit_task(self.key_names(), &t, self.tz), &t);
                                f.focus_on(if k == Key::Char('K') { "Calendar" } else { "Due" });
                                self.message = None;
                                self.mode = Mode::Form(f);
                            }
                            None => self.set_message(format!("“{title}” is finished — reopen it in T first"), true),
                        },
                        Key::Char('m') => {
                            self.message = None;
                            self.mode = Mode::Prompt(PromptKind::MoveDeadline { task_id: tid }, String::new());
                        }
                        // Removing an open task's deadline removes the task and
                        // all its planned time (Alec, 2026-10-13); a finished
                        // task just loses its ⚑.
                        Key::Char('d') if self.tasks.iter().any(|t| t.id == tid) => {
                            self.mode = Mode::Confirm(Confirm::DropTask(tid));
                            self.set_message(
                                format!("remove “{title}”: its deadline and all its planned time? y/N"),
                                false,
                            );
                        }
                        Key::Char('d') => {
                            self.mode = Mode::Confirm(Confirm::ClearDue(tid));
                            self.set_message(format!("remove the deadline of finished “{title}”? y/N"), false);
                        }
                        _ => {}
                    }
                    return;
                }
                // `d` on a work block skips it: the work is planned elsewhere.
                if i.meta.kind == EventKind::TaskBlock && k == Key::Char('d') {
                    self.mode = Mode::Confirm(Confirm::Skip);
                    self.set_message(
                        format!("skip this “{}” block? (the work is planned elsewhere) y/N", i.title),
                        false,
                    );
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
                    // Not going to one of your own events (spec K6): greyed, and it stops
                    // blocking the scheduler, as a Family event does until you opt in.
                    Key::Char('B') if i.overlay == Overlay::Personal && i.meta.kind == EventKind::Event => {
                        if self.daemon.is_none() {
                            return self.set_message("kairosd not configured — writes unavailable", true);
                        }
                        let off = !i.meta.not_going;
                        self.set_not_going_local(&i.uid, off);
                        self.set_message(
                            if off {
                                format!("not going to “{}”: your time is free (B again to undo)", i.title)
                            } else {
                                format!("going to “{}” again", i.title)
                            },
                            false,
                        );
                        self.deferred.push(Box::new(move |app: &mut App| {
                            let req = api::UpdateEventReq {
                                not_going: Some(off),
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
                                    app.set_not_going_local(&i.uid, !off);
                                    app.set_message(e, true);
                                }
                            }
                        }));
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
                    let what = if p.created.is_empty() && p.deleted.is_empty() {
                        format!(
                            "plan is up to date: {} block{} kept, nothing to change",
                            p.kept,
                            if p.kept == 1 { "" } else { "s" }
                        )
                    } else {
                        format!(
                            "plan: {} new, {} removed ({} moved)",
                            p.created.len(),
                            p.deleted.len(),
                            p.moved
                        )
                    };
                    self.set_message(format!("{what}{}", warn.clone().unwrap_or_default()), warn.is_some());
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
            Confirm::DeleteProject(id) => {
                if let Some(r) = self.with_daemon(|d| d.delete_project(&id)) {
                    self.write_result(r);
                    self.sel = self.sel.min(self.projects.len().saturating_sub(1));
                }
            }
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
                if let Some(r) = self.with_daemon(|d| d.undo_with_replans(&id)) {
                    let ok = matches!(&r, Ok(Ok(_)));
                    self.write_result(r);
                    if ok {
                        // The local copies of what was undone (e.g. a moved
                        // block shown at its new time) would hide the undo
                        // until they expire: drop them; the reload brings
                        // the calendar as it really is.
                        self.patches.clear();
                        self.blocking_overrides.clear();
                    }
                }
            }
            Confirm::Skip => {
                if let Some(cl) = self.selected() {
                    let i = cl.primary;
                    if self.daemon.is_none() {
                        return self.set_message("kairosd not configured — writes unavailable", true);
                    }
                    self.patch_local(&i.uid, |_| false);
                    self.deferred.push(Box::new(move |app: &mut App| {
                        if let Some(r) = app.with_daemon(|d| d.skip_event(&i.source, &i.uid))
                            && !app.write_applied(r)
                        {
                            app.unpatch(Some(i.clone()), &i.uid);
                        }
                    }));
                }
            }
            Confirm::ClearDue(tid) => {
                let req = api::UpdateTaskReq {
                    due: Some(String::new()),
                    ..Default::default()
                };
                self.update_task_and_report(&tid, &req);
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
        if let PromptKind::Remaining { t, outcome, actual } = &kind {
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
            return self.send_checkin(&t.clone(), &outcome.clone(), *actual, left);
        }
        if let PromptKind::Partial { t } = &kind {
            let Ok(actual) = dates::parse_minutes(&buf) else {
                self.set_message("how long: e.g. 45m, 1h30", true);
                self.mode = Mode::Prompt(kind.clone(), buf);
                return;
            };
            return self.check_in_after(t.clone(), "partial", Some(actual));
        }
        if let PromptKind::LogWork { task_id, title } = &kind {
            let Ok(m) = dates::parse_minutes(&buf) else {
                self.set_message("how long: e.g. 40m, 1h", true);
                self.mode = Mode::Prompt(kind.clone(), buf);
                return;
            };
            let (task_id, title) = (task_id.clone(), title.clone());
            match self.with_daemon(|d| d.log_work(&task_id, m)) {
                Some(Ok(Ok(r))) => {
                    let warn = r.plan.as_ref().and_then(plan_warning).unwrap_or_default();
                    self.set_message(
                        format!("✓ logged {} on {title}{warn}", crate::form::fmt_minutes(i64::from(m))),
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
        if let PromptKind::MoveDeadline { task_id } = &kind {
            let due = match self.when_str(&buf) {
                Ok(s) => s,
                Err(e) => {
                    self.set_message(e, true);
                    self.mode = Mode::Prompt(kind.clone(), buf);
                    return;
                }
            };
            let req = api::UpdateTaskReq {
                due: Some(due),
                ..Default::default()
            };
            return self.update_task_and_report(&task_id.clone(), &req);
        }
        if let PromptKind::Reopen { id, .. } = &kind {
            let Ok(left) = dates::parse_minutes(&buf) else {
                self.set_message("how much is left: e.g. 2h, 90m", true);
                self.mode = Mode::Prompt(kind.clone(), buf);
                return;
            };
            let req = api::UpdateTaskReq {
                reopen: Some(true),
                remaining_min: Some(left),
                ..Default::default()
            };
            self.update_task_and_report(&id.clone(), &req);
            self.sel = 0;
            return;
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
        if let PromptKind::NewProject { proposal } = &kind {
            if buf.is_empty() {
                return;
            }
            let req = api::CreateProjectReq {
                name: buf.clone(),
                ..Default::default()
            };
            match self.with_daemon(|d| d.create_project(&req)) {
                Some(Ok(Ok(p))) => {
                    let req = api::AcceptProposalReq {
                        project_id: Some(p.id.clone()),
                        title: None,
                    };
                    self.accept_proposal(proposal.clone(), req, p.name);
                }
                Some(Ok(Err(e))) => self.set_message(e.to_string(), true),
                Some(Err(e)) => self.set_message(e.to_string(), true),
                None => {}
            }
            return;
        }
        if buf.is_empty() && kind != PromptKind::Search {
            return;
        }
        match kind {
            PromptKind::Remaining { .. }
            | PromptKind::Partial { .. }
            | PromptKind::LogWork { .. }
            | PromptKind::NewProject { .. }
            | PromptKind::MoreTime { .. }
            | PromptKind::MoveDeadline { .. }
            | PromptKind::Reopen { .. } => {}
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
            FormKind::AddSession => {
                let when = f.get("When");
                if when.is_empty() {
                    return Err("When: e.g. thu 14:00".into());
                }
                let dur = match f.get("Duration") {
                    "" => 120,
                    d => dates::parse_minutes(d).map_err(|_| "Duration: e.g. 90m, 2h".to_string())?,
                };
                match dates::parse_when(when, self.today()).map_err(|e| e.to_string())? {
                    When::Date(_) => Err("When: give a time too, e.g. thu 14:00".into()),
                    When::DateTime(n) => {
                        let s = resolve_local(n, self.tz);
                        let e = s + Duration::minutes(i64::from(dur));
                        Ok(format!("{} · {}", describe_span(s, e, self.tz), f.get("Project")))
                    }
                }
            }
            FormKind::AddProject | FormKind::EditProject { .. } => {
                if f.get("Name").is_empty() {
                    return Err("Name: what the project is called".into());
                }
                Ok(match f.get("Status") {
                    "active" => "active: working on it now — project time suggests it".to_string(),
                    "next" => "next: should be working on it soon".to_string(),
                    "paused" => "paused: on hold".to_string(),
                    _ => "done: finished, kept as a record".to_string(),
                })
            }
            FormKind::TookLonger { .. } => {
                let spent = dates::parse_minutes(f.get("Spent")).map_err(|_| "Spent: e.g. 3h or 2h30".to_string())?;
                if !f.fields.iter().any(|x| x.label == "Needs") {
                    return Ok(format!("{} spent", crate::form::fmt_minutes(i64::from(spent))));
                }
                let needs =
                    dates::parse_minutes(f.get("Needs")).map_err(|_| "Needs: e.g. 1h, or 0 if finished".to_string())?;
                if needs == 0 {
                    return Ok(format!(
                        "{} spent · finished",
                        crate::form::fmt_minutes(i64::from(spent))
                    ));
                }
                let from = match f.get("From") {
                    "" => "as soon as there's time".to_string(),
                    d => match dates::parse_when(d, self.today()).map_err(|e| e.to_string())? {
                        When::Date(d) => format!("from {}", d.format("%a %-d %b")),
                        When::DateTime(n) => format!("from {}", n.format("%a %-d %b %H:%M")),
                    },
                };
                Ok(format!(
                    "{} spent · {} more, planned {from}",
                    crate::form::fmt_minutes(i64::from(spent)),
                    crate::form::fmt_minutes(i64::from(needs))
                ))
            }
            FormKind::EditTask { .. } => {
                let left = dates::parse_minutes(f.get("Left")).map_err(|_| "Left: e.g. 3h or 90m".to_string())?;
                let due = match f.get("Due") {
                    "" => "no deadline".to_string(),
                    d => match dates::parse_when(d, self.today()).map_err(|e| e.to_string())? {
                        When::Date(d) => format!("due {}", d.format("%a %-d %b")),
                        When::DateTime(n) => format!("due {}", n.format("%a %-d %b %H:%M")),
                    },
                };
                Ok(format!(
                    "{} left · {} · {due} · {key}",
                    crate::form::fmt_minutes(i64::from(left)),
                    f.get("Type")
                ))
            }
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
        if f.kind == FormKind::AddSession {
            if let Err(e) = self.form_preview(&f) {
                self.set_message(e, true);
                self.mode = Mode::Form(f);
                return;
            }
            let start = match self.when_str(f.get("When")) {
                Ok(s) => s,
                Err(e) => return self.set_message(e, true),
            };
            let req = api::SessionReq {
                start,
                end: None,
                duration_min: dates::parse_minutes(f.get("Duration")).ok(),
            };
            let project = f.get("Project").to_string();
            // Close the form and say so first, then do the round trip.
            self.set_message(format!("◇ planning {project}…"), false);
            self.deferred.push(Box::new(move |app: &mut App| {
                if let Some(r) = app.with_daemon(|d| d.create_session(&project, &req)) {
                    app.write_result(r);
                }
            }));
            return;
        }
        if let FormKind::AddProject | FormKind::EditProject { .. } = &f.kind {
            let name = f.get("Name").to_string();
            if name.is_empty() {
                self.set_message("Name is empty", true);
                self.mode = Mode::Form(f);
                return;
            }
            let (status, note) = (f.get("Status").to_string(), f.get("Note").to_string());
            let key = Some(f.get("Calendar").to_string());
            if let FormKind::EditProject { id } = &f.kind {
                let req = api::UpdateProjectReq {
                    name: Some(name),
                    status: Some(status),
                    note: Some(note),
                    key,
                };
                return self.update_project_and_report(&id.clone(), &req);
            }
            let req = api::CreateProjectReq {
                name,
                status: Some(status),
                note: Some(note),
                key,
            };
            match self.with_daemon(|d| d.create_project(&req)) {
                Some(Ok(Ok(p))) => {
                    self.set_message(format!("✓ project {} · {}", p.name, p.status.as_str()), false);
                    self.refresh_remote();
                    if let Some(k) = self.projects.iter().position(|x| x.project.id == p.id) {
                        self.sel = k;
                    }
                }
                Some(Ok(Err(e))) => {
                    self.set_message(e.to_string(), true);
                    self.mode = Mode::Form(f);
                }
                Some(Err(e)) => self.set_message(e.to_string(), true),
                None => {}
            }
            return;
        }
        if let FormKind::TookLonger { t } = &f.kind {
            if let Err(e) = self.form_preview(&f) {
                self.set_message(e, true);
                self.mode = Mode::Form(f);
                return;
            }
            let spent = dates::parse_minutes(f.get("Spent")).ok();
            // A project session asks only how long it took.
            let needs = if t.project {
                None
            } else {
                dates::parse_minutes(f.get("Needs")).ok()
            };
            let from = match f.get("From") {
                "" => None,
                w => match self.when_str(w) {
                    Ok(s) => Some(s),
                    Err(e) => return self.set_message(e, true),
                },
            };
            let outcome = if needs == Some(0) { "finished" } else { "longer" };
            return self.send_checkin_full(&t.clone(), outcome, spent, needs, from);
        }
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
            FormKind::EditTask { id } => {
                let changed = |l: &str| f.changed(l).then(|| f.get(l).to_string());
                let due = if f.changed("Due") {
                    match f.get("Due") {
                        "" => Some(String::new()), // clears the deadline (and its ⚑)
                        d => match self.when_str(d) {
                            Ok(s) => Some(s),
                            Err(e) => return self.set_message(e, true),
                        },
                    }
                } else {
                    None
                };
                let req = api::UpdateTaskReq {
                    title: changed("Title"),
                    key: changed("Calendar"),
                    kind: changed("Type"),
                    remaining_min: if f.changed("Left") { dur(f.get("Left")) } else { None },
                    due,
                    project: f.changed("Project").then(|| match f.get("Project") {
                        crate::form::NO_PROJECT => String::new(),
                        p => p.to_string(),
                    }),
                    ..Default::default()
                };
                if req == api::UpdateTaskReq::default() {
                    return self.set_message("no changes", false);
                }
                if let Some(r) = self.with_daemon(|d| d.update_task(&id, &req)) {
                    match r {
                        Ok(Ok(t)) => {
                            let pending = t.changesets.iter().any(|c| c.status == Status::Pending);
                            let warn = t.plan.as_ref().and_then(plan_warning).unwrap_or_default();
                            self.set_message(
                                if pending {
                                    format!("{}: waiting for approval (A)", t.task.title)
                                } else {
                                    format!("✓ {}{warn}", t.task.title)
                                },
                                !warn.is_empty(),
                            );
                            self.wants_sync = true;
                            self.refresh_remote();
                        }
                        Ok(Err(e)) => self.set_message(e.to_string(), true),
                        Err(e) => self.set_message(e.to_string(), true),
                    }
                }
            }
            FormKind::TookLonger { .. }
            | FormKind::AddProject
            | FormKind::EditProject { .. }
            | FormKind::AddSession => {}
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
                    project: Some(f.get("Project").to_string())
                        .filter(|p| !p.is_empty() && p != crate::form::NO_PROJECT),
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
        let t = CheckinTarget::of(item, true);
        match k {
            // Same keys as on a block in the calendar (p is the Private toggle there).
            Key::Char('y') | Key::Char('d') => self.check_in(t, "done"),
            Key::Char('c') | Key::Char('p') => self.check_in(t, "partial"),
            Key::Char('x') | Key::Char('m') => self.check_in(t, "missed"),
            Key::Char('F') | Key::Char('f') => self.check_in(t, "finished"),
            Key::Char('+') => self.mode = Mode::Form(Form::took_longer(t)),
            Key::Char('s') | Key::Down => {
                self.mode = Mode::Checkin((i + 1).min(open.len().saturating_sub(1)));
            }
            Key::Up => self.mode = Mode::Checkin(i.saturating_sub(1)),
            Key::Esc | Key::Char('q') => {}
            _ => self.mode = Mode::Checkin(i),
        }
    }

    /// What accepting proposal `id` can go to: its suggested project first,
    /// the other active and next projects, then a new one, or none.
    pub fn pick_options(&self, id: &str) -> Vec<PickOption> {
        use kairos_core::project::ProjectStatus;
        let suggested = self
            .proposals
            .iter()
            .find(|p| p.id == id)
            .and_then(|p| p.project_id.clone());
        let mut out: Vec<PickOption> = Vec::new();
        let mut push = |p: &kairos_core::project::Project| {
            out.push(PickOption::Project {
                id: p.id.clone(),
                name: p.name.clone(),
            })
        };
        if let Some(s) = self.projects.iter().find(|p| Some(&p.project.id) == suggested.as_ref()) {
            push(&s.project);
        }
        for st in [ProjectStatus::Active, ProjectStatus::Next, ProjectStatus::Paused] {
            for p in self.projects.iter().filter(|p| p.project.status == st) {
                if Some(&p.project.id) != suggested.as_ref() {
                    push(&p.project);
                }
            }
        }
        out.push(PickOption::New);
        out.push(PickOption::Plain);
        out
    }

    fn pick_project_key(&mut self, proposal: String, sel: usize, k: Key) {
        let opts = self.pick_options(&proposal);
        match k {
            Key::Down | Key::Tab => {
                self.mode = Mode::PickProject {
                    proposal,
                    sel: (sel + 1).min(opts.len().saturating_sub(1)),
                };
            }
            Key::Up | Key::BackTab => {
                self.mode = Mode::PickProject {
                    proposal,
                    sel: sel.saturating_sub(1),
                };
            }
            Key::Enter | Key::Char('y') => match opts.get(sel).cloned() {
                Some(PickOption::Project { id, name }) => {
                    let req = api::AcceptProposalReq {
                        project_id: Some(id),
                        title: None,
                    };
                    self.accept_proposal(proposal, req, name);
                }
                Some(PickOption::New) => {
                    self.message = None;
                    self.mode = Mode::Prompt(PromptKind::NewProject { proposal }, String::new());
                }
                Some(PickOption::Plain) | None => {
                    let title = self.cfg_project_title();
                    let req = api::AcceptProposalReq {
                        title: Some(title.clone()),
                        project_id: None,
                    };
                    self.accept_proposal(proposal, req, title);
                }
            },
            Key::Esc | Key::Char('q') => self.message = None,
            _ => self.mode = Mode::PickProject { proposal, sel },
        }
    }

    /// The generic title of project time ("Project time").
    fn cfg_project_title(&self) -> String {
        "Project time".to_string()
    }

    fn update_project_and_report(&mut self, id: &str, req: &api::UpdateProjectReq) {
        match self.with_daemon(|d| d.update_project(id, req)) {
            Some(Ok(Ok(r))) => {
                let p = &r.project;
                if r.changeset.status == Status::Pending {
                    self.set_message(
                        format!("◌ needs approval: {} (A to review)", r.changeset.summary),
                        false,
                    );
                } else {
                    self.set_message(format!("✓ {} · {}", p.name, p.status.as_str()), false);
                }
                self.wants_sync = true;
                self.refresh_remote();
            }
            Some(Ok(Err(e))) => self.set_message(e.to_string(), true),
            Some(Err(e)) => self.set_message(e.to_string(), true),
            None => {}
        }
    }

    /// The keys that do something for what's selected, for the bottom-right
    /// edge ("e edit · m move · d delete").
    pub fn key_hints(&self) -> String {
        let h: &[&str] = match self.view {
            View::Tasks => {
                if self.sel < self.tasks.len() {
                    &["e edit", "D done", "W log work", "+ time", "n start", "d drop"]
                } else if self.sel < self.tasks.len() + self.done_tasks.len().min(DONE_SHOWN) {
                    &["o reopen"]
                } else {
                    &["a add task"]
                }
            }
            View::Approvals => {
                if self.pending.is_empty() {
                    &[]
                } else {
                    &["y approve", "n reject"]
                }
            }
            View::Projects => {
                if self.projects.is_empty() {
                    &["a add project"]
                } else {
                    &["n plan time", "a add", "e edit", "s status", "d delete"]
                }
            }
            View::Week | View::Day | View::Month | View::Agenda => match self.selected().map(|c| c.primary) {
                None => &["a add"],
                Some(i) => match (i.meta.kind, i.overlay) {
                    (EventKind::Proposal, _) => &["y accept", "d drop"],
                    (EventKind::Deadline, _) if i.meta.task_id.is_some() => {
                        &["e edit", "m move", "d remove", "n start"]
                    }
                    (EventKind::TaskBlock, _) if kairos_core::build::is_checked_in(&i.title) => &["W log more work"],
                    _ if self.awaits_checkin(&i) => &["y done", "c partly", "+ longer", "x missed", "F finished"],
                    (EventKind::TaskBlock, _) => &["n start", "m move", "d skip", "e edit"],
                    (_, Overlay::Family) if i.meta.origin_key.is_none() => {
                        if i.meta.blocking {
                            &["B not attending"]
                        } else {
                            &["B attending"]
                        }
                    }
                    (_, Overlay::Background | Overlay::Imported) => &[],
                    _ => &["e edit", "m move", "d delete", "s share"],
                },
            },
        };
        h.join(" · ")
    }

    /// Plan project time by hand (for `project`, or the first active one).
    fn open_add_session(&mut self, project: Option<String>) {
        use kairos_core::project::ProjectStatus;
        let names: Vec<String> = self
            .projects
            .iter()
            .filter(|p| p.project.status != ProjectStatus::Done)
            .map(|p| p.project.name.clone())
            .collect();
        if names.is_empty() {
            self.mode = Mode::Normal;
            return self.set_message("no projects yet — add one in R first", true);
        }
        let first = project.unwrap_or_else(|| names[0].clone());
        self.message = None;
        self.mode = Mode::Form(Form::add_session(
            names,
            &first,
            when_prefill(self.cursor, self.today()),
        ));
    }

    /// Projects a task can belong to (not done), by name.
    fn open_project_names(&self) -> Vec<String> {
        self.projects
            .iter()
            .filter(|p| p.project.status != kairos_core::project::ProjectStatus::Done)
            .map(|p| p.project.name.clone())
            .collect()
    }

    /// A task's edit form with its Project picker.
    fn task_form_projects(&self, f: Form, t: &kairos_core::task::Task) -> Form {
        let current = t
            .project_id
            .as_ref()
            .and_then(|id| self.projects.iter().find(|p| &p.project.id == id))
            .map(|p| p.project.name.clone());
        f.with_project(self.open_project_names(), current.as_deref())
    }

    /// The calendar project sessions go in by default.
    fn projects_key(&self) -> String {
        self.cfg.keys.iter().find(|k| k.name == "Projects").map_or_else(
            || self.cfg.default_key().map(|k| k.name.clone()).unwrap_or_default(),
            |k| k.name.clone(),
        )
    }

    /// Open the add form: a task, or an event (prefilled with the cursor day).
    fn open_add(&mut self, task: bool) {
        let keys = self.key_names();
        let default = self.cfg.default_key().map(|k| k.name.clone()).unwrap_or_default();
        self.message = None;
        self.mode = Mode::Form(if task {
            let names = self.open_project_names();
            Form::add_task(keys, &default).with_project(names, None)
        } else {
            Form::add_event(keys, &default, when_prefill(self.cursor, self.today()))
        });
    }

    /// Send a task update and say how it went (with any plan warning).
    fn update_task_and_report(&mut self, id: &str, req: &api::UpdateTaskReq) {
        match self.with_daemon(|d| d.update_task(id, req)) {
            Some(Ok(Ok(t))) => {
                let pending = t.changesets.iter().any(|c| c.status == Status::Pending);
                let warn = t.plan.as_ref().and_then(plan_warning).unwrap_or_default();
                self.set_message(
                    if pending {
                        format!("{}: waiting for approval (A)", t.task.title)
                    } else {
                        format!("✓ {}{warn}", t.task.title)
                    },
                    !warn.is_empty(),
                );
                self.wants_sync = true;
                self.refresh_remote();
            }
            Some(Ok(Err(e))) => self.set_message(e.to_string(), true),
            Some(Err(e)) => self.set_message(e.to_string(), true),
            None => {}
        }
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

    fn accept_proposal(&mut self, id: String, req: api::AcceptProposalReq, title: String) {
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
            made: Utc::now(),
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
            let failed = match app.with_daemon(|d| d.accept_proposal(&id, &req)) {
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

    /// What a check-in needs about a block (from the C list's data if it's
    /// there, else from the block and its task).
    /// A work block or project session that has started and isn't
    /// checked in yet.
    pub fn awaits_checkin(&self, i: &Instance) -> bool {
        if kairos_core::build::is_checked_in(&i.title) || i.start(self.tz) > self.now {
            return false;
        }
        i.meta.kind == EventKind::TaskBlock
            || i.meta.project_id.is_some()
            || self
                .checkins
                .iter()
                .any(|c| c.project && c.block_uid == i.uid && c.outcome.is_none())
    }

    fn checkin_target(&self, i: &Instance) -> CheckinTarget {
        if let Some(item) = self.checkins.iter().find(|c| c.block_uid == i.uid) {
            return CheckinTarget::of(item, false);
        }
        let task = i
            .meta
            .task_id
            .as_deref()
            .and_then(|id| self.tasks.iter().find(|t| t.id == id));
        let (s, e) = i.span.bounds(self.tz);
        let planned = (e - s).num_minutes().max(0) as u32;
        CheckinTarget {
            uid: i.uid.clone(),
            title: kairos_core::build::bare_block_title(&i.title).to_string(),
            planned_min: planned,
            ask_remaining: task.is_some_and(|t| t.kind.as_deref() == Some("coursework") || planned >= t.remaining_min),
            remaining_min: task.map_or(0, |t| t.remaining_min),
            from_list: false,
            project: i.meta.project_id.is_some(),
        }
    }

    /// How many blocks are waiting for a check-in (the status bar's nudge).
    pub fn to_check_in(&self) -> usize {
        self.checkins.iter().filter(|c| c.outcome.is_none()).count()
    }

    /// Check a block in: done / partial (asks how long) / missed / finished
    /// (done, and the task is finished). Coursework, and a block that uses up
    /// the task, also ask how much is left.
    pub fn check_in(&mut self, t: CheckinTarget, outcome: &str) {
        if outcome == "partial" {
            self.message = None;
            let guess = crate::form::fmt_minutes(i64::from(t.planned_min / 2));
            self.mode = Mode::Prompt(PromptKind::Partial { t }, guess);
            return;
        }
        self.check_in_after(t, outcome, None);
    }

    fn check_in_after(&mut self, t: CheckinTarget, outcome: &str, actual: Option<u32>) {
        if t.ask_remaining && (outcome == "done" || outcome == "partial") {
            let did = actual.unwrap_or(t.planned_min);
            let guess = t.remaining_min.saturating_sub(did);
            self.message = None;
            self.mode = Mode::Prompt(
                PromptKind::Remaining {
                    t,
                    outcome: outcome.into(),
                    actual,
                },
                crate::form::fmt_minutes(i64::from(guess)),
            );
            return;
        }
        self.send_checkin(&t, outcome, actual, None);
    }

    fn send_checkin(&mut self, t: &CheckinTarget, outcome: &str, actual: Option<u32>, remaining: Option<u32>) {
        self.send_checkin_full(t, outcome, actual, remaining, None);
    }

    fn send_checkin_full(
        &mut self,
        t: &CheckinTarget,
        outcome: &str,
        actual: Option<u32>,
        remaining: Option<u32>,
        not_before: Option<String>,
    ) {
        let req = api::CheckinReq {
            block_uid: t.uid.clone(),
            outcome: outcome.into(),
            actual_min: actual,
            remaining_min: remaining,
            not_before,
        };
        if let Some(r) = self.with_daemon(|d| d.checkin(&req)) {
            match r {
                Ok(Ok(p)) => {
                    let what = match (outcome, actual) {
                        ("partial", Some(m)) => format!("{} done", crate::form::fmt_minutes(i64::from(m))),
                        ("longer", Some(m)) => format!("{} spent", crate::form::fmt_minutes(i64::from(m))),
                        ("finished", _) => "finished ✓".to_string(),
                        (o, _) => o.to_string(),
                    };
                    let left = remaining.map(|m| format!(" · {} left", crate::form::fmt_minutes(i64::from(m))));
                    let warn = plan_warning(&p).unwrap_or_default();
                    self.set_message(
                        format!("{}: {what}{}{warn}", t.title, left.unwrap_or_default()),
                        !warn.is_empty(),
                    );
                    self.wants_sync = true;
                }
                Ok(Err(e)) => self.set_message(e.to_string(), true),
                Err(e) => self.set_message(e.to_string(), true),
            }
        }
        self.refresh_remote();
        if t.from_list && self.checkins.iter().any(|c| c.outcome.is_none()) {
            self.mode = Mode::Checkin(0);
        }
    }
}

/// Apply a new start (RFC 3339 or a date) and optional duration to a local copy.
fn retime(x: &mut Instance, start: &str, dur: Option<u32>, tz: Tz) {
    // A work block moved by hand is pinned (as kairosd records it): solid at once.
    if x.meta.kind == EventKind::TaskBlock {
        x.meta.pinned = true;
        x.meta.flex = false;
    }
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

/// `B4 sheet #Studies ~4h due fri 17:00` → (title, key, minutes, due)
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
    ShiftLeft,
    ShiftRight,
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
    // The areas (ADR-008) form group 0: "1 Studies  2 Projects  3 Careers"; Personal is group 1,
    // drawn "4 Personal ▸ 5 Family  6 Sport  7 Social  8 Errands" (its streams follow the ▸);
    // read-only feeds group 2. `p` still hides all your own calendars at once, unlisted.
    let shown = |k: &kairos_core::config::KeyDef| {
        app.filter.personal && !app.filter.hidden_keys.contains(&k.name.to_lowercase())
    };
    let mut keys = app.cfg.keys.clone();
    keys.sort_by_key(|k| k.index);
    let (streams, areas): (Vec<_>, Vec<_>) = keys.into_iter().partition(|k| k.area.is_some());
    let mut v = Vec::new();
    for k in &areas {
        v.push((k.index.to_string(), k.name.clone(), shown(k), k.colour, 0));
    }
    v.push((
        app.cfg.tui.personal_index.to_string(),
        "Personal".to_string(),
        app.filter.area_on(&app.cfg, "Personal"),
        kairos_core::colour::Rgb::mocha("lavender"),
        1,
    ));
    let mut tail = vec![(
        app.cfg.tui.family_index,
        "Family".to_string(),
        app.filter.family,
        app.cfg.overlays.family_colour,
    )];
    for k in &streams {
        tail.push((k.index, k.name.clone(), shown(k), k.colour));
    }
    tail.sort_by_key(|t| t.0);
    for (i, n, on, col) in tail {
        v.push((i.to_string(), n, on, col, 1));
    }
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
    matches!(i.overlay, Overlay::Imported | Overlay::Background)
        || i.tentative
        || i.meta.kind == EventKind::Proposal
        || i.meta.not_going
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
    fn moving_a_work_block_pins_it_locally() {
        let mut i = Instance {
            uid: "b".into(),
            recurrence_id: None,
            source: "personal".into(),
            overlay: Overlay::Personal,
            key: None,
            title: "Purchase Furniture".into(),
            location: None,
            description: None,
            span: Span::Timed {
                start: Utc::now(),
                end: Utc::now() + Duration::minutes(30),
            },
            recurring: false,
            transparent: false,
            tentative: false,
            meta: Meta {
                kind: EventKind::TaskBlock,
                flex: true,
                ..Default::default()
            },
            href: None,
            alarms: Vec::new(),
        };
        retime(&mut i, "2026-10-13T18:00:00+01:00", None, chrono_tz::Europe::London);
        assert!(i.meta.pinned && !i.meta.flex);
    }

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
