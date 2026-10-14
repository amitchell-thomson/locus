#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::engine::Actuator;
use axum::body::Body;
use axum::http::Request as HttpRequest;
use keryx_core::changeset::{MsgRef, MsgState};
use keryx_core::config::Account;
use keryx_store::db::Db;
use tower::ServiceExt;

struct Ok_;
impl Actuator for Ok_ {
    fn apply(&mut self, _: &Account, _: &MsgRef, _: &MsgState, after: &MsgState) -> Result<Option<MsgState>, String> {
        Ok(Some(after.clone()))
    }
    fn resync(&mut self, _: &mut Db, _: &Account) -> Result<(), String> {
        Ok(())
    }
    fn send(&mut self, _: &Account, _: &keryx_core::changeset::Outgoing) -> Result<(), String> {
        Ok(())
    }
    fn unsubscribe(&mut self, _: &str) -> Result<(), String> {
        Ok(())
    }
}

const TOKEN: &str = "test-token";

fn app() -> (Router, Arc<Mutex<Engine>>) {
    let (mut cfg, db) = keryx_store::testkit::fixture_db();
    cfg.live = true;
    let engine = Arc::new(Mutex::new(Engine::new(cfg, db, Box::new(Ok_)).unwrap()));
    let (tx, _rx) = std::sync::mpsc::sync_channel(1);
    let st = AppState {
        engine: engine.clone(),
        token: Arc::new(TOKEN.into()),
        sync_now: tx,
    };
    (router(st, None), engine)
}

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut b = HttpRequest::builder().method(method).uri(path);
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let req = match body {
        Some(v) => b
            .header("content-type", "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let v = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, v)
}

const AUTH: (&str, &str) = ("authorization", "Bearer test-token");

fn lab_ref(e: &Arc<Mutex<Engine>>) -> String {
    let e = e.lock().unwrap();
    keryx_store::view::list(&e.db, &e.cfg, &keryx_store::view::View::Inbox, &Default::default())
        .unwrap()
        .into_iter()
        .find(|r| r.subject == "Lab partner for Thursday?")
        .unwrap()
        .r#ref
}

#[tokio::test]
async fn token_required() {
    let (app, _) = app();
    let (s, _) = call(&app, "GET", "/healthz", &[], None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, v) = call(&app, "GET", "/api/status", &[], None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert!(v["error"].as_str().unwrap().contains("bearer"));
    let (s, _) = call(&app, "GET", "/api/status", &[("authorization", "Bearer nope")], None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, v) = call(&app, "GET", "/api/status", &[AUTH], None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["live"], true);
}

#[tokio::test]
async fn authors_and_interactivity() {
    let (app, e) = app();
    let r = lab_ref(&e);
    let body = || Some(json!({"action": "archive", "refs": [r.clone()]}));
    // No author header: a script → people mail becomes a proposal.
    let (s, v) = call(&app, "POST", "/api/triage", &[AUTH], body()).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(v["applied"].is_null() && v["proposed"] == 1, "{v}");
    // "alec" without the interactive header isn't Alec typing.
    let (_, v) = call(&app, "POST", "/api/triage", &[AUTH, ("x-keryx-author", "alec")], body()).await;
    assert!(v["applied"].is_null(), "{v}");
    // Claude claiming interactivity isn't Alec either.
    let (_, v) = call(
        &app,
        "POST",
        "/api/triage",
        &[AUTH, ("x-keryx-author", "claude"), ("x-keryx-interactive", "1")],
        body(),
    )
    .await;
    assert!(v["applied"].is_null(), "{v}");
    // Alec in the TUI: applied.
    let (_, v) = call(
        &app,
        "POST",
        "/api/triage",
        &[AUTH, ("x-keryx-author", "alec"), ("x-keryx-interactive", "1")],
        body(),
    )
    .await;
    assert_eq!(v["applied"]["status"], "applied", "{v}");
    // And undone.
    let (s, v) = call(
        &app,
        "POST",
        "/api/undo-last",
        &[AUTH, ("x-keryx-author", "alec"), ("x-keryx-interactive", "1")],
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(v["summary"].as_str().unwrap().starts_with("undo: "));
}

#[tokio::test]
async fn only_alec_decides_and_errors_map_to_codes() {
    let (app, e) = app();
    let r = lab_ref(&e);
    let (_, v) = call(
        &app,
        "POST",
        "/api/reviews",
        &[AUTH, ("x-keryx-author", "claude")],
        Some(json!({"title": "cleanup", "items": [{"ref": r, "action": "archive", "reason": "done with"}]})),
    )
    .await;
    let rid = v["review_id"].as_str().unwrap().to_string();
    let (_, rv) = call(&app, "GET", &format!("/api/reviews/{rid}"), &[AUTH], None).await;
    let pid = rv["items"][0]["proposal"]["id"].as_str().unwrap().to_string();
    let (s, v) = call(
        &app,
        "POST",
        &format!("/api/proposals/{pid}/decide"),
        &[AUTH, ("x-keryx-author", "claude")],
        Some(json!({"decision": "accept"})),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
    let (s, v) = call(
        &app,
        "POST",
        &format!("/api/proposals/{pid}/decide"),
        &[AUTH, ("x-keryx-author", "alec"), ("x-keryx-interactive", "1")],
        Some(json!({"decision": "accept"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["changeset"]["status"], "applied");

    let (s, _) = call(&app, "GET", "/api/changesets/cs-nope", &[AUTH], None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, v) = call(
        &app,
        "POST",
        "/api/triage",
        &[AUTH],
        Some(json!({"action": "archive", "refs": []})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    let (s, _) = call(&app, "GET", "/api/changesets?status=bogus", &[AUTH], None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[test]
fn token_compare_is_exact() {
    assert!(constant_eq(b"abc", b"abc"));
    assert!(!constant_eq(b"abc", b"abd"));
    assert!(!constant_eq(b"abc", b"abcd"));
}

#[derive(Clone, Default)]
struct Who_(Arc<Mutex<Vec<keryx_store::kairos::As>>>);
impl keryx_store::kairos::Kairos for Who_ {
    fn create_task(&self, _: &Value, who: keryx_store::kairos::As) -> keryx_store::kairos::Result<Value> {
        self.0.lock().unwrap().push(who);
        Ok(json!({"task": {"id": "t-1", "title": "x"}}))
    }
    fn create_event(&self, _: &Value, who: keryx_store::kairos::As) -> keryx_store::kairos::Result<Value> {
        self.0.lock().unwrap().push(who);
        Ok(json!({"changeset_id": "c-1"}))
    }
    fn task(&self, _: &str) -> keryx_store::kairos::Result<Option<Value>> {
        Ok(None)
    }
    fn free(&self, _: &str, _: &str, _: u32) -> keryx_store::kairos::Result<Vec<keryx_core::freetime::Slot>> {
        Ok(vec![])
    }
}

#[tokio::test]
async fn kairos_author_comes_from_the_caller() {
    use keryx_store::kairos::As;
    let (app, engine) = app();
    let k = Who_::default();
    engine.lock().unwrap().kairos = Some(Box::new(k.clone()));
    let r = lab_ref(&engine);
    let body = json!({"ref": r, "title": "Lab partner"});
    // The TUI / a CLI on a terminal.
    let (s, _) = call(
        &app,
        "POST",
        "/api/kairos/task",
        &[AUTH, ("x-keryx-author", "alec"), ("x-keryx-interactive", "1")],
        Some(body.clone()),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    // The CLI without a terminal (no author header = script).
    let (s, _) = call(&app, "POST", "/api/kairos/task", &[AUTH], Some(body.clone())).await;
    assert_eq!(s, StatusCode::OK);
    // Claude claiming to be Claude over HTTP is refused too.
    let (s, _) = call(
        &app,
        "POST",
        "/api/kairos/task",
        &[AUTH, ("x-keryx-author", "claude")],
        Some(body),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(*k.0.lock().unwrap(), vec![As::Alec, As::Script]);
}
