//! The write engine: turns intents into changesets, asks the risk policy,
//! applies / approves / rejects / undoes, and runs the scheduler.
//!
//! Invariants:
//! - kairosd is the only writer to iCloud; every write is a changeset with
//!   full pre/post images, logged in SQLite.
//! - Risk comes from [`kairos_core::risk::assess`] over facts gathered here;
//!   callers can't set it.
//! - With `live = false`, new events are redirected into `test_calendar` and
//!   anything touching another calendar is refused.
//! - Undo refuses if an item changed since (semantic comparison, so iCloud's
//!   cosmetic rewrites don't block it) and shows a diff.

use chrono::{DateTime, Duration, NaiveTime, Utc};
use chrono_tz::Tz;
use kairos_core::api::{self, TimeIn};
use kairos_core::build::{self, Edit, NewEvent, NewTime};
use kairos_core::changeset::{self, Changeset, Item, Op, Risk, Status, WriteResult};
use kairos_core::config::{Config, Overlay, Source, SourceKind};
use kairos_core::constraints::Constraints;
use kairos_core::ics;
use kairos_core::model::{Author, EventKind, EventSeries, Instance, Meta, Span};
use kairos_core::normalize::series_from_calendars;
use kairos_core::project::{self, Project, ProjectStatus};
use kairos_core::risk::{self, Action, Assessment, CalendarFacts, EventFacts, OpFacts};
use kairos_core::sched::{self, Block, Busy, PlanInput};
use kairos_core::task::{Due, Task, TaskStatus};
use kairos_core::view::LoadedSource;
use kairos_store::db::{Checkin, Db, EstimateRow, Outcome};
use kairos_store::load::{self, Loaded};
use kairos_store::vdir::{self, Collection};
use kairos_store::{StoreError, cache};
use std::path::PathBuf;
use std::sync::Arc;

pub type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("forbidden: {}", .0.join("; "))]
    Forbidden(Vec<String>),
    #[error("{0}")]
    LiveDisabled(String),
    #[error("conflict: {message}")]
    Conflict { message: String, diff: String },
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<kairos_core::api::TimeParseError> for EngineError {
    fn from(e: kairos_core::api::TimeParseError) -> Self {
        EngineError::BadRequest(e.0)
    }
}

pub type Result<T> = std::result::Result<T, EngineError>;

/// Upper bound on one batch (a term of lectures is ~200).
pub const MAX_BATCH: usize = 1000;

fn check_batch(n: usize) -> Result<()> {
    if n > MAX_BATCH {
        return Err(EngineError::BadRequest(format!(
            "{n} events in one batch; split it (max {MAX_BATCH})"
        )));
    }
    Ok(())
}

/// A collection to push after a write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncTarget {
    /// vdirsyncer `pair/collection`.
    pub name: String,
    /// The write removed the collection's last item. vdirsyncer refuses to
    /// sync a "completely emptied" storage unless told the emptying was
    /// deliberate; it was, because kairos just did it.
    pub emptied: bool,
}

/// Runs vdirsyncer after writes. Tests use a recording fake.
pub trait Syncer: Send {
    fn sync_collections(&self, collections: &[SyncTarget]);
}

/// Sends approval requests etc. Tests use a recording fake.
pub trait Notifier: Send {
    fn pending(&self, cs: &Changeset);
}

pub struct ShellSyncer {
    pub cmd: String,
}

impl Syncer for ShellSyncer {
    fn sync_collections(&self, collections: &[SyncTarget]) {
        if self.cmd.trim().is_empty() {
            return;
        }
        // Order matters for moves: the source collection (delete) first.
        let cmds: Vec<String> = collections.iter().map(|t| collection_cmd(&self.cmd, t)).collect();
        std::thread::spawn(move || {
            for c in cmds {
                match std::process::Command::new("sh").arg("-c").arg(&c).output() {
                    Ok(o) if o.status.success() => tracing::info!(cmd = %c, "post-write sync ok"),
                    Ok(o) => {
                        tracing::warn!(cmd = %c, stderr = %String::from_utf8_lossy(&o.stderr), "post-write sync failed")
                    }
                    Err(e) => tracing::warn!(cmd = %c, "post-write sync: {e}"),
                }
            }
        });
    }
}

/// `{collection}` → `pair/coll`, prefixed with `--force-delete` when the
/// write emptied it (scoped to that one collection).
pub fn collection_cmd(template: &str, t: &SyncTarget) -> String {
    let arg = if t.emptied {
        format!("--force-delete {}", t.name)
    } else {
        t.name.clone()
    };
    template.replace("{collection}", &arg)
}

pub struct NoNotifier;
impl Notifier for NoNotifier {
    fn pending(&self, _cs: &Changeset) {}
}

pub struct Engine {
    pub cfg: Config,
    pub cfg_path: Option<PathBuf>,
    pub constraints: Constraints,
    pub db: Db,
    pub clock: Clock,
    pub syncer: Box<dyn Syncer>,
    /// The current request comes from Alec at a keyboard (set per request by
    /// the HTTP layer; never by MCP). His own edits then skip approval.
    pub direct: bool,
    pub notifier: Box<dyn Notifier>,
    /// An event changed under the plan: replan as soon as the current
    /// request is done (see [`Engine::replan_if_needed`]).
    pub replan_soon: bool,
    /// The locus registry (spec K7), read from `taxonomy/registry.toml`: kairos's projects are
    /// its repos. `None` (tests, or the file unreadable) leaves projects as kairos keeps them.
    pub registry: Option<locus_contract::Registry>,
}

/// A loaded snapshot of all calendars.
pub struct World {
    pub loaded: Loaded,
    pub cols: Vec<Collection>,
    /// Instances from 7 days ago to `horizon` ahead (all overlays).
    pub instances: Vec<Instance>,
}

/// An existing event, found by UID.
pub struct Located {
    pub source: Source,
    pub dir: PathBuf,
    pub href: String,
    pub text: String,
    pub series: EventSeries,
}

fn cal_facts(s: &Source) -> CalendarFacts {
    CalendarFacts {
        id: s.id.clone(),
        overlay: s.overlay,
        writable: s.writable,
        readonly_feed: s.kind == SourceKind::IcsReadonly,
    }
}

/// Semantic lines of an item, ignoring stamps and vendor properties, for
/// "changed since?" checks and diffs.
pub fn semantic(text: &str) -> Vec<String> {
    const KEEP: &[&str] = &[
        "UID",
        "RECURRENCE-ID",
        "DTSTART",
        "DTEND",
        "DURATION",
        "SUMMARY",
        "LOCATION",
        "DESCRIPTION",
        "RRULE",
        "RDATE",
        "EXDATE",
        "STATUS",
        "TRANSP",
    ];
    let Ok(cals) = ics::parse(text) else {
        return vec![format!("<unparseable: {} bytes>", text.len())];
    };
    let mut out = Vec::new();
    for cal in &cals {
        let mut evs: Vec<&ics::Component> = cal.children_named("VEVENT").collect();
        evs.sort_by_key(|e| {
            (
                e.value("UID").unwrap_or("").to_string(),
                e.value("RECURRENCE-ID").unwrap_or("").to_string(),
            )
        });
        for ev in evs {
            out.push("VEVENT".into());
            for k in KEEP {
                for p in ev.props_named(k) {
                    let tz = p.param("TZID").map(|t| format!(";TZID={t}")).unwrap_or_default();
                    out.push(format!("  {k}{tz}:{}", p.value));
                }
            }
        }
    }
    out
}

fn diff(a: &[String], b: &[String]) -> String {
    let mut s = String::new();
    for l in a {
        if !b.contains(l) {
            s.push_str(&format!("- {l}\n"));
        }
    }
    for l in b {
        if !a.contains(l) {
            s.push_str(&format!("+ {l}\n"));
        }
    }
    s
}

fn first_series(text: &str, source: &str) -> Result<EventSeries> {
    let cals = ics::parse(text).map_err(|e| EngineError::BadRequest(e.to_string()))?;
    let mut w = Vec::new();
    series_from_calendars(&cals, source, None, &mut w)
        .into_iter()
        .next()
        .ok_or_else(|| EngineError::BadRequest("item has no VEVENT".into()))
}

impl Engine {
    pub fn now(&self) -> DateTime<Utc> {
        (self.clock)()
    }

    pub fn tz(&self) -> Tz {
        self.cfg.tz()
    }

    fn author_meta(&self, author: Author) -> Meta {
        Meta {
            created_by: Some(author),
            ..Default::default()
        }
    }

    // ------------------------------------------------------------ world

    pub fn world(&self, horizon_days: i64) -> Result<World> {
        let sidecar = self.db.meta_all()?;
        let loaded = load::load_all(&self.cfg, &sidecar)?;
        let cols = vdir::collections(&self.cfg.vdir())?;
        let now = self.now();
        let mut w = Vec::new();
        // Back far enough to see every block still waiting for a check-in.
        let back = i64::from(self.cfg.notify.autodone_after_days.max(6)) + 1;
        let instances = kairos_core::view::instances(
            &loaded.sources,
            now - Duration::days(back),
            now + Duration::days(horizon_days.max(1)),
            self.tz(),
            &mut w,
        );
        Ok(World {
            loaded,
            cols,
            instances,
        })
    }

    /// The instance index for `[from, to)`, as clients draw it.
    pub fn index(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<cache::Index> {
        let mut idx = cache::build(&self.cfg, None, &self.db.meta_all()?, from, to)?;
        idx.add_due_marks(&self.db.tasks(Some(TaskStatus::Open))?, self.tz());
        let worked = self.worked_instances(from, to)?;
        if !worked.is_empty() {
            idx.instances.extend(worked);
            let tz = self.tz();
            idx.instances
                .sort_by(|a, b| a.start(tz).cmp(&b.start(tz)).then_with(|| a.title.cmp(&b.title)));
        }
        Ok(idx)
    }

    /// Store one machine's measured spans for a window (spec K6), replacing what it sent for
    /// that window before. Kept in kairos's own store: never a calendar write.
    pub fn set_worked(&mut self, req: &api::WorkedReq) -> Result<usize> {
        let k = format!("worked.{}", req.machine);
        let mut v: Vec<api::WorkedSpan> = self
            .db
            .kv_get(&k)?
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let keep = self.now() - Duration::days(kairos_store::worked::KEEP_DAYS);
        v.retain(|s| s.end >= keep && (s.end <= req.from || s.start >= req.to));
        v.extend(req.spans.iter().filter(|s| s.end > s.start).cloned());
        v.sort_by_key(|s| s.start);
        let text = serde_json::to_string(&v).map_err(|e| EngineError::BadRequest(e.to_string()))?;
        self.db.kv_set(&k, &text)?;
        Ok(req.spans.len())
    }

    /// Every machine's spans overlapping `[from, to)`, merged per project.
    pub fn worked(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<Vec<api::WorkedSpan>> {
        let mut all = Vec::new();
        for m in ["mac", "server"] {
            if let Some(t) = self.db.kv_get(&format!("worked.{m}"))?
                && let Ok(v) = serde_json::from_str::<Vec<api::WorkedSpan>>(&t)
            {
                all.extend(v.into_iter().filter(|s| s.start < to && s.end > from));
            }
        }
        Ok(kairos_store::worked::merge(all))
    }

    /// Worked spans as faint, non-blocking instances in their project's area colour.
    fn worked_instances(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<Vec<Instance>> {
        let spans = self.worked(from, to)?;
        if spans.is_empty() {
            return Ok(Vec::new());
        }
        let reg = kairos_store::taxonomy::local().ok().map(|s| s.registry);
        Ok(spans
            .into_iter()
            .map(|s| {
                let entry = reg.as_ref().and_then(|r| r.get(&s.project));
                let key = entry
                    .and_then(|p| self.cfg.key(&p.area))
                    .or_else(|| self.cfg.key("Projects"))
                    .map(|k| k.name.clone());
                Instance {
                    uid: format!("worked-{}-{}", s.project, s.start.timestamp()),
                    recurrence_id: None,
                    source: "worked".into(),
                    overlay: Overlay::Personal,
                    key,
                    title: entry.map_or_else(|| s.project.clone(), |p| p.name.clone()),
                    location: None,
                    description: Some("measured from your Claude sessions".into()),
                    span: Span::Timed {
                        start: s.start,
                        end: s.end,
                    },
                    recurring: false,
                    transparent: true,
                    tentative: false,
                    meta: Meta {
                        kind: EventKind::Worked,
                        activity: s.activity,
                        ..Meta::default()
                    },
                    href: None,
                    alarms: Vec::new(),
                }
            })
            .collect())
    }

    fn source(&self, id: &str) -> Result<Source> {
        self.cfg
            .source(id)
            .cloned()
            .ok_or_else(|| EngineError::NotFound(format!("source {id:?}")))
    }

    fn test_source(&self) -> Option<&Source> {
        self.cfg.sources.iter().find(|s| {
            s.kind == SourceKind::Icloud
                && (s.calendar.as_deref() == Some(self.cfg.test_calendar.as_str())
                    || s.path.as_deref() == Some(self.cfg.test_calendar.as_str()))
        })
    }

    fn is_test(&self, source_id: &str) -> bool {
        self.test_source().is_some_and(|t| t.id == source_id)
    }

    /// Calendar new events for `key` go into (redirected while not live).
    fn target_for_key(&self, key: &str) -> Result<(Source, Option<String>)> {
        let k = self
            .cfg
            .key(key)
            .ok_or_else(|| EngineError::BadRequest(format!("unknown key {key:?}")))?;
        if !self.cfg.live {
            let t = self.test_source().ok_or_else(|| {
                EngineError::LiveDisabled(format!(
                    "live = false and no source for test calendar {:?} is configured",
                    self.cfg.test_calendar
                ))
            })?;
            return Ok((t.clone(), Some(k.name.clone())));
        }
        let s = self
            .cfg
            .source_for_key(&k.name)
            .ok_or_else(|| EngineError::BadRequest(format!("no writable calendar for key {:?}", k.name)))?;
        Ok((s.clone(), None))
    }

    fn dir_of(&self, source: &Source, cols: &[Collection]) -> Result<PathBuf> {
        Ok(vdir::source_path(&self.cfg, source, cols)?)
    }

    /// Find an event by UID (optionally within one source).
    pub fn locate(&self, world: &World, source: Option<&str>, uid: &str) -> Result<Located> {
        let hits: Vec<(&LoadedSource, &EventSeries)> = world
            .loaded
            .sources
            .iter()
            .filter(|s| source.is_none_or(|x| x == s.id))
            .flat_map(|s| s.series.iter().filter(|e| e.uid == uid).map(move |e| (s, e)))
            .collect();
        // A client's view can lag a move it just made (the Mac pulls from iCloud
        // after the server pushes). The UID still identifies the event.
        let hits = if hits.is_empty() && source.is_some() {
            world
                .loaded
                .sources
                .iter()
                .flat_map(|s| s.series.iter().filter(|e| e.uid == uid).map(move |e| (s, e)))
                .collect()
        } else {
            hits
        };
        let (ls, series) = match hits.as_slice() {
            [] => return Err(EngineError::NotFound(format!("event {uid:?}"))),
            [one] => *one,
            many => {
                // Prefer a writable iCloud copy.
                *many
                    .iter()
                    .find(|(s, _)| s.kind == SourceKind::Icloud)
                    .ok_or_else(|| EngineError::BadRequest(format!("{uid:?} is in several sources; pass source")))?
            }
        };
        let src = self.source(&ls.id)?;
        let dir = self.dir_of(&src, &world.cols)?;
        let href = series
            .href
            .clone()
            .ok_or_else(|| EngineError::Forbidden(vec![format!("{} is read-only", src.id)]))?;
        let text = vdir::read_item(&dir, &href)?;
        Ok(Located {
            source: src,
            dir,
            href,
            text,
            series: series.clone(),
        })
    }

    fn event_facts(&self, series: &EventSeries) -> Option<EventFacts> {
        let m = series.master.as_ref().or(series.overrides.first())?;
        let side = self
            .db
            .meta_get(&series.source, &series.uid)
            .ok()
            .flatten()
            .unwrap_or_default();
        let meta = m.meta.merged_with(&side);
        let tz = self.tz();
        Some(EventFacts {
            kind: meta.kind,
            created_by: meta.created_by,
            flex: meta.flex,
            recurring: series.is_recurring(),
            start: m.start.instant(tz),
            end: m.end.instant(tz),
            project_session: meta.project_id.is_some(),
        })
    }

    fn facts_from_text(&self, text: &str, source: &str) -> Result<Option<EventFacts>> {
        let s = first_series(text, source)?;
        Ok(self.event_facts(&s))
    }

    /// Does `[start, end)` overlap a fixed (opaque, non-flex, blocking) event?
    fn overlaps_fixed(&self, world: &World, uid: &str, ef: &EventFacts) -> bool {
        if ef.kind == EventKind::Deadline {
            return false;
        }
        let tz = self.tz();
        world.instances.iter().any(|i| {
            i.uid != uid && is_busy(i) && {
                let (s, e) = i.span.bounds(tz);
                s < ef.end && ef.start < e && ef.start != ef.end
            }
        })
    }

    // ------------------------------------------------------------ propose / apply

    /// Is this event the ⚑ of a task that's already done or dropped?
    fn finished_task_deadline(&self, text: &str, source: &str) -> Result<bool> {
        let Ok(s) = first_series(text, source) else {
            return Ok(false);
        };
        let Some(m) = s.master.as_ref() else { return Ok(false) };
        if m.meta.kind != EventKind::Deadline {
            return Ok(false);
        }
        Ok(match m.meta.task_id.as_deref() {
            Some(id) => self.db.task_get(id)?.is_some_and(|t| t.status != TaskStatus::Open),
            None => false,
        })
    }

    fn op_facts(&self, world: &World, op: &Op) -> Result<OpFacts> {
        let src = |id: &str| -> Result<CalendarFacts> { Ok(cal_facts(&self.source(id)?)) };
        Ok(match op {
            Op::CreateEvent { item, uid, after } => {
                let after_f = self.facts_from_text(after, &item.source)?;
                OpFacts {
                    action: Action::Create,
                    target: Some(src(&item.source)?),
                    origin: None,
                    overlaps_fixed: after_f.as_ref().is_some_and(|f| self.overlaps_fixed(world, uid, f)),
                    before: None,
                    after: after_f,
                    finished_task_deadline: false,
                }
            }
            Op::UpdateEvent {
                item,
                uid,
                before,
                after,
            } => {
                let after_f = self.facts_from_text(after, &item.source)?;
                OpFacts {
                    action: Action::Update,
                    target: Some(src(&item.source)?),
                    origin: None,
                    overlaps_fixed: after_f.as_ref().is_some_and(|f| self.overlaps_fixed(world, uid, f)),
                    before: self.facts_from_text(before, &item.source)?,
                    after: after_f,
                    finished_task_deadline: false,
                }
            }
            Op::DeleteEvent { item, before, .. } => OpFacts {
                action: Action::Delete,
                target: Some(src(&item.source)?),
                origin: None,
                overlaps_fixed: false,
                finished_task_deadline: self.finished_task_deadline(before, &item.source)?,
                before: self.facts_from_text(before, &item.source)?,
                after: None,
            },
            Op::MoveEvent {
                from,
                to,
                before,
                after,
                share_to_family,
                ..
            } => OpFacts {
                action: if *share_to_family {
                    Action::ShareToFamily
                } else {
                    Action::Move
                },
                target: Some(src(&to.source)?),
                origin: Some(src(&from.source)?),
                overlaps_fixed: false,
                before: self.facts_from_text(before, &from.source)?,
                after: self.facts_from_text(after, &from.source)?,
                finished_task_deadline: false,
            },
            Op::CreateTask { .. } => OpFacts {
                action: Action::TaskCreate,
                target: None,
                origin: None,
                before: None,
                after: None,
                overlaps_fixed: false,
                finished_task_deadline: false,
            },
            Op::UpdateTask { before, after } => OpFacts {
                action: match (before.status, after.status) {
                    (TaskStatus::Open, TaskStatus::Dropped) => Action::TaskDrop,
                    (_, TaskStatus::Done) => Action::TaskComplete,
                    _ => Action::TaskUpdate,
                },
                target: None,
                origin: None,
                before: None,
                after: None,
                overlaps_fixed: false,
                finished_task_deadline: false,
            },
        })
    }

    fn live_guard(&self, ops: &[Op]) -> Result<()> {
        if self.cfg.live {
            return Ok(());
        }
        for op in ops {
            for it in op.items() {
                if !self.is_test(&it.source) {
                    return Err(EngineError::LiveDisabled(format!(
                        "live = false: kairos only writes to {:?} until you set live = true (refused: {})",
                        self.cfg.test_calendar,
                        op.describe()
                    )));
                }
            }
        }
        Ok(())
    }

    /// Assess and record a changeset; apply it if low risk.
    pub fn propose(&mut self, world: &World, author: Author, ops: Vec<Op>, summary: String) -> Result<Changeset> {
        let facts = ops
            .iter()
            .map(|o| self.op_facts(world, o))
            .collect::<Result<Vec<_>>>()?;
        let now = self.now();
        let assessed = if self.direct && author == Author::Alec {
            risk::assess_direct(&facts)
        } else {
            risk::assess(&facts, now)
        };
        let (risk, reasons) = match assessed {
            Assessment::Forbidden { reasons } => return Err(EngineError::Forbidden(reasons)),
            Assessment::Allowed { risk, reasons } => (risk, reasons),
        };
        self.live_guard(&ops)?;
        let mut cs = Changeset {
            id: changeset::new_id(),
            author,
            created_at: now,
            ops,
            risk,
            reasons,
            status: Status::Pending,
            summary,
            error: None,
            decided_at: None,
        };
        self.db.cs_put(&cs)?;
        if cs.risk == Risk::Low {
            self.apply(&mut cs)?;
        } else {
            tracing::info!(id = %cs.id, reasons = ?cs.reasons, "changeset pending approval");
            self.notifier.pending(&cs);
        }
        Ok(cs)
    }

    /// Check preconditions for every op, then execute all.
    fn apply(&mut self, cs: &mut Changeset) -> Result<()> {
        if let Err(e) = self.live_guard(&cs.ops) {
            cs.status = Status::Failed;
            cs.error = Some(e.to_string());
            self.db.cs_put(cs)?;
            return Err(e);
        }
        let cols = vdir::collections(&self.cfg.vdir())?;
        let dir = |s: &str| -> Result<PathBuf> { self.dir_of(&self.source(s)?, &cols) };
        // Preconditions: items are as the changeset expects.
        for op in &cs.ops {
            let check = |item: &Item, expect: Option<&str>| -> Result<()> {
                let cur = vdir::read_item(&dir(&item.source)?, &item.href).ok();
                match (expect, cur) {
                    (None, None) => Ok(()),
                    (None, Some(_)) => Err(EngineError::Conflict {
                        message: format!("{} already exists in {}", item.href, item.source),
                        diff: String::new(),
                    }),
                    (Some(_), None) => Err(EngineError::Conflict {
                        message: format!("{} no longer exists in {}", item.href, item.source),
                        diff: String::new(),
                    }),
                    (Some(e), Some(c)) => {
                        let (se, sc) = (semantic(e), semantic(&c));
                        if se == sc {
                            Ok(())
                        } else {
                            Err(EngineError::Conflict {
                                message: format!(
                                    "{} changed in {} since this change was proposed",
                                    item.href, item.source
                                ),
                                diff: diff(&se, &sc),
                            })
                        }
                    }
                }
            };
            let r = match op {
                Op::CreateEvent { item, .. } => check(item, None),
                Op::UpdateEvent { item, before, .. } | Op::DeleteEvent { item, before, .. } => {
                    check(item, Some(before))
                }
                Op::MoveEvent { from, to, before, .. } => check(from, Some(before)).and_then(|_| check(to, None)),
                _ => Ok(()),
            };
            if let Err(e) = r {
                cs.status = Status::Failed;
                cs.error = Some(e.to_string());
                self.db.cs_put(cs)?;
                return Err(e);
            }
        }
        let mut touched: Vec<String> = Vec::new();
        for op in &cs.ops {
            match op {
                Op::CreateEvent { item, uid, after } | Op::UpdateEvent { item, uid, after, .. } => {
                    vdir::write_item(&dir(&item.source)?, &item.href, after)?;
                    self.store_meta(&item.source, uid, after)?;
                }
                Op::DeleteEvent { item, uid, .. } => {
                    vdir::delete_item(&dir(&item.source)?, &item.href)?;
                    self.db.meta_delete(&item.source, uid)?;
                }
                Op::MoveEvent {
                    from, to, uid, after, ..
                } => {
                    vdir::delete_item(&dir(&from.source)?, &from.href)?;
                    self.db.meta_delete(&from.source, uid)?;
                    vdir::write_item(&dir(&to.source)?, &to.href, after)?;
                    self.store_meta(&to.source, uid, after)?;
                }
                Op::CreateTask { after } => self.db.task_put(after)?,
                Op::UpdateTask { after, .. } => self.db.task_put(after)?,
            }
            for it in op.items() {
                touched.push(it.source.clone());
            }
        }
        cs.status = Status::Applied;
        cs.decided_at = Some(self.now());
        self.db.cs_put(cs)?;
        self.sync_after(&touched, &cols);
        // Someone else's event change: work blocks may need to move.
        if cs.author != Author::Scheduler
            && cs.ops.iter().any(|o| {
                matches!(
                    o,
                    Op::CreateEvent { .. } | Op::UpdateEvent { .. } | Op::DeleteEvent { .. } | Op::MoveEvent { .. }
                )
            })
        {
            self.replan_soon = true;
        }
        Ok(())
    }

    /// Replan if an event changed since the last plan (after a write, after
    /// a sync). Stable: work only moves where it has to.
    pub fn replan_if_needed(&mut self) -> Result<Option<api::PlanResp>> {
        if !std::mem::take(&mut self.replan_soon) {
            return Ok(None);
        }
        self.plan(false, Author::Scheduler).map(Some)
    }

    fn store_meta(&self, source: &str, uid: &str, text: &str) -> Result<()> {
        let s = first_series(text, source)?;
        let mut m = s.master.as_ref().map(|m| m.meta.clone()).unwrap_or_default();
        // `blocking` is a sidecar-only preference; never lose it on rewrite.
        if let Some(old) = self.db.meta_get(source, uid)? {
            m.blocking |= old.blocking;
            m.not_going |= old.not_going;
            if m.locus.is_none() {
                m.locus = old.locus;
            }
            // So is the project a session counts for.
            if m.project_id.is_none() {
                m.project_id = old.project_id;
            }
        }
        if m == Meta::default() {
            self.db.meta_delete(source, uid)?;
        } else {
            self.db.meta_set(source, uid, &m)?;
        }
        Ok(())
    }

    fn sync_after(&self, sources: &[String], cols: &[Collection]) {
        let mut names: Vec<SyncTarget> = Vec::new();
        for s in sources {
            let Ok(src) = self.source(s) else { continue };
            let Ok(dir) = self.dir_of(&src, cols) else { continue };
            let coll = dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let name = format!("{}/{}", self.cfg.sync.pair, coll);
            if !names.iter().any(|t| t.name == name) {
                let emptied = vdir::is_empty(&dir);
                names.push(SyncTarget { name, emptied });
            }
        }
        if !names.is_empty() {
            self.syncer.sync_collections(&names);
        }
    }

    pub fn approve(&mut self, id: &str) -> Result<Changeset> {
        let mut cs = self
            .db
            .cs_get(id)?
            .ok_or_else(|| EngineError::NotFound(format!("changeset {id}")))?;
        if cs.status != Status::Pending {
            return Err(EngineError::BadRequest(format!(
                "{id} is {}, not pending",
                cs.status.as_str()
            )));
        }
        self.apply(&mut cs)?;
        if cs
            .ops
            .iter()
            .any(|o| matches!(o, Op::UpdateTask { .. } | Op::CreateTask { .. }))
            && let Err(e) = self.plan(false, Author::Scheduler)
        {
            tracing::warn!("replan after approval failed: {e}");
        }
        Ok(cs)
    }

    pub fn reject(&mut self, id: &str) -> Result<Changeset> {
        let mut cs = self
            .db
            .cs_get(id)?
            .ok_or_else(|| EngineError::NotFound(format!("changeset {id}")))?;
        if cs.status != Status::Pending {
            return Err(EngineError::BadRequest(format!(
                "{id} is {}, not pending",
                cs.status.as_str()
            )));
        }
        cs.status = Status::Rejected;
        cs.decided_at = Some(self.now());
        self.db.cs_put(&cs)?;
        Ok(cs)
    }

    /// Restore pre-images. Refuses (with a diff) if anything changed since.
    pub fn undo(&mut self, id: &str) -> Result<Changeset> {
        let mut cs = self
            .db
            .cs_get(id)?
            .ok_or_else(|| EngineError::NotFound(format!("changeset {id}")))?;
        if cs.status != Status::Applied {
            return Err(EngineError::BadRequest(format!(
                "{id} is {}, only applied changesets can be undone",
                cs.status.as_str()
            )));
        }
        let cols = vdir::collections(&self.cfg.vdir())?;
        let dir = |s: &str| -> Result<PathBuf> { self.dir_of(&self.source(s)?, &cols) };
        let expect = |item: &Item, want: Option<&str>| -> Result<()> {
            let cur = vdir::read_item(&dir(&item.source)?, &item.href).ok();
            let ok = match (want, &cur) {
                (None, None) => true,
                (Some(w), Some(c)) => semantic(w) == semantic(c),
                _ => false,
            };
            if ok {
                Ok(())
            } else {
                Err(EngineError::Conflict {
                    message: format!(
                        "can't undo {id}: {} in {} was edited elsewhere since (iPhone/Calendar.app?)",
                        item.href, item.source
                    ),
                    diff: diff(
                        &want.map(semantic).unwrap_or_default(),
                        &cur.as_deref().map(semantic).unwrap_or_default(),
                    ),
                })
            }
        };
        for op in cs.ops.iter().rev() {
            match op {
                Op::CreateEvent { item, after, .. } | Op::UpdateEvent { item, after, .. } => expect(item, Some(after))?,
                Op::DeleteEvent { item, .. } => expect(item, None)?,
                Op::MoveEvent { from, to, after, .. } => {
                    expect(to, Some(after))?;
                    expect(from, None)?;
                }
                _ => {}
            }
        }
        let mut touched = Vec::new();
        for op in cs.ops.iter().rev() {
            match op {
                Op::CreateEvent { item, uid, .. } => {
                    vdir::delete_item(&dir(&item.source)?, &item.href)?;
                    self.db.meta_delete(&item.source, uid)?;
                }
                Op::UpdateEvent { item, uid, before, .. } | Op::DeleteEvent { item, uid, before } => {
                    vdir::write_item(&dir(&item.source)?, &item.href, before)?;
                    self.store_meta(&item.source, uid, before)?;
                }
                Op::MoveEvent {
                    from, to, uid, before, ..
                } => {
                    vdir::delete_item(&dir(&to.source)?, &to.href)?;
                    self.db.meta_delete(&to.source, uid)?;
                    vdir::write_item(&dir(&from.source)?, &from.href, before)?;
                    self.store_meta(&from.source, uid, before)?;
                }
                Op::CreateTask { after } => {
                    let mut t = (**after).clone();
                    t.status = TaskStatus::Dropped;
                    t.notes = format!("{} [undone {}]", t.notes, cs.id).trim().to_string();
                    self.db.task_put(&t)?;
                }
                Op::UpdateTask { before, .. } => self.db.task_put(before)?,
            }
            // Moves: delete from `to` first, then recreate in `from`.
            let items = op.items();
            for it in items.iter().rev() {
                touched.push(it.source.clone());
            }
        }
        cs.status = Status::Undone;
        cs.decided_at = Some(self.now());
        self.db.cs_put(&cs)?;
        // A skipped slot comes back with its block.
        self.db.skips_forget(&cs.id)?;
        self.sync_after(&touched, &cols);
        Ok(cs)
    }

    /// Undo `id` together with the scheduler's replans applied after it
    /// (newest first), so "undo" puts things back as they were before that
    /// change — not just the change with the work it displaced left moved.
    pub fn undo_with_replans(&mut self, id: &str) -> Result<Changeset> {
        let target = self
            .db
            .cs_get(id)?
            .ok_or_else(|| EngineError::NotFound(format!("changeset {id}")))?;
        let later: Vec<Changeset> = self
            .db
            .cs_since(target.created_at)?
            .into_iter()
            .filter(|c| c.id != target.id && c.status == Status::Applied && c.author == Author::Scheduler)
            .collect();
        let mut later = later;
        later.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        for c in later {
            // A replan that can't be undone (edited since) is left as it is.
            if let Err(e) = self.undo(&c.id) {
                tracing::info!("undo: leaving replan {}: {e}", c.id);
            }
        }
        self.undo(id)
    }

    /// Skip a work block: it's removed and no work goes in that time (the
    /// task's work is planned elsewhere). Undo brings it back.
    pub fn skip_block(&mut self, author: Author, source: Option<&str>, uid: &str) -> Result<Changeset> {
        let world = self.world(2)?;
        let loc = self.locate(&world, source, uid)?;
        let tz = self.tz();
        let (start, end) = match world.instances.iter().find(|i| i.uid == uid).map(|i| i.span) {
            Some(Span::Timed { start, end }) => (start, end),
            _ => return Err(EngineError::BadRequest("not a timed work block".into())),
        };
        if loc
            .series
            .master
            .as_ref()
            .is_none_or(|m| m.meta.kind != EventKind::TaskBlock)
        {
            return Err(EngineError::BadRequest("only work blocks can be skipped".into()));
        }
        let cs = self.delete_events(
            author,
            &[(Some(loc.source.id.clone()), uid.to_string())],
            Some(&format!(
                "Skip work block {}–{} (planned elsewhere)",
                start.with_timezone(&tz).format("%a %H:%M"),
                end.with_timezone(&tz).format("%H:%M")
            )),
        )?;
        self.db.skip_add(&cs.id, start, end)?;
        if cs.status == Status::Applied {
            self.replan_soon = true;
        }
        Ok(cs)
    }

    // ------------------------------------------------------------ event intents

    fn parse_span(&self, start: &str, end: Option<&str>, duration_min: Option<u32>) -> Result<NewTime> {
        let now = self.now();
        let tz = self.tz();
        let s = api::parse_time(start, tz, now)?;
        let e = end.map(|e| api::parse_time(e, tz, now)).transpose()?;
        Ok(match (s, e) {
            (TimeIn::Date(d), None) => NewTime::AllDay {
                start: d,
                end: d + Duration::days(i64::from(duration_min.map_or(1, |m| m.div_ceil(1440).max(1)))),
            },
            (TimeIn::Date(d), Some(TimeIn::Date(e))) => {
                // Inclusive end date from humans: "23 oct" to "25 oct" covers 3 days.
                NewTime::AllDay {
                    start: d,
                    end: e.max(d) + Duration::days(1),
                }
            }
            (TimeIn::At(a), Some(TimeIn::At(b))) => {
                if b < a {
                    return Err(EngineError::BadRequest("end before start".into()));
                }
                NewTime::Timed { start: a, end: b }
            }
            (TimeIn::At(a), None) => NewTime::Timed {
                start: a,
                end: a + Duration::minutes(i64::from(duration_min.unwrap_or(60))),
            },
            _ => {
                return Err(EngineError::BadRequest(
                    "start and end must both be dates or both be times".into(),
                ));
            }
        })
    }

    pub fn create_event(&mut self, author: Author, req: &api::CreateEventReq) -> Result<Changeset> {
        if req.title.trim().is_empty() {
            return Err(EngineError::BadRequest("empty title".into()));
        }
        let world = self.world(60)?;
        let key = match &req.key {
            Some(k) => k.clone(),
            None => self
                .cfg
                .default_key()
                .map(|k| k.name.clone())
                .ok_or_else(|| EngineError::BadRequest("no keys configured".into()))?,
        };
        let (target, origin_key) = self.target_for_key(&key)?;
        let time = self.parse_span(&req.start, req.end.as_deref(), req.duration_min)?;
        let mut meta = self.author_meta(author);
        meta.origin_key = origin_key;
        meta.locus = self.locus_ref(req.locus.as_deref())?;
        if let Some(tid) = &req.task_id {
            self.db
                .task_get(tid)?
                .ok_or_else(|| EngineError::NotFound(format!("task {tid}")))?;
            meta.kind = EventKind::TaskBlock;
            meta.task_id = Some(tid.clone());
            meta.flex = true;
            meta.pinned = true;
        }
        let series = match &req.repeat {
            None => vec![(time, None)],
            Some(r) => self.repeat_series(time, r)?,
        };
        let tz = self.tz();
        let mut ops = Vec::new();
        for (time, rrule) in &series {
            let uid = build::new_uid();
            let ev = NewEvent {
                uid: uid.clone(),
                // A work block's title carries its box: "☐ B4 sheet".
                title: if req.task_id.is_some() {
                    build::todo_block_title(req.title.trim())
                } else {
                    req.title.trim().to_string()
                },
                time: *time,
                location: req.location.clone().filter(|l| !l.is_empty()),
                description: req.description.clone(),
                meta: meta.clone(),
                alarms: req.alarms.clone().unwrap_or_default(),
                transparent: false,
                rrule: rrule.clone(),
            };
            let after = build::event_file(&ev, tz, self.now());
            let item = Item {
                source: target.id.clone(),
                href: vdir::href_for_uid(&uid),
            };
            ops.push(Op::CreateEvent { item, uid, after });
        }
        let title = req.title.trim();
        let summary = match &req.repeat {
            None => format!("Add {title:?} {} to {key}", describe_time(time, tz)),
            Some(r) => {
                let every = if r.every.as_deref() == Some("day") {
                    "daily"
                } else {
                    "weekly"
                };
                let first = series.first().map_or(time, |s| s.0);
                let scope = if r.term_only {
                    format!(
                        ", term time ({} term{})",
                        series.len(),
                        if series.len() == 1 { "" } else { "s" }
                    )
                } else {
                    r.until.as_deref().map(|u| format!(" until {u}")).unwrap_or_default()
                };
                format!(
                    "Add {every} {title:?} from {}{scope} to {key}",
                    describe_time(first, tz)
                )
            }
        };
        self.propose(&world, author, ops, summary)
    }

    /// The series for a repeating event: one per term (term time only) or one
    /// from the start until a date (or forever). Each starts on the first
    /// matching day in its range.
    fn repeat_series(&self, time: NewTime, r: &api::Repeat) -> Result<Vec<(NewTime, Option<String>)>> {
        let tz = self.tz();
        let daily = match r.every.as_deref().unwrap_or("week") {
            "day" | "daily" => true,
            "week" | "weekly" => false,
            x => return Err(EngineError::BadRequest(format!("repeat every {x:?}: day or week"))),
        };
        let (d0, at_day) = match time {
            NewTime::Timed { start, end } => {
                let l = start.with_timezone(&tz);
                let len = end - start;
                let t = l.time();
                (
                    l.date_naive(),
                    Box::new(move |d: chrono::NaiveDate| {
                        let s = kairos_core::time::resolve_local(d.and_time(t), tz);
                        NewTime::Timed { start: s, end: s + len }
                    }) as Box<dyn Fn(chrono::NaiveDate) -> NewTime>,
                )
            }
            NewTime::AllDay { start, end } => {
                let len = end - start;
                (
                    start,
                    Box::new(move |d: chrono::NaiveDate| NewTime::AllDay { start: d, end: d + len })
                        as Box<dyn Fn(chrono::NaiveDate) -> NewTime>,
                )
            }
        };
        let until_of = |d: chrono::NaiveDate| match time {
            NewTime::Timed { .. } => kairos_core::time::resolve_local(
                d.and_time(NaiveTime::from_hms_opt(23, 59, 59).unwrap_or(NaiveTime::MIN)),
                tz,
            )
            .format("%Y%m%dT%H%M%SZ")
            .to_string(),
            NewTime::AllDay { .. } => d.format("%Y%m%d").to_string(),
        };
        let freq = if daily { "DAILY" } else { "WEEKLY" };
        // First day ≥ `from` on the start's weekday (any day if daily).
        let first_from = |from: chrono::NaiveDate| {
            let mut d = from.max(d0);
            while !daily && chrono::Datelike::weekday(&d) != chrono::Datelike::weekday(&d0) {
                d += Duration::days(1);
            }
            d
        };
        let mut out = Vec::new();
        if r.term_only {
            let term = self
                .constraints
                .profiles
                .iter()
                .find(|p| p.name.eq_ignore_ascii_case("term"))
                .ok_or_else(|| EngineError::BadRequest("no `term` profile in constraints.toml".into()))?;
            let limit = match &r.until {
                Some(u) => Some(self.date_of(u)?),
                None => None,
            };
            for range in &term.dates {
                let to = limit.map_or(range.to, |l| l.min(range.to));
                let first = first_from(range.from);
                if first > to {
                    continue;
                }
                out.push((at_day(first), Some(format!("FREQ={freq};UNTIL={}", until_of(to)))));
            }
            if out.is_empty() {
                return Err(EngineError::BadRequest("no term time left in that range".into()));
            }
        } else {
            let rule = match &r.until {
                Some(u) => format!("FREQ={freq};UNTIL={}", until_of(self.date_of(u)?)),
                None => format!("FREQ={freq}"),
            };
            out.push((time, Some(rule)));
        }
        Ok(out)
    }

    fn date_of(&self, s: &str) -> Result<chrono::NaiveDate> {
        let tz = self.tz();
        Ok(match api::parse_time(s, tz, self.now())? {
            TimeIn::Date(d) => d,
            TimeIn::At(t) => t.with_timezone(&tz).date_naive(),
        })
    }

    pub fn update_event(
        &mut self,
        author: Author,
        source: Option<&str>,
        uid: &str,
        req: &api::UpdateEventReq,
    ) -> Result<Option<Changeset>> {
        let world = self.world(60)?;
        let Some((op, summary)) = self.update_op(&world, source, uid, req)? else {
            return Ok(None);
        };
        self.propose(&world, author, vec![op], summary).map(Some)
    }

    /// Several edits as one changeset: one approval, one undo. `label` is a
    /// human description of the batch (e.g. "Move room codes into location").
    pub fn update_events(
        &mut self,
        author: Author,
        updates: &[api::EventUpdate],
        label: Option<&str>,
    ) -> Result<Changeset> {
        check_batch(updates.len())?;
        let world = self.world(60)?;
        let mut ops = Vec::new();
        let mut first = String::new();
        for u in updates {
            if let Some((op, s)) = self.update_op(&world, u.source.as_deref(), &u.uid, &u.change)? {
                if first.is_empty() {
                    first = s;
                }
                ops.push(op);
            }
        }
        let summary = match (ops.len(), label) {
            (0, _) => return Err(EngineError::BadRequest("nothing to change".into())),
            (1, None) => first,
            (n, Some(l)) => format!("{l} ({n} events)"),
            (n, None) => format!("Update {n} events ({first} …)"),
        };
        self.propose(&world, author, ops, summary)
    }

    /// The op for one edit (`None` if it only touched the sidecar `blocking` flag).
    fn update_op(
        &mut self,
        world: &World,
        source: Option<&str>,
        uid: &str,
        req: &api::UpdateEventReq,
    ) -> Result<Option<(Op, String)>> {
        let loc = self.locate(world, source, uid)?;
        if req.blocking.is_some() || req.not_going.is_some() || req.locus.is_some() {
            // Sidecar-only flags (no iCloud write): a Family event opting in to block the
            // scheduler, or one of yours you're not going to.
            let mut m = self.db.meta_get(&loc.source.id, uid)?.unwrap_or_default();
            if let Some(b) = req.blocking {
                m.blocking = b;
            }
            if let Some(n) = req.not_going {
                m.not_going = n;
            }
            if let Some(l) = &req.locus {
                m.locus = self.locus_ref(Some(l))?;
            }
            self.db.meta_set(&loc.source.id, uid, &m)?;
            // Time it blocked is free now (or blocked again): the plan follows.
            self.replan_soon = true;
            if req.title.is_none()
                && req.start.is_none()
                && req.end.is_none()
                && req.location.is_none()
                && req.description.is_none()
                && req.duration_min.is_none()
            {
                return Ok(None);
            }
        }
        let master = loc
            .series
            .master
            .as_ref()
            .ok_or_else(|| EngineError::BadRequest("event has no master component".into()))?;
        let tz = self.tz();
        let time = match (&req.start, &req.end, req.duration_min) {
            (None, None, None) => None,
            (Some(s), e, d) => {
                let d = d.or_else(|| {
                    e.is_none()
                        .then(|| ((master.end.instant(tz) - master.start.instant(tz)).num_minutes().max(0)) as u32)
                });
                Some(self.parse_span(s, e.as_deref(), d)?)
            }
            (None, e, d) => {
                let start = master.start.instant(tz).to_rfc3339();
                Some(self.parse_span(&start, e.as_deref(), d)?)
            }
        };
        // A work block that has started is history: check in instead.
        if time.is_some() && master.meta.kind == EventKind::TaskBlock && master.start.instant(tz) <= self.now() {
            return Err(EngineError::BadRequest(
                "that work block has already started — check in on it (C) instead of moving it".into(),
            ));
        }
        // Moving a work block by hand pins it: the scheduler won't move it back.
        let meta = (time.is_some() && master.meta.kind == EventKind::TaskBlock).then(|| {
            let mut m = master.meta.clone();
            m.pinned = true;
            m.flex = false;
            m
        });
        let edit = Edit {
            title: req.title.clone(),
            time,
            location: req.location.clone().map(|l| if l.is_empty() { None } else { Some(l) }),
            description: req
                .description
                .clone()
                .map(|d| if d.is_empty() { None } else { Some(d) }),
            meta,
        };
        let after = build::edit_file(&loc.text, uid, &edit, tz, self.now())
            .map_err(|e| EngineError::BadRequest(e.to_string()))?;
        let summary = format!(
            "Update {:?}{}",
            master.summary,
            time.map(|t| format!(" → {}", describe_time(t, tz))).unwrap_or_default()
        );
        let op = Op::UpdateEvent {
            item: Item {
                source: loc.source.id.clone(),
                href: loc.href.clone(),
            },
            uid: uid.into(),
            before: loc.text,
            after,
        };
        Ok(Some((op, summary)))
    }

    pub fn delete_event(&mut self, author: Author, source: Option<&str>, uid: &str) -> Result<Changeset> {
        self.delete_events(author, &[(source.map(str::to_string), uid.to_string())], None)
    }

    /// Delete several events as one changeset (one approval, one undo).
    pub fn delete_events(
        &mut self,
        author: Author,
        events: &[(Option<String>, String)],
        label: Option<&str>,
    ) -> Result<Changeset> {
        check_batch(events.len())?;
        let world = self.world(60)?;
        let mut ops = Vec::new();
        let mut first = String::new();
        for (source, uid) in events {
            let loc = self.locate(&world, source.as_deref(), uid)?;
            if first.is_empty() {
                let title = loc
                    .series
                    .master
                    .as_ref()
                    .map(|m| m.summary.clone())
                    .unwrap_or_default();
                first = format!("Delete {title:?} from {}", loc.source.id);
            }
            ops.push(Op::DeleteEvent {
                item: Item {
                    source: loc.source.id.clone(),
                    href: loc.href.clone(),
                },
                uid: uid.clone(),
                before: loc.text,
            });
        }
        // Deleting an open task's ⚑ deadline drops the task and all its
        // planned time with it (Alec, 2026-10-13): same changeset, one undo.
        let deadlines: Vec<String> = ops
            .iter()
            .filter_map(|o| match o {
                Op::DeleteEvent { uid, .. } => Some(uid.clone()),
                _ => None,
            })
            .collect();
        let now = self.now();
        let mut dropped = Vec::new();
        for t in self.db.tasks(Some(TaskStatus::Open))? {
            if !t.deadline_uid.as_ref().is_some_and(|d| deadlines.contains(d)) {
                continue;
            }
            let mut after = t.clone();
            after.status = TaskStatus::Dropped;
            after.updated_at = now;
            ops.extend(self.block_deletes(&world, &t.id, |b| b.end > now)?);
            dropped.push(t.title.clone());
            ops.push(Op::UpdateTask {
                before: Box::new(t),
                after: Box::new(after),
            });
        }
        if events.len() == 1
            && let [title] = dropped.as_slice()
        {
            first = format!("Remove deadline of {title:?} with the task and its planned time");
        }
        let summary = match (events.len(), label) {
            (0, _) => return Err(EngineError::BadRequest("no events given".into())),
            (1, None) => first,
            (1, Some(l)) => l.to_string(),
            (n, Some(l)) => format!("{l} ({n} events)"),
            (n, None) => format!("Delete {n} events ({first} …)"),
        };
        self.propose(&world, author, ops, summary)
    }

    /// Re-key: move to another key's calendar (keeps UID).
    pub fn move_event(&mut self, author: Author, source: Option<&str>, uid: &str, key: &str) -> Result<Changeset> {
        self.move_events(author, &[(source.map(str::to_string), uid.to_string())], key)
    }

    /// Re-key several events as one changeset. Events already in `key` are skipped.
    pub fn move_events(&mut self, author: Author, events: &[(Option<String>, String)], key: &str) -> Result<Changeset> {
        self.move_events_labelled(author, events, key, None)
    }

    pub fn move_events_labelled(
        &mut self,
        author: Author,
        events: &[(Option<String>, String)],
        key: &str,
        label: Option<&str>,
    ) -> Result<Changeset> {
        check_batch(events.len())?;
        let world = self.world(60)?;
        let (target, _) = self.target_for_key(key)?;
        let mut ops = Vec::new();
        let mut first_title = String::new();
        for (source, uid) in events {
            let loc = self.locate(&world, source.as_deref(), uid)?;
            if target.id == loc.source.id {
                continue;
            }
            if first_title.is_empty() {
                first_title = loc
                    .series
                    .master
                    .as_ref()
                    .map(|m| m.summary.clone())
                    .unwrap_or_default();
            }
            ops.push(Op::MoveEvent {
                from: Item {
                    source: loc.source.id.clone(),
                    href: loc.href.clone(),
                },
                to: Item {
                    source: target.id.clone(),
                    href: loc.href.clone(),
                },
                uid: uid.clone(),
                before: loc.text.clone(),
                after: loc.text,
                share_to_family: false,
            });
        }
        let summary = match (ops.len(), label) {
            (0, _) => return Err(EngineError::BadRequest(format!("already in {key}"))),
            (1, None) => format!("Re-key {first_title:?} → {key}"),
            (n, Some(l)) => format!("{l} ({n} events → {key})"),
            (n, None) => format!("Re-key {n} events ({first_title:?} …) → {key}"),
        };
        self.propose(&world, author, ops, summary)
    }

    pub fn share_to_family(&mut self, author: Author, source: Option<&str>, uid: &str) -> Result<Changeset> {
        let world = self.world(60)?;
        let loc = self.locate(&world, source, uid)?;
        let fam = self
            .cfg
            .family_source()
            .cloned()
            .ok_or_else(|| EngineError::BadRequest("no Family source configured".into()))?;
        let side = self.db.meta_get(&loc.source.id, uid)?.unwrap_or_default();
        let base = loc
            .series
            .master
            .as_ref()
            .map(|m| m.meta.merged_with(&side))
            .unwrap_or(side);
        let meta = Meta {
            origin_key: base.origin_key.clone().or_else(|| loc.source.key.clone()),
            ..base
        };
        let after = build::set_meta_file(&loc.text, &meta).map_err(|e| EngineError::BadRequest(e.to_string()))?;
        let title = loc
            .series
            .master
            .as_ref()
            .map(|m| m.summary.clone())
            .unwrap_or_default();
        let op = Op::MoveEvent {
            from: Item {
                source: loc.source.id.clone(),
                href: loc.href.clone(),
            },
            to: Item {
                source: fam.id.clone(),
                href: loc.href.clone(),
            },
            uid: uid.into(),
            before: loc.text,
            after,
            share_to_family: true,
        };
        self.propose(&world, author, vec![op], format!("Share {title:?} to Family"))
    }

    /// Copy an Imported/feed event into a key calendar.
    pub fn adopt(&mut self, author: Author, uid: &str, key: &str) -> Result<Changeset> {
        let world = self.world(60)?;
        let (ls, series) = world
            .loaded
            .sources
            .iter()
            .filter(|s| s.kind == SourceKind::IcsReadonly)
            .find_map(|s| s.series.iter().find(|e| e.uid == uid).map(|e| (s, e)))
            .ok_or_else(|| EngineError::NotFound(format!("imported event {uid:?}")))?;
        let path = vdir::feed_path(&self.cfg, &ls.id);
        let text = std::fs::read_to_string(&path).map_err(|e| {
            EngineError::Store(StoreError::Io {
                path: path.clone(),
                source: e,
            })
        })?;
        let cals = ics::parse(&text).map_err(|e| EngineError::BadRequest(e.to_string()))?;
        let mut out = ics::Component::new("VCALENDAR");
        out.push(ics::Property::new("VERSION", "2.0"));
        out.push(ics::Property::new("PRODID", build::PRODID));
        let (target, origin_key) = self.target_for_key(key)?;
        let meta = Meta {
            created_by: Some(author),
            adopted_from: Some(ls.id.clone()),
            origin_key,
            ..Default::default()
        };
        for cal in &cals {
            for tzc in cal.children_named("VTIMEZONE") {
                if !out
                    .children
                    .iter()
                    .any(|c| c.name == "VTIMEZONE" && c.value("TZID") == tzc.value("TZID"))
                {
                    out.children.push(tzc.clone());
                }
            }
            for ev in cal
                .children_named("VEVENT")
                .filter(|e| e.text("UID").as_deref() == Some(uid))
            {
                let mut ev = ev.clone();
                // Never carry invitations into iCloud (it would email people).
                for p in ["ATTENDEE", "ORGANIZER", "METHOD", "X-GOOGLE-CONFERENCE"] {
                    ev.remove(p);
                }
                meta.apply_to(&mut ev);
                out.children.push(ev);
            }
        }
        let title = series.master.as_ref().map(|m| m.summary.clone()).unwrap_or_default();
        let op = Op::CreateEvent {
            item: Item {
                source: target.id.clone(),
                href: vdir::href_for_uid(uid),
            },
            uid: uid.into(),
            after: out.to_ics(),
        };
        self.propose(&world, author, vec![op], format!("Adopt {title:?} into {key}"))
    }

    /// Import an `.ics` file into a key calendar as one changeset (one item per
    /// UID; invitations stripped; UIDs already present in the target skipped).
    pub fn import_ics(&mut self, author: Author, text: &str, key: &str, name: &str) -> Result<Changeset> {
        let world = self.world(60)?;
        let (target, origin_key) = self.target_for_key(key)?;
        let cals = ics::parse(text).map_err(|e| EngineError::BadRequest(format!("not valid iCalendar: {e}")))?;
        let existing: std::collections::HashSet<&str> = world
            .loaded
            .sources
            .iter()
            .filter(|s| s.id == target.id)
            .flat_map(|s| s.series.iter().map(|e| e.uid.as_str()))
            .collect();
        let mut tzs: Vec<ics::Component> = Vec::new();
        let mut by_uid: std::collections::BTreeMap<String, Vec<ics::Component>> = Default::default();
        for cal in &cals {
            for c in &cal.children {
                match c.name.as_str() {
                    "VTIMEZONE" if !tzs.iter().any(|t| t.value("TZID") == c.value("TZID")) => tzs.push(c.clone()),
                    "VEVENT" => {
                        let uid = c.text("UID").unwrap_or_else(build::new_uid);
                        by_uid.entry(uid).or_default().push(c.clone());
                    }
                    _ => {}
                }
            }
        }
        let meta = Meta {
            created_by: Some(author),
            adopted_from: Some(format!("import:{name}")),
            origin_key,
            ..Default::default()
        };
        let mut ops = Vec::new();
        let mut skipped = 0;
        for (uid, evs) in by_uid {
            if existing.contains(uid.as_str()) {
                skipped += 1;
                continue;
            }
            let mut out = ics::Component::new("VCALENDAR");
            out.push(ics::Property::new("VERSION", "2.0"));
            out.push(ics::Property::new("PRODID", build::PRODID));
            out.children.extend(tzs.iter().cloned());
            for mut ev in evs {
                for p in ["ATTENDEE", "ORGANIZER", "METHOD"] {
                    ev.remove(p);
                }
                ev.set(ics::Property::text("UID", &uid));
                meta.apply_to(&mut ev);
                out.children.push(ev);
            }
            ops.push(Op::CreateEvent {
                item: Item {
                    source: target.id.clone(),
                    href: vdir::href_for_uid(&uid),
                },
                uid,
                after: out.to_ics(),
            });
        }
        if ops.is_empty() {
            return Err(EngineError::BadRequest(format!(
                "nothing to import ({skipped} already present)"
            )));
        }
        let summary = format!(
            "Import {} event(s) from {name} into {key}{}",
            ops.len(),
            if skipped > 0 {
                format!(" ({skipped} already present)")
            } else {
                String::new()
            }
        );
        self.propose(&world, author, ops, summary)
    }

    // ------------------------------------------------------------ deadlines & tasks

    /// A ⚑ deadline (spec K5): a due date on a task, never a calendar event. With `task_id` it
    /// sets that task's due date; without, it makes a task with nothing to plan, only the date.
    pub fn create_deadline(&mut self, author: Author, req: &api::DeadlineReq) -> Result<Changeset> {
        let world = self.world(60)?;
        let due = api::parse_time(&req.due, self.tz(), self.now())?.as_due();
        let mut ops = Vec::new();
        if let Some(tid) = &req.task_id {
            let before = self
                .db
                .task_get(tid)?
                .ok_or_else(|| EngineError::NotFound(format!("task {tid}")))?;
            let mut after = before.clone();
            after.due = Some(due);
            after.updated_at = self.now();
            ops.push(Op::UpdateTask {
                before: Box::new(before),
                after: Box::new(after),
            });
        } else {
            let key = self
                .cfg
                .key(&req.key)
                .ok_or_else(|| EngineError::BadRequest(format!("unknown key {:?}", req.key)))?
                .name
                .clone();
            let mut t = Task::new(req.title.trim(), &key, 0, author, self.now());
            t.remaining_min = 0;
            t.due = Some(due);
            ops.push(Op::CreateTask { after: Box::new(t) });
        }
        self.propose(
            &world,
            author,
            ops,
            format!("Deadline ⚑ {} {}", req.title, due.describe(self.tz())),
        )
    }

    /// ⚑ events from before K5 become their tasks' due dates, in one changeset proposed for
    /// approval: a linked one is deleted (its task already carries the date), an unlinked one
    /// becomes a task with only that date. None to migrate proposes nothing.
    pub fn migrate_deadlines(&mut self) -> Result<Option<Changeset>> {
        let world = self.world(400)?;
        let now = self.now();
        let tasks = self.db.tasks(None)?;
        let mut ops = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for i in self.events(now - chrono::Duration::days(60), now + chrono::Duration::days(400))? {
            if i.meta.kind != EventKind::Deadline || !seen.insert((i.source.clone(), i.uid.clone())) {
                continue;
            }
            let Ok(loc) = self.locate(&world, Some(&i.source), &i.uid) else {
                continue;
            };
            let linked = i
                .meta
                .task_id
                .as_deref()
                .and_then(|id| tasks.iter().find(|t| t.id == id));
            match linked {
                Some(t) if t.due.is_some() => {
                    let mut after = t.clone();
                    after.deadline_uid = None;
                    after.updated_at = now;
                    ops.push(Op::UpdateTask {
                        before: Box::new(t.clone()),
                        after: Box::new(after),
                    });
                }
                _ => {
                    let title = i.title.trim_start_matches('⚑').trim().to_string();
                    let key = i
                        .key
                        .clone()
                        .unwrap_or_else(|| self.cfg.default_key().map(|k| k.name.clone()).unwrap_or_default());
                    let mut t = Task::new(&title, &key, 0, Author::Claude, now);
                    t.remaining_min = 0;
                    t.due = Some(match i.span {
                        kairos_core::model::Span::AllDay { start, .. } => Due::Date(start),
                        kairos_core::model::Span::Timed { start, .. } => Due::At(start),
                    });
                    ops.push(Op::CreateTask { after: Box::new(t) });
                }
            }
            ops.push(Op::DeleteEvent {
                item: Item {
                    source: loc.source.id.clone(),
                    href: loc.href.clone(),
                },
                uid: loc.series.uid.clone(),
                before: loc.text,
            });
        }
        let n = ops.iter().filter(|o| matches!(o, Op::DeleteEvent { .. })).count();
        let summary = format!("Deadlines become due dates: {n} ⚑ event(s) move onto their tasks");
        // Proposed once: a restart while it waits for approval does not propose it again.
        let waiting = self
            .db
            .cs_list(Some(Status::Pending), 100)?
            .iter()
            .any(|c| c.summary.starts_with("Deadlines become due dates"));
        if ops.is_empty() || waiting {
            return Ok(None);
        }
        self.propose(&world, Author::Claude, ops, summary).map(Some)
    }

    pub fn create_task(&mut self, author: Author, req: &api::CreateTaskReq) -> Result<api::TaskResp> {
        if self.cfg.key(&req.key).is_none() {
            return Err(EngineError::BadRequest(format!("unknown key {:?}", req.key)));
        }
        let now = self.now();
        let tz = self.tz();
        let mut t = Task::new(req.title.trim(), &req.key, req.estimate_min, author, now);
        let ty = match &req.kind {
            Some(k) => self.task_type(k)?,
            None => self.inferred_type(req.estimate_min),
        };
        if let Some(ty) = &ty {
            t.apply_type(ty);
        }
        // No estimate is allowed only for a bare due date (`kairos deadline`): nothing to plan.
        let bare_due = req.estimate_min == 0 && req.kind.is_none() && req.due.is_some();
        if t.estimate_min == 0 && !bare_due {
            return Err(EngineError::BadRequest(format!(
                "estimate required{}",
                ty.map(|t| format!(" for {}", t.name)).unwrap_or_default()
            )));
        }
        if bare_due {
            t.estimate_min = 0;
            t.remaining_min = 0;
        }
        if let Some(k) = self.cfg.key(&req.key) {
            t.key = k.name.clone();
        }
        t.due = req
            .due
            .as_deref()
            .filter(|d| !d.trim().is_empty())
            .map(|d| api::parse_time(d, tz, now))
            .transpose()?
            .map(TimeIn::as_due);
        t.earliest_start = req
            .earliest_start
            .as_deref()
            .filter(|d| !d.trim().is_empty())
            .map(|d| api::parse_time(d, tz, now))
            .transpose()?
            .map(|x| match x {
                TimeIn::At(t) => t,
                TimeIn::Date(d) => kairos_core::time::resolve_local(d.and_time(NaiveTime::MIN), tz),
            });
        if let Some(p) = req.priority {
            t.priority = p.clamp(1, 3);
        }
        if let Some(s) = req.splittable {
            t.splittable = s;
        }
        if let Some(m) = req.min_chunk_min {
            t.min_chunk_min = m.max(15);
        }
        if let Some(m) = req.max_chunk_min {
            t.max_chunk_min = m.max(t.min_chunk_min);
        }
        if let Some(w) = &req.preferred_window {
            if self.constraints.window(w).is_none() {
                return Err(EngineError::BadRequest(format!("unknown window {w:?}")));
            }
            t.preferred_window = Some(w.clone());
        }
        t.notes = req.notes.clone().unwrap_or_default();
        t.locus = self.locus_ref(req.locus.as_deref())?;
        if let Some(p) = req.project.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
            t.project_id = Some(self.find_project(p)?.id);
        }
        let world = self.world(60)?;
        // The due date stays on the task: no ⚑ event is written (spec K5).
        let ops = vec![Op::CreateTask {
            after: Box::new(t.clone()),
        }];
        let summary = format!(
            "Task {:?} ({} min{})",
            t.title,
            t.estimate_min,
            t.due.map(|d| format!(", due {}", d.describe(tz))).unwrap_or_default()
        );
        let cs = self.propose(&world, author, ops, summary)?;
        let mut resp = api::TaskResp {
            task: t,
            changesets: vec![WriteResult::from(&cs)],
            plan: None,
        };
        if req.plan.unwrap_or(true) && cs.status == Status::Applied {
            resp.plan = Some(self.plan(false, Author::Scheduler)?);
        }
        Ok(resp)
    }

    pub fn update_task(&mut self, author: Author, id: &str, req: &api::UpdateTaskReq) -> Result<api::TaskResp> {
        let before = self.find_task(id)?;
        let now = self.now();
        let tz = self.tz();
        let mut t = before.clone();
        if req.reopen == Some(true) && t.status != TaskStatus::Open {
            if req.remaining_min.is_none() {
                return Err(EngineError::BadRequest(
                    "reopening needs how much is left (remaining_min)".into(),
                ));
            }
            t.status = TaskStatus::Open;
            // It wasn't finished after all: don't learn from that run.
            self.db.estimate_unlog(&t.id)?;
        } else if t.status != TaskStatus::Open {
            return Err(EngineError::BadRequest(format!(
                "{:?} is {} (reopen it first)",
                t.title,
                t.status.as_str()
            )));
        }
        if let Some(x) = &req.title {
            t.title = x.clone();
        }
        if let Some(k) = &req.kind
            && let Some(ty) = self.task_type(k)?
        {
            t.apply_type(&ty);
        }
        if let Some(k) = &req.key {
            t.key = self
                .cfg
                .key(k)
                .ok_or_else(|| EngineError::BadRequest(format!("unknown key {k:?}")))?
                .name
                .clone();
        }
        if let Some(e) = req.estimate_min {
            let done = t.estimate_min.saturating_sub(t.remaining_min);
            t.estimate_min = e;
            t.remaining_min = e.saturating_sub(done);
        }
        if let Some(r) = req.remaining_min {
            t.remaining_min = r;
        }
        let mut due_changed = false;
        if let Some(d) = &req.due {
            t.due = if d.is_empty() {
                None
            } else {
                Some(api::parse_time(d, tz, now)?.as_due())
            };
            due_changed = t.due != before.due;
        }
        if let Some(p) = req.priority {
            t.priority = p.clamp(1, 3);
        }
        if let Some(s) = req.splittable {
            t.splittable = s;
        }
        if let Some(m) = req.min_chunk_min {
            t.min_chunk_min = m.max(15);
        }
        if let Some(m) = req.max_chunk_min {
            t.max_chunk_min = m.max(t.min_chunk_min);
        }
        if let Some(w) = &req.preferred_window {
            t.preferred_window = if w.is_empty() { None } else { Some(w.clone()) };
        }
        if let Some(n) = &req.notes {
            t.notes = n.clone();
        }
        if let Some(l) = &req.locus {
            t.locus = self.locus_ref(Some(l))?;
        }
        if let Some(p) = req.project.as_deref().map(str::trim) {
            t.project_id = if p.is_empty() {
                None
            } else {
                Some(self.find_project(p)?.id)
            };
        }
        t.updated_at = now;
        let world = self.world(60)?;
        let mut ops = Vec::new();
        if due_changed {
            // Update / create / delete the linked deadline event.
            let existing = t
                .deadline_uid
                .as_deref()
                .and_then(|u| self.locate(&world, None, u).ok());
            // A due date is the task's attribute, not an event (ADR-006, spec K5): a ⚑ event
            // left from before goes when the date is touched; none is ever made.
            match (existing, t.due) {
                (Some(loc), _) => {
                    ops.push(Op::DeleteEvent {
                        item: Item {
                            source: loc.source.id.clone(),
                            href: loc.href.clone(),
                        },
                        uid: loc.series.uid.clone(),
                        before: loc.text,
                    });
                    t.deadline_uid = None;
                }
                (None, _) => t.deadline_uid = None,
            }
        }
        // A new title or calendar carries through to the task's future blocks
        // and its ⚑ (one changeset, one undo).
        if t.title != before.title || t.key != before.key {
            ops.extend(self.follow_task_ops(&world, &t, &before, &ops)?);
        }
        ops.insert(
            0,
            Op::UpdateTask {
                before: Box::new(before),
                after: Box::new(t.clone()),
            },
        );
        let cs = self.propose(&world, author, ops, format!("Update task {:?}", t.title))?;
        let mut resp = api::TaskResp {
            task: t,
            changesets: vec![WriteResult::from(&cs)],
            plan: None,
        };
        if req.plan.unwrap_or(true) && cs.status == Status::Applied {
            resp.plan = Some(self.plan(false, Author::Scheduler)?);
        }
        Ok(resp)
    }

    // ------------------------------------------------------------ proposed project time

    /// Recompute proposed project time around `p`'s task work (rejected
    /// spans are never proposed again).
    fn refresh_proposals(
        &mut self,
        input: &PlanInput<'_>,
        p: &sched::Plan,
        session_days: &[chrono::NaiveDate],
    ) -> Result<()> {
        let rows = self.db.proposals()?;
        let avoid: Vec<(DateTime<Utc>, DateTime<Utc>)> =
            rows.iter().filter(|r| r.3 == "rejected").map(|r| (r.1, r.2)).collect();
        // One project session a day: days with an accepted one get no more.
        let taken: Vec<chrono::NaiveDate> = rows
            .iter()
            .filter(|r| r.3 == "accepted")
            .map(|r| r.1.with_timezone(&input.tz).date_naive())
            .chain(session_days.iter().copied())
            .collect();
        let spans: Vec<(String, DateTime<Utc>, DateTime<Utc>)> = sched::propose(input, p, &avoid, &taken)
            .into_iter()
            .map(|x| {
                (
                    format!("p{}-{}", x.start.timestamp() / 60, (x.end - x.start).num_minutes()),
                    x.start,
                    x.end,
                )
            })
            .collect();
        self.db.proposals_replace(&spans, input.now)?;
        Ok(())
    }

    /// Open proposals, soonest first, each suggesting an active project:
    /// the most neglected first (least time this week, counting sessions
    /// already planned; then longest since last worked), in turn.
    pub fn proposals(&self) -> Result<Vec<api::ProposalItem>> {
        let now = self.now();
        let title = self.constraints.projects.title.clone();
        let (items, upcoming) = self.project_stats()?;
        let mut order: Vec<&api::ProjectItem> = items
            .iter()
            .filter(|p| p.project.status == ProjectStatus::Active)
            .collect();
        order.sort_by_key(|p| {
            (
                p.week_min + p.unconfirmed_min + upcoming.get(&p.project.id).copied().unwrap_or(0),
                p.last_worked,
                p.project.created_at,
                p.project.name.to_lowercase(),
            )
        });
        Ok(self
            .db
            .proposals()?
            .into_iter()
            .filter(|r| r.3 == "proposed" && r.2 > now)
            .enumerate()
            .map(|(k, (id, start, end, _))| {
                let suggested = (!order.is_empty()).then(|| order[k % order.len()]);
                api::ProposalItem {
                    id,
                    start,
                    end,
                    title: suggested.map_or_else(|| title.clone(), |p| p.project.name.clone()),
                    project_id: suggested.map(|p| p.project.id.clone()),
                }
            })
            .collect())
    }

    // ------------------------------------------------------------ projects

    /// Projects are tasks of kind `project`; this is the calendar their
    /// sessions default to.
    fn projects_key(&self) -> Option<String> {
        self.constraints
            .projects
            .key
            .clone()
            .filter(|k| self.cfg.key(k).is_some())
    }

    fn project_view(&self, t: &Task) -> Project {
        let mut p = Project::of(t, self.projects_key().as_deref());
        // A linked project shows the registry's name: one name in every app.
        if let (Some(id), Some(reg)) = (&t.registry, &self.registry)
            && let Some(e) = reg.get(id)
        {
            p.name = e.name.clone();
        }
        p
    }

    /// Bring kairos's projects in line with the locus registry (spec K7), in one changeset: a
    /// project whose name is a registry repo's (its id, name or a component's name) is linked to
    /// it; a registry repo with no project gets one (active if the registry says so, else
    /// paused). Projects that match nothing stay as they are, and `unregistered` names them.
    /// Run at start and every few minutes, so an edit to registry.toml arrives by itself.
    pub fn sync_registry(&mut self) -> Result<Option<Changeset>> {
        let Some(reg) = self.registry.clone() else {
            return Ok(None);
        };
        let now = self.now();
        let mut projects: Vec<Task> = self
            .db
            .projects()?
            .into_iter()
            .filter(|p| p.status != TaskStatus::Dropped)
            .collect();
        let mut ops = Vec::new();
        let mut linked = Vec::new();
        for p in projects.iter_mut().filter(|p| p.registry.is_none()) {
            if let Some(e) = project::registry_match(&p.title, &reg)
                && !self
                    .db
                    .projects()?
                    .iter()
                    .any(|o| o.registry.as_deref() == Some(e.id.as_str()))
                && !linked.iter().any(|(_, id): &(String, String)| id == &e.id)
            {
                let mut after = p.clone();
                after.registry = Some(e.id.clone());
                after.updated_at = now;
                linked.push((p.title.clone(), e.id.clone()));
                ops.push(Op::UpdateTask {
                    before: Box::new(p.clone()),
                    after: Box::new(after.clone()),
                });
                *p = after;
            }
        }
        let key = self.projects_key().unwrap_or_default();
        let mut made = Vec::new();
        for e in reg
            .projects
            .iter()
            .filter(|e| e.kind == locus_contract::ProjectKind::Repo)
        {
            if projects.iter().any(|p| p.registry.as_deref() == Some(e.id.as_str())) {
                continue;
            }
            let status = if e.active {
                ProjectStatus::Active
            } else {
                ProjectStatus::Paused
            };
            let area_key = self.cfg.key(&e.area).map(|k| k.name.clone()).unwrap_or(key.clone());
            let mut t = project::new_task(&e.name, status, "", &area_key, Author::Claude, now);
            t.registry = Some(e.id.clone());
            made.push(e.name.clone());
            ops.push(Op::CreateTask { after: Box::new(t) });
        }
        if ops.is_empty() {
            return Ok(None);
        }
        let mut parts = Vec::new();
        if !linked.is_empty() {
            let l: Vec<String> = linked.iter().map(|(t, id)| format!("{t} = {id}")).collect();
            parts.push(format!("linked {}", l.join(", ")));
        }
        if !made.is_empty() {
            parts.push(format!("added {}", made.join(", ")));
        }
        let world = self.world(1)?;
        let summary = format!("Projects from the locus registry: {}", parts.join("; "));
        self.propose(&world, Author::Claude, ops, summary).map(Some)
    }

    /// A locus item named on an event or task (spec K8): a registry id, checked against the
    /// registry when kairosd has one. `""` clears.
    fn locus_ref(&self, id: Option<&str>) -> Result<Option<String>> {
        let Some(id) = id.map(str::trim).filter(|s| !s.is_empty()) else {
            return Ok(None);
        };
        if let Some(reg) = &self.registry
            && reg.get(id).is_none()
        {
            return Err(EngineError::BadRequest(format!(
                "{id:?} is not in the locus registry (taxonomy/registry.toml)"
            )));
        }
        Ok(Some(id.to_string()))
    }

    /// Open chores (spec K9), for locus's Home: how long each needs, how often it was put off,
    /// whether it is overdue, and whether it fits the gap you are in now. Most put off first.
    pub fn chores(&self) -> Result<Vec<api::ChoreItem>> {
        let now = self.now();
        let tz = self.tz();
        let checkins = self.db.checkins(false)?;
        let mut v: Vec<api::ChoreItem> = self
            .db
            .tasks(Some(TaskStatus::Open))?
            .into_iter()
            .filter(|t| t.kind.as_deref() == Some(kairos_core::sched::CHORE))
            .map(|t| {
                let minutes = if t.remaining_min > 0 {
                    t.remaining_min
                } else {
                    t.estimate_min
                }
                .max(5);
                let due = t.due.map(|d| d.instant(tz));
                let fits_now = self
                    .find_free(now, now + Duration::hours(4), i64::from(minutes))
                    .ok()
                    .and_then(|s| s.first().cloned())
                    .is_some_and(|s| s.start <= now + Duration::minutes(5));
                api::ChoreItem {
                    put_off: checkins
                        .iter()
                        .filter(|c| c.task_id == t.id && c.outcome == Some(Outcome::Missed))
                        .count() as u32,
                    overdue: due.is_some_and(|d| d < now),
                    id: t.id,
                    title: t.title,
                    minutes,
                    due,
                    fits_now,
                }
            })
            .collect();
        v.sort_by_key(|c| (std::cmp::Reverse((c.overdue, c.put_off)), c.due));
        Ok(v)
    }

    /// Open projects kairos has that the registry does not (shown by `kairos doctor`).
    pub fn unregistered(&self) -> Result<Vec<String>> {
        if self.registry.is_none() {
            return Ok(Vec::new());
        }
        Ok(self
            .db
            .projects()?
            .into_iter()
            .filter(|p| p.status == TaskStatus::Open && p.registry.is_none())
            .map(|p| p.title)
            .collect())
    }

    /// Project sessions among `instances`: `(instance, project id)`. A
    /// session is any timed event linked to a project (`Meta.project_id`:
    /// planned or accepted through kairos, or attributed).
    fn sessions_in<'a>(&self, instances: &'a [Instance]) -> Vec<(&'a Instance, String)> {
        instances
            .iter()
            .filter(|i| matches!(i.span, Span::Timed { .. }) && i.meta.kind != EventKind::TaskBlock)
            .filter_map(|i| Some((i, i.meta.project_id.clone()?)))
            .collect()
    }

    /// Projects with the time spent on them (what was checked in on their
    /// sessions and on their tasks' blocks), and each project's minutes
    /// planned in the next 7 days.
    fn project_stats(&self) -> Result<(Vec<api::ProjectItem>, std::collections::HashMap<String, u32>)> {
        let projects = self.db.projects()?;
        let mut upcoming = std::collections::HashMap::new();
        if projects.is_empty() {
            return Ok((Vec::new(), upcoming));
        }
        let now = self.now();
        let tz = self.tz();
        let sidecar = self.db.meta_all()?;
        let loaded = load::load_all(&self.cfg, &sidecar)?;
        let mut w = Vec::new();
        let instances = kairos_core::view::instances(
            &loaded.sources,
            now - Duration::days(30),
            now + Duration::days(14),
            tz,
            &mut w,
        );
        let checkins = self.db.checkins(false)?;
        let answered: std::collections::HashSet<&str> = checkins
            .iter()
            .filter(|c| c.outcome.is_some())
            .map(|c| c.block_uid.as_str())
            .collect();
        let today = now.with_timezone(&tz).date_naive();
        let monday = today - Duration::days(i64::from(chrono::Datelike::weekday(&today).num_days_from_monday()));
        let month_ago = today - Duration::days(30);
        let mut items: Vec<api::ProjectItem> = projects
            .iter()
            .map(|t| api::ProjectItem {
                project: self.project_view(t),
                week_min: 0,
                month_min: 0,
                unconfirmed_min: 0,
                last_worked: None,
                next_session: None,
            })
            .collect();
        // Sessions: over and waiting for a check-in, or still to come.
        let mut started: std::collections::HashMap<&str, DateTime<Utc>> = std::collections::HashMap::new();
        for (i, pid) in self.sessions_in(&instances) {
            let Span::Timed { start, end } = i.span else { continue };
            started.insert(i.uid.as_str(), start);
            let Some(it) = items.iter_mut().find(|p| p.project.id == pid) else {
                continue;
            };
            if start < now {
                if !answered.contains(i.uid.as_str()) && !build::is_checked_in(&i.title) {
                    it.unconfirmed_min += (end.min(now) - start).num_minutes().max(0) as u32;
                }
            } else {
                if it.next_session.is_none_or(|n| start < n) {
                    it.next_session = Some(start);
                }
                if start < now + Duration::days(7) {
                    *upcoming.entry(it.project.id.clone()).or_insert(0) += (end - start).num_minutes().max(0) as u32;
                }
            }
        }
        // Time spent is what Alec checked in, never what was planned: on the
        // project's sessions, and on the blocks of its tasks.
        let project_of: std::collections::HashMap<String, String> = self
            .db
            .tasks(None)?
            .into_iter()
            .filter_map(|t| Some((t.id, t.project_id?)))
            .chain(projects.iter().map(|p| (p.id.clone(), p.id.clone())))
            .collect();
        for c in &checkins {
            let (Some(_), Some(pid)) = (c.outcome, project_of.get(&c.task_id)) else {
                continue;
            };
            let Some(it) = items.iter_mut().find(|p| &p.project.id == pid) else {
                continue;
            };
            let actual = c.actual_min.unwrap_or(0);
            if c.day >= month_ago {
                it.month_min += actual;
            }
            if c.day >= monday {
                it.week_min += actual;
            }
            if actual > 0 {
                let t = started
                    .get(c.block_uid.as_str())
                    .copied()
                    .unwrap_or_else(|| kairos_core::time::resolve_local(c.day.and_time(NaiveTime::MIN), tz));
                if it.last_worked.is_none_or(|l| l < t) {
                    it.last_worked = Some(t);
                }
            }
        }
        // Measured time (spec K6): worked spans from Claude sessions, by registry id. They are
        // what was done, so they count without a check-in.
        let spans = self.worked(
            kairos_core::time::resolve_local(month_ago.and_time(NaiveTime::MIN), tz),
            now,
        )?;
        let week_start = kairos_core::time::resolve_local(monday.and_time(NaiveTime::MIN), tz);
        for s in spans {
            let Some(it) = items
                .iter_mut()
                .find(|p| p.project.registry.as_deref() == Some(s.project.as_str()))
            else {
                continue;
            };
            let mins = (s.end - s.start).num_minutes().max(0) as u32;
            it.month_min += mins;
            if s.start >= week_start {
                it.week_min += mins;
            }
            if it.last_worked.is_none_or(|l| l < s.end) {
                it.last_worked = Some(s.end);
            }
        }
        Ok((items, upcoming))
    }

    /// Every project (active, next, paused, done) with time spent.
    pub fn projects(&self) -> Result<Vec<api::ProjectItem>> {
        Ok(self.project_stats()?.0)
    }

    /// A project by id or unique case-insensitive name (prefix).
    pub fn find_project(&self, q: &str) -> Result<Task> {
        let all = self.db.projects()?;
        if let Some(p) = all.iter().find(|p| p.id == q) {
            return Ok(p.clone());
        }
        let ql = q.trim().to_lowercase();
        if let Some(p) = all.iter().find(|p| p.title.to_lowercase() == ql) {
            return Ok(p.clone());
        }
        let mut hits: Vec<Task> = all
            .into_iter()
            .filter(|p| p.title.to_lowercase().starts_with(&ql))
            .collect();
        match hits.len() {
            0 => Err(EngineError::NotFound(format!("project {q:?}"))),
            1 => Ok(hits.remove(0)),
            n => Err(EngineError::BadRequest(format!(
                "{q:?} matches {n} projects: {}",
                hits.iter().map(|p| p.title.as_str()).collect::<Vec<_>>().join(", ")
            ))),
        }
    }

    fn project_status(s: Option<&str>) -> Result<Option<ProjectStatus>> {
        s.map(|s| {
            ProjectStatus::parse(s)
                .ok_or_else(|| EngineError::BadRequest(format!("status must be active|next|paused|done, got {s:?}")))
        })
        .transpose()
    }

    fn project_name_free(&self, name: &str, except: Option<&str>) -> Result<()> {
        if self
            .db
            .projects()?
            .iter()
            .any(|o| Some(o.id.as_str()) != except && o.title.eq_ignore_ascii_case(name))
        {
            return Err(EngineError::BadRequest(format!(
                "there's already a project called {name:?}"
            )));
        }
        Ok(())
    }

    pub fn create_project(&mut self, author: Author, req: &api::CreateProjectReq) -> Result<Project> {
        let mut name = req.name.trim().to_string();
        if name.is_empty() {
            return Err(EngineError::BadRequest("a project needs a name".into()));
        }
        // New projects come from the registry (spec K7), never from kairos alone.
        let mut registry = None;
        if let Some(reg) = &self.registry {
            let Some(e) = project::registry_match(&name, reg) else {
                return Err(EngineError::BadRequest(format!(
                    "{name:?} is not in the locus registry: add it to taxonomy/registry.toml (one list of \
                     projects for every app), then it appears here by itself"
                )));
            };
            if self
                .db
                .projects()?
                .iter()
                .any(|p| p.registry.as_deref() == Some(e.id.as_str()))
            {
                return Err(EngineError::BadRequest(format!("{} is already a project", e.name)));
            }
            name = e.name.clone();
            registry = Some(e.id.clone());
        }
        let name = name.as_str();
        self.project_name_free(name, None)?;
        let status = Self::project_status(req.status.as_deref())?.unwrap_or_default();
        let key = self.project_key(req.key.as_deref())?;
        let mut t = project::new_task(
            name,
            status,
            req.note.as_deref().unwrap_or(""),
            &key,
            author,
            self.now(),
        );
        t.registry = registry;
        let world = self.world(1)?;
        self.propose(
            &world,
            author,
            vec![Op::CreateTask {
                after: Box::new(t.clone()),
            }],
            format!("New project {name:?}"),
        )?;
        Ok(self.project_view(&t))
    }

    /// Rename a project, change its status, note or calendar: a task update,
    /// so a new name or calendar carries through to its sessions still to
    /// come (one changeset, one undo).
    pub fn update_project(
        &mut self,
        author: Author,
        id: &str,
        req: &api::UpdateProjectReq,
    ) -> Result<api::UpdateProjectResp> {
        let before = self.find_project(id)?;
        let mut t = before.clone();
        if let Some(n) = req.name.as_deref().map(str::trim) {
            if n.is_empty() {
                return Err(EngineError::BadRequest("a project needs a name".into()));
            }
            self.project_name_free(n, Some(&t.id))?;
            t.title = n.to_string();
        }
        if let Some(s) = Self::project_status(req.status.as_deref())? {
            project::set_status(&mut t, s);
        }
        if let Some(n) = &req.note {
            t.notes = n.trim().to_string();
        }
        if req.key.is_some() {
            t.key = self.project_key(req.key.as_deref())?;
        }
        t.updated_at = self.now();
        let world = self.world(60)?;
        let mut ops = vec![Op::UpdateTask {
            before: Box::new(before.clone()),
            after: Box::new(t.clone()),
        }];
        if t.title != before.title || t.key != before.key {
            ops.extend(self.follow_task_ops(&world, &t, &before, &[])?);
        }
        let summary = if t.title != before.title {
            format!("Rename project {:?} to {:?}", before.title, t.title)
        } else {
            format!("Update project {:?}", t.title)
        };
        let cs = self.propose(&world, author, ops, summary)?;
        Ok(api::UpdateProjectResp {
            project: self.project_view(&t),
            changeset: WriteResult::from(&cs),
        })
    }

    /// A project's calendar: a configured key, or "" for the projects key
    /// (the default key if that isn't configured).
    fn project_key(&self, k: Option<&str>) -> Result<String> {
        match k.map(str::trim).filter(|k| !k.is_empty()) {
            Some(k) => Ok(self
                .cfg
                .key(k)
                .map(|x| x.name.clone())
                .ok_or_else(|| EngineError::BadRequest(format!("no calendar called {k:?}")))?),
            None => self
                .projects_key()
                .or_else(|| self.cfg.default_key().map(|x| x.name.clone()))
                .ok_or_else(|| EngineError::BadRequest("no calendar configured".into())),
        }
    }

    /// The calendar a project's sessions go in.
    fn session_key(&self, p: &Task) -> Option<String> {
        Some(p.key.clone())
            .filter(|k| self.cfg.key(k).is_some())
            .or_else(|| self.projects_key())
    }

    /// Planning time for a "next" project means it's under way.
    fn project_under_way(&mut self, p: &Task) -> Result<()> {
        if p.status == TaskStatus::Open && p.stage == Some(ProjectStatus::Next) {
            let mut x = p.clone();
            x.stage = Some(ProjectStatus::Active);
            x.updated_at = self.now();
            self.db.task_put(&x)?;
        }
        Ok(())
    }

    /// Plan project time by hand: an event in the project's calendar, titled
    /// with the project and linked to it as a session (normal risk rules).
    pub fn create_session(&mut self, author: Author, project: &str, req: &api::SessionReq) -> Result<WriteResult> {
        let p = self.find_project(project)?;
        let cs = self.create_event(
            author,
            &api::CreateEventReq {
                title: build::session_title(&p.title),
                start: req.start.clone(),
                end: req.end.clone(),
                duration_min: req.duration_min.or(Some(120)).filter(|_| req.end.is_none()),
                key: self.session_key(&p),
                description: Some(PROJECT_MARK.into()),
                ..Default::default()
            },
        )?;
        self.link_created(&cs, &p.id)?;
        self.project_under_way(&p)?;
        self.replan_soon = true;
        Ok(WriteResult::from(&cs))
    }

    /// Count existing events as a project's sessions (time spent comes from
    /// their check-ins). Returns how many were linked.
    pub fn link_sessions(&mut self, project: &str, uids: &[String]) -> Result<usize> {
        let p = self.find_project(project)?;
        let world = self.world(60)?;
        let mut n = 0;
        for uid in uids {
            let src = self.locate(&world, None, uid)?.source.id;
            self.link_session(&src, uid, &p.id)?;
            n += 1;
        }
        Ok(n)
    }

    /// Events that look like a project's sessions but aren't linked: titled
    /// with its name, in the last 30 days or the next 14 (for "attribute").
    pub fn session_suggestions(&self, project: &str) -> Result<Vec<Instance>> {
        let p = self.find_project(project)?;
        let world = self.world(14)?;
        let name = p.title.trim().to_lowercase();
        let since = self.now() - Duration::days(30);
        let mut seen = std::collections::HashSet::new();
        Ok(world
            .instances
            .iter()
            .filter(|i| i.meta.project_id.is_none() && i.meta.kind == EventKind::Event && i.start(self.tz()) >= since)
            .filter(|i| build::bare_block_title(&i.title).to_lowercase().contains(&name))
            .filter(|i| seen.insert(i.uid.clone()))
            .cloned()
            .collect())
    }

    /// Link an event as a project's session. The link lives in the sidecar
    /// (authoritative, preserved on rewrites), so the TUI draws it as a
    /// session too.
    fn link_session(&mut self, source: &str, uid: &str, project_id: &str) -> Result<()> {
        let mut m = self.db.meta_get(source, uid)?.unwrap_or_default();
        m.project_id = Some(project_id.to_string());
        self.db.meta_set(source, uid, &m)?;
        Ok(())
    }

    /// Link the event a changeset created as a project's session.
    fn link_created(&mut self, cs: &Changeset, project_id: &str) -> Result<()> {
        if let Some((src, uid)) = cs.ops.iter().find_map(|o| match o {
            Op::CreateEvent { item, uid, .. } => Some((item.source.clone(), uid.clone())),
            _ => None,
        }) {
            self.link_session(&src, &uid, project_id)?;
        }
        Ok(())
    }

    /// Delete a project: dropped like a task (its sessions stay on the
    /// calendar; their time just isn't counted anywhere).
    pub fn delete_project(&mut self, author: Author, id: &str) -> Result<WriteResult> {
        let before = self.find_project(id)?;
        let mut t = before.clone();
        t.status = TaskStatus::Dropped;
        t.updated_at = self.now();
        let world = self.world(1)?;
        let cs = self.propose(
            &world,
            author,
            vec![Op::UpdateTask {
                before: Box::new(before),
                after: Box::new(t.clone()),
            }],
            format!("Delete project {:?}", t.title),
        )?;
        Ok(WriteResult::from(&cs))
    }

    /// One-off: projects and session links from before projects were tasks.
    /// Projects become project tasks (same ids, so check-ins still count);
    /// links move into the sidecar, and so do events that counted only
    /// because they were titled with a project's name. Then the old tables go.
    pub fn migrate_projects(&mut self) -> Result<usize> {
        let (old, links) = self.db.legacy_projects()?;
        if old.is_empty() && links.is_empty() {
            self.db.drop_legacy_projects()?;
            return Ok(0);
        }
        let pkey = self.projects_key();
        for p in &old {
            if self.db.task_get(&p.id)?.is_some() {
                continue;
            }
            let key = match p.key.clone().filter(|k| self.cfg.key(k).is_some()) {
                Some(k) => k,
                None => self.project_key(None)?,
            };
            let mut t = project::new_task(&p.name, p.status, &p.note, &key, Author::Alec, p.created_at);
            t.id = p.id.clone();
            t.updated_at = p.updated_at;
            self.db.task_put(&t)?;
        }
        let world = self.world(1)?;
        let mut n = 0;
        let mut linked: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (uid, pid) in &links {
            let Ok(loc) = self.locate(&world, None, uid) else {
                continue;
            };
            self.link_session(&loc.source.id, uid, pid)?;
            linked.insert(uid.clone());
            n += 1;
        }
        for s in &world.loaded.sources {
            for e in &s.series {
                let Some(m) = &e.master else { continue };
                if linked.contains(&e.uid) || m.meta.project_id.is_some() {
                    continue;
                }
                let bare = build::bare_block_title(&m.summary);
                let hit = old.iter().find(|p| {
                    let key = p.key.clone().or_else(|| pkey.clone());
                    key.is_some() && s.key == key && p.name.trim().eq_ignore_ascii_case(bare)
                });
                if let Some(p) = hit {
                    self.link_session(&s.id, &e.uid, &p.id)?;
                    n += 1;
                }
            }
        }
        self.db.drop_legacy_projects()?;
        tracing::info!(projects = old.len(), sessions = n, "migrated projects to tasks");
        Ok(n)
    }

    /// Accept proposed project time: it becomes an event (titled with what
    /// to work on) in the projects key.
    pub fn accept_proposal(&mut self, author: Author, id: &str, req: &api::AcceptProposalReq) -> Result<WriteResult> {
        let p = self
            .proposals()?
            .into_iter()
            .find(|p| p.id == id)
            .ok_or_else(|| EngineError::NotFound(format!("proposal {id}")))?;
        // What to work on: the project picked, a project's name typed, the
        // suggested project — or just a title.
        let typed = req.title.as_deref().map(str::trim).filter(|t| !t.is_empty());
        let project = match (&req.project_id, typed) {
            (Some(pid), _) => Some(self.find_project(pid)?),
            (None, Some(t)) => self
                .db
                .projects()?
                .into_iter()
                .find(|x| x.title.eq_ignore_ascii_case(t)),
            (None, None) => match &p.project_id {
                Some(pid) => self.find_project(pid).ok(),
                None => None,
            },
        };
        let title = project
            .as_ref()
            .map(|x| build::session_title(&x.title))
            .or(typed.map(str::to_string))
            .unwrap_or_else(|| p.title.clone());
        let key = match &project {
            Some(x) => self.session_key(x),
            None => self.projects_key(),
        };
        let cs = self.create_event(
            author,
            &api::CreateEventReq {
                title,
                start: p.start.to_rfc3339(),
                end: Some(p.end.to_rfc3339()),
                // The project's calendar, else the projects key (else the default key).
                key,
                description: Some(PROJECT_MARK.into()),
                ..Default::default()
            },
        )?;
        self.db.proposal_set_status(id, "accepted")?;
        if let Some(x) = project {
            self.link_created(&cs, &x.id)?;
            self.project_under_way(&x)?;
        }
        self.replan_soon = true;
        Ok(WriteResult::from(&cs))
    }

    /// Drop proposed project time; that span isn't proposed again.
    pub fn reject_proposal(&mut self, id: &str) -> Result<()> {
        if !self.db.proposal_set_status(id, "rejected")? {
            return Err(EngineError::NotFound(format!("proposal {id}")));
        }
        Ok(())
    }

    fn task_type(&self, name: &str) -> Result<Option<kairos_core::constraints::TaskType>> {
        match self.constraints.task_type(name) {
            Some(t) => Ok(Some(t.clone())),
            None => Err(EngineError::BadRequest(format!(
                "unknown task type {name:?} (have: {})",
                self.constraints
                    .task_types
                    .iter()
                    .map(|t| t.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    }

    /// Type for a task given none: up to an hour is admin, longer is a sheet.
    fn inferred_type(&self, estimate_min: u32) -> Option<kairos_core::constraints::TaskType> {
        let name = if estimate_min > 0 && estimate_min <= 60 {
            "admin"
        } else {
            "sheet"
        };
        self.constraints.task_type(name).cloned()
    }

    /// The learned ratio for a task's type (1 when there's none yet).
    fn ratio_for(&self, t: &Task) -> Result<f32> {
        let kind = t
            .kind
            .clone()
            .or_else(|| self.inferred_type(t.estimate_min).map(|x| x.name));
        let r = kind
            .and_then(|k| self.estimate_ratios().ok()?.get(&k).map(|x| x.0))
            .filter(|r| (r - 1.0).abs() >= 0.05);
        Ok(r.unwrap_or(1.0))
    }

    /// Learned estimate ratios per task type: median actual/estimate of
    /// finished tasks (≥ 3), clamped to ×0.7–×2.
    fn estimate_ratios(&self) -> Result<std::collections::BTreeMap<String, (f32, usize)>> {
        let mut by: std::collections::BTreeMap<String, Vec<f32>> = std::collections::BTreeMap::new();
        for r in self.db.estimates()? {
            if r.estimate_min == 0 {
                continue;
            }
            let kind = match self.db.task_get(&r.task_id)? {
                Some(t) => t.kind.or_else(|| self.inferred_type(t.estimate_min).map(|x| x.name)),
                None => None,
            };
            if let Some(k) = kind {
                by.entry(k)
                    .or_default()
                    .push(r.actual_min as f32 / r.estimate_min as f32);
            }
        }
        Ok(by
            .into_iter()
            .filter(|(_, v)| v.len() >= 3)
            .map(|(k, mut v)| {
                v.sort_by(|a, b| a.total_cmp(b));
                let n = v.len();
                let med = if n % 2 == 1 {
                    v[n / 2]
                } else {
                    (v[n / 2 - 1] + v[n / 2]) / 2.0
                };
                (k, (med.clamp(0.7, 2.0), n))
            })
            .collect())
    }

    pub fn find_task(&self, q: &str) -> Result<Task> {
        let mut hits = self.db.task_find(q)?;
        match hits.len() {
            0 => Err(EngineError::NotFound(format!("task {q:?}"))),
            1 => Ok(hits.remove(0)),
            n => Err(EngineError::BadRequest(format!(
                "{q:?} matches {n} tasks: {}",
                hits.iter()
                    .map(|t| format!("{} ({})", t.title, t.id))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    }

    /// Log work done on a task that wasn't in a block ("did 40 min just now"):
    /// a done block ending now (✓, pinned), counted like a check-in.
    pub fn log_work(&mut self, author: Author, id: &str, minutes: u32) -> Result<api::TaskResp> {
        let t = self.find_task(id)?;
        if t.status != TaskStatus::Open {
            return Err(EngineError::BadRequest(format!("{:?} isn't open", t.title)));
        }
        if minutes == 0 {
            return Err(EngineError::BadRequest("how long? (minutes > 0)".into()));
        }
        let tz = self.tz();
        let now = self.now();
        let end = DateTime::from_timestamp(now.timestamp() / 60 * 60, 0).unwrap_or(now);
        let start = end - Duration::minutes(i64::from(minutes));
        let world = self.world(1)?;
        let (target, origin_key) = self.target_for_key(&t.key)?;
        let uid = build::new_uid();
        let mut meta = self.author_meta(author);
        meta.origin_key = origin_key;
        meta.kind = EventKind::TaskBlock;
        meta.task_id = Some(t.id.clone());
        meta.pinned = true;
        let ev = NewEvent {
            uid: uid.clone(),
            title: build::done_block_title(&t.title),
            time: NewTime::Timed { start, end },
            location: None,
            description: Some("Logged work (wasn't a planned block)".into()),
            meta,
            alarms: vec![],
            transparent: false,
            rrule: None,
        };
        let after = build::event_file(&ev, tz, now);
        let item = Item {
            source: target.id.clone(),
            href: vdir::href_for_uid(&uid),
        };
        let summary = format!(
            "Log {} on {:?} ({}–{})",
            fmt_min(minutes),
            t.title,
            start.with_timezone(&tz).format("%H:%M"),
            end.with_timezone(&tz).format("%H:%M")
        );
        let cs = self.propose(
            &world,
            author,
            vec![Op::CreateEvent {
                item,
                uid: uid.clone(),
                after,
            }],
            summary,
        )?;
        if cs.status == Status::Applied {
            let b = Block {
                uid: Some(uid.clone()),
                task_id: t.id.clone(),
                start,
                end,
                pinned: true,
                why: None,
            };
            self.answer_checkin(&uid, &b, true, Outcome::Done, Some(minutes))?;
        }
        let plan = if cs.status == Status::Applied {
            self.replan_soon = false;
            Some(self.plan(false, Author::Scheduler)?)
        } else {
            None
        };
        Ok(api::TaskResp {
            task: self.find_task(&t.id)?,
            changesets: vec![WriteResult::from(&cs)],
            plan,
        })
    }

    /// Start working on a task right now: a pinned block from now (a full
    /// block, or `minutes`; never past the next fixed event or the deadline),
    /// then a replan, so later blocks shrink or go.
    pub fn start_task(&mut self, author: Author, id: &str, minutes: Option<u32>) -> Result<api::TaskResp> {
        let t = self.find_task(id)?;
        if t.status != TaskStatus::Open {
            return Err(EngineError::BadRequest(format!("{:?} isn't open", t.title)));
        }
        let tz = self.tz();
        let now = self.now();
        // Now, rounded down to the scheduler's slot (so the plan adds up exactly).
        let slot = i64::from(self.constraints.slot_minutes.max(1)) * 60;
        let start = DateTime::from_timestamp(now.timestamp() / slot * slot, 0).unwrap_or(now);
        let want = minutes.unwrap_or_else(|| t.remaining_min.min(t.max_chunk_min.max(t.min_chunk_min)));
        let mut end = start + Duration::minutes(i64::from(want.max(10)));
        let mut why = String::new();
        let world = self.world(2)?;
        if let Some(next) = world
            .instances
            .iter()
            .filter(|i| is_busy(i))
            .map(|i| i.start(tz))
            .filter(|s| *s > start)
            .min()
            && next < end
        {
            end = next;
            why = format!(
                " (until {} — something starts then)",
                next.with_timezone(&tz).format("%H:%M")
            );
        }
        if let Some(d) = t.due.map(|d| d.instant(tz))
            && d < end
        {
            end = d;
            why = " (until the deadline)".into();
        }
        if end - now < Duration::minutes(15) {
            return Err(EngineError::BadRequest(format!(
                "no time now: something starts at {}",
                end.with_timezone(&tz).format("%H:%M")
            )));
        }
        // This replaces the task's own (unpinned) blocks it overlaps, e.g. one
        // that was planned to have started already.
        let mut changesets = Vec::new();
        let overlapped: Vec<(Option<String>, String)> = self
            .blocks(&world)
            .into_iter()
            .filter(|(b, _)| b.task_id == t.id && !b.pinned && b.start < end && b.end > start)
            .map(|(_, i)| (Some(i.source.clone()), i.uid.clone()))
            .collect();
        if !overlapped.is_empty() {
            let d = self.delete_events(
                author,
                &overlapped,
                Some(&format!("Make room to start {:?} now", t.title)),
            )?;
            changesets.push(WriteResult::from(&d));
        }
        let cs = self.create_event(
            author,
            &api::CreateEventReq {
                title: t.title.clone(),
                start: start.to_rfc3339(),
                end: Some(end.to_rfc3339()),
                key: Some(t.key.clone()),
                description: Some(format!("Started now{why}")),
                task_id: Some(t.id.clone()),
                ..Default::default()
            },
        )?;
        let plan = if cs.status == Status::Applied {
            self.replan_soon = false;
            Some(self.plan(false, Author::Scheduler)?)
        } else {
            None
        };
        changesets.push(WriteResult::from(&cs));
        Ok(api::TaskResp {
            task: self.find_task(&t.id)?,
            changesets,
            plan,
        })
    }

    pub fn complete_task(&mut self, author: Author, id: &str, actual_min: Option<u32>) -> Result<api::TaskResp> {
        let before = self.find_task(id)?;
        let mut t = before.clone();
        t.status = TaskStatus::Done;
        t.remaining_min = 0;
        t.updated_at = self.now();
        let world = self.world(i64::from(self.constraints.horizon_days) + 60)?;
        // Blocks not started yet (pinned too) are no longer needed.
        let now = self.now();
        let mut ops = vec![Op::UpdateTask {
            before: Box::new(before.clone()),
            after: Box::new(t.clone()),
        }];
        ops.extend(self.block_deletes(&world, &t.id, |b| b.start >= now)?);
        // A finished task's ⚑ still ahead goes too (nothing is due any more).
        let deadline = self.deadline_delete(&world, &t, true);
        let and_deadline = if deadline.is_some() { " (and its deadline)" } else { "" };
        ops.extend(deadline);
        let cs = self.propose(&world, author, ops, format!("Complete {:?}{and_deadline}", t.title))?;
        let actual = actual_min.unwrap_or_else(|| self.actual_minutes(&t.id).unwrap_or(t.estimate_min));
        self.db.estimate_log(&EstimateRow {
            task_id: t.id.clone(),
            title: t.title.clone(),
            estimate_min: t.estimate_min,
            actual_min: actual,
            completed_at: self.now(),
        })?;
        let plan = if cs.status == Status::Applied {
            Some(self.plan(false, Author::Scheduler)?)
        } else {
            None
        };
        Ok(api::TaskResp {
            task: t,
            changesets: vec![WriteResult::from(&cs)],
            plan,
        })
    }

    fn actual_minutes(&self, task_id: &str) -> Option<u32> {
        let rows = self.db.checkins(false).ok()?;
        let total: u32 = rows
            .iter()
            .filter(|c| c.task_id == task_id)
            .filter_map(|c| c.actual_min)
            .sum();
        (total > 0).then_some(total)
    }

    pub fn drop_task(&mut self, author: Author, id: &str) -> Result<api::TaskResp> {
        let before = self.find_task(id)?;
        let mut t = before.clone();
        t.status = TaskStatus::Dropped;
        t.updated_at = self.now();
        let world = self.world(i64::from(self.constraints.horizon_days) + 60)?;
        // The task and every block still ahead of it (pinned and in-progress
        // ones too) go in one changeset: one approval, one undo.
        let now = self.now();
        let mut ops = vec![Op::UpdateTask {
            before: Box::new(before),
            after: Box::new(t.clone()),
        }];
        ops.extend(self.block_deletes(&world, &t.id, |b| b.end > now)?);
        let n = ops.len() - 1;
        // Its ⚑ deadline goes too (a dropped task has no deadline).
        let deadline = self.deadline_delete(&world, &t, false);
        let and_deadline = if deadline.is_some() { " and its deadline" } else { "" };
        ops.extend(deadline);
        let summary = match n {
            0 => format!("Drop task {:?}{and_deadline}", t.title),
            1 => format!("Drop task {:?}, its scheduled block{and_deadline}", t.title),
            n => format!("Drop task {:?}, its {n} scheduled blocks{and_deadline}", t.title),
        };
        let cs = self.propose(&world, author, ops, summary)?;
        let plan = if cs.status == Status::Applied {
            Some(self.plan(false, Author::Scheduler)?)
        } else {
            None
        };
        Ok(api::TaskResp {
            task: t,
            changesets: vec![WriteResult::from(&cs)],
            plan,
        })
    }

    /// Retitle / re-key a task's future blocks and its ⚑ after the task's
    /// title or calendar changed (skipping events `already` touches).
    fn follow_task_ops(&self, world: &World, t: &Task, before: &Task, already: &[Op]) -> Result<Vec<Op>> {
        let tz = self.tz();
        let now = self.now();
        let touched: Vec<&str> = already
            .iter()
            .filter_map(|o| match o {
                Op::UpdateEvent { uid, .. } | Op::DeleteEvent { uid, .. } | Op::CreateEvent { uid, .. } => {
                    Some(uid.as_str())
                }
                _ => None,
            })
            .collect();
        let (target, _) = self.target_for_key(&t.key)?;
        // A task's blocks still to come; a project's sessions still to come
        // that are titled with its name (one Alec attributed under another
        // title keeps it) and are in its calendar (or they stay put).
        let mut uids: Vec<(String, bool)> = if t.is_project() {
            let old_target = self.target_for_key(&before.key).map(|x| x.0.id).unwrap_or_default();
            self.sessions_in(&world.instances)
                .into_iter()
                .filter(|(i, pid)| pid == &t.id && i.span.bounds(tz).1 > now)
                .filter(|(i, _)| build::bare_block_title(&i.title).eq_ignore_ascii_case(before.title.trim()))
                .filter(|(i, _)| t.key == before.key || i.source == old_target)
                .map(|(i, _)| (i.uid.clone(), false))
                .collect()
        } else {
            self.blocks(world)
                .into_iter()
                .filter(|(b, _)| b.task_id == t.id && b.end > now)
                .filter_map(|(b, _)| b.uid.map(|u| (u, false)))
                .collect()
        };
        uids.dedup();
        if let Some(d) = &t.deadline_uid {
            uids.push((d.clone(), true));
        }
        let mut ops = Vec::new();
        for (uid, is_deadline) in uids {
            if touched.contains(&uid.as_str()) {
                continue;
            }
            let Ok(loc) = self.locate(world, None, &uid) else {
                continue;
            };
            let after = if t.title != before.title {
                let title = if is_deadline {
                    build::deadline(
                        &uid,
                        &t.title,
                        t.due.unwrap_or(api::TimeIn::At(now).as_due()),
                        Meta::default(),
                        &[],
                    )
                    .title
                } else {
                    // Keeps its box or diamond (☐ / ✓ / ◇ / ◆).
                    let cur = loc.series.master.as_ref().map_or("", |m| m.summary.as_str());
                    build::retitle_block(cur, &t.title)
                };
                let edit = Edit {
                    title: Some(title),
                    ..Default::default()
                };
                build::edit_file(&loc.text, &uid, &edit, tz, now).map_err(|e| EngineError::BadRequest(e.to_string()))?
            } else {
                loc.text.clone()
            };
            let from = Item {
                source: loc.source.id.clone(),
                href: loc.href.clone(),
            };
            if t.key != before.key && loc.source.id != target.id {
                ops.push(Op::MoveEvent {
                    from,
                    to: Item {
                        source: target.id.clone(),
                        href: loc.href.clone(),
                    },
                    uid,
                    before: loc.text,
                    after,
                    share_to_family: false,
                });
            } else if after != loc.text {
                ops.push(Op::UpdateEvent {
                    item: from,
                    uid,
                    before: loc.text,
                    after,
                });
            }
        }
        Ok(ops)
    }

    /// The op deleting a task's ⚑ (only if it's still ahead, when `future_only`).
    fn deadline_delete(&self, world: &World, t: &Task, future_only: bool) -> Option<Op> {
        let loc = t
            .deadline_uid
            .as_deref()
            .and_then(|u| self.locate(world, None, u).ok())?;
        if future_only && t.due.is_some_and(|d| d.instant(self.tz()) <= self.now()) {
            return None;
        }
        Some(Op::DeleteEvent {
            item: Item {
                source: loc.source.id.clone(),
                href: loc.href.clone(),
            },
            uid: loc.series.uid.clone(),
            before: loc.text,
        })
    }

    /// Delete ops for a task's blocks that match `want`.
    fn block_deletes(&self, world: &World, task_id: &str, want: impl Fn(&Block) -> bool) -> Result<Vec<Op>> {
        let mut ops = Vec::new();
        for (b, _) in self.blocks(world) {
            if b.task_id != task_id || !want(&b) {
                continue;
            }
            let Some(uid) = &b.uid else { continue };
            let loc = self.locate(world, None, uid)?;
            ops.push(Op::DeleteEvent {
                item: Item {
                    source: loc.source.id.clone(),
                    href: loc.href.clone(),
                },
                uid: uid.clone(),
                before: loc.text,
            });
        }
        Ok(ops)
    }

    // ------------------------------------------------------------ scheduling

    /// Task blocks in the world (any writable source), as scheduler blocks.
    fn blocks(&self, world: &World) -> Vec<(Block, Instance)> {
        world
            .instances
            .iter()
            .filter(|i| i.meta.kind == EventKind::TaskBlock && i.meta.task_id.is_some())
            .filter_map(|i| match i.span {
                Span::Timed { start, end } => Some((
                    Block {
                        uid: Some(i.uid.clone()),
                        task_id: i.meta.task_id.clone().unwrap_or_default(),
                        start,
                        end,
                        pinned: i.meta.pinned,
                        why: None,
                    },
                    i.clone(),
                )),
                Span::AllDay { .. } => None,
            })
            .collect()
    }

    /// Settle past blocks: unanswered ones that ended before today's
    /// auto-done cutoff count as done.
    pub fn settle(&mut self) -> Result<usize> {
        let now = self.now();
        let tz = self.tz();
        let world = self.world(1)?;
        let today = now.with_timezone(&tz).date_naive();
        // Only blocks that have waited `autodone_after_days` (a week) count as
        // done by themselves; anything newer waits for Alec's check-in.
        let days = i64::from(self.cfg.notify.autodone_after_days);
        let boundary = kairos_core::time::resolve_local((today - Duration::days(days)).and_time(NaiveTime::MIN), tz);
        let answered: std::collections::HashSet<String> = self
            .db
            .checkins(false)?
            .into_iter()
            .filter(|c| c.outcome.is_some())
            .map(|c| c.block_uid)
            .collect();
        let mut n = 0;
        // Work blocks and project sessions alike.
        for (b, inst) in self.checkin_blocks(&world) {
            let Some(uid) = b.uid.clone() else { continue };
            if b.end > boundary || answered.contains(&uid) || build::is_checked_in(&inst.title) {
                continue;
            }
            self.answer_checkin(&uid, &b, recordable(&inst), Outcome::Done, None)?;
            n += 1;
        }
        Ok(n)
    }

    /// Record a check-in on a work block or a project session: the time
    /// counts (for the task's remaining, or the project's time spent) and the
    /// calendar shows it — except on events kairos didn't make (Alec's own,
    /// attributed to a project) or repeating ones, which aren't touched.
    fn answer_checkin(
        &mut self,
        uid: &str,
        b: &Block,
        record: bool,
        outcome: Outcome,
        actual: Option<u32>,
    ) -> Result<()> {
        let tz = self.tz();
        let planned = b.minutes().max(0) as u32;
        self.db.checkin_upsert_planned(&Checkin {
            block_uid: uid.into(),
            task_id: b.task_id.clone(),
            day: b.start.with_timezone(&tz).date_naive(),
            planned_min: planned,
            outcome: None,
            actual_min: None,
            answered_at: None,
        })?;
        let actual = match outcome {
            Outcome::Done => actual.unwrap_or(planned),
            Outcome::Partial => actual.unwrap_or(planned / 2).min(planned),
            Outcome::Missed => 0,
        };
        self.db.checkin_answer(uid, outcome, actual, self.now())?;
        if let Some(mut t) = self.db.task_get(&b.task_id)?
            && t.status == TaskStatus::Open
            && !t.is_project()
        {
            // Remaining is in estimate units: with a learned ratio (sheets take
            // ×1.5), 90 min of work uses up 60 min of the estimate. A task
            // never finishes by itself when it reaches 0 — only when Alec says
            // so (it then asks "finished, or how much more?").
            let ratio = self.ratio_for(&t)?;
            let used = (actual as f32 / ratio).round() as u32;
            t.remaining_min = t.remaining_min.saturating_sub(used);
            t.updated_at = self.now();
            self.db.task_put(&t)?;
        }
        if record {
            self.record_checkin(uid, b, outcome, actual)?;
        }
        Ok(())
    }

    /// Everything that gets a check-in: work blocks, and project sessions
    /// (their `task_id` is the project's).
    fn checkin_blocks(&self, world: &World) -> Vec<(Block, Instance)> {
        let mut out = self.blocks(world);
        out.extend(self.sessions_in(&world.instances).into_iter().filter_map(|(i, pid)| {
            let Span::Timed { start, end } = i.span else {
                return None;
            };
            Some((
                Block {
                    uid: Some(i.uid.clone()),
                    task_id: pid,
                    start,
                    end,
                    pinned: true,
                    why: None,
                },
                i.clone(),
            ))
        }));
        out
    }

    /// Make the calendar show what happened to a checked-in block: `✓` in
    /// its title (trimmed to the time actually done), or gone if missed (that
    /// time is planned again elsewhere).
    fn record_checkin(&mut self, uid: &str, b: &Block, outcome: Outcome, actual: u32) -> Result<()> {
        let world = self.world(2)?;
        let Ok(loc) = self.locate(&world, None, uid) else {
            return Ok(());
        };
        let title = loc
            .series
            .master
            .as_ref()
            .map(|m| m.summary.clone())
            .unwrap_or_default();
        let item = Item {
            source: loc.source.id.clone(),
            href: loc.href.clone(),
        };
        let (op, what) = match outcome {
            Outcome::Missed => (
                Op::DeleteEvent {
                    item,
                    uid: uid.into(),
                    before: loc.text,
                },
                "missed",
            ),
            Outcome::Done | Outcome::Partial => {
                let planned = b.minutes().max(0);
                // Took longer: the block grows to the time spent, as far as
                // free time after it (and now) allows.
                let longer = (outcome == Outcome::Done && i64::from(actual) > planned).then(|| {
                    let next_busy = world
                        .instances
                        .iter()
                        .filter(|i| i.uid != uid && is_busy(i))
                        .filter_map(|i| match i.span {
                            Span::Timed { start, end } if end > b.end => Some(start.max(b.end)),
                            _ => None,
                        })
                        .min();
                    let mut end = (b.start + Duration::minutes(i64::from(actual))).min(self.now().max(b.end));
                    if let Some(n) = next_busy {
                        end = end.min(n);
                    }
                    end
                });
                let edit = Edit {
                    title: (!build::is_checked_in(&title)).then(|| {
                        if self
                            .db
                            .task_get(&b.task_id)
                            .ok()
                            .flatten()
                            .is_some_and(|t| t.is_project())
                        {
                            build::session_done_title(&title)
                        } else {
                            build::done_block_title(&title)
                        }
                    }),
                    time: match longer {
                        Some(end) if end > b.end => Some(NewTime::Timed { start: b.start, end }),
                        _ => (actual > 0 && i64::from(actual) < planned).then(|| NewTime::Timed {
                            start: b.start,
                            end: b.start + Duration::minutes(i64::from(actual)),
                        }),
                    },
                    ..Default::default()
                };
                if edit.title.is_none() && edit.time.is_none() {
                    return Ok(());
                }
                let after = build::edit_file(&loc.text, uid, &edit, self.tz(), self.now())
                    .map_err(|e| EngineError::BadRequest(e.to_string()))?;
                (
                    Op::UpdateEvent {
                        item,
                        uid: uid.into(),
                        before: loc.text,
                        after,
                    },
                    if outcome == Outcome::Done {
                        "done"
                    } else {
                        "partly done"
                    },
                )
            }
        };
        let summary = format!("Check-in: {:?} {what}", build::bare_block_title(&title));
        self.propose(&world, Author::Scheduler, vec![op], summary)?;
        Ok(())
    }

    /// Today's blocks and project sessions for the check-in.
    pub fn checkin_items(&self) -> Result<Vec<api::CheckinItem>> {
        let now = self.now();
        let tz = self.tz();
        let today = now.with_timezone(&tz).date_naive();
        let world = self.world(1)?;
        let rows = self.db.checkins(false)?;
        let mut out = Vec::new();
        // Today's started blocks (answered or not), and every earlier block
        // still unanswered (the last week; older ones were auto-done).
        for (b, inst) in self.checkin_blocks(&world) {
            if b.start > now {
                continue;
            }
            let uid = b.uid.clone().unwrap_or_default();
            let outcome = rows
                .iter()
                .find(|r| r.block_uid == uid)
                .and_then(|r| r.outcome.map(|o| o.as_str().to_string()));
            let day = b.start.with_timezone(&tz).date_naive();
            if day != today && (outcome.is_some() || build::is_checked_in(&inst.title)) {
                continue;
            }
            let task = self.db.task_get(&b.task_id)?;
            let project = task.as_ref().is_some_and(|t| t.is_project());
            out.push(api::CheckinItem {
                block_uid: uid,
                task_id: b.task_id.clone(),
                title: build::bare_block_title(&inst.title).to_string(),
                start: b.start,
                end: b.end,
                planned_min: b.minutes().max(0) as u32,
                outcome,
                // Coursework (estimates move), or this block uses up what's
                // left: "finished, or how much more?". Projects have no estimate.
                ask_remaining: match &task {
                    Some(t) if !project => {
                        let used = (b.minutes().max(0) as f32 / self.ratio_for(t)?).round() as u32;
                        t.kind.as_deref() == Some("coursework") || used >= t.remaining_min
                    }
                    _ => false,
                },
                remaining_min: if project {
                    0
                } else {
                    task.as_ref().map_or(0, |t| t.remaining_min)
                },
                project,
            });
        }
        out.sort_by_key(|c| c.start);
        Ok(out)
    }

    pub fn checkin(&mut self, req: &api::CheckinReq) -> Result<api::PlanResp> {
        // "finished" = done, and the whole task is finished.
        let finished = req.outcome == "finished";
        let mut req = req.clone();
        if finished {
            req.outcome = "done".into();
            req.remaining_min = Some(0);
        }
        // "longer" = done, taking `actual_min` (more than planned), with
        // `remaining_min` still to do, planned from `not_before`.
        if req.outcome == "longer" {
            req.outcome = "done".into();
        }
        let req = &req;
        let outcome = Outcome::parse(&req.outcome).ok_or_else(|| {
            EngineError::BadRequest(format!(
                "outcome must be done|partial|missed|finished|longer, got {:?}",
                req.outcome
            ))
        })?;
        let world = self.world(1)?;
        let (b, inst) = self
            .checkin_blocks(&world)
            .into_iter()
            .find(|(b, _)| b.uid.as_deref() == Some(req.block_uid.as_str()))
            .ok_or_else(|| EngineError::NotFound(format!("block {}", req.block_uid)))?;
        self.answer_checkin(&req.block_uid, &b, recordable(&inst), outcome, req.actual_min)?;
        // A project session: "finished" finishes the project; there's no
        // estimate to revise.
        if let Some(p) = self.db.task_get(&b.task_id)?.filter(|t| t.is_project()) {
            if finished && p.status == TaskStatus::Open {
                let mut done = p.clone();
                project::set_status(&mut done, ProjectStatus::Done);
                done.updated_at = self.now();
                self.propose(
                    &world,
                    Author::Scheduler,
                    vec![Op::UpdateTask {
                        before: Box::new(p),
                        after: Box::new(done.clone()),
                    }],
                    format!("Finished project {:?}", done.title),
                )?;
            }
            return self.plan(false, Author::Scheduler);
        }
        // "How much is left?": the new remaining estimate (coursework).
        if let Some(r) = req.remaining_min
            && let Some(mut t) = self.db.task_get(&b.task_id)?
            && t.status == TaskStatus::Open
        {
            if r == 0 {
                // "0 left" = finished: the same as completing it.
                return Ok(self.complete_task(Author::Alec, &t.id, None)?.plan.unwrap_or_default());
            }
            t.remaining_min = r;
            t.updated_at = self.now();
            self.db.task_put(&t)?;
        }
        // "Took longer — plan the rest from <date>".
        if let Some(nb) = req.not_before.as_deref().filter(|s| !s.trim().is_empty())
            && let Some(mut t) = self.db.task_get(&b.task_id)?
            && t.status == TaskStatus::Open
        {
            let tz = self.tz();
            t.earliest_start = Some(match api::parse_time(nb, tz, self.now())? {
                TimeIn::At(x) => x,
                TimeIn::Date(d) => kairos_core::time::resolve_local(d.and_time(NaiveTime::MIN), tz),
            });
            t.updated_at = self.now();
            self.db.task_put(&t)?;
        }
        self.plan(false, Author::Scheduler)
    }

    /// Run the scheduler; unless `dry_run`, propose the resulting changeset.
    pub fn plan(&mut self, dry_run: bool, author: Author) -> Result<api::PlanResp> {
        if !dry_run {
            self.settle()?;
        }
        let now = self.now();
        let tz = self.tz();
        let world = self.world(i64::from(self.constraints.horizon_days) + 60)?;
        let mut tasks = self.db.tasks(Some(TaskStatus::Open))?;
        let blocks = self.blocks(&world);
        // Past blocks not yet checked in are presumed done (they count toward the task).
        let answered: std::collections::HashSet<String> = self
            .db
            .checkins(false)?
            .into_iter()
            .filter(|c| c.outcome.is_some())
            .map(|c| c.block_uid)
            .collect();
        // Tasks from before task types get one (in memory), for block sizes.
        for t in tasks.iter_mut().filter(|t| t.kind.is_none()) {
            if let Some(ty) = self.inferred_type(t.estimate_min) {
                let (e, r) = (t.estimate_min, t.remaining_min);
                t.apply_type(&ty);
                (t.estimate_min, t.remaining_min) = (e, r);
            }
        }
        // Learned estimates: plan remaining × how long this type usually takes.
        let ratios = self.estimate_ratios()?;
        let mut adjusted = Vec::new();
        for t in tasks.iter_mut() {
            let Some(k) = t.kind.clone() else { continue };
            if let Some(&(ratio, samples)) = ratios.get(&k)
                && (ratio - 1.0).abs() >= 0.05
                && t.remaining_min > 0
            {
                let planned = (t.remaining_min as f32 * ratio).round() as u32;
                adjusted.push(api::Adjusted {
                    task_id: t.id.clone(),
                    title: t.title.clone(),
                    kind: k,
                    remaining_min: t.remaining_min,
                    planned_min: planned,
                    ratio,
                    samples,
                });
                t.remaining_min = planned;
            }
        }
        // …then the past blocks not checked in yet (presumed done), in real
        // minutes, now that remaining is in real minutes too.
        for (b, _) in &blocks {
            if b.end <= now
                && !b.uid.as_ref().is_some_and(|u| answered.contains(u))
                && let Some(t) = tasks.iter_mut().find(|t| t.id == b.task_id)
            {
                t.remaining_min = t.remaining_min.saturating_sub(b.minutes().max(0) as u32);
            }
        }
        // Blocks of tasks that are no longer open must go too.
        let open_ids: std::collections::HashSet<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
        let existing: Vec<Block> = blocks.iter().map(|(b, _)| b.clone()).collect();
        let busy: Vec<Busy> = world
            .instances
            .iter()
            .filter(|i| is_busy(i))
            .filter_map(|i| match i.span {
                Span::Timed { start, end } if end > start => Some(Busy {
                    start,
                    end,
                    location: i.location.clone(),
                    rest_after_min: i.colour_key().map_or(0, |k| self.constraints.recovery_after(k)),
                    counts_for_load: i.colour_key().is_none_or(|k| self.constraints.counts_for_load(k)),
                }),
                _ => None,
            })
            .collect();
        // Scheduler-owned blocks are low-risk at any time (risk policy), so
        // new work can start `min_lead_minutes` (15) from now.
        let cons = self.constraints.clone();
        let skips: Vec<(DateTime<Utc>, DateTime<Utc>)> = self
            .db
            .skips()?
            .into_iter()
            .filter(|s| s.2 > now)
            .map(|s| (s.1, s.2))
            .collect();
        // Freed time (an event deleted, shortened or moved away since the
        // last plan, here or on the phone): work re-flows into it, from the
        // imminent horizon (2 h) on.
        let reflow_from = now + risk::IMMINENT;
        let spans: Vec<(i64, i64)> = merged_spans(&busy, reflow_from);
        let reflow_after = match self.db.kv_get(BUSY_KEY)? {
            Some(prev) => {
                let prev: Vec<(i64, i64)> = serde_json::from_str(&prev).unwrap_or_default();
                let freed = prev.iter().any(|&(s, e)| {
                    e > reflow_from.timestamp()
                        && !spans
                            .iter()
                            .any(|&(cs, ce)| cs <= s.max(reflow_from.timestamp()) && e <= ce)
                });
                freed.then_some(reflow_from)
            }
            None => None,
        };
        if !dry_run {
            self.db
                .kv_set(BUSY_KEY, &serde_json::to_string(&spans).unwrap_or_default())?;
        }
        let input = PlanInput {
            now,
            tz,
            constraints: &cons,
            tasks: &tasks,
            busy: &busy,
            existing: &existing,
            unavailable: &skips,
            reflow_after,
        };
        let p = sched::plan(&input);
        if !dry_run {
            // Days that already have an accepted session (also ones accepted
            // before accepted proposals were recorded): marked by the description.
            let session_days: Vec<chrono::NaiveDate> = world
                .instances
                .iter()
                .filter(|i| i.description.as_deref() == Some(PROJECT_MARK))
                .map(|i| i.start(tz).with_timezone(&tz).date_naive())
                .collect();
            self.refresh_proposals(&input, &p, &session_days)?;
        }
        let mut delete: Vec<Block> = p.delete.clone();
        for (b, _) in &blocks {
            if b.start >= now
                && !open_ids.contains(b.task_id.as_str())
                && !delete.iter().any(|d| d.uid == b.uid)
                && !b.pinned
            {
                delete.push(b.clone());
            }
        }
        let title_of = |id: &str| {
            tasks
                .iter()
                .find(|t| t.id == id)
                .map(|t| t.title.clone())
                .or_else(|| self.db.task_get(id).ok().flatten().map(|t| t.title))
                .unwrap_or_else(|| id.to_string())
        };
        let mut resp = api::PlanResp {
            dry_run,
            created: p
                .create
                .iter()
                .map(|b| api::PlannedBlock {
                    task_id: b.task_id.clone(),
                    title: title_of(&b.task_id),
                    start: b.start,
                    end: b.end,
                    why: b.why.clone(),
                })
                .collect(),
            deleted: delete
                .iter()
                .map(|b| api::PlannedBlock {
                    task_id: b.task_id.clone(),
                    title: title_of(&b.task_id),
                    start: b.start,
                    end: b.end,
                    why: None,
                })
                .collect(),
            kept: p.keep.len(),
            moved: p.moved,
            shortfalls: p.shortfalls.clone(),
            squeezed: p.squeezed.clone(),
            adjusted,
            changeset: None,
            summary: String::new(),
        };
        resp.summary = plan_summary(&resp, tz);
        if dry_run || (p.create.is_empty() && delete.is_empty()) {
            return Ok(resp);
        }
        let mut ops = Vec::new();
        for b in &delete {
            let Some(uid) = &b.uid else { continue };
            let loc = self.locate(&world, None, uid)?;
            ops.push(Op::DeleteEvent {
                item: Item {
                    source: loc.source.id.clone(),
                    href: loc.href.clone(),
                },
                uid: uid.clone(),
                before: loc.text,
            });
        }
        for b in &p.create {
            let task = tasks
                .iter()
                .find(|t| t.id == b.task_id)
                .ok_or_else(|| EngineError::NotFound(format!("task {}", b.task_id)))?;
            let (target, origin_key) = self.target_for_key(&task.key)?;
            let uid = build::new_uid();
            let ev = NewEvent {
                uid: uid.clone(),
                title: build::todo_block_title(&task.title),
                time: NewTime::Timed {
                    start: b.start,
                    end: b.end,
                },
                location: None,
                description: Some(format!(
                    "kairos task block ({} remaining of {} min)",
                    task.remaining_min, task.estimate_min
                )),
                meta: Meta {
                    created_by: Some(Author::Scheduler),
                    kind: EventKind::TaskBlock,
                    flex: true,
                    task_id: Some(task.id.clone()),
                    origin_key,
                    ..Default::default()
                },
                alarms: vec![],
                transparent: false,
                rrule: None,
            };
            ops.push(Op::CreateEvent {
                item: Item {
                    source: target.id.clone(),
                    href: vdir::href_for_uid(&uid),
                },
                uid,
                after: build::event_file(&ev, tz, now),
            });
        }
        let cs = self.propose(&world, author, ops, resp.summary.clone())?;
        resp.changeset = Some(WriteResult::from(&cs));
        Ok(resp)
    }

    // ------------------------------------------------------------ reads

    pub fn events(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<Vec<Instance>> {
        let (idx, _) = cache::get(&self.cfg, self.cfg_path.as_deref(), self.now(), from, to)?;
        // The cache can predate sidecar-only changes (e.g. a Family event's
        // `blocking` flag); the database is authoritative.
        let side = self.db.meta_all()?;
        let mut evs = idx.between(from, to, self.tz());
        for i in &mut evs {
            if let Some(m) = side.get(&(i.source.clone(), i.uid.clone())) {
                i.meta = i.meta.merged_with(m);
            }
        }
        Ok(evs)
    }

    /// Free slots of at least `minutes` in `[from, to)` (respecting the day window).
    pub fn find_free(&self, from: DateTime<Utc>, to: DateTime<Utc>, minutes: i64) -> Result<Vec<api::FreeSlot>> {
        let tz = self.tz();
        let mut busy: Vec<(DateTime<Utc>, DateTime<Utc>)> = self
            .events(from, to)?
            .iter()
            .filter(|i| is_busy(i) || i.meta.kind == EventKind::TaskBlock)
            .filter_map(|i| match i.span {
                Span::Timed { start, end } => Some((start, end)),
                Span::AllDay { .. } => None,
            })
            .collect();
        busy.sort();
        let mut out = Vec::new();
        let mut day = from.with_timezone(&tz).date_naive();
        let last = to.with_timezone(&tz).date_naive();
        while day <= last {
            let Some(p) = self.constraints.profile_for(day) else {
                break;
            };
            let ws = kairos_core::time::resolve_local(day.and_time(p.day_window.start.to_time()), tz).max(from);
            let we = if p.day_window.end.minutes() >= 1440 {
                kairos_core::time::resolve_local((day + Duration::days(1)).and_time(NaiveTime::MIN), tz)
            } else {
                kairos_core::time::resolve_local(day.and_time(p.day_window.end.to_time()), tz)
            }
            .min(to);
            let mut cur = ws;
            for &(s, e) in &busy {
                if e <= cur || s >= we {
                    continue;
                }
                if s > cur && (s - cur).num_minutes() >= minutes {
                    out.push(api::FreeSlot {
                        start: cur,
                        end: s,
                        minutes: (s - cur).num_minutes(),
                    });
                }
                cur = cur.max(e);
            }
            if we > cur && (we - cur).num_minutes() >= minutes {
                out.push(api::FreeSlot {
                    start: cur,
                    end: we,
                    minutes: (we - cur).num_minutes(),
                });
            }
            day += Duration::days(1);
        }
        Ok(out)
    }
}

/// Does this instance block time for the scheduler?
pub fn is_busy(i: &Instance) -> bool {
    let overlay_ok = match i.overlay {
        Overlay::Personal => !i.meta.not_going,
        Overlay::Family => i.meta.blocking,
        Overlay::Background | Overlay::Imported => false,
    };
    overlay_ok && !i.transparent && i.meta.kind == EventKind::Event && !i.span.is_all_day()
}

fn describe_time(t: NewTime, tz: Tz) -> String {
    match t {
        NewTime::Timed { start, end } => format!(
            "{}–{}",
            start.with_timezone(&tz).format("%a %-d %b %H:%M"),
            end.with_timezone(&tz).format("%H:%M")
        ),
        NewTime::AllDay { start, end } => {
            if end - start <= Duration::days(1) {
                start.format("%a %-d %b").to_string()
            } else {
                format!(
                    "{}–{}",
                    start.format("%a %-d %b"),
                    (end - Duration::days(1)).format("%a %-d %b")
                )
            }
        }
    }
}

fn plan_summary(r: &api::PlanResp, tz: Tz) -> String {
    let mut s = format!(
        "Plan: {} new block{}, {} removed, {} kept, {} moved",
        r.created.len(),
        if r.created.len() == 1 { "" } else { "s" },
        r.deleted.len(),
        r.kept,
        r.moved
    );
    for b in &r.created {
        s.push_str(&format!(
            "\n  + {} {}–{}",
            b.title,
            b.start.with_timezone(&tz).format("%a %-d %b %H:%M"),
            b.end.with_timezone(&tz).format("%H:%M")
        ));
    }
    for sh in &r.shortfalls {
        s.push_str(&format!("\n  ! {}", sh.message));
    }
    for sq in &r.squeezed {
        s.push_str(&format!("\n  ⚠ {}", sq.message));
    }
    for a in &r.adjusted {
        s.push_str(&format!(
            "\n  ≈ {}: planning {} for {} left ({} tasks usually take ×{:.2})",
            a.title,
            fmt_min(a.planned_min),
            fmt_min(a.remaining_min),
            a.kind,
            a.ratio
        ));
    }
    s
}

/// Description of an accepted project session (how a day is known to have one).
/// The busy spans the last plan saw (to notice freed time).
const BUSY_KEY: &str = "plan.busy_spans";

/// Busy time from `from` on, merged into disjoint `(start, end)` unix spans.
fn merged_spans(busy: &[Busy], from: DateTime<Utc>) -> Vec<(i64, i64)> {
    let mut v: Vec<(i64, i64)> = busy
        .iter()
        .filter(|b| b.end > from)
        .map(|b| (b.start.max(from).timestamp(), b.end.timestamp()))
        .collect();
    v.sort_unstable();
    let mut out: Vec<(i64, i64)> = Vec::new();
    for (s, e) in v {
        match out.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => out.push((s, e)),
        }
    }
    out
}

/// Does a check-in show on this event? Only on ones kairos made (not Alec's
/// own, attributed to a project) and not on a repeating series.
fn recordable(i: &Instance) -> bool {
    i.meta.created_by.is_some() && !i.recurring
}

const PROJECT_MARK: &str = "Project time (accepted kairos proposal)";

fn fmt_min(m: u32) -> String {
    match (m / 60, m % 60) {
        (0, m) => format!("{m} m"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m:02} m"),
    }
}
