//! TUI state and key handling (no terminal I/O here, so it's testable).

use chrono::{DateTime, Duration, NaiveDate, NaiveTime, Utc};
use chrono_tz::Tz;
use kairos_core::api;
use kairos_core::changeset::{Changeset, Op, Status, WriteResult};
use kairos_core::config::{Config, Overlay};
use kairos_core::dates::{self, When};
use kairos_core::dedup::Cluster;
use kairos_core::model::{EventKind, Instance, Span};
use kairos_core::normalize::series_from_calendars;
use kairos_core::task::Task;
use kairos_core::time::resolve_local;
use kairos_core::view::{Filter, view_with};
use kairos_store::cache::Index;
use kairos_store::client::{Daemon, Rejected};

use crate::form::{ForOption, Form, FormAction, FormKind, describe_span, when_prefill};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Week,
    Day,
    Month,
    Agenda,
}

impl View {
    pub fn name(self) -> &'static str {
        match self {
            View::Week => "week",
            View::Day => "day",
            View::Month => "month",
            View::Agenda => "agenda",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptKind {
    Move,
    Search,
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
    /// Move a task's deadline (from its ⚑).
    MoveDeadline {
        task_id: String,
    },
}

impl PromptKind {
    pub fn label(&self) -> &'static str {
        match self {
            PromptKind::Move => "move to (when)",
            PromptKind::Search => "/",
            PromptKind::Remaining { .. } => "how much is left? (e.g. 3h, 0 = finished)",
            PromptKind::LogWork { .. } => "log work — how long, ending now? (e.g. 40m)",
            PromptKind::MoveDeadline { .. } => "due (when, e.g. fri 17:00)",
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
        }
    }
}

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
    /// Projects (active, next, paused, done) with time spent.
    pub projects: Vec<api::ProjectItem>,
    /// What things can be for (ADR-009): the registry's items and Personal's streams.
    pub fors: Vec<ForOption>,
    /// What was used, into locus's one use log (L8); off in tests and dumps.
    pub uselog: locus_contract::uselog::UseLog,
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

/// What things can be for (ADR-009): every registry item, in its area's calendar, then
/// Personal's streams that have a calendar (Family is the shared calendar: `s` shares to it).
/// Active items first; inactive ones (a finished project) last.
pub fn build_fors(cfg: &Config, shared: Option<&locus_contract::taxonomy::Shared>) -> Vec<ForOption> {
    let fallback = cfg.default_key().map(|k| k.name.clone()).unwrap_or_default();
    let mut v: Vec<(bool, ForOption)> = Vec::new();
    if let Some(s) = shared {
        for p in &s.registry.projects {
            let key = cfg.key(&p.area).map_or_else(|| fallback.clone(), |k| k.name.clone());
            v.push((
                p.active,
                ForOption {
                    id: p.id.clone(),
                    label: p.name.clone(),
                    key,
                    locus: Some(p.id.clone()),
                    course: p.kind == locus_contract::ProjectKind::Course,
                    application: p.kind == locus_contract::ProjectKind::Application,
                },
            ));
        }
    }
    let mut keys = cfg.keys.clone();
    keys.sort_by_key(|k| k.index);
    for k in keys.iter().filter(|k| k.area.is_some()) {
        v.push((
            true,
            ForOption {
                id: k.name.to_lowercase(),
                label: k.name.clone(),
                key: k.name.clone(),
                locus: None,
                course: false,
                application: false,
            },
        ));
    }
    v.sort_by_key(|(active, _)| !*active);
    v.into_iter().map(|(_, o)| o).collect()
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
            projects: Vec::new(),
            fors: build_fors(&cfg, kairos_store::taxonomy::get(&cfg).map(|(s, _)| s).as_ref()),
            uselog: locus_contract::uselog::UseLog::off(),
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
        }
    }

    fn list_len(&self) -> usize {
        match self.view {
            View::Agenda => self.agenda_rows().len(),
            View::Week | View::Day | View::Month => self.day_clusters(self.cursor).len(),
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

    /// Replace the index (after a reload) and re-apply recent local patches. The selection
    /// stays on the item it was on, not on its place in the list (2026-10-27: a reload that
    /// reordered the day moved it onto the next event, and the next ⇧↓ nudged that one).
    pub fn set_index(&mut self, idx: Index) {
        let held = self.selected().map(|c| c.primary.uid);
        self.set_index_keep(idx);
        if let Some(uid) = held {
            self.reselect(&uid);
        }
    }

    /// Select the item `uid` again where it now is, if it is still in view.
    fn reselect(&mut self, uid: &str) {
        let at = match self.view {
            View::Agenda => self
                .agenda_rows()
                .iter()
                .position(|(_, c)| c.all().any(|i| i.uid == uid)),
            View::Week | View::Day | View::Month => self
                .day_clusters(self.cursor)
                .iter()
                .position(|c| c.all().any(|i| i.uid == uid)),
        };
        if let Some(k) = at {
            self.sel = k;
        }
    }

    fn set_index_keep(&mut self, idx: Index) {
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
        let name = match k {
            Key::Char(c) => c.to_string(),
            other => format!("{other:?}").to_lowercase(),
        };
        self.uselog.record(&format!("calendar:{}", self.view.name()), &name);
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
            Mode::AddChoice => match k {
                Key::Char('e') => self.open_add(false),
                Key::Char('t') => self.open_add(true),
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
            Key::Esc => self.search = None,
            Key::Char('?') => self.mode = Mode::Help,
            // Arrows and Tab move, as everywhere in locus (spec K6): ←/→ days, [ ] weeks.
            Key::Left => self.shift(-1),
            Key::Right => self.shift(1),
            Key::Char('[') => self.shift(-7),
            Key::Char(']') => self.shift(7),
            // Nudging the selected item (spec E3): Shift-↑/↓ 30 minutes, Shift-←/→ a day.
            Key::ShiftUp => self.nudge(-30),
            Key::ShiftDown => self.nudge(30),
            Key::ShiftLeft => self.nudge(-24 * 60),
            Key::ShiftRight => self.nudge(24 * 60),
            // Its length (2026-10-27): ⌥↑ shorter, ⌥↓ longer, 15 min at a time.
            Key::AltUp => self.stretch(-15),
            Key::AltDown => self.stretch(15),
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
            // Enter on an item linked to locus opens it there (spec K8); otherwise, and `v`
            // always, the details.
            Key::Enter => {
                let link = (!self.expanded && matches!(self.view, View::Week | View::Day | View::Month | View::Agenda))
                    .then(|| self.selected().and_then(|c| self.locus_link(&c.primary)))
                    .flatten();
                match link {
                    Some(id) => self.open_in_locus(&id),
                    None => self.expanded = !self.expanded,
                }
            }
            Key::Char('v') => self.expanded = !self.expanded,
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
            // Tasks, projects and Approve have one home each, in the locus window (spec E7, round
            // 7c, L7): T, P and A open them there.
            Key::Char('T') => self.open_locus_page("tasks"),
            Key::Char('A') => self.open_locus_page("approve"),
            Key::Char('P') => self.open_locus_page("home:p"),
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
            // + adds, as everywhere in locus (L4).
            Key::Char('+') => {
                self.mode = Mode::AddChoice;
                self.set_message("add: e event · t task (esc cancels)", false);
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
            View::Week | View::Day | View::Month | View::Agenda => {
                if k == Key::Char('S') {
                    self.plan();
                    return;
                }
                let Some(c) = self.selected() else { return };
                let i = c.primary.clone();
                // L: say what it is for afterwards (ADR-009), with the same type-ahead.
                if k == Key::Char('L') && i.meta.kind != EventKind::Worked {
                    let task = i.meta.task_id.clone();
                    let current = match task.as_ref().and_then(|t| self.tasks.iter().find(|x| &x.id == t)) {
                        Some(t) => self.for_label(t.locus.as_deref(), Some(&t.key)),
                        None => self.for_label(i.meta.locus.as_deref(), i.key.as_deref()),
                    };
                    self.message = None;
                    self.mode = Mode::Form(
                        Form::link(i.source.clone(), i.uid.clone(), task, &current).with_fors(self.fors_recent_first()),
                    );
                    return;
                }
                // A work block that has started, checked in or not: y it happened as drawn (fix
                // its start and length first with ⇧ and ⌥) · x it didn't · d it did and the
                // task is finished (L4: d is done and x removes, as everywhere in locus).
                // Answering a ✓ block again corrects it; u takes any answer back.
                if self.checkable(&i) && matches!(k, Key::Char('y' | 'x' | 'd')) {
                    let t = self.checkin_target(&i);
                    let outcome = match k {
                        Key::Char('y') => "done",
                        Key::Char('d') => "finished",
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
                // d on a task's block or ⚑: the task is done (L4: d is done everywhere a task is).
                if k == Key::Char('d')
                    && let Some(tid) = i.meta.task_id.clone()
                    && self.tasks.iter().any(|t| t.id == tid)
                {
                    let title =
                        kairos_core::build::bare_block_title(i.title.trim_start_matches('⚑').trim()).to_string();
                    return self.complete_task_now(&tid, &title);
                }
                // A task's ⚑ edits the task's due date, not just the event.
                if i.meta.kind == EventKind::Deadline
                    && let Some(tid) = i.meta.task_id.clone()
                {
                    let title = i.title.trim_start_matches('⚑').trim().to_string();
                    match k {
                        Key::Char('e') | Key::Char('K') => match self.tasks.iter().find(|t| t.id == tid).cloned() {
                            Some(t) => {
                                let mut f = self.task_form(&t);
                                f.focus_on(if k == Key::Char('K') { "For" } else { "Due" });
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
                        Key::Char('x') if self.tasks.iter().any(|t| t.id == tid) => {
                            self.mode = Mode::Confirm(Confirm::DropTask(tid));
                            self.set_message(
                                format!("drop “{title}”: its due date and all its planned time? y/N"),
                                false,
                            );
                        }
                        Key::Char('x') => {
                            self.mode = Mode::Confirm(Confirm::ClearDue(tid));
                            self.set_message(format!("remove the due date of finished “{title}”? y/N"), false);
                        }
                        _ => {}
                    }
                    return;
                }
                // `x` on a work block skips it: the work is planned elsewhere.
                if i.meta.kind == EventKind::TaskBlock && k == Key::Char('x') {
                    self.mode = Mode::Confirm(Confirm::Skip);
                    self.set_message(
                        format!("skip this “{}” block? (the work is planned elsewhere) y/N", i.title),
                        false,
                    );
                    return;
                }
                match k {
                    Key::Char('e') | Key::Char('K') => {
                        let label = self.for_label(i.meta.locus.as_deref(), i.key.as_deref());
                        let mut f =
                            Form::edit_event(self.key_names(), &i, self.tz, &label).with_fors(self.fors_recent_first());
                        if k == Key::Char('K') {
                            f.focus_on("For");
                        }
                        self.message = None;
                        self.mode = Mode::Form(f);
                    }
                    Key::Char('m') => self.mode = Mode::Prompt(PromptKind::Move, String::new()),
                    Key::Char('x') => {
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
        if buf.is_empty() && kind != PromptKind::Search {
            return;
        }
        match kind {
            PromptKind::Remaining { .. } | PromptKind::LogWork { .. } | PromptKind::MoveDeadline { .. } => {}
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

    /// Move the selected item by `minutes` (spec E3): an event or block to a new start, a task's
    /// ⚑ to a new due time; all-day ones by whole days. One changeset each, with an undo; a block
    /// you nudge stays where you put it. The selection follows it, onto the next day if need be.
    pub fn nudge(&mut self, minutes: i64) {
        let (i, new_start, day) = match self.nudge_target(minutes) {
            Ok(Some(t)) => t,
            Ok(None) => return,
            Err(e) => return self.set_message(e, false),
        };
        if self.daemon.is_none() {
            return self.set_message("kairosd not configured — writes unavailable", true);
        }
        let tz = self.tz;
        // A task's flag: its due date moves.
        if i.meta.kind == EventKind::Deadline
            && let Some(tid) = i.meta.task_id.clone()
        {
            let due = if i.span.is_all_day() {
                day.format("%Y-%m-%d").to_string()
            } else {
                new_start.to_rfc3339()
            };
            let req = api::UpdateTaskReq {
                due: Some(due),
                ..Default::default()
            };
            if let Some(t) = self.tasks.iter_mut().find(|t| t.id == tid) {
                t.due = Some(if i.span.is_all_day() {
                    kairos_core::task::Due::Date(day)
                } else {
                    kairos_core::task::Due::At(new_start)
                });
            }
            self.follow(day, &i.uid);
            self.deferred
                .push(Box::new(move |app: &mut App| app.update_task_and_report(&tid, &req)));
            return;
        }
        let start = if i.span.is_all_day() && i.meta.kind != EventKind::TaskBlock {
            day.format("%Y-%m-%d").to_string()
        } else {
            new_start.to_rfc3339()
        };
        let req = api::UpdateEventReq {
            start: Some(start.clone()),
            ..Default::default()
        };
        self.patch_local(&i.uid, |x| {
            retime(x, &start, None, tz);
            true
        });
        self.follow(day, &i.uid);
        self.deferred.push(Box::new(move |app: &mut App| {
            if let Some(r) = app.with_daemon(|d| d.update_event(&i.source, &i.uid, &req).map(|r| r.map(to_write)))
                && !app.write_applied(r)
            {
                app.unpatch(Some(i.clone()), &i.uid);
            }
        }));
    }

    /// Make the selected item `minutes` longer (or shorter): its end moves, its start stays.
    /// Together with the nudge that is every start and end (started later and kept going: ⇧↓
    /// then ⌥↓). On a work block it means one thing everywhere (kairosd, 2026-10-27): ahead,
    /// the task gets that much more (or less) time; checked in, it is what you did.
    pub fn stretch(&mut self, minutes: i64) {
        if !matches!(self.view, View::Week | View::Day) {
            return;
        }
        let Some(i) = self.selected().map(|c| c.primary) else {
            return;
        };
        if matches!(i.meta.kind, EventKind::Worked | EventKind::Deadline) {
            return;
        }
        if i.span.is_all_day() {
            return self.set_message("an all-day item has no length to change: e to edit its days", false);
        }
        if self.daemon.is_none() {
            return self.set_message("kairosd not configured — writes unavailable", true);
        }
        let (s, e) = i.span.bounds(self.tz);
        let end = e + Duration::minutes(minutes);
        if end - s < Duration::minutes(15) {
            let what = if i.meta.kind == EventKind::TaskBlock {
                "x removes it"
            } else {
                "x deletes it"
            };
            return self.set_message(format!("15 min is as short as it goes: {what}"), false);
        }
        let len = (end - s).num_minutes().max(0) as u32;
        let start = s.to_rfc3339();
        let tz = self.tz;
        self.patch_local(&i.uid, |x| {
            retime(x, &start, Some(len), tz);
            true
        });
        let day = s.with_timezone(&tz).date_naive();
        self.follow(day, &i.uid);
        let req = api::UpdateEventReq {
            end: Some(end.to_rfc3339()),
            ..Default::default()
        };
        self.deferred.push(Box::new(move |app: &mut App| {
            if let Some(r) = app.with_daemon(|d| d.update_event(&i.source, &i.uid, &req).map(|r| r.map(to_write)))
                && !app.write_applied(r)
            {
                app.unpatch(Some(i.clone()), &i.uid);
            }
        }));
    }

    /// Where a nudge would put the selected item: (it, its new start, its new day). `Ok(None)`
    /// when nothing nudgeable is selected; `Err` says why not.
    pub fn nudge_target(&self, minutes: i64) -> Result<Option<(Instance, DateTime<Utc>, NaiveDate)>, String> {
        if !matches!(self.view, View::Week | View::Day) {
            return Ok(None);
        }
        let Some(i) = self.selected().map(|c| c.primary) else {
            return Ok(None);
        };
        if i.meta.kind == EventKind::Worked {
            return Ok(None);
        }
        // A work block on a whole day (from before kairos refused them) becomes a timed one at
        // 09:00 that day, then moves (round 7b).
        let block = i.meta.kind == EventKind::TaskBlock;
        if i.span.is_all_day() && !block && minutes % (24 * 60) != 0 {
            return Err("an all-day item moves by days: Shift-←/→".into());
        }
        let (s, _) = i.span.bounds(self.tz);
        let s = if i.span.is_all_day() && block {
            resolve_local(
                s.with_timezone(&self.tz)
                    .date_naive()
                    .and_time(NaiveTime::from_hms_opt(9, 0, 0).unwrap_or(NaiveTime::MIN)),
                self.tz,
            )
        } else {
            s
        };
        let new_start = s + Duration::minutes(minutes);
        let day = new_start.with_timezone(&self.tz).date_naive();
        Ok(Some((i, new_start, day)))
    }

    /// Keep `uid` selected after it moved to `day`.
    fn follow(&mut self, day: NaiveDate, uid: &str) {
        self.cursor = day;
        if let Some(k) = self
            .day_clusters(day)
            .iter()
            .position(|c| c.all().any(|x| x.uid == uid))
        {
            self.sel = k;
        }
    }

    pub fn key_names(&self) -> Vec<String> {
        let mut keys = self.cfg.keys.clone();
        keys.sort_by_key(|k| k.index);
        keys.into_iter().map(|k| k.name).collect()
    }

    /// How the form's time fields were understood, or why they weren't.
    pub fn form_preview(&self, f: &Form) -> Result<String, String> {
        // What it is for (ADR-009): shown as "for B15 Control Systems"; empty is your inbox.
        let has_for = f.fields.iter().any(|x| x.label == "For");
        if has_for && !f.get("For").is_empty() && f.chosen_for().is_none() {
            return Err(format!(
                "For: nothing called “{}” (a course, an application, a project, or Errands, Sport, Social)",
                f.get("For")
            ));
        }
        let key = match f.chosen_for() {
            Some(o) => format!("for {}", o.label),
            None if has_for => format!(
                "for {}",
                self.cfg.default_key().map(|k| k.name.clone()).unwrap_or_default()
            ),
            None => f.get("Calendar").to_string(),
        };
        match f.kind {
            FormKind::Link { .. } => match f.chosen_for() {
                Some(o) => Ok(format!(
                    "for {}{}",
                    o.label,
                    if o.locus.is_some() {
                        " · opens there in locus"
                    } else {
                        ""
                    }
                )),
                None => Err("For: type what it belongs to".into()),
            },
            FormKind::EditTask { .. } => {
                let left = dates::parse_minutes(f.get("Left")).map_err(|_| "Left: e.g. 3h or 90m".to_string())?;
                let due = match f.get("Due") {
                    "" => "no due date".to_string(),
                    d => match dates::parse_when(d, self.today()).map_err(|e| e.to_string())? {
                        When::Date(d) => format!("due {}", d.format("%a %-d %b")),
                        When::DateTime(n) => format!("due {}", n.format("%a %-d %b %H:%M")),
                    },
                };
                Ok(format!(
                    "{} left · {due} · {key}",
                    crate::form::fmt_minutes(i64::from(left))
                ))
            }
            FormKind::AddTask => {
                // How it is planned follows from the estimate (ADR-009).
                let target = f.chosen_for();
                let course = target.is_some_and(|o| o.course);
                let errand = target.is_none_or(|o| o.locus.is_none());
                let est = match f.get("Estimate") {
                    "" if course && f.get("Title").to_lowercase().contains("sheet") => 300,
                    "" if errand => 20,
                    "" => return Err("Estimate: how long in total (e.g. 3h or 45m)".into()),
                    e => dates::parse_minutes(e).map_err(|_| "Estimate: e.g. 4h or 90m".to_string())?,
                };
                let ty = if est <= 60 { "in one go" } else { "in 2–3 h blocks" };
                // For an application with no due date: before its next event (spec E4).
                let next_event = target.filter(|o| o.application).and_then(|o| {
                    let l = o.locus.as_deref()?;
                    self.index
                        .instances
                        .iter()
                        .filter(|i| i.meta.locus.as_deref() == Some(l) && i.start(self.tz) > self.now)
                        .min_by_key(|i| i.start(self.tz))
                });
                let due = match (f.get("Due"), next_event) {
                    ("", Some(i)) => format!(
                        "before {} {} (moves with it)",
                        i.title,
                        i.start(self.tz).with_timezone(&self.tz).format("%a %-d %b %H:%M")
                    ),
                    ("", None) => "no due date".to_string(),
                    (d, _) => match dates::parse_when(d, self.today()).map_err(|e| e.to_string())? {
                        When::Date(d) => format!("due {}", d.format("%a %-d %b")),
                        When::DateTime(n) => format!("due {}", n.format("%a %-d %b %H:%M")),
                    },
                };
                Ok(format!(
                    "{} {ty} · {due} · {key}",
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
        // The link form (L) has no title: only what it is for. Checking one here made Enter on it
        // answer "Title is empty" and do nothing (25 Oct, a B14 lecture).
        if !matches!(f.kind, FormKind::Link { .. }) && f.get("Title").is_empty() {
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
                let target = f.chosen_for().cloned();
                let req = api::CreateEventReq {
                    title: f.get("Title").to_string(),
                    start,
                    duration_min: dur(f.get("Duration")),
                    key: target.as_ref().map(|o| o.key.clone()),
                    locus: target.and_then(|o| o.locus),
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
                // A new For: its link, and its calendar when that differs (ADR-009).
                let target = f.changed("For").then(|| f.chosen_for().cloned()).flatten();
                let current_key = self
                    .index
                    .instances
                    .iter()
                    .find(|i| i.uid == uid)
                    .and_then(|i| i.key.clone());
                let req = api::UpdateEventReq {
                    title: changed("Title"),
                    start,
                    duration_min: if time_changed { dur(f.get("Duration")) } else { None },
                    location: changed("Location"),
                    description: changed("Notes"),
                    locus: target.as_ref().map(|o| o.locus.clone().unwrap_or_default()),
                    ..Default::default()
                };
                let rekey = target
                    .as_ref()
                    .map(|o| o.key.clone())
                    .filter(|k| current_key.as_deref().is_none_or(|c| !c.eq_ignore_ascii_case(k)));
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
                let target = f.changed("For").then(|| f.chosen_for().cloned()).flatten();
                let req = api::UpdateTaskReq {
                    title: changed("Title"),
                    key: target.as_ref().map(|o| o.key.clone()),
                    locus: target.as_ref().map(|o| o.locus.clone().unwrap_or_default()),
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
            FormKind::Link { source, uid, task } => {
                let Some(o) = f.chosen_for().cloned() else {
                    return self.set_message("For: type what it belongs to", true);
                };
                let locus = o.locus.clone().unwrap_or_default();
                let r = match &task {
                    Some(id) => self.with_daemon(|d| {
                        d.update_task(
                            id,
                            &api::UpdateTaskReq {
                                locus: Some(locus.clone()),
                                key: Some(o.key.clone()),
                                ..Default::default()
                            },
                        )
                        .map(|r| r.map(|_| ()))
                    }),
                    None => self.with_daemon(|d| {
                        d.update_event(
                            &source,
                            &uid,
                            &api::UpdateEventReq {
                                locus: Some(locus.clone()),
                                ..Default::default()
                            },
                        )
                        .map(|r| r.map(|_| ()))
                    }),
                };
                match r {
                    Some(Ok(Ok(()))) => {
                        if task.is_none() {
                            for i in self.index.instances.iter_mut().filter(|i| i.uid == uid) {
                                i.meta.locus = o.locus.clone();
                            }
                        }
                        self.set_message(format!("✓ for {}", o.label), false);
                        self.wants_sync = true;
                        self.refresh_remote();
                    }
                    Some(Ok(Err(e))) => self.set_message(e.to_string(), true),
                    Some(Err(e)) => self.set_message(e.to_string(), true),
                    None => {}
                }
            }
            FormKind::AddTask => {
                let due = match opt(f.get("Due")).map(|d| self.when_str(&d)).transpose() {
                    Ok(d) => d,
                    Err(e) => return self.set_message(e, true),
                };
                let target = f.chosen_for().cloned();
                let default = self.cfg.default_key().map(|k| k.name.clone()).unwrap_or_default();
                let req = api::CreateTaskReq {
                    title: f.get("Title").to_string(),
                    key: target.as_ref().map_or(default, |o| o.key.clone()),
                    // How it is planned follows from the estimate (ADR-009): kairosd derives it.
                    estimate_min: dur(f.get("Estimate")).unwrap_or(0),
                    due,
                    locus: target.and_then(|o| o.locus),
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
            // The same keys as on a past block in the calendar (L4).
            Key::Char('y') => self.check_in(t, "done"),
            Key::Char('x') => self.check_in(t, "missed"),
            Key::Char('d') => self.check_in(t, "finished"),
            // Not as planned: show the block on its day, to fix with ⇧ and ⌥, then y.
            Key::Enter => {
                self.mode = Mode::Normal;
                let day = item.start.with_timezone(&self.tz).date_naive();
                self.open_day(day, Some(&item.block_uid));
            }
            Key::Char('s') | Key::Down => {
                self.mode = Mode::Checkin((i + 1).min(open.len().saturating_sub(1)));
            }
            Key::Up => self.mode = Mode::Checkin(i.saturating_sub(1)),
            Key::Esc | Key::Char('q') => {}
            _ => self.mode = Mode::Checkin(i),
        }
    }

    /// The keys that do something for what's selected, for the bottom edge, as `(key, what)`
    /// pairs drawn the locus way (L2): `e edit · m move · x remove`.
    pub fn key_hint_pairs(&self) -> Vec<(&'static str, &'static str)> {
        let mut v: Vec<(&'static str, &'static str)> = Vec::new();
        let Some(i) = self.selected().map(|c| c.primary) else {
            return vec![("+", "add")];
        };
        // A linked item opens in locus (spec K8).
        if self.locus_link(&i).is_some() {
            v.push(("⏎", "open in locus"));
        }
        let h: &[(&str, &str)] = match (i.meta.kind, i.overlay) {
            (EventKind::Deadline, _) if i.meta.task_id.is_some() => &[
                ("d", "done"),
                ("e", "edit"),
                ("m", "move"),
                ("n", "start"),
                ("x", "drop"),
            ],
            (EventKind::TaskBlock, _) if kairos_core::build::is_checked_in(&i.title) => {
                &[("⌥↑↓", "length"), ("x", "didn't happen"), ("W", "log more work")]
            }
            _ if self.awaits_checkin(&i) => &[("y", "done"), ("⌥↑↓", "length"), ("x", "missed"), ("d", "task done")],
            (EventKind::TaskBlock, _) => &[
                ("n", "start"),
                ("⌥↑↓", "length"),
                ("e", "edit"),
                ("d", "done"),
                ("x", "skip"),
            ],
            (_, Overlay::Family) if i.meta.origin_key.is_none() => {
                if i.meta.blocking {
                    &[("B", "not attending")]
                } else {
                    &[("B", "attending")]
                }
            }
            (_, Overlay::Background | Overlay::Imported) => &[],
            _ => &[("e", "edit"), ("m", "move"), ("s", "share"), ("x", "delete")],
        };
        v.extend_from_slice(h);
        v
    }

    /// The same, as text (`e edit · m move`).
    pub fn key_hints(&self) -> String {
        self.key_hint_pairs()
            .iter()
            .map(|(k, w)| format!("{k} {w}"))
            .collect::<Vec<_>>()
            .join(" · ")
    }

    /// The locus item an item belongs to (spec K8): its own link, else its task's (or the
    /// task's project's registry entry).
    pub fn locus_link(&self, i: &Instance) -> Option<String> {
        if let Some(l) = &i.meta.locus {
            return Some(l.clone());
        }
        if let Some(tid) = &i.meta.task_id
            && let Some(t) = self.tasks.iter().chain(self.done_tasks.iter()).find(|t| &t.id == tid)
            && let Some(l) = t.locus.clone().or_else(|| {
                let pid = t.project_id.as_ref()?;
                self.projects
                    .iter()
                    .find(|p| &p.project.id == pid)?
                    .project
                    .registry
                    .clone()
            })
        {
            return Some(l);
        }
        None
    }

    /// Open a day at an event, as locus asks (spec K8): the day view, the event selected.
    pub fn open_day(&mut self, day: NaiveDate, uid: Option<&str>) {
        self.view = View::Day;
        self.cursor = day;
        self.expanded = false;
        self.day_top.set(None);
        self.sel = uid
            .and_then(|u| self.day_clusters(day).iter().position(|c| c.all().any(|i| i.uid == u)))
            .unwrap_or(0);
    }

    /// Open one of locus's pages (`tasks`, `approve`, `home:p`) in the locus window.
    fn open_locus_page(&mut self, view: &str) {
        if !self.open_in_locus_view(view) {
            self.set_message("the locus window isn't running (Alt-1 opens it)", true);
        }
    }

    fn open_in_locus_view(&mut self, view: &str) -> bool {
        if locus_contract::link::OpenFile::locus().write(view).is_err() || !locus_contract::link::switch_to("locus") {
            return false;
        }
        let what = match view {
            "tasks" => "tasks",
            "approve" => "approve",
            _ => "projects",
        };
        self.set_message(format!("opened {what} in locus"), false);
        true
    }

    /// Ask the locus window to open `id`, and bring its tmux session forward.
    fn open_in_locus(&mut self, id: &str) {
        match locus_contract::link::OpenFile::locus().write(id) {
            Ok(()) => {
                let shown = locus_contract::link::switch_to("locus");
                self.set_message(
                    if shown {
                        format!("opened {id} in locus")
                    } else {
                        format!("asked locus to open {id} (its window isn't open here)")
                    },
                    false,
                );
            }
            Err(e) => self.set_message(format!("couldn't ask locus: {e}"), true),
        }
    }

    /// A task's edit form with its Project picker.
    /// The For options, the items your open tasks and recent events are for first (ADR-009).
    pub fn fors_recent_first(&self) -> Vec<ForOption> {
        let mut recent: Vec<(DateTime<Utc>, String)> = self
            .tasks
            .iter()
            .filter_map(|t| Some((t.updated_at, t.locus.clone()?)))
            .collect();
        recent.extend(self.index.instances.iter().filter_map(|i| {
            let l = i.meta.locus.clone()?;
            Some((i.start(self.tz), l))
        }));
        recent.sort_by(|a, b| b.0.cmp(&a.0));
        let mut order: Vec<String> = Vec::new();
        for (_, id) in recent {
            if !order.contains(&id) {
                order.push(id);
            }
        }
        let mut v = self.fors.clone();
        v.sort_by_key(|o| {
            o.locus
                .as_ref()
                .and_then(|l| order.iter().position(|x| x == l))
                .unwrap_or(usize::MAX)
        });
        v
    }

    /// What an item is for, as the For field shows it: its link's label, else its stream's.
    pub fn for_label(&self, locus: Option<&str>, key: Option<&str>) -> String {
        if let Some(l) = locus
            && let Some(o) = self.fors.iter().find(|o| o.locus.as_deref() == Some(l))
        {
            return o.label.clone();
        }
        key.and_then(|k| {
            self.fors
                .iter()
                .find(|o| o.locus.is_none() && o.key.eq_ignore_ascii_case(k))
        })
        .map(|o| o.label.clone())
        .unwrap_or_default()
    }

    fn task_form(&self, t: &kairos_core::task::Task) -> Form {
        let label = self.for_label(t.locus.as_deref(), Some(&t.key));
        Form::edit_task(self.key_names(), t, self.tz, &label).with_fors(self.fors_recent_first())
    }

    /// Mark a task done from its block or ⚑: its blocks still to come go at once.
    fn complete_task_now(&mut self, id: &str, title: &str) {
        let Some(r) = self.with_daemon(|d| d.complete_task(id, None)) else {
            return;
        };
        match r {
            Ok(Ok(r)) => {
                if r.changesets.first().is_some_and(|w| w.status == Status::Applied) {
                    let now = self.now;
                    let gone: Vec<String> = self
                        .index
                        .instances
                        .iter()
                        .filter(|i| i.meta.task_id.as_deref() == Some(id) && i.start(self.tz) >= now)
                        .map(|i| i.uid.clone())
                        .collect();
                    for uid in gone {
                        self.patch_local(&uid, |_| false);
                    }
                }
                self.set_message(format!("✓ done: {title}"), false);
                self.wants_sync = true;
                self.refresh_remote();
            }
            Ok(Err(e)) => self.set_message(e.to_string(), true),
            Err(e) => self.set_message(e.to_string(), true),
        }
    }

    /// A task's edit form, as locus's Tasks page asks for it (`e` there; L7: one task list,
    /// locus's, and one edit form, kairos's).
    pub fn open_edit_task(&mut self, id: &str) {
        let Some(t) = self
            .tasks
            .iter()
            .chain(self.done_tasks.iter())
            .find(|t| t.id == id)
            .cloned()
        else {
            return self.set_message("that task isn't here yet: r refreshes", true);
        };
        self.message = None;
        self.mode = Mode::Form(self.task_form(&t));
    }

    /// The add-task form with For filled from a registry id or stream (locus's `+`).
    pub fn open_add_task_for(&mut self, for_id: Option<&str>) {
        self.open_add(true);
        let label = for_id.and_then(|id| {
            self.fors
                .iter()
                .find(|o| o.locus.as_deref() == Some(id) || o.id == id)
                .map(|o| o.label.clone())
        });
        if let Mode::Form(f) = &mut self.mode {
            if let Some(l) = label
                && let Some(fld) = f.fields.iter_mut().find(|x| x.label == "For")
            {
                fld.value = l;
            }
            f.focus = 0;
        }
    }

    /// Open the add form: a task, or an event (prefilled with the cursor day).
    fn open_add(&mut self, task: bool) {
        let keys = self.key_names();
        self.message = None;
        self.mode = Mode::Form(if task {
            Form::add_task(keys).with_fors(self.fors_recent_first())
        } else {
            Form::add_event(keys, when_prefill(self.cursor, self.today())).with_fors(self.fors_recent_first())
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

    /// What a check-in needs about a block (from the C list's data if it's
    /// there, else from the block and its task).
    /// A work block that has started, checked in or not: y x d answer it (again).
    pub fn checkable(&self, i: &Instance) -> bool {
        i.meta.kind == EventKind::TaskBlock && i.start(self.tz) <= self.now
    }

    /// A work block that has started and isn't checked in yet.
    pub fn awaits_checkin(&self, i: &Instance) -> bool {
        if kairos_core::build::is_checked_in(&i.title) || i.start(self.tz) > self.now {
            return false;
        }
        i.meta.kind == EventKind::TaskBlock
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
        }
    }

    /// How many blocks are waiting for a check-in (the status bar's nudge).
    pub fn to_check_in(&self) -> usize {
        self.checkins.iter().filter(|c| c.outcome.is_none()).count()
    }

    /// Check a block in: done (as drawn) / missed / finished (done, and the task is
    /// finished). Coursework, and a block that uses up the task, also ask how much is left.
    pub fn check_in(&mut self, t: CheckinTarget, outcome: &str) {
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
    ShiftUp,
    ShiftDown,
    AltUp,
    AltDown,
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
    matches!(i.overlay, Overlay::Imported | Overlay::Background) || i.tentative || i.meta.not_going
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
            meta: kairos_core::model::Meta {
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
}
