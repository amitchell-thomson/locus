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
    Add,
    Edit,
    Move,
    Rekey,
    Search,
    TaskAdd,
}

impl PromptKind {
    pub fn label(&self) -> &'static str {
        match self {
            PromptKind::Add => "add (title @ when [#key] [for 90m])",
            PromptKind::Edit => "rename",
            PromptKind::Move => "move to (when)",
            PromptKind::Rekey => "key",
            PromptKind::Search => "/",
            PromptKind::TaskAdd => "task (title #key ~4h [due fri 17:00])",
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

pub struct App {
    pub cfg: Config,
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
    pub daemon: Option<Daemon>,
    pub daemon_error: Option<String>,
    pub quit: bool,
    /// Set after a write so the runner kicks a sync + reload.
    pub wants_sync: bool,
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
            daemon,
            daemon_error: None,
            quit: false,
            wants_sync: false,
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

    pub fn day_clusters(&self, d: NaiveDate) -> Vec<Cluster> {
        let mut v: Vec<Cluster> = self
            .clusters(midnight(d, self.tz), midnight(d + Duration::days(1), self.tz))
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
    }

    fn write_result(&mut self, r: Result<Result<WriteResult, Rejected>, kairos_store::StoreError>) {
        match r {
            Ok(Ok(w)) => {
                let msg = match w.status {
                    Status::Applied => format!("✓ {}", w.summary.lines().next().unwrap_or("")),
                    Status::Pending => format!(
                        "⏳ needs approval: {} (A to review)",
                        w.summary.lines().next().unwrap_or("")
                    ),
                    s => format!("{} {}", s.as_str(), w.summary),
                };
                self.set_message(msg, false);
                self.wants_sync = true;
                self.refresh_remote();
            }
            Ok(Err(e)) => self.set_message(e.to_string().replace('\n', " · "), true),
            Err(e) => self.set_message(e.to_string(), true),
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
                self.mode = Mode::Prompt(
                    if self.view == View::Tasks {
                        PromptKind::TaskAdd
                    } else {
                        PromptKind::Add
                    },
                    String::new(),
                )
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
                                Ok(Ok(_)) => {
                                    self.set_message(format!("✓ done: {}", t.title), false);
                                    self.wants_sync = true;
                                    self.refresh_remote();
                                }
                                Ok(Err(e)) => self.set_message(e.to_string(), true),
                                Err(e) => self.set_message(e.to_string(), true),
                            }
                        }
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
                match k {
                    Key::Char('e') => self.mode = Mode::Prompt(PromptKind::Edit, i.title.clone()),
                    Key::Char('m') => self.mode = Mode::Prompt(PromptKind::Move, String::new()),
                    Key::Char('K') => self.mode = Mode::Prompt(PromptKind::Rekey, String::new()),
                    Key::Char('d') => {
                        self.mode = Mode::Confirm(Confirm::Delete);
                        self.set_message(format!("delete “{}”? y/N", i.title), false);
                    }
                    Key::Char('B') if i.overlay == Overlay::Family => {
                        let req = api::UpdateEventReq {
                            blocking: Some(!i.meta.blocking),
                            ..Default::default()
                        };
                        if let Some(r) = self.with_daemon(|d| d.update_event(&i.source, &i.uid, &req)) {
                            match r {
                                Ok(Ok(_)) => {
                                    let msg = if i.meta.blocking {
                                        "no longer blocks your time"
                                    } else {
                                        "now blocks your time for scheduling"
                                    };
                                    self.set_message(msg, false);
                                    self.wants_sync = true;
                                }
                                Ok(Err(e)) => self.set_message(e.to_string(), true),
                                Err(e) => self.set_message(e.to_string(), true),
                            }
                        }
                    }
                    Key::Char('s') => {
                        self.mode = Mode::Confirm(Confirm::Share);
                        self.set_message(
                            format!("share “{}” to Family (moves it; needs approval)? y/N", i.title),
                            false,
                        );
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
                    let short = p
                        .shortfalls
                        .first()
                        .map(|s| format!(" · ! {}", s.message))
                        .unwrap_or_default();
                    self.set_message(
                        format!(
                            "plan: +{} −{} ({} moved){short}",
                            p.created.len(),
                            p.deleted.len(),
                            p.moved
                        ),
                        !p.shortfalls.is_empty(),
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
                    if let Some(r) = self.with_daemon(|d| d.delete_event(&i.source, &i.uid)) {
                        self.write_result(r);
                    }
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
                if let Some(r) = self.with_daemon(|d| d.drop_task(&id)) {
                    match r {
                        Ok(Ok(t)) => {
                            if let Some(w) = t.changesets.into_iter().next() {
                                self.write_result(Ok(Ok(w)));
                            }
                        }
                        Ok(Err(e)) => self.set_message(e.to_string(), true),
                        Err(e) => self.set_message(e.to_string(), true),
                    }
                }
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
        if buf.is_empty() && kind != PromptKind::Search {
            return;
        }
        match kind {
            PromptKind::Search => {
                self.search = if buf.is_empty() { None } else { Some(buf) };
            }
            PromptKind::Add => match parse_add(&buf) {
                Some((title, when, key, dur)) => match self.when_str(&when) {
                    Ok(start) => {
                        let req = api::CreateEventReq {
                            title,
                            start,
                            duration_min: dur,
                            key,
                            ..Default::default()
                        };
                        if let Some(r) = self.with_daemon(|d| d.create_event(&req)) {
                            self.write_result(r);
                        }
                    }
                    Err(e) => self.set_message(e, true),
                },
                None => self.set_message("format: title @ when [#key] [for 90m]", true),
            },
            PromptKind::TaskAdd => match parse_task(&buf) {
                Some((title, key, est, due)) => {
                    let due = match due.map(|d| self.when_str(&d)).transpose() {
                        Ok(d) => d,
                        Err(e) => return self.set_message(e, true),
                    };
                    let req = api::CreateTaskReq {
                        title,
                        key,
                        estimate_min: est,
                        due,
                        ..Default::default()
                    };
                    if let Some(r) = self.with_daemon(|d| d.create_task(&req)) {
                        match r {
                            Ok(Ok(t)) => {
                                let short = t
                                    .plan
                                    .as_ref()
                                    .and_then(|p| p.shortfalls.first())
                                    .map(|s| format!(" · ! {}", s.message));
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
                None => self.set_message("format: title #key ~4h [due fri 17:00]", true),
            },
            PromptKind::Edit | PromptKind::Move | PromptKind::Rekey => {
                let Some(cl) = self.selected() else { return };
                let i = cl.primary;
                let r = match kind {
                    PromptKind::Rekey => self.with_daemon(|d| d.move_event(&i.source, &i.uid, &buf)),
                    PromptKind::Edit => {
                        let req = api::UpdateEventReq {
                            title: Some(buf.clone()),
                            ..Default::default()
                        };
                        self.with_daemon(|d| d.update_event(&i.source, &i.uid, &req).map(|r| r.map(to_write)))
                    }
                    _ => match self.when_str(&buf) {
                        Ok(start) => {
                            let req = api::UpdateEventReq {
                                start: Some(start),
                                ..Default::default()
                            };
                            self.with_daemon(|d| d.update_event(&i.source, &i.uid, &req).map(|r| r.map(to_write)))
                        }
                        Err(e) => return self.set_message(e, true),
                    },
                };
                if let Some(r) = r {
                    self.write_result(r);
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
        let req = api::CheckinReq {
            block_uid: item.block_uid.clone(),
            outcome: outcome.into(),
            actual_min: None,
        };
        if let Some(r) = self.with_daemon(|d| d.checkin(&req)) {
            match r {
                Ok(Ok(_)) => {
                    self.set_message(format!("{}: {outcome}", item.title), false);
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
    Backspace,
    Up,
    Down,
    Left,
    Right,
}

/// What overlays/keys look like in the status bar.
pub fn overlay_flags(app: &App) -> Vec<(String, bool, kairos_core::colour::Rgb)> {
    let mut v = vec![
        (
            "P".to_string(),
            app.filter.personal,
            kairos_core::colour::Rgb::mocha("lavender"),
        ),
        ("F".to_string(), app.filter.family, app.cfg.overlays.family_colour),
        (
            "b".to_string(),
            app.filter.background,
            app.cfg.overlays.background_colour,
        ),
        ("i".to_string(), app.filter.imported, app.cfg.overlays.imported_colour),
    ];
    let mut keys = app.cfg.keys.clone();
    keys.sort_by_key(|k| k.index);
    for k in keys {
        v.push((
            k.index.to_string(),
            !app.filter.hidden_keys.contains(&k.name.to_lowercase()),
            k.colour,
        ));
    }
    v
}

pub fn is_dimmed(i: &Instance) -> bool {
    matches!(i.overlay, Overlay::Imported | Overlay::Background) || i.tentative
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
