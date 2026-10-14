//! Blocking client for keryxd's HTTP API (TUI, CLI). It always says who is
//! asking: `alec` + interactive from the TUI or a terminal, else `script`.

use crate::api::*;
use keryx_core::changeset::Changeset;
use keryx_core::config::Config;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("keryxd isn't reachable at {url}: {msg} (is it running? `keryx doctor`)")]
    Unreachable { url: String, msg: String },
    #[error("{0}")]
    Daemon(String),
    #[error("no API token: {0}")]
    Token(String),
    #[error("unexpected reply from keryxd: {0}")]
    Shape(String),
}

pub type Result<T> = std::result::Result<T, ClientError>;

/// The calls the TUI and CLI make, so tests can stand in for keryxd.
pub trait Daemon: Send {
    fn triage(&self, req: &TriageReq) -> Result<TriageOutcome>;
    fn undo_last(&self) -> Result<Changeset>;
    fn reviews(&self, status: Option<&str>) -> Result<Vec<ReviewView>>;
    fn review(&self, id: &str) -> Result<ReviewView>;
    fn decide(&self, proposal: &str, d: &Decision) -> Result<Option<Changeset>>;
    fn accept_group(&self, review: &str, group: &str) -> Result<AcceptGroup>;
    fn undo_review(&self, review: &str) -> Result<UndoReview>;
    fn tag(&self, r: &str, reason: &str) -> Result<()>;
    fn untag(&self, r: &str) -> Result<()>;
    fn sync(&self) -> Result<()>;
    fn send(&self, r: &SendReq) -> Result<Changeset>;
    fn approve(&self, id: &str) -> Result<Changeset>;
    fn reject(&self, id: &str) -> Result<Changeset>;
    fn cancel(&self, id: &str) -> Result<Changeset>;
    fn replace(&self, id: &str, r: &SendReq) -> Result<Changeset>;
    fn holding(&self) -> Result<Option<Changeset>>;
    fn unsubscribe(&self, r: &str) -> Result<UnsubscribeOutcome>;
    fn kairos_task(&self, r: &KairosTaskReq) -> Result<KairosOutcome>;
    fn kairos_event(&self, r: &KairosEventReq) -> Result<KairosOutcome>;
    fn kairos_free(&self, r: &FreeReq) -> Result<FreeOut>;
    fn task_status(&self, id: &str) -> Result<Option<TaskStatus>>;
    fn dismiss_suggestion(&self, id: &str) -> Result<()>;
}

pub struct Client {
    base: String,
    token: String,
    author: &'static str,
    interactive: bool,
    http: reqwest::blocking::Client,
}

impl Client {
    pub fn from_config(cfg: &Config) -> Result<Client> {
        let token = crate::keychain::get("keryx-api-token").map_err(|e| ClientError::Token(e.to_string()))?;
        let http = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(|e| ClientError::Shape(e.to_string()))?;
        Ok(Client {
            base: cfg.server_url(),
            token,
            author: "script",
            interactive: false,
            http,
        })
    }

    /// Alec typing (TUI, CLI on a TTY): his edits skip approval.
    pub fn interactive(mut self, yes: bool) -> Client {
        self.author = if yes { "alec" } else { "script" };
        self.interactive = yes;
        self
    }

    fn send<T: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&impl Serialize>,
    ) -> Result<T> {
        let url = format!("{}{path}", self.base);
        let mut req = self
            .http
            .request(method, &url)
            .bearer_auth(&self.token)
            .header("x-keryx-author", self.author)
            .header("x-keryx-interactive", if self.interactive { "1" } else { "0" });
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().map_err(|e| ClientError::Unreachable {
            url: self.base.clone(),
            msg: e.to_string(),
        })?;
        let status = resp.status();
        let text = resp.text().map_err(|e| ClientError::Shape(e.to_string()))?;
        if !status.is_success() {
            let msg = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string))
                .unwrap_or(text);
            return Err(ClientError::Daemon(msg));
        }
        serde_json::from_str(&text).map_err(|e| ClientError::Shape(e.to_string()))
    }

    fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.send(reqwest::Method::GET, path, None::<&Value>)
    }

    fn post<T: DeserializeOwned>(&self, path: &str, body: &impl Serialize) -> Result<T> {
        self.send(reqwest::Method::POST, path, Some(body))
    }
}

impl Client {
    pub fn undo(&self, id: &str) -> Result<Changeset> {
        self.post(&format!("/api/changesets/{id}/undo"), &json!({}))
    }
    pub fn changesets(&self, status: Option<&str>, limit: u32) -> Result<Vec<Changeset>> {
        match status {
            Some(s) => self.get(&format!("/api/changesets?status={s}&limit={limit}")),
            None => self.get(&format!("/api/changesets?limit={limit}")),
        }
    }
}

impl Daemon for Client {
    fn triage(&self, req: &TriageReq) -> Result<TriageOutcome> {
        self.post("/api/triage", req)
    }
    fn undo_last(&self) -> Result<Changeset> {
        self.post("/api/undo-last", &json!({}))
    }
    fn reviews(&self, status: Option<&str>) -> Result<Vec<ReviewView>> {
        match status {
            Some(s) => self.get(&format!("/api/reviews?status={s}")),
            None => self.get("/api/reviews"),
        }
    }
    fn review(&self, id: &str) -> Result<ReviewView> {
        self.get(&format!("/api/reviews/{id}"))
    }
    fn decide(&self, proposal: &str, d: &Decision) -> Result<Option<Changeset>> {
        let v: Value = self.post(&format!("/api/proposals/{proposal}/decide"), d)?;
        serde_json::from_value(v.get("changeset").cloned().unwrap_or(Value::Null))
            .map_err(|e| ClientError::Shape(e.to_string()))
    }
    fn accept_group(&self, review: &str, group: &str) -> Result<AcceptGroup> {
        self.post(
            &format!("/api/reviews/{review}/accept-group"),
            &json!({ "group": group }),
        )
    }
    fn undo_review(&self, review: &str) -> Result<UndoReview> {
        self.post(&format!("/api/reviews/{review}/undo"), &json!({}))
    }
    fn tag(&self, r: &str, reason: &str) -> Result<()> {
        let _: Value = self.post("/api/tags", &json!({ "ref": r, "reason": reason }))?;
        Ok(())
    }
    fn untag(&self, r: &str) -> Result<()> {
        let _: Value = self.send(reqwest::Method::DELETE, &format!("/api/tags/{r}"), None::<&Value>)?;
        Ok(())
    }
    fn sync(&self) -> Result<()> {
        let _: Value = self.post("/api/sync", &json!({}))?;
        Ok(())
    }
    fn send(&self, r: &SendReq) -> Result<Changeset> {
        self.post("/api/send", r)
    }
    fn approve(&self, id: &str) -> Result<Changeset> {
        self.post(&format!("/api/changesets/{id}/approve"), &json!({}))
    }
    fn reject(&self, id: &str) -> Result<Changeset> {
        self.post(&format!("/api/changesets/{id}/reject"), &json!({}))
    }
    fn cancel(&self, id: &str) -> Result<Changeset> {
        self.post(&format!("/api/changesets/{id}/cancel"), &json!({}))
    }
    fn replace(&self, id: &str, r: &SendReq) -> Result<Changeset> {
        self.post(&format!("/api/changesets/{id}/replace"), r)
    }
    fn holding(&self) -> Result<Option<Changeset>> {
        self.get("/api/holding")
    }
    fn unsubscribe(&self, r: &str) -> Result<UnsubscribeOutcome> {
        self.post("/api/unsubscribe", &json!({ "ref": r }))
    }
    fn kairos_task(&self, r: &KairosTaskReq) -> Result<KairosOutcome> {
        self.post("/api/kairos/task", r)
    }
    fn kairos_event(&self, r: &KairosEventReq) -> Result<KairosOutcome> {
        self.post("/api/kairos/event", r)
    }
    fn kairos_free(&self, r: &FreeReq) -> Result<FreeOut> {
        self.post("/api/kairos/free", r)
    }
    fn task_status(&self, id: &str) -> Result<Option<TaskStatus>> {
        self.get(&format!("/api/kairos/tasks/{id}"))
    }
    fn dismiss_suggestion(&self, id: &str) -> Result<()> {
        let _: Value = self.post(&format!("/api/suggestions/{id}/dismiss"), &json!({}))?;
        Ok(())
    }
}
