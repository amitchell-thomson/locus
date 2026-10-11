//! Blocking client for kairosd's HTTP API (used by the CLI and TUI).

use crate::{Result, StoreError, run_secret_cmd};
use kairos_core::api::{self, ApiError};
use kairos_core::changeset::{Changeset, WriteResult};
use kairos_core::config::Config;
use kairos_core::model::Meta;
use kairos_core::task::Task;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::time::Duration;

/// Sidecar rows as served by `/api/sidecar`: `(source, uid, meta)`.
pub type SidecarRows = Vec<(String, String, Meta)>;

pub struct Daemon {
    base: String,
    token: String,
    http: reqwest::blocking::Client,
}

/// An API error with the server's reasons (forbidden / conflict diff…).
#[derive(Debug)]
pub struct Rejected {
    pub status: u16,
    pub error: ApiError,
}

impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.error.error)?;
        for r in &self.error.reasons {
            write!(f, "\n  {r}")?;
        }
        Ok(())
    }
}

impl Daemon {
    pub fn from_config(cfg: &Config) -> Result<Daemon> {
        let base = cfg
            .daemon
            .url
            .clone()
            .ok_or_else(|| StoreError::Missing("daemon.url is not set in config".into()))?;
        let token = match std::env::var("KAIROS_TOKEN") {
            Ok(t) if !t.trim().is_empty() => t.trim().to_string(),
            _ => match &cfg.daemon.token_cmd {
                Some(cmd) => run_secret_cmd(cmd)?,
                None => return Err(StoreError::Missing("daemon.token_cmd is not set".into())),
            },
        };
        Daemon::new(&base, &token)
    }

    pub fn new(base: &str, token: &str) -> Result<Daemon> {
        Ok(Daemon {
            base: base.trim_end_matches('/').to_string(),
            token: token.to_string(),
            http: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(30))
                .connect_timeout(Duration::from_secs(3))
                .build()
                .map_err(|e| StoreError::Http(e.to_string()))?,
        })
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    fn send<T: DeserializeOwned>(
        &self,
        req: reqwest::blocking::RequestBuilder,
    ) -> Result<std::result::Result<T, Rejected>> {
        let resp = req
            .bearer_auth(&self.token)
            .header("x-kairos-author", "alec")
            .send()
            .map_err(|e| {
                StoreError::Http(format!(
                    "kairosd unreachable at {} ({}). Writes need the server; Calendar.app still works.",
                    self.base,
                    e.without_url()
                ))
            })?;
        let status = resp.status();
        let text = resp.text().map_err(|e| StoreError::Http(e.without_url().to_string()))?;
        if status.is_success() {
            Ok(Ok(serde_json::from_str(&text)?))
        } else {
            let error = serde_json::from_str::<ApiError>(&text).unwrap_or(ApiError {
                error: format!("HTTP {status}: {text}"),
                reasons: vec![],
            });
            Ok(Err(Rejected {
                status: status.as_u16(),
                error,
            }))
        }
    }

    pub fn get<T: DeserializeOwned>(&self, path: &str) -> Result<std::result::Result<T, Rejected>> {
        self.send(self.http.get(format!("{}{path}", self.base)))
    }

    pub fn post<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<std::result::Result<T, Rejected>> {
        self.send(self.http.post(format!("{}{path}", self.base)).json(body))
    }

    pub fn patch<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<std::result::Result<T, Rejected>> {
        self.send(self.http.patch(format!("{}{path}", self.base)).json(body))
    }

    pub fn delete<T: DeserializeOwned>(&self, path: &str) -> Result<std::result::Result<T, Rejected>> {
        self.send(self.http.delete(format!("{}{path}", self.base)))
    }

    pub fn healthy(&self) -> bool {
        self.http
            .get(format!("{}/healthz", self.base))
            .timeout(Duration::from_secs(2))
            .send()
            .is_ok_and(|r| r.status().is_success())
    }

    // -------------------------------------------------- typed helpers

    pub fn create_event(&self, r: &api::CreateEventReq) -> Result<std::result::Result<WriteResult, Rejected>> {
        self.post("/api/events", r)
    }
    pub fn update_event(
        &self,
        source: &str,
        uid: &str,
        r: &api::UpdateEventReq,
    ) -> Result<std::result::Result<serde_json::Value, Rejected>> {
        self.patch(&format!("/api/events/{}/{}", enc(source), enc(uid)), r)
    }
    pub fn delete_event(&self, source: &str, uid: &str) -> Result<std::result::Result<WriteResult, Rejected>> {
        self.delete(&format!("/api/events/{}/{}", enc(source), enc(uid)))
    }
    pub fn move_event(&self, source: &str, uid: &str, key: &str) -> Result<std::result::Result<WriteResult, Rejected>> {
        self.post(
            &format!("/api/events/{}/{}/move", enc(source), enc(uid)),
            &api::KeyReq { key: key.into() },
        )
    }
    pub fn move_events(
        &self,
        events: &[(String, String)],
        key: &str,
    ) -> Result<std::result::Result<WriteResult, Rejected>> {
        let events = events
            .iter()
            .map(|(source, uid)| api::EventRef {
                source: source.clone(),
                uid: uid.clone(),
            })
            .collect();
        self.post(
            "/api/events/move",
            &api::MoveManyReq {
                key: key.into(),
                events,
            },
        )
    }
    pub fn share_family(&self, source: &str, uid: &str) -> Result<std::result::Result<WriteResult, Rejected>> {
        self.post(
            &format!("/api/events/{}/{}/share-family", enc(source), enc(uid)),
            &serde_json::json!({}),
        )
    }
    pub fn adopt(&self, source: &str, uid: &str, key: &str) -> Result<std::result::Result<WriteResult, Rejected>> {
        self.post(
            &format!("/api/events/{}/{}/adopt", enc(source), enc(uid)),
            &api::KeyReq { key: key.into() },
        )
    }
    pub fn import(&self, r: &api::ImportReq) -> Result<std::result::Result<WriteResult, Rejected>> {
        self.post("/api/import", r)
    }
    pub fn tasks(&self, status: &str) -> Result<std::result::Result<Vec<Task>, Rejected>> {
        self.get(&format!("/api/tasks?status={status}"))
    }
    pub fn create_task(&self, r: &api::CreateTaskReq) -> Result<std::result::Result<api::TaskResp, Rejected>> {
        self.post("/api/tasks", r)
    }
    pub fn update_task(
        &self,
        id: &str,
        r: &api::UpdateTaskReq,
    ) -> Result<std::result::Result<api::TaskResp, Rejected>> {
        self.patch(&format!("/api/tasks/{}", enc(id)), r)
    }
    pub fn complete_task(&self, id: &str, actual: Option<u32>) -> Result<std::result::Result<api::TaskResp, Rejected>> {
        self.post(
            &format!("/api/tasks/{}/complete", enc(id)),
            &api::CompleteTaskReq { actual_min: actual },
        )
    }
    pub fn drop_task(&self, id: &str) -> Result<std::result::Result<api::TaskResp, Rejected>> {
        self.post(&format!("/api/tasks/{}/drop", enc(id)), &serde_json::json!({}))
    }
    pub fn deadline(&self, r: &api::DeadlineReq) -> Result<std::result::Result<WriteResult, Rejected>> {
        self.post("/api/deadlines", r)
    }
    pub fn plan(&self, dry_run: bool) -> Result<std::result::Result<api::PlanResp, Rejected>> {
        self.post("/api/plan", &api::PlanReq { dry_run })
    }
    pub fn changesets(
        &self,
        status: Option<&str>,
        limit: usize,
    ) -> Result<std::result::Result<Vec<Changeset>, Rejected>> {
        let q = status.map(|s| format!("status={s}&")).unwrap_or_default();
        self.get(&format!("/api/changesets?{q}limit={limit}"))
    }
    pub fn changeset(&self, id: &str) -> Result<std::result::Result<Changeset, Rejected>> {
        self.get(&format!("/api/changesets/{}", enc(id)))
    }
    pub fn decide(&self, id: &str, action: &str) -> Result<std::result::Result<WriteResult, Rejected>> {
        self.post(&format!("/api/changesets/{}/{action}", enc(id)), &serde_json::json!({}))
    }
    pub fn sidecar(&self) -> Result<std::result::Result<SidecarRows, Rejected>> {
        self.get("/api/sidecar")
    }
    pub fn checkin_items(&self) -> Result<std::result::Result<Vec<api::CheckinItem>, Rejected>> {
        self.get("/api/checkin")
    }
    pub fn checkin(&self, r: &api::CheckinReq) -> Result<std::result::Result<api::PlanResp, Rejected>> {
        self.post("/api/checkin", r)
    }
    pub fn status(&self) -> Result<std::result::Result<serde_json::Value, Rejected>> {
        self.get("/api/status")
    }
}

/// Percent-encode a path segment.
fn enc(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

#[cfg(test)]
mod tests {
    #[test]
    fn encodes_uids() {
        assert_eq!(super::enc("a@b/c d"), "a%40b%2Fc%20d");
    }
}
