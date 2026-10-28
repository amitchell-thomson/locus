//! HTTP API (`/api/...`) and the health check. Nothing here is a page for the phone: changes are
//! approved in locus's Approve, and you check in from the calendar (2026-10-25).

use crate::engine::{Engine, EngineError};
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Duration, Utc};
use kairos_core::api::{self, ApiError};
use kairos_core::changeset::{Changeset, Status, WriteResult};
use kairos_core::model::Author;
use kairos_core::task::TaskStatus;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct AppState {
    pub engine: Arc<Mutex<Engine>>,
    pub token: Arc<String>,
}

fn ct_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub struct HttpError(StatusCode, ApiError);

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        (self.0, Json(self.1)).into_response()
    }
}

impl From<EngineError> for HttpError {
    fn from(e: EngineError) -> Self {
        let (code, reasons) = match &e {
            EngineError::BadRequest(_) => (StatusCode::BAD_REQUEST, vec![]),
            EngineError::NotFound(_) => (StatusCode::NOT_FOUND, vec![]),
            EngineError::Forbidden(r) => (StatusCode::FORBIDDEN, r.clone()),
            EngineError::LiveDisabled(_) => (StatusCode::FORBIDDEN, vec![]),
            EngineError::Conflict { diff, .. } => (StatusCode::CONFLICT, diff.lines().map(String::from).collect()),
            EngineError::Store(_) => (StatusCode::INTERNAL_SERVER_ERROR, vec![]),
        };
        if code == StatusCode::INTERNAL_SERVER_ERROR {
            tracing::error!("{e}");
        }
        HttpError(
            code,
            ApiError {
                error: e.to_string(),
                reasons,
            },
        )
    }
}

type HResult<T> = Result<Json<T>, HttpError>;

/// Run engine work on the blocking pool (SQLite + file I/O).
pub async fn with_engine<T: Send + 'static>(
    st: &AppState,
    f: impl FnOnce(&mut Engine) -> Result<T, EngineError> + Send + 'static,
) -> Result<T, HttpError> {
    let eng = st.engine.clone();
    tokio::task::spawn_blocking(move || {
        let mut g = eng
            .lock()
            .map_err(|_| EngineError::BadRequest("engine lock poisoned; restart kairosd".into()))?;
        let r = f(&mut g);
        if g.replan_soon {
            // An event changed: move work out of its way right after answering, or, while
            // Alec is moving things by hand, once he has paused (a later edit pushes the pause
            // back, and this replan then finds nothing due yet).
            let eng = eng.clone();
            let wait = g.replan_wait();
            tokio::runtime::Handle::current().spawn(async move {
                if let Some(w) = wait {
                    tokio::time::sleep(w + std::time::Duration::from_millis(50)).await;
                }
                let _ = tokio::task::spawn_blocking(move || {
                    if let Ok(mut e) = eng.lock()
                        && let Err(err) = e.replan_due()
                    {
                        tracing::warn!("replan after write: {err}");
                    }
                })
                .await;
            });
        }
        r
    })
    .await
    .map_err(|e| {
        HttpError(
            StatusCode::INTERNAL_SERVER_ERROR,
            ApiError {
                error: e.to_string(),
                reasons: vec![],
            },
        )
    })?
    .map_err(HttpError::from)
}

fn author(h: &HeaderMap) -> Author {
    h.get("x-kairos-author")
        .and_then(|v| v.to_str().ok())
        .and_then(Author::parse)
        .filter(|a| *a != Author::Scheduler)
        .unwrap_or(Author::Alec)
}

/// Alec typing in the TUI / on a terminal (see `Daemon::interactive`).
fn direct(h: &HeaderMap) -> bool {
    author(h) == Author::Alec
        && h.get("x-kairos-interactive")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.trim() == "1")
}

/// [`with_engine`] for writes: marks Alec's interactive requests as direct.
pub async fn with_engine_as<T: Send + 'static>(
    st: &AppState,
    h: &HeaderMap,
    f: impl FnOnce(&mut Engine) -> Result<T, EngineError> + Send + 'static,
) -> Result<T, HttpError> {
    let d = direct(h);
    with_engine(st, move |e| {
        e.direct = d;
        let r = f(e);
        e.direct = false;
        r
    })
    .await
}

async fn auth(State(st): State<AppState>, req: Request, next: Next) -> Response {
    let ok = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| ct_eq(t.trim(), &st.token));
    if ok {
        next.run(req).await
    } else {
        HttpError(
            StatusCode::UNAUTHORIZED,
            ApiError {
                error: "missing or bad bearer token".into(),
                reasons: vec![],
            },
        )
        .into_response()
    }
}

pub fn router(st: AppState, mcp: Option<Router<AppState>>) -> Router {
    let api = Router::new()
        .route("/api/events", get(list_events).post(create_event))
        .route(
            "/api/events/{source}/{uid}",
            axum::routing::patch(update_event).delete(delete_event),
        )
        .route("/api/events/{source}/{uid}/move", post(move_event))
        .route("/api/events/{source}/{uid}/skip", post(skip_event))
        .route("/api/events/move", post(move_events))
        .route("/api/events/{source}/{uid}/share-family", post(share_family))
        .route("/api/events/{source}/{uid}/adopt", post(adopt))
        .route("/api/free", get(find_free))
        .route("/api/import", post(import))
        .route("/api/tasks", get(list_tasks).post(create_task))
        .route("/api/tasks/{id}", axum::routing::patch(update_task).get(get_task))
        .route("/api/tasks/{id}/complete", post(complete_task))
        .route("/api/tasks/{id}/start", post(start_task))
        .route("/api/tasks/{id}/log", post(log_work))
        .route("/api/tasks/{id}/drop", post(drop_task))
        .route("/api/deadlines", post(create_deadline))
        .route("/api/plan", post(plan))
        .route("/api/projects", get(list_projects).post(create_project))
        .route(
            "/api/projects/{id}",
            axum::routing::patch(update_project).delete(delete_project),
        )
        .route("/api/changesets", get(list_changesets))
        .route("/api/changesets/{id}", get(get_changeset))
        .route("/api/changesets/{id}/approve", post(approve))
        .route("/api/changesets/{id}/reject", post(reject))
        .route("/api/changesets/{id}/undo", post(undo))
        .route("/api/sidecar", get(sidecar))
        // ~70 KB of JSON for a normal window: gzip makes it ~10× smaller
        // over the home connection's upload.
        .route(
            "/api/index",
            get(index).layer(tower_http::compression::CompressionLayer::new()),
        )
        .route("/api/checkin", get(checkin_items).post(checkin))
        .route("/api/status", get(status))
        .route("/api/taxonomy", get(taxonomy))
        .route("/api/worked", post(set_worked))
        .route("/api/chores", get(chores))
        .route("/api/tasks/overview", get(tasks_overview));
    let mut protected = api;
    if let Some(m) = mcp {
        protected = protected.merge(m);
    }
    let protected = protected.layer(middleware::from_fn_with_state(st.clone(), auth));
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .merge(protected)
        .with_state(st)
}

// ---------------------------------------------------------------- events

#[derive(Deserialize)]
struct Window {
    from: Option<String>,
    to: Option<String>,
    duration_min: Option<i64>,
}

fn window(st: &Engine, w: &Window) -> Result<(DateTime<Utc>, DateTime<Utc>), EngineError> {
    let now = st.now();
    let tz = st.tz();
    let p = |s: &str| -> Result<DateTime<Utc>, EngineError> {
        Ok(match api::parse_time(s, tz, now)? {
            api::TimeIn::At(t) => t,
            api::TimeIn::Date(d) => kairos_core::time::resolve_local(d.and_time(chrono::NaiveTime::MIN), tz),
        })
    };
    let from = w
        .from
        .as_deref()
        .map(p)
        .transpose()?
        .unwrap_or(now - Duration::hours(1));
    let to = w.to.as_deref().map(p).transpose()?.unwrap_or(from + Duration::days(7));
    if to <= from {
        return Err(EngineError::BadRequest("`to` must be after `from`".into()));
    }
    if to - from > Duration::days(400) {
        return Err(EngineError::BadRequest("window too large (max 400 days)".into()));
    }
    Ok((from, to))
}

async fn list_events(State(st): State<AppState>, Query(w): Query<Window>) -> HResult<api::EventsResp> {
    let r = with_engine(&st, move |e| {
        let (from, to) = window(e, &w)?;
        Ok(api::EventsResp {
            from,
            to,
            events: e.events(from, to)?,
        })
    })
    .await?;
    Ok(Json(r))
}

async fn find_free(State(st): State<AppState>, Query(w): Query<Window>) -> HResult<Vec<api::FreeSlot>> {
    let r = with_engine(&st, move |e| {
        let (from, to) = window(e, &w)?;
        e.find_free(from, to, w.duration_min.unwrap_or(30))
    })
    .await?;
    Ok(Json(r))
}

async fn create_event(
    State(st): State<AppState>,
    h: HeaderMap,
    Json(req): Json<api::CreateEventReq>,
) -> HResult<WriteResult> {
    let a = author(&h);
    let cs = with_engine_as(&st, &h, move |e| e.create_event(a, &req)).await?;
    Ok(Json(WriteResult::from(&cs)))
}

async fn update_event(
    State(st): State<AppState>,
    h: HeaderMap,
    Path((source, uid)): Path<(String, String)>,
    Json(req): Json<api::UpdateEventReq>,
) -> Result<Response, HttpError> {
    let a = author(&h);
    let cs = with_engine_as(&st, &h, move |e| e.update_event(a, Some(&source), &uid, &req)).await?;
    Ok(match cs {
        Some(cs) => Json(WriteResult::from(&cs)).into_response(),
        None => Json(serde_json::json!({"ok": true, "summary": "sidecar flag updated"})).into_response(),
    })
}

async fn delete_event(
    State(st): State<AppState>,
    h: HeaderMap,
    Path((source, uid)): Path<(String, String)>,
) -> HResult<WriteResult> {
    let a = author(&h);
    let cs = with_engine_as(&st, &h, move |e| e.delete_event(a, Some(&source), &uid)).await?;
    Ok(Json(WriteResult::from(&cs)))
}

async fn move_event(
    State(st): State<AppState>,
    h: HeaderMap,
    Path((source, uid)): Path<(String, String)>,
    Json(req): Json<api::KeyReq>,
) -> HResult<WriteResult> {
    let a = author(&h);
    let cs = with_engine_as(&st, &h, move |e| e.move_event(a, Some(&source), &uid, &req.key)).await?;
    Ok(Json(WriteResult::from(&cs)))
}

async fn move_events(
    State(st): State<AppState>,
    h: HeaderMap,
    Json(req): Json<api::MoveManyReq>,
) -> HResult<WriteResult> {
    let a = author(&h);
    let evs: Vec<(Option<String>, String)> = req.events.into_iter().map(|r| (Some(r.source), r.uid)).collect();
    let cs = with_engine_as(&st, &h, move |e| e.move_events(a, &evs, &req.key)).await?;
    Ok(Json(WriteResult::from(&cs)))
}

async fn share_family(
    State(st): State<AppState>,
    h: HeaderMap,
    Path((source, uid)): Path<(String, String)>,
) -> HResult<WriteResult> {
    let a = author(&h);
    let cs = with_engine_as(&st, &h, move |e| e.share_to_family(a, Some(&source), &uid)).await?;
    Ok(Json(WriteResult::from(&cs)))
}

async fn adopt(
    State(st): State<AppState>,
    h: HeaderMap,
    Path((_source, uid)): Path<(String, String)>,
    Json(req): Json<api::KeyReq>,
) -> HResult<WriteResult> {
    let a = author(&h);
    let cs = with_engine_as(&st, &h, move |e| e.adopt(a, &uid, &req.key)).await?;
    Ok(Json(WriteResult::from(&cs)))
}

async fn import(State(st): State<AppState>, h: HeaderMap, Json(req): Json<api::ImportReq>) -> HResult<WriteResult> {
    let a = author(&h);
    let cs = with_engine_as(&st, &h, move |e| e.import_ics(a, &req.ics, &req.key, &req.name)).await?;
    Ok(Json(WriteResult::from(&cs)))
}

// ---------------------------------------------------------------- tasks

#[derive(Deserialize)]
struct TaskFilter {
    status: Option<String>,
}

async fn list_tasks(State(st): State<AppState>, Query(f): Query<TaskFilter>) -> HResult<Vec<kairos_core::task::Task>> {
    let r = with_engine(&st, move |e| {
        let status = match f.status.as_deref() {
            None | Some("open") => Some(TaskStatus::Open),
            Some("all") => None,
            Some(s) => Some(TaskStatus::parse(s).ok_or_else(|| EngineError::BadRequest(format!("bad status {s:?}")))?),
        };
        Ok(e.db.tasks(status)?)
    })
    .await?;
    Ok(Json(r))
}

async fn get_task(State(st): State<AppState>, Path(id): Path<String>) -> HResult<kairos_core::task::Task> {
    Ok(Json(with_engine(&st, move |e| e.find_task(&id)).await?))
}

async fn create_task(
    State(st): State<AppState>,
    h: HeaderMap,
    Json(req): Json<api::CreateTaskReq>,
) -> HResult<api::TaskResp> {
    let a = author(&h);
    Ok(Json(with_engine_as(&st, &h, move |e| e.create_task(a, &req)).await?))
}

async fn update_task(
    State(st): State<AppState>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<api::UpdateTaskReq>,
) -> HResult<api::TaskResp> {
    let a = author(&h);
    Ok(Json(
        with_engine_as(&st, &h, move |e| e.update_task(a, &id, &req)).await?,
    ))
}

async fn complete_task(
    State(st): State<AppState>,
    h: HeaderMap,
    Path(id): Path<String>,
    body: Option<Json<api::CompleteTaskReq>>,
) -> HResult<api::TaskResp> {
    let a = author(&h);
    let actual = body.and_then(|b| b.0.actual_min);
    Ok(Json(
        with_engine_as(&st, &h, move |e| e.complete_task(a, &id, actual)).await?,
    ))
}

async fn start_task(
    State(st): State<AppState>,
    h: HeaderMap,
    Path(id): Path<String>,
    body: Option<Json<api::StartTaskReq>>,
) -> HResult<api::TaskResp> {
    let minutes = body.and_then(|b| b.0.minutes);
    let a = author(&h);
    Ok(Json(
        with_engine_as(&st, &h, move |e| e.start_task(a, &id, minutes)).await?,
    ))
}

async fn log_work(
    State(st): State<AppState>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<api::LogWorkReq>,
) -> HResult<api::TaskResp> {
    let a = author(&h);
    Ok(Json(
        with_engine_as(&st, &h, move |e| e.log_work(a, &id, req.minutes)).await?,
    ))
}

async fn drop_task(State(st): State<AppState>, h: HeaderMap, Path(id): Path<String>) -> HResult<api::TaskResp> {
    let a = author(&h);
    Ok(Json(with_engine_as(&st, &h, move |e| e.drop_task(a, &id)).await?))
}

async fn create_deadline(
    State(st): State<AppState>,
    h: HeaderMap,
    Json(req): Json<api::DeadlineReq>,
) -> HResult<WriteResult> {
    let a = author(&h);
    let cs = with_engine_as(&st, &h, move |e| e.create_deadline(a, &req)).await?;
    Ok(Json(WriteResult::from(&cs)))
}

async fn plan(State(st): State<AppState>, body: Option<Json<api::PlanReq>>) -> HResult<api::PlanResp> {
    let dry = body.is_some_and(|b| b.0.dry_run);
    Ok(Json(with_engine(&st, move |e| e.plan(dry, Author::Scheduler)).await?))
}

// ---------------------------------------------------------------- projects

async fn list_projects(State(st): State<AppState>) -> HResult<Vec<api::ProjectItem>> {
    Ok(Json(with_engine(&st, move |e| e.projects()).await?))
}

async fn create_project(
    State(st): State<AppState>,
    h: HeaderMap,
    Json(req): Json<api::CreateProjectReq>,
) -> HResult<kairos_core::project::Project> {
    let a = author(&h);
    Ok(Json(with_engine_as(&st, &h, move |e| e.create_project(a, &req)).await?))
}

async fn update_project(
    State(st): State<AppState>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<api::UpdateProjectReq>,
) -> HResult<api::UpdateProjectResp> {
    let a = author(&h);
    Ok(Json(
        with_engine_as(&st, &h, move |e| e.update_project(a, &id, &req)).await?,
    ))
}

async fn delete_project(State(st): State<AppState>, h: HeaderMap, Path(id): Path<String>) -> HResult<WriteResult> {
    let a = author(&h);
    Ok(Json(with_engine_as(&st, &h, move |e| e.delete_project(a, &id)).await?))
}

// ---------------------------------------------------------------- changesets

#[derive(Deserialize)]
struct CsFilter {
    status: Option<String>,
    limit: Option<usize>,
}

async fn list_changesets(State(st): State<AppState>, Query(f): Query<CsFilter>) -> HResult<Vec<Changeset>> {
    let r = with_engine(&st, move |e| {
        let status = f
            .status
            .as_deref()
            .map(|s| Status::parse(s).ok_or_else(|| EngineError::BadRequest(format!("bad status {s:?}"))))
            .transpose()?;
        Ok(e.db.cs_list(status, f.limit.unwrap_or(50).min(500))?)
    })
    .await?;
    Ok(Json(r))
}

async fn get_changeset(State(st): State<AppState>, Path(id): Path<String>) -> HResult<Changeset> {
    let r = with_engine(&st, move |e| {
        e.db.cs_get(&id)?
            .ok_or_else(|| EngineError::NotFound(format!("changeset {id}")))
    })
    .await?;
    Ok(Json(r))
}

async fn approve(State(st): State<AppState>, Path(id): Path<String>) -> HResult<WriteResult> {
    let cs = with_engine(&st, move |e| e.approve(&id)).await?;
    Ok(Json(WriteResult::from(&cs)))
}

async fn reject(State(st): State<AppState>, Path(id): Path<String>) -> HResult<WriteResult> {
    let cs = with_engine(&st, move |e| e.reject(&id)).await?;
    Ok(Json(WriteResult::from(&cs)))
}

#[derive(Deserialize)]
struct UndoQ {
    /// Also undo the scheduler's replans applied after it.
    with_replans: Option<bool>,
}

async fn undo(State(st): State<AppState>, Path(id): Path<String>, Query(q): Query<UndoQ>) -> HResult<WriteResult> {
    let cs = with_engine(&st, move |e| {
        if q.with_replans == Some(true) {
            e.undo_with_replans(&id)
        } else {
            e.undo(&id)
        }
    })
    .await?;
    Ok(Json(WriteResult::from(&cs)))
}

async fn skip_event(
    State(st): State<AppState>,
    h: HeaderMap,
    Path((source, uid)): Path<(String, String)>,
) -> HResult<WriteResult> {
    let a = author(&h);
    let cs = with_engine_as(&st, &h, move |e| e.skip_block(a, Some(&source), &uid)).await?;
    Ok(Json(WriteResult::from(&cs)))
}

#[derive(Deserialize)]
struct IndexQuery {
    from: DateTime<Utc>,
    to: DateTime<Utc>,
}

/// The instance index the TUI draws, built from kairosd's own vdir and
/// sidecar (the source of truth, so a write shows at once instead of after
/// a sync). The ETag is a hash of what's drawn: polls that find nothing new
/// get an empty 304.
async fn index(State(st): State<AppState>, h: HeaderMap, Query(q): Query<IndexQuery>) -> Result<Response, HttpError> {
    let idx = with_engine(&st, move |e| e.index(q.from, q.to)).await?;
    let drawn = serde_json::to_vec(&(&idx.instances, &idx.sources, &idx.missing, &idx.warnings)).unwrap_or_default();
    let tag = format!("\"{}\"", hex::encode(&Sha256::digest(&drawn)[..12]));
    let unchanged = h
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == tag);
    if unchanged {
        return Ok((StatusCode::NOT_MODIFIED, [(header::ETAG, tag)]).into_response());
    }
    Ok(([(header::ETAG, tag)], Json(idx)).into_response())
}

async fn sidecar(State(st): State<AppState>) -> HResult<Vec<(String, String, kairos_core::model::Meta)>> {
    let r = with_engine(&st, move |e| {
        let mut v: Vec<_> = e.db.meta_all()?.into_iter().map(|((s, u), m)| (s, u, m)).collect();
        v.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
        Ok(v)
    })
    .await?;
    Ok(Json(r))
}

async fn checkin_items(State(st): State<AppState>) -> HResult<Vec<api::CheckinItem>> {
    Ok(Json(with_engine(&st, move |e| e.checkin_items()).await?))
}

async fn checkin(State(st): State<AppState>, h: HeaderMap, Json(req): Json<api::CheckinReq>) -> HResult<api::PlanResp> {
    let a = author(&h);
    Ok(Json(with_engine_as(&st, &h, move |e| e.checkin(a, &req)).await?))
}

/// Every task for locus's Tasks page (spec E7).
async fn tasks_overview(State(st): State<AppState>) -> HResult<Vec<locus_contract::TaskItem>> {
    Ok(Json(with_engine(&st, move |e| e.tasks_overview()).await?))
}

/// Open chores for locus's Home (spec K9).
async fn chores(State(st): State<AppState>) -> HResult<Vec<locus_contract::Chore>> {
    Ok(Json(with_engine(&st, move |e| e.chores()).await?))
}

/// A machine's measured project time (spec K6), from its Claude sessions.
async fn set_worked(State(st): State<AppState>, Json(req): Json<api::WorkedReq>) -> HResult<serde_json::Value> {
    let n = with_engine(&st, move |e| e.set_worked(&req)).await?;
    Ok(Json(serde_json::json!({ "spans": n })))
}

/// The locus taxonomy and registry (`taxonomy/` in the checkout kairosd was built from; ADR-008),
/// read afresh each time so a deploy that changes them needs no restart. The Mac reads them here.
async fn taxonomy() -> HResult<locus_contract::taxonomy::Shared> {
    locus_contract::taxonomy::load(&locus_contract::taxonomy::root())
        .map(Json)
        .map_err(|e| {
            HttpError(
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiError {
                    error: e.to_string(),
                    reasons: vec![],
                },
            )
        })
}

async fn status(State(st): State<AppState>) -> HResult<serde_json::Value> {
    let r = with_engine(&st, move |e| {
        let pending = e.db.cs_list(Some(Status::Pending), 100)?.len();
        let open = e.db.tasks(Some(TaskStatus::Open))?.len();
        let newest = e
            .cfg
            .sources
            .iter()
            .filter_map(|s| {
                let cols = kairos_store::vdir::collections(&e.cfg.vdir()).ok()?;
                let p = kairos_store::vdir::source_path(&e.cfg, s, &cols).ok()?;
                kairos_store::vdir::newest_mtime(&p)
            })
            .max()
            .map(DateTime::<Utc>::from);
        Ok(serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "live": e.cfg.live,
            "test_calendar": e.cfg.test_calendar,
            "pending_changesets": pending,
            "open_tasks": open,
            "unregistered_projects": e.unregistered()?,
            "newest_item": newest,
            "now": e.now(),
        }))
    })
    .await?;
    Ok(Json(r))
}
