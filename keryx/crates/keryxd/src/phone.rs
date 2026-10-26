//! Phone pages (SPEC §8.4): reached from ntfy buttons over Tailscale, no
//! bearer token, each URL signed for one changeset (or run) and action.
//! **GET only shows a page**; the button on it POSTs, so link previews and
//! prefetchers can't approve anything.

use crate::engine_runs::actionable;
use crate::http::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Form, Router};
use keryx_core::changeset::{Changeset, Status};
use keryx_core::model::Address;
use serde::Deserialize;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/a/{id}/{action}", get(show).post(act))
        .route("/r/{id}", get(run_page))
}

#[derive(Deserialize)]
pub struct Sig {
    #[serde(default)]
    s: String,
}

/// HTML-escape (everything shown here can come from email or Claude).
pub fn esc(s: &str) -> String {
    keryx_core::text::sanitize(s)
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

const STYLE: &str = "body{font:16px -apple-system,system-ui,sans-serif;margin:0;padding:16px;background:#1e1e2e;color:#cdd6f4}\
h1{font-size:20px}.l{color:#9399b2;display:inline-block;min-width:4.5em}.to{font-weight:700}\
.w{color:#f9e2af;font-weight:700}pre{white-space:pre-wrap;background:#181825;padding:12px;border-radius:10px}\
button{font-size:18px;padding:12px 20px;border:0;border-radius:10px;margin:6px 6px 0 0}\
.ok{background:#a6e3a1;color:#11111b}.no{background:#f38ba8;color:#11111b}.mid{background:#89b4fa;color:#11111b}\
a{color:#89b4fa}.card{border:1px solid #45475a;border-radius:12px;padding:12px;margin:12px 0}";

fn page(title: &str, body: &str) -> Html<String> {
    Html(format!(
        "<!doctype html><html><head><meta charset=utf-8><meta name=viewport content=\"width=device-width,initial-scale=1\">\
         <meta name=robots content=noindex><title>{t}</title><style>{STYLE}</style></head><body>{body}</body></html>",
        t = esc(title)
    ))
}

fn denied() -> Response {
    (
        StatusCode::FORBIDDEN,
        page(
            "Link not valid",
            "<h1>This link isn't valid</h1><p>Nothing was changed.</p>",
        ),
    )
        .into_response()
}

fn addrs(v: &[Address]) -> String {
    v.iter().map(|a| esc(&a.display())).collect::<Vec<_>>().join("<br>")
}

/// The approval card: recipients first, warnings, then the message.
fn card(st: &AppState, cs: &Changeset) -> String {
    let Some(o) = cs.outgoing() else {
        return format!("<p>{}</p>", esc(&cs.summary));
    };
    let e = st.engine.lock().ok();
    let from = e
        .as_ref()
        .and_then(|e| e.cfg.account(&o.account).map(|a| format!("{} · {}", a.name, a.address)))
        .unwrap_or_else(|| o.account.clone());
    let mut h = format!("<div class=card><div><span class=l>From</span>{}</div>", esc(&from));
    for (label, list) in [("To", &o.to), ("Cc", &o.cc), ("Bcc", &o.bcc)] {
        if !list.is_empty() {
            h.push_str(&format!(
                "<div><span class=l>{label}</span><span class=to>{}</span></div>",
                addrs(list)
            ));
        }
    }
    for w in &cs.warnings {
        h.push_str(&format!("<div class=w>{}</div>", esc(w)));
    }
    h.push_str(&format!(
        "<div><span class=l>Subject</span>{}</div><pre>{}</pre>",
        esc(&o.subject),
        esc(&o.body)
    ));
    let urls = keryx_core::outgoing::urls(&o.body);
    if !urls.is_empty() {
        h.push_str("<div class=l>Links in this message:</div><ul>");
        for u in urls {
            h.push_str(&format!("<li>{}</li>", esc(&u)));
        }
        h.push_str("</ul>");
    }
    h.push_str(&format!(
        "<div class=l>Drafted by {} · {}</div></div>",
        esc(cs.author.as_str()),
        esc(cs.status.as_str())
    ));
    h
}

fn button(st: &AppState, id: &str, action: &str) -> String {
    let Some(sig) = st
        .engine
        .lock()
        .ok()
        .and_then(|e| e.links.as_ref().map(|l| l.sign(id, action)))
    else {
        return String::new();
    };
    let (label, class) = match action {
        "approve" => ("Approve and send", "ok"),
        "reject" => ("Reject", "no"),
        _ => ("Cancel the send", "mid"),
    };
    format!(
        "<form method=post action=\"/a/{id}/{action}\" style=display:inline>\
         <input type=hidden name=s value=\"{sig}\"><button class={class}>{label}</button></form>",
        id = esc(id)
    )
}

fn verified(st: &AppState, id: &str, action: &str, sig: &str) -> bool {
    st.engine
        .lock()
        .ok()
        .and_then(|e| e.links.as_ref().map(|l| l.verify(id, action, sig)))
        .unwrap_or(false)
}

/// `[notify] approvals`: off means pages only show, never act.
fn approvals_on(st: &AppState) -> bool {
    st.engine.lock().map(|e| e.cfg.notify.approvals).unwrap_or(false)
}

const OFF_NOTE: &str = "<p class=l>Phone approvals are off: approve or reject in keryx (A).</p>";

fn changeset(st: &AppState, id: &str) -> Option<Changeset> {
    st.engine.lock().ok().and_then(|e| e.db.changeset(id).ok().flatten())
}

/// GET: the confirmation page. Never changes anything.
async fn show(
    State(st): State<AppState>,
    Path((id, action)): Path<(String, String)>,
    Query(q): Query<Sig>,
) -> Response {
    if !matches!(action.as_str(), "approve" | "reject" | "cancel" | "view") || !verified(&st, &id, &action, &q.s) {
        return denied();
    }
    let Some(cs) = changeset(&st, &id) else {
        return denied();
    };
    let mut body = String::new();
    let heading = match (action.as_str(), cs.status) {
        (_, Status::Pending) if action == "reject" => "Reject this send?",
        (_, Status::Pending) => "Approve this send?",
        (_, Status::Holding) => "Sending shortly",
        _ => "Already decided",
    };
    body.push_str(&format!("<h1>{heading}</h1>"));
    body.push_str(&card(&st, &cs));
    if actionable(&cs) && !approvals_on(&st) {
        body.push_str(OFF_NOTE);
    } else if actionable(&cs) {
        let buttons: Vec<&str> = match (cs.status, action.as_str()) {
            (Status::Pending, "approve") => vec!["approve"],
            (Status::Pending, "reject") => vec!["reject"],
            (Status::Pending, _) => vec!["approve", "reject"],
            (Status::Holding, _) => vec!["cancel"],
            _ => vec![],
        };
        for b in buttons {
            body.push_str(&button(&st, &id, b));
        }
    }
    page(heading, &body).into_response()
}

/// POST from the page's button: act, then say what happened.
async fn act(State(st): State<AppState>, Path((id, action)): Path<(String, String)>, Form(f): Form<Sig>) -> Response {
    if !matches!(action.as_str(), "approve" | "reject" | "cancel") || !verified(&st, &id, &action, &f.s) {
        return denied();
    }
    let engine = st.engine.clone();
    let (id2, action2) = (id.clone(), action.clone());
    let res = tokio::task::spawn_blocking(move || {
        engine
            .lock()
            .map_err(|_| "engine lock poisoned".to_string())
            .and_then(|mut e| e.phone_action(&id2, &action2).map_err(|e| e.to_string()))
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|r| r);
    match res {
        Ok(cs) => {
            let (title, msg) = match cs.status {
                Status::Holding => {
                    let secs = st.engine.lock().map(|e| e.cfg.send.hold_seconds).unwrap_or(10);
                    ("Approved", format!("Sending in {secs} s."))
                }
                Status::Rejected => ("Rejected", "Nothing was sent.".to_string()),
                Status::Cancelled => ("Cancelled", "Nothing was sent.".to_string()),
                s => ("Done", s.as_str().to_string()),
            };
            let mut body = format!("<h1>{title}</h1><p>{}</p>", esc(&msg));
            if cs.status == Status::Holding {
                body.push_str(&button(&st, &id, "cancel"));
            }
            page(title, &body).into_response()
        }
        Err(e) => (
            StatusCode::CONFLICT,
            page("Not done", &format!("<h1>Not done</h1><p>{}</p>", esc(&e))),
        )
            .into_response(),
    }
}

/// A run's page: what it did, and its drafts with approve / reject.
async fn run_page(State(st): State<AppState>, Path(id): Path<String>, Query(q): Query<Sig>) -> Response {
    if !verified(&st, &id, "run", &q.s) {
        return denied();
    }
    let view = st.engine.lock().ok().and_then(|e| {
        let v = e.run_view(&id).ok()?;
        let drafts = e.run_drafts(&v).ok()?;
        Some((v, drafts))
    });
    let Some((v, drafts)) = view else { return denied() };
    let mut body = format!("<h1>{} run</h1><p>{}</p>", esc(v.run.kind.as_str()), esc(&v.counts()));
    if !v.run.summary.is_empty() {
        body.push_str(&format!("<pre>{}</pre>", esc(&v.run.summary)));
    }
    if let Some(e) = &v.run.error {
        body.push_str(&format!("<p class=w>{}</p>", esc(e)));
    }
    let on = approvals_on(&st);
    for cs in &drafts {
        body.push_str(&card(&st, cs));
        if actionable(cs) && !on {
            body.push_str(OFF_NOTE);
        } else if actionable(cs) {
            if cs.status == Status::Pending {
                body.push_str(&button(&st, &cs.id, "approve"));
                body.push_str(&button(&st, &cs.id, "reject"));
            } else {
                body.push_str(&button(&st, &cs.id, "cancel"));
            }
        }
    }
    body.push_str("<p class=l>Cleanups, tags and suggestions are reviewed in keryx.</p>");
    page("keryx run", &body).into_response()
}
