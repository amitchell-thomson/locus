//! keryxd's HTTP API (SPEC §8.3). Bearer token on everything but /healthz;
//! the author comes from `x-keryx-author` (missing = script) and Alec's own
//! interactive edits carry `x-keryx-interactive: 1`.

use crate::engine::{Engine, EngineError, Who};
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use keryx_core::changeset::{Author, Status};
use keryx_store::api::*;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct AppState {
    pub engine: Arc<Mutex<Engine>>,
    pub token: Arc<String>,
    /// Ask the jobs loop to sync now.
    pub sync_now: std::sync::mpsc::SyncSender<()>,
}

pub struct HttpError(StatusCode, String);

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

impl From<EngineError> for HttpError {
    fn from(e: EngineError) -> Self {
        let code = match &e {
            EngineError::BadRequest(_) => StatusCode::BAD_REQUEST,
            EngineError::NotFound(_) => StatusCode::NOT_FOUND,
            EngineError::Forbidden(_) => StatusCode::FORBIDDEN,
            EngineError::Conflict(_) => StatusCode::CONFLICT,
            EngineError::Source(_) => StatusCode::BAD_GATEWAY,
            EngineError::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        HttpError(code, e.to_string())
    }
}

type HResult<T> = Result<Json<T>, HttpError>;

pub fn who(h: &HeaderMap) -> Who {
    let author = Author::from_header(h.get("x-keryx-author").and_then(|v| v.to_str().ok()));
    let interactive = h
        .get("x-keryx-interactive")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim() == "1");
    Who { author, interactive }
}

/// Run engine work off the async runtime (it may wait on Mail / Gmail).
async fn with_engine<T: Send + 'static>(
    st: &AppState,
    f: impl FnOnce(&mut Engine) -> Result<T, EngineError> + Send + 'static,
) -> Result<T, HttpError> {
    let engine = st.engine.clone();
    tokio::task::spawn_blocking(move || {
        let mut e = engine
            .lock()
            .map_err(|_| HttpError(StatusCode::INTERNAL_SERVER_ERROR, "engine lock poisoned".into()))?;
        f(&mut e).map_err(HttpError::from)
    })
    .await
    .map_err(|e| HttpError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
}

/// The run a request's token belongs to (set by [`auth`] for run tokens).
#[derive(Clone, Debug)]
pub struct RunCtx(pub keryx_store::runs::Run);

async fn auth(State(st): State<AppState>, mut req: Request, next: Next) -> Response {
    let token = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string);
    let Some(token) = token else {
        return HttpError(StatusCode::UNAUTHORIZED, "missing or bad bearer token".into()).into_response();
    };
    if constant_eq(token.as_bytes(), st.token.as_bytes()) {
        return next.run(req).await;
    }
    // A run token: valid for its run only, on /mcp only, while it runs.
    let engine = st.engine.clone();
    let run =
        tokio::task::spawn_blocking(move || engine.lock().ok().and_then(|e| e.run_for_token(&token).ok().flatten()))
            .await
            .ok()
            .flatten();
    match run {
        Some(r) if req.uri().path().starts_with("/mcp") => {
            req.extensions_mut().insert(RunCtx(r));
            next.run(req).await
        }
        Some(_) => HttpError(StatusCode::FORBIDDEN, "a run token only works for MCP".into()).into_response(),
        None => HttpError(StatusCode::UNAUTHORIZED, "missing or bad bearer token".into()).into_response(),
    }
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn router(st: AppState, mcp: Option<Router<AppState>>) -> Router {
    let api = Router::new()
        .route("/api/status", get(status))
        .route("/api/sync", post(sync_now))
        .route("/api/check/automation", post(check_automation))
        .route("/api/triage", post(triage))
        .route("/api/undo-last", post(undo_last))
        .route("/api/changesets", get(changesets))
        .route("/api/changesets/{id}", get(changeset))
        .route("/api/changesets/{id}/undo", post(undo))
        .route("/api/reviews", get(reviews).post(propose))
        .route("/api/reviews/{id}", get(review))
        .route("/api/reviews/{id}/accept-group", post(accept_group))
        .route("/api/reviews/{id}/undo", post(undo_review))
        .route("/api/proposals/{id}/decide", post(decide))
        .route("/api/tags", post(tag))
        .route("/api/for", post(mark_for))
        .route("/api/tags/{thread}", axum::routing::delete(untag))
        .route("/api/feedback", get(feedback))
        .route("/api/send", post(send))
        .route("/api/drafts/reply", post(draft_reply))
        .route("/api/drafts/email", post(draft_email))
        .route("/api/holding", get(holding))
        .route("/api/changesets/{id}/approve", post(approve))
        .route("/api/changesets/{id}/reject", post(reject))
        .route("/api/changesets/{id}/cancel", post(cancel))
        .route("/api/changesets/{id}/replace", post(replace))
        .route("/api/unsubscribe", post(unsubscribe))
        .route("/api/kairos/task", post(kairos_task))
        .route("/api/kairos/event", post(kairos_event))
        .route("/api/kairos/free", post(kairos_free))
        .route("/api/kairos/tasks/{id}", get(task_status))
        .route("/api/suggestions", get(suggestions))
        .route("/api/suggestions/{id}/dismiss", post(dismiss_suggestion))
        .route("/api/runs", get(runs).post(start_run))
        .route("/api/runs/{id}", get(run_view))
        .route("/api/runs/{id}/fail", post(fail_run));
    let api = match mcp {
        Some(m) => api.merge(m),
        None => api,
    };
    let api = api.layer(middleware::from_fn_with_state(st.clone(), auth));
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .merge(api)
        .with_state(st)
}

async fn status(State(st): State<AppState>) -> HResult<Value> {
    let v = with_engine(&st, |e| {
        let accounts: Vec<Value> = e
            .cfg
            .accounts
            .iter()
            .map(|a| {
                let synced = e.db.sync_state(&a.id).ok().flatten().map(|(_, at)| at);
                json!({"id": a.id, "name": a.name, "synced_at": synced, "messages": e.db.count(Some(&a.id)).unwrap_or(0)})
            })
            .collect();
        let open_reviews = e.db.reviews(Some("open"))?.len();
        Ok(json!({"live": e.cfg.live, "accounts": accounts, "open_reviews": open_reviews}))
    })
    .await?;
    Ok(Json(v))
}

/// A harmless AppleScript run *by keryxd*, so macOS asks once for
/// keryxd's own "control Mail" permission (doctor calls this).
async fn check_automation() -> HResult<Value> {
    let out = tokio::task::spawn_blocking(|| {
        std::process::Command::new("osascript")
            .args(["-e", "tell application \"Mail\" to get name"])
            .output()
    })
    .await
    .map_err(|e| HttpError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(|e| HttpError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Ok(Json(json!({ "ok": out.status.success(), "error": err })))
}

async fn sync_now(State(st): State<AppState>) -> HResult<Value> {
    let _ = st.sync_now.try_send(());
    Ok(Json(json!({"ok": true})))
}

async fn triage(State(st): State<AppState>, h: HeaderMap, Json(req): Json<TriageReq>) -> HResult<TriageOutcome> {
    let w = who(&h);
    Ok(Json(with_engine(&st, move |e| e.triage(req, w)).await?))
}

async fn undo_last(State(st): State<AppState>, h: HeaderMap) -> HResult<keryx_core::changeset::Changeset> {
    let w = who(&h);
    Ok(Json(with_engine(&st, move |e| e.undo_last(w)).await?))
}

#[derive(Deserialize)]
struct ListQ {
    status: Option<String>,
    limit: Option<u32>,
}

async fn changesets(
    State(st): State<AppState>,
    Query(q): Query<ListQ>,
) -> HResult<Vec<keryx_core::changeset::Changeset>> {
    let status = match q.status.as_deref() {
        Some(s) => {
            Some(Status::parse(s).ok_or_else(|| HttpError(StatusCode::BAD_REQUEST, format!("unknown status {s}")))?)
        }
        None => None,
    };
    let limit = q.limit.unwrap_or(50);
    Ok(Json(
        with_engine(&st, move |e| Ok(e.db.changesets(status, limit)?)).await?,
    ))
}

async fn changeset(State(st): State<AppState>, Path(id): Path<String>) -> HResult<keryx_core::changeset::Changeset> {
    Ok(Json(
        with_engine(&st, move |e| {
            e.db.changeset(&id)?
                .ok_or_else(|| EngineError::NotFound(format!("changeset {id}")))
        })
        .await?,
    ))
}

async fn undo(
    State(st): State<AppState>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> HResult<keryx_core::changeset::Changeset> {
    let w = who(&h);
    Ok(Json(with_engine(&st, move |e| e.undo(&id, w)).await?))
}

async fn propose(State(st): State<AppState>, h: HeaderMap, Json(req): Json<ProposeReq>) -> HResult<ProposeOutcome> {
    let w = who(&h);
    Ok(Json(with_engine(&st, move |e| e.propose(req, w)).await?))
}

#[derive(Deserialize)]
struct ReviewsQ {
    status: Option<String>,
}

async fn reviews(State(st): State<AppState>, Query(q): Query<ReviewsQ>) -> HResult<Vec<ReviewView>> {
    Ok(Json(
        with_engine(&st, move |e| {
            let list = e.db.reviews(q.status.as_deref())?;
            list.iter().map(|r| e.review(&r.id)).collect()
        })
        .await?,
    ))
}

async fn review(State(st): State<AppState>, Path(id): Path<String>) -> HResult<ReviewView> {
    Ok(Json(with_engine(&st, move |e| e.review(&id)).await?))
}

#[derive(Deserialize)]
struct GroupReq {
    group: String,
}

async fn accept_group(
    State(st): State<AppState>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(g): Json<GroupReq>,
) -> HResult<AcceptGroup> {
    let w = who(&h);
    Ok(Json(with_engine(&st, move |e| e.accept_group(&id, &g.group, w)).await?))
}

async fn undo_review(State(st): State<AppState>, h: HeaderMap, Path(id): Path<String>) -> HResult<UndoReview> {
    let w = who(&h);
    Ok(Json(with_engine(&st, move |e| e.undo_review(&id, w)).await?))
}

async fn decide(
    State(st): State<AppState>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(d): Json<Decision>,
) -> HResult<Value> {
    let w = who(&h);
    let cs = with_engine(&st, move |e| e.decide(&id, d, w)).await?;
    Ok(Json(json!({ "changeset": cs })))
}

#[derive(Deserialize)]
struct ForReq {
    r#ref: String,
    #[serde(default)]
    id: Option<String>,
}

/// `m`: what a thread is for, kept in keryx; the mail does not move (round 4).
async fn mark_for(State(st): State<AppState>, Json(f): Json<ForReq>) -> HResult<Value> {
    with_engine(&st, move |e| e.mark_for(&f.r#ref, f.id.as_deref())).await?;
    Ok(Json(json!({"ok": true})))
}

#[derive(Deserialize)]
struct TagReq {
    r#ref: String,
    reason: String,
}

async fn tag(State(st): State<AppState>, h: HeaderMap, Json(t): Json<TagReq>) -> HResult<Value> {
    let w = who(&h);
    with_engine(&st, move |e| e.tag(&t.r#ref, &t.reason, w)).await?;
    Ok(Json(json!({"ok": true})))
}

async fn untag(State(st): State<AppState>, Path(thread): Path<String>) -> HResult<Value> {
    let cleared = with_engine(&st, move |e| e.untag(&thread)).await?;
    Ok(Json(json!({ "cleared": cleared })))
}

#[derive(Deserialize)]
struct FeedbackQ {
    account: Option<String>,
    limit: Option<usize>,
}

async fn feedback(State(st): State<AppState>, Query(q): Query<FeedbackQ>) -> HResult<Vec<Feedback>> {
    Ok(Json(
        with_engine(&st, move |e| {
            e.cleanup_feedback(q.account.as_deref(), q.limit.unwrap_or(50))
        })
        .await?,
    ))
}

type Cs = keryx_core::changeset::Changeset;

async fn send(State(st): State<AppState>, h: HeaderMap, Json(r): Json<SendReq>) -> HResult<Cs> {
    let w = who(&h);
    Ok(Json(with_engine(&st, move |e| e.send(&r, w)).await?))
}

async fn draft_reply(State(st): State<AppState>, h: HeaderMap, Json(r): Json<DraftReplyReq>) -> HResult<Cs> {
    let w = who(&h);
    Ok(Json(with_engine(&st, move |e| e.draft_reply(&r, w)).await?))
}

async fn draft_email(State(st): State<AppState>, h: HeaderMap, Json(r): Json<DraftEmailReq>) -> HResult<Cs> {
    let w = who(&h);
    Ok(Json(with_engine(&st, move |e| e.draft_email(&r, w)).await?))
}

async fn kairos_task(State(st): State<AppState>, h: HeaderMap, Json(r): Json<KairosTaskReq>) -> HResult<KairosOutcome> {
    let w = who(&h);
    Ok(Json(with_engine(&st, move |e| e.kairos_task(&r, w)).await?))
}

async fn kairos_event(
    State(st): State<AppState>,
    h: HeaderMap,
    Json(r): Json<KairosEventReq>,
) -> HResult<KairosOutcome> {
    let w = who(&h);
    Ok(Json(with_engine(&st, move |e| e.kairos_event(&r, w)).await?))
}

async fn kairos_free(State(st): State<AppState>, Json(r): Json<FreeReq>) -> HResult<FreeOut> {
    Ok(Json(with_engine(&st, move |e| e.kairos_free(&r)).await?))
}

async fn task_status(State(st): State<AppState>, Path(id): Path<String>) -> HResult<Option<TaskStatus>> {
    Ok(Json(with_engine(&st, move |e| e.kairos_task_status(&id)).await?))
}

async fn suggestions(
    State(st): State<AppState>,
    Query(q): Query<ReviewsQ>,
) -> HResult<Vec<keryx_store::state::Suggestion>> {
    Ok(Json(
        with_engine(&st, move |e| Ok(e.db.suggestions(q.status.as_deref(), None)?)).await?,
    ))
}

async fn dismiss_suggestion(State(st): State<AppState>, h: HeaderMap, Path(id): Path<String>) -> HResult<Value> {
    let w = who(&h);
    with_engine(&st, move |e| e.dismiss_suggestion(&id, w)).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct StartRunReq {
    kind: String,
    #[serde(default)]
    scope: Value,
}

async fn start_run(State(st): State<AppState>, h: HeaderMap, Json(r): Json<StartRunReq>) -> HResult<Value> {
    let w = who(&h);
    let kind = keryx_store::runs::RunKind::parse(&r.kind)
        .ok_or_else(|| HttpError(StatusCode::BAD_REQUEST, format!("unknown run kind {:?}", r.kind)))?;
    let (run, token) = with_engine(&st, move |e| e.start_run(kind, r.scope, w)).await?;
    Ok(Json(json!({ "run": run, "token": token })))
}

#[derive(Deserialize)]
struct LimitQ {
    limit: Option<u32>,
}

async fn runs(State(st): State<AppState>, Query(q): Query<LimitQ>) -> HResult<Vec<keryx_store::runs::RunView>> {
    let limit = q.limit.unwrap_or(30).min(200);
    Ok(Json(
        with_engine(&st, move |e| {
            let list = e.db.runs(limit)?;
            list.iter().map(|r| e.run_view(&r.id)).collect()
        })
        .await?,
    ))
}

async fn run_view(State(st): State<AppState>, Path(id): Path<String>) -> HResult<keryx_store::runs::RunView> {
    Ok(Json(with_engine(&st, move |e| e.run_view(&id)).await?))
}

#[derive(Deserialize)]
struct FailReq {
    error: String,
}

async fn fail_run(
    State(st): State<AppState>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(r): Json<FailReq>,
) -> HResult<keryx_store::runs::RunView> {
    let w = who(&h);
    Ok(Json(with_engine(&st, move |e| e.fail_run(&id, &r.error, w)).await?))
}

async fn holding(State(st): State<AppState>) -> HResult<Option<Cs>> {
    Ok(Json(with_engine(&st, |e| e.holding()).await?))
}

async fn approve(State(st): State<AppState>, h: HeaderMap, Path(id): Path<String>) -> HResult<Cs> {
    let w = who(&h);
    Ok(Json(with_engine(&st, move |e| e.approve(&id, w)).await?))
}

async fn reject(State(st): State<AppState>, h: HeaderMap, Path(id): Path<String>) -> HResult<Cs> {
    let w = who(&h);
    Ok(Json(with_engine(&st, move |e| e.reject(&id, w)).await?))
}

async fn cancel(State(st): State<AppState>, h: HeaderMap, Path(id): Path<String>) -> HResult<Cs> {
    let w = who(&h);
    Ok(Json(with_engine(&st, move |e| e.cancel(&id, w)).await?))
}

async fn replace(
    State(st): State<AppState>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(r): Json<SendReq>,
) -> HResult<Cs> {
    let w = who(&h);
    Ok(Json(with_engine(&st, move |e| e.replace(&id, &r, w)).await?))
}

#[derive(Deserialize)]
struct UnsubReq {
    r#ref: String,
}

async fn unsubscribe(State(st): State<AppState>, h: HeaderMap, Json(r): Json<UnsubReq>) -> HResult<UnsubscribeOutcome> {
    let w = who(&h);
    Ok(Json(with_engine(&st, move |e| e.unsubscribe(&r.r#ref, w)).await?))
}

#[cfg(test)]
#[path = "http_tests.rs"]
mod tests;
