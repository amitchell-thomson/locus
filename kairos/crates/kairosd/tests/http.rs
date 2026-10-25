#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::Env;
use kairosd::http::{self, AppState};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

const TOKEN: &str = "test-token-0123456789abcdef";

fn state(env: &Env) -> AppState {
    AppState {
        engine: Arc::new(Mutex::new(env.engine())),
        token: Arc::new(TOKEN.into()),
        sign_key: Arc::new(b"sign-key".to_vec()),
    }
}

async fn serve(st: AppState, listener: tokio::net::TcpListener) -> tokio::task::JoinHandle<()> {
    let app = http::router(
        st.clone(),
        Some(kairosd::mcp::router(st, vec!["127.0.0.1".into(), "localhost".into()])),
    );
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    })
}

async fn start(env: &Env) -> (String, tokio::task::JoinHandle<()>, std::net::SocketAddr) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let h = serve(state(env), l).await;
    (format!("http://{addr}"), h, addr)
}

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

#[tokio::test(flavor = "multi_thread")]
async fn health_and_auth() {
    let env = Env::new(true);
    let (base, _h, _) = start(&env).await;
    let c = client();
    assert_eq!(
        c.get(format!("{base}/healthz"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "ok"
    );
    assert_eq!(c.get(format!("{base}/api/status")).send().await.unwrap().status(), 401);
    assert_eq!(
        c.get(format!("{base}/api/status"))
            .bearer_auth("wrong")
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let s: Value = c
        .get(format!("{base}/api/status"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(s["live"], true);
    // MCP is behind the same auth.
    assert_eq!(
        c.post(format!("{base}/mcp"))
            .json(&json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn create_list_and_errors() {
    let env = Env::new(true);
    env.phone_event("Family", "fam", "Dinner", "2026-10-17 19:00", "2026-10-17 21:00");
    let (base, _h, _) = start(&env).await;
    let c = client();
    let r: Value = c
        .post(format!("{base}/api/events"))
        .bearer_auth(TOKEN)
        .header("x-kairos-author", "claude")
        .json(&json!({"title": "Squash", "start": "2026-10-13T19:00", "duration_min": 60, "key": "Sport"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["status"], "applied");
    assert_eq!(r["risk"], "low");
    assert!(r["changeset_id"].as_str().unwrap().starts_with("cs-"));
    let evs: Value = c
        .get(format!("{base}/api/events?from=2026-10-13&to=2026-10-14"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(evs["events"].as_array().unwrap().iter().any(|e| e["title"] == "Squash"));
    // Forbidden → 403 with reasons; bad input → 400.
    let resp = c
        .delete(format!("{base}/api/events/family/fam"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);
    let resp = c
        .post(format!("{base}/api/events"))
        .bearer_auth(TOKEN)
        .json(&json!({"title": "x", "start": "mon 9"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "ambiguous time is an error");
}

#[tokio::test(flavor = "multi_thread")]
async fn signed_links_approve_from_phone() {
    let env = Env::new(true);
    env.phone_event("Sport", "squash", "Squash", "2026-10-15 19:00", "2026-10-15 20:00");
    let st = state(&env);
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    let _h = serve(st.clone(), l).await;
    let c = client();
    let r: Value = c
        .delete(format!("{base}/api/events/sport/squash"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["status"], "pending");
    let id = r["changeset_id"].as_str().unwrap().to_string();
    // Bad signature, or a reject-signature used for approve: refused.
    assert_eq!(
        c.post(format!("{base}/a/{id}/approve?sig=nope"))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let rej = st.sign(&format!("{id}:reject"));
    assert_eq!(
        c.post(format!("{base}/a/{id}/approve?sig={rej}"))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    // GET only shows a confirmation page (link previews can't approve).
    let sig = st.sign(&format!("{id}:approve"));
    let page = c
        .get(format!("{base}/a/{id}/approve?sig={sig}"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(page.contains("Approve change?"));
    assert!(!env.items("Sport").is_empty());
    let done = c
        .post(format!("{base}/a/{id}/approve?sig={sig}"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(done.contains("applied"), "{done}");
    assert!(env.items("Sport").is_empty());
    // There is no phone check-in page any more (2026-10-25): you check in from the calendar.
    // An old link now reaches only the token-protected API, which refuses it.
    let day = "2026-10-12";
    let csig = st.sign(&format!("checkin:{day}"));
    let old = c
        .get(format!("{base}/checkin?day={day}&sig={csig}"))
        .send()
        .await
        .unwrap()
        .status();
    assert!(matches!(old.as_u16(), 401 | 404), "{old}");
}

async fn rpc(c: &reqwest::Client, base: &str, id: u64, method: &str, params: Value) -> Value {
    let resp = c
        .post(format!("{base}/mcp"))
        .bearer_auth(TOKEN)
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-06-18")
        .json(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let ct = resp
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    let text = resp.text().await.unwrap();
    assert!(status.is_success(), "{method}: {status} {text}");
    if ct.starts_with("text/event-stream") {
        let data = text
            .lines()
            .filter_map(|l| l.strip_prefix("data:"))
            .map(str::trim)
            .find(|d| d.contains("\"id\""))
            .unwrap();
        serde_json::from_str(data).unwrap()
    } else {
        serde_json::from_str(&text).unwrap()
    }
}

async fn initialize(c: &reqwest::Client, base: &str) {
    let init = rpc(
        c,
        base,
        1,
        "initialize",
        json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}),
    )
    .await;
    assert_eq!(init["result"]["serverInfo"]["name"], "kairos");
    assert!(
        init["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("QUEUED FOR ALEC'S APPROVAL")
    );
    let n = c
        .post(format!("{base}/mcp"))
        .bearer_auth(TOKEN)
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-06-18")
        .json(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        .send()
        .await
        .unwrap();
    assert!(n.status().is_success());
}

fn tool_text(v: &Value) -> Value {
    let t = v["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("{v}"));
    serde_json::from_str(t).unwrap_or(Value::String(t.into()))
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_tools_and_no_approve_tool() {
    let env = Env::new(true);
    let (base, _h, _) = start(&env).await;
    let c = client();
    initialize(&c, &base).await;
    let tools = rpc(&c, &base, 2, "tools/list", json!({})).await;
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for want in [
        "list_events",
        "find_free",
        "create_event",
        "update_event",
        "delete_event",
        "move_event",
        "share_to_family",
        "list_tasks",
        "create_task",
        "update_task",
        "complete_task",
        "create_deadline",
        "plan",
        "list_changesets",
        "get_changeset",
        "undo",
        "list_checkins",
        "checkin",
        "log_work",
        "start_task",
        "list_projects",
        "create_project",
        "update_project",
    ] {
        assert!(names.contains(&want), "missing tool {want}: {names:?}");
    }
    assert!(
        !names.iter().any(|n| n.contains("approve")),
        "there must be no approve tool"
    );
    // Proposed project time and project sessions were removed (2026-10-25).
    for gone in [
        "list_proposals",
        "accept_proposal",
        "reject_proposal",
        "plan_project_time",
        "attribute_to_project",
    ] {
        assert!(!names.contains(&gone), "{gone} should be gone: {names:?}");
    }

    // "Add a B4 problem sheet due Friday 17:00, about 4 hours"
    let r = rpc(
        &c,
        &base,
        3,
        "tools/call",
        json!({"name": "create_task", "arguments": {"title": "B4 problem sheet", "key": "Engineering", "estimate_min": 240, "due": "2026-10-16T17:00"}}),
    )
    .await;
    let v = tool_text(&r);
    assert_eq!(v["task"]["title"], "B4 problem sheet");
    assert_eq!(v["plan"]["changeset"]["status"], "applied", "{v}");
    assert_eq!(v["task"]["due"]["value"], "2026-10-16T16:00:00Z", "{v}");
    assert!(!env.read_all("Engineering").contains("⚑"), "a due date is not an event");

    // Engine errors come back as tool errors Claude can read.
    let r = rpc(
        &c,
        &base,
        4,
        "tools/call",
        json!({"name": "delete_event", "arguments": {"uid": "nope"}}),
    )
    .await;
    assert_eq!(r["result"]["isError"], true);
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_checkin_partly_done_plans_the_rest_again() {
    // "I only managed 20 min of the furniture" — told to Claude.
    let env = Env::new(true);
    let (base, _h, _) = start(&env).await;
    let c = client();
    initialize(&c, &base).await;
    let r = rpc(
        &c,
        &base,
        2,
        "tools/call",
        json!({"name": "create_task", "arguments": {"title": "Purchase Furniture", "key": "Personal", "estimate_min": 60, "due": "fri 17:00"}}),
    )
    .await;
    let v = tool_text(&r);
    let end: chrono::DateTime<chrono::Utc> = v["plan"]["created"][0]["end"].as_str().unwrap().parse().unwrap();
    *env.now.lock().unwrap() = end + chrono::Duration::minutes(5);
    let r = rpc(
        &c,
        &base,
        3,
        "tools/call",
        json!({"name": "list_checkins", "arguments": {}}),
    )
    .await;
    let items = tool_text(&r);
    let uid = items[0]["block_uid"].as_str().unwrap().to_string();
    let r = rpc(
        &c,
        &base,
        4,
        "tools/call",
        json!({"name": "checkin", "arguments": {"block_uid": uid, "outcome": "partial", "actual_min": 20}}),
    )
    .await;
    let p = tool_text(&r);
    let again: i64 = p["created"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| {
            let s: chrono::DateTime<chrono::Utc> = b["start"].as_str().unwrap().parse().unwrap();
            let e: chrono::DateTime<chrono::Utc> = b["end"].as_str().unwrap().parse().unwrap();
            (e - s).num_minutes()
        })
        .sum();
    assert_eq!(again, 40, "{p}");
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_survives_daemon_restart_mid_session() {
    let env = Env::new(true);
    let (base, h, addr) = start(&env).await;
    let c = client();
    initialize(&c, &base).await;
    let r = rpc(
        &c,
        &base,
        2,
        "tools/call",
        json!({"name": "create_task", "arguments": {"title": "Essay", "key": "Personal", "estimate_min": 60}}),
    )
    .await;
    assert_eq!(tool_text(&r)["task"]["title"], "Essay");

    // Kill the server (as if systemd restarted it / the network dropped)...
    h.abort();
    let _ = h.await;
    assert!(c.get(format!("{base}/healthz")).send().await.is_err(), "server is down");
    // ...and bring a fresh process up on the same port.
    let l = tokio::net::TcpListener::bind(addr).await.unwrap();
    let _h2 = serve(state(&env), l).await;
    // Same client, no re-initialize: the next tool call just works (stateless).
    let r = rpc(
        &c,
        &base,
        3,
        "tools/call",
        json!({"name": "list_tasks", "arguments": {}}),
    )
    .await;
    let tasks = tool_text(&r);
    assert!(
        tasks.as_array().unwrap().iter().any(|t| t["title"] == "Essay"),
        "{tasks}"
    );
}

/// The TUI draws kairosd's index: a write shows on the next fetch, and a
/// fetch that finds nothing new is an empty 304.
#[tokio::test(flavor = "multi_thread")]
async fn index_for_the_tui_with_etag() {
    let env = Env::new(true);
    let (base, _h, _) = start(&env).await;
    let from = common::at("2026-10-12 00:00");
    let to = common::at("2026-10-19 00:00");
    let b = base.clone();
    let (first, again) = tokio::task::spawn_blocking(move || {
        let d = kairos_store::client::Daemon::new(&b, TOKEN).unwrap();
        let (idx, tag) = d.index(from, to, None).unwrap().unwrap();
        (
            idx.instances.len(),
            (tag.clone(), d.index(from, to, Some(&tag)).unwrap().is_none()),
        )
    })
    .await
    .unwrap();
    assert_eq!(first, 0);
    assert!(again.1, "unchanged → 304");
    let r: Value = client()
        .post(format!("{base}/api/events"))
        .bearer_auth(TOKEN)
        .json(&json!({"title": "Squash", "start": "2026-10-13T19:00", "duration_min": 60, "key": "Sport"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["status"], "applied");
    let tag = again.0;
    let titles = tokio::task::spawn_blocking(move || {
        let d = kairos_store::client::Daemon::new(&base, TOKEN).unwrap();
        let (idx, _) = d.index(from, to, Some(&tag)).unwrap().expect("changed");
        idx.instances.into_iter().map(|i| i.title).collect::<Vec<_>>()
    })
    .await
    .unwrap();
    assert_eq!(titles, vec!["Squash".to_string()]);
}

#[tokio::test(flavor = "multi_thread")]
async fn serves_the_locus_taxonomy() {
    let env = Env::new(true);
    let (base, _h, _) = start(&env).await;
    let v: Value = client()
        .get(format!("{base}/api/taxonomy"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let areas: Vec<&str> = v["taxonomy"]["areas"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_str().unwrap())
        .collect();
    assert_eq!(areas, ["Studies", "Projects", "Careers", "Personal"]);
    assert!(
        v["registry"]["projects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == "tanker-flow")
    );
}
