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
        let instances = kairos_core::view::instances(
            &loaded.sources,
            now - Duration::days(7),
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
                }
            }
            Op::DeleteEvent { item, before, .. } => OpFacts {
                action: Action::Delete,
                target: Some(src(&item.source)?),
                origin: None,
                overlaps_fixed: false,
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
            },
            Op::CreateTask { .. } => OpFacts {
                action: Action::TaskCreate,
                target: None,
                origin: None,
                before: None,
                after: None,
                overlaps_fixed: false,
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
        if self.db.meta_get(source, uid)?.is_some_and(|old| old.blocking) {
            m.blocking = true;
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
        self.sync_after(&touched, &cols);
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
        if let Some(tid) = &req.task_id {
            self.db
                .task_get(tid)?
                .ok_or_else(|| EngineError::NotFound(format!("task {tid}")))?;
            meta.kind = EventKind::TaskBlock;
            meta.task_id = Some(tid.clone());
            meta.flex = true;
            meta.pinned = true;
        }
        let uid = build::new_uid();
        let ev = NewEvent {
            uid: uid.clone(),
            title: req.title.trim().to_string(),
            time,
            location: req.location.clone().filter(|l| !l.is_empty()),
            description: req.description.clone(),
            meta,
            alarms: req.alarms.clone().unwrap_or_default(),
            transparent: false,
        };
        let after = build::event_file(&ev, self.tz(), self.now());
        let item = Item {
            source: target.id.clone(),
            href: vdir::href_for_uid(&uid),
        };
        let summary = format!("Add {:?} {} to {}", ev.title, describe_time(time, self.tz()), key);
        self.propose(&world, author, vec![Op::CreateEvent { item, uid, after }], summary)
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
        if let Some(b) = req.blocking {
            // Sidecar-only flag (no iCloud write): Family events opting in to block the scheduler.
            let mut m = self.db.meta_get(&loc.source.id, uid)?.unwrap_or_default();
            m.blocking = b;
            self.db.meta_set(&loc.source.id, uid, &m)?;
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
        let summary = match (ops.len(), label) {
            (0, _) => return Err(EngineError::BadRequest("no events given".into())),
            (1, None) => first,
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

    fn deadline_op(
        &self,
        title: &str,
        due: Due,
        key: &str,
        task_id: Option<&str>,
        author: Author,
    ) -> Result<(Op, String)> {
        let (target, origin_key) = self.target_for_key(key)?;
        let uid = build::new_uid();
        let meta = Meta {
            created_by: Some(author),
            task_id: task_id.map(String::from),
            origin_key,
            ..Default::default()
        };
        let ev = build::deadline(&uid, title, due, meta, &self.cfg.deadlines.alarms_minutes_before);
        let after = build::event_file(&ev, self.tz(), self.now());
        Ok((
            Op::CreateEvent {
                item: Item {
                    source: target.id,
                    href: vdir::href_for_uid(&uid),
                },
                uid: uid.clone(),
                after,
            },
            uid,
        ))
    }

    pub fn create_deadline(&mut self, author: Author, req: &api::DeadlineReq) -> Result<Changeset> {
        let world = self.world(60)?;
        let due = api::parse_time(&req.due, self.tz(), self.now())?.as_due();
        let (op, uid) = self.deadline_op(&req.title, due, &req.key, req.task_id.as_deref(), author)?;
        let mut ops = vec![op];
        if let Some(tid) = &req.task_id {
            let before = self
                .db
                .task_get(tid)?
                .ok_or_else(|| EngineError::NotFound(format!("task {tid}")))?;
            let mut after = before.clone();
            after.due = Some(due);
            after.deadline_uid = Some(uid);
            after.updated_at = self.now();
            ops.push(Op::UpdateTask {
                before: Box::new(before),
                after: Box::new(after),
            });
        }
        self.propose(
            &world,
            author,
            ops,
            format!("Deadline ⚑ {} {}", req.title, due.describe(self.tz())),
        )
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
        if t.estimate_min == 0 {
            return Err(EngineError::BadRequest(format!(
                "estimate required{}",
                ty.map(|t| format!(" for {}", t.name)).unwrap_or_default()
            )));
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
        let world = self.world(60)?;
        let mut ops = Vec::new();
        if let (Some(due), true) = (t.due, req.create_deadline.unwrap_or(true)) {
            let (op, uid) = self.deadline_op(&t.title, due, &t.key, Some(&t.id), author)?;
            t.deadline_uid = Some(uid);
            ops.push(op);
        }
        ops.insert(
            0,
            Op::CreateTask {
                after: Box::new(t.clone()),
            },
        );
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
        t.updated_at = now;
        let world = self.world(60)?;
        let mut ops = Vec::new();
        if due_changed {
            // Update / create / delete the linked deadline event.
            let existing = t
                .deadline_uid
                .as_deref()
                .and_then(|u| self.locate(&world, None, u).ok());
            match (existing, t.due) {
                (Some(loc), Some(due)) => {
                    let ev = build::deadline(&loc.series.uid, &t.title, due, Meta::default(), &[]);
                    let edit = Edit {
                        time: Some(ev.time),
                        ..Default::default()
                    };
                    let after = build::edit_file(&loc.text, &loc.series.uid, &edit, tz, now)
                        .map_err(|e| EngineError::BadRequest(e.to_string()))?;
                    ops.push(Op::UpdateEvent {
                        item: Item {
                            source: loc.source.id.clone(),
                            href: loc.href.clone(),
                        },
                        uid: loc.series.uid.clone(),
                        before: loc.text,
                        after,
                    });
                }
                (Some(loc), None) => {
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
                (None, Some(due)) => {
                    let (op, uid) = self.deadline_op(&t.title, due, &t.key, Some(&t.id), author)?;
                    t.deadline_uid = Some(uid);
                    ops.push(op);
                }
                (None, None) => {}
            }
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

    /// Open proposals, soonest first.
    pub fn proposals(&self) -> Result<Vec<api::ProposalItem>> {
        let now = self.now();
        let title = self.constraints.projects.title.clone();
        Ok(self
            .db
            .proposals()?
            .into_iter()
            .filter(|r| r.3 == "proposed" && r.2 > now)
            .map(|(id, start, end, _)| api::ProposalItem {
                id,
                start,
                end,
                title: title.clone(),
            })
            .collect())
    }

    /// Accept proposed project time: it becomes an event (titled with what
    /// to work on) in the projects key.
    pub fn accept_proposal(&mut self, author: Author, id: &str, req: &api::AcceptProposalReq) -> Result<WriteResult> {
        let p = self
            .proposals()?
            .into_iter()
            .find(|p| p.id == id)
            .ok_or_else(|| EngineError::NotFound(format!("proposal {id}")))?;
        let title = req
            .title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .unwrap_or(&p.title)
            .to_string();
        let cs = self.create_event(
            author,
            &api::CreateEventReq {
                title,
                start: p.start.to_rfc3339(),
                end: Some(p.end.to_rfc3339()),
                key: self.constraints.projects.key.clone(),
                description: Some(PROJECT_MARK.into()),
                ..Default::default()
            },
        )?;
        self.db.proposal_set_status(id, "accepted")?;
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
        let cs = self.propose(&world, author, ops, format!("Complete {:?}", t.title))?;
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
        let summary = match n {
            0 => format!("Drop task {:?}", t.title),
            1 => format!("Drop task {:?} and its scheduled block", t.title),
            n => format!("Drop task {:?} and its {n} scheduled blocks", t.title),
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
        let cutoff_time = kairos_core::constraints::Hm::parse(&self.cfg.notify.autodone_at)
            .map_or(NaiveTime::from_hms_opt(9, 0, 0).unwrap_or(NaiveTime::MIN), |h| {
                h.to_time()
            });
        let cutoff = kairos_core::time::resolve_local(today.and_time(cutoff_time), tz);
        // Blocks from before today are auto-done once we're past today's cutoff.
        let boundary = if now >= cutoff {
            kairos_core::time::resolve_local(today.and_time(NaiveTime::MIN), tz)
        } else {
            cutoff - Duration::days(1)
        };
        let answered: std::collections::HashSet<String> = self
            .db
            .checkins(false)?
            .into_iter()
            .filter(|c| c.outcome.is_some())
            .map(|c| c.block_uid)
            .collect();
        let mut n = 0;
        for (b, _) in self.blocks(&world) {
            let Some(uid) = &b.uid else { continue };
            if b.end > boundary || answered.contains(uid) {
                continue;
            }
            self.answer_checkin(uid, &b, Outcome::Done, None)?;
            n += 1;
        }
        Ok(n)
    }

    fn answer_checkin(&mut self, uid: &str, b: &Block, outcome: Outcome, actual: Option<u32>) -> Result<()> {
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
        {
            t.remaining_min = t.remaining_min.saturating_sub(actual);
            t.updated_at = self.now();
            if t.remaining_min == 0 {
                t.status = TaskStatus::Done;
                let total = self.actual_minutes(&t.id).unwrap_or(t.estimate_min);
                self.db.estimate_log(&EstimateRow {
                    task_id: t.id.clone(),
                    title: t.title.clone(),
                    estimate_min: t.estimate_min,
                    actual_min: total,
                    completed_at: self.now(),
                })?;
            }
            self.db.task_put(&t)?;
        }
        Ok(())
    }

    /// Today's blocks for the check-in.
    pub fn checkin_items(&mut self) -> Result<Vec<api::CheckinItem>> {
        let now = self.now();
        let tz = self.tz();
        let today = now.with_timezone(&tz).date_naive();
        let world = self.world(1)?;
        let rows = self.db.checkins(false)?;
        let mut out = Vec::new();
        for (b, inst) in self.blocks(&world) {
            if b.start.with_timezone(&tz).date_naive() != today || b.start > now {
                continue;
            }
            let uid = b.uid.clone().unwrap_or_default();
            let outcome = rows
                .iter()
                .find(|r| r.block_uid == uid)
                .and_then(|r| r.outcome.map(|o| o.as_str().to_string()));
            let task = self.db.task_get(&b.task_id)?;
            out.push(api::CheckinItem {
                block_uid: uid,
                task_id: b.task_id.clone(),
                title: inst.title.clone(),
                start: b.start,
                end: b.end,
                planned_min: b.minutes().max(0) as u32,
                outcome,
                ask_remaining: task.as_ref().is_some_and(|t| t.kind.as_deref() == Some("coursework")),
                remaining_min: task.as_ref().map_or(0, |t| t.remaining_min),
            });
        }
        out.sort_by_key(|c| c.start);
        Ok(out)
    }

    pub fn checkin(&mut self, req: &api::CheckinReq) -> Result<api::PlanResp> {
        let outcome = Outcome::parse(&req.outcome).ok_or_else(|| {
            EngineError::BadRequest(format!("outcome must be done|partial|missed, got {:?}", req.outcome))
        })?;
        let world = self.world(1)?;
        let (b, _) = self
            .blocks(&world)
            .into_iter()
            .find(|(b, _)| b.uid.as_deref() == Some(req.block_uid.as_str()))
            .ok_or_else(|| EngineError::NotFound(format!("block {}", req.block_uid)))?;
        self.answer_checkin(&req.block_uid, &b, outcome, req.actual_min)?;
        // "How much is left?": the new remaining estimate (coursework).
        if let Some(r) = req.remaining_min
            && let Some(mut t) = self.db.task_get(&b.task_id)?
            && t.status == TaskStatus::Open
        {
            t.remaining_min = r;
            t.updated_at = self.now();
            if r == 0 {
                t.status = TaskStatus::Done;
                let total = self.actual_minutes(&t.id).unwrap_or(t.estimate_min);
                self.db.estimate_log(&EstimateRow {
                    task_id: t.id.clone(),
                    title: t.title.clone(),
                    estimate_min: t.estimate_min,
                    actual_min: total,
                    completed_at: self.now(),
                })?;
            }
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
        for (b, _) in &blocks {
            if b.end <= now
                && !b.uid.as_ref().is_some_and(|u| answered.contains(u))
                && let Some(t) = tasks.iter_mut().find(|t| t.id == b.task_id)
            {
                t.remaining_min = t.remaining_min.saturating_sub(b.minutes().max(0) as u32);
            }
        }
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
        let input = PlanInput {
            now,
            tz,
            constraints: &cons,
            tasks: &tasks,
            busy: &busy,
            existing: &existing,
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
                title: task.title.clone(),
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
        Overlay::Personal => true,
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
const PROJECT_MARK: &str = "Project time (accepted kairos proposal)";

fn fmt_min(m: u32) -> String {
    match (m / 60, m % 60) {
        (0, m) => format!("{m} m"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m:02} m"),
    }
}
