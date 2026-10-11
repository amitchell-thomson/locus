//! ntfy push: approval requests (Approve/Reject buttons → signed links),
//! morning summary, evening check-in.

use crate::engine::Notifier;
use kairos_core::changeset::Changeset;
use serde_json::{Value, json};
use std::time::Duration;

#[derive(Clone)]
pub struct Ntfy {
    pub server: String,
    pub topic: String,
    pub token: Option<String>,
    /// Base URL the phone uses to reach kairosd (Tailscale).
    pub public_url: String,
    pub sign_key: Vec<u8>,
}

impl Ntfy {
    pub fn signed(&self, id: &str, action: &str) -> String {
        format!(
            "{}/a/{id}/{action}?sig={}",
            self.public_url.trim_end_matches('/'),
            crate::http::sign(&self.sign_key, &format!("{id}:{action}"))
        )
    }

    pub fn checkin_url(&self, day: &str) -> String {
        format!(
            "{}/checkin?day={day}&sig={}",
            self.public_url.trim_end_matches('/'),
            crate::http::sign(&self.sign_key, &format!("checkin:{day}"))
        )
    }

    pub fn approval_message(&self, cs: &Changeset) -> Value {
        let mut msg = cs.summary.clone();
        if !cs.reasons.is_empty() {
            msg.push_str(&format!("\nNeeds approval: {}", cs.reasons.join("; ")));
        }
        json!({
            "topic": self.topic,
            "title": format!("kairos · {} wants to change your calendar", cs.author.as_str()),
            "message": msg,
            "priority": 4,
            "tags": ["calendar"],
            "actions": [
                {"action": "http", "label": "Approve", "url": self.signed(&cs.id, "approve"), "method": "POST", "clear": true},
                {"action": "http", "label": "Reject", "url": self.signed(&cs.id, "reject"), "method": "POST", "clear": true},
                {"action": "view", "label": "Details", "url": self.signed(&cs.id, "approve")}
            ]
        })
    }

    /// Blocking send (call from a thread / spawn_blocking).
    pub fn send(&self, body: &Value) -> Result<(), String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| e.to_string())?;
        let mut req = client.post(self.server.trim_end_matches('/')).json(body);
        if let Some(t) = &self.token {
            req = req.bearer_auth(t);
        }
        let r = req.send().map_err(|e| e.without_url().to_string())?;
        if r.status().is_success() {
            Ok(())
        } else {
            Err(format!("ntfy HTTP {}", r.status()))
        }
    }

    pub fn simple(&self, title: &str, message: &str, view: Option<(&str, String)>) -> Value {
        let mut v = json!({"topic": self.topic, "title": title, "message": message, "tags": ["calendar"]});
        if let Some((label, url)) = view {
            v["actions"] = json!([{"action": "view", "label": label, "url": url}]);
        }
        v
    }
}

impl Notifier for Ntfy {
    fn pending(&self, cs: &Changeset) {
        let me = self.clone();
        let body = self.approval_message(cs);
        std::thread::spawn(move || {
            if let Err(e) = me.send(&body) {
                tracing::warn!("ntfy approval push failed: {e}");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use kairos_core::changeset::{Risk, Status};
    use kairos_core::model::Author;

    #[test]
    fn approval_message_has_signed_actions() {
        let n = Ntfy {
            server: "https://ntfy.sh".into(),
            topic: "t".into(),
            token: None,
            public_url: "http://compute-node:7420/".into(),
            sign_key: b"k".to_vec(),
        };
        let cs = Changeset {
            id: "cs-1".into(),
            author: Author::Claude,
            created_at: Utc::now(),
            ops: vec![],
            risk: Risk::High,
            reasons: vec!["changes an event not created by kairos".into()],
            status: Status::Pending,
            summary: "Move Squash".into(),
            error: None,
            decided_at: None,
        };
        let v = n.approval_message(&cs);
        let url = v["actions"][0]["url"].as_str().unwrap();
        assert!(url.starts_with("http://compute-node:7420/a/cs-1/approve?sig="), "{url}");
        assert_eq!(v["actions"][0]["method"], "POST");
        assert!(v["message"].as_str().unwrap().contains("not created by kairos"));
        assert_ne!(n.signed("cs-1", "approve"), n.signed("cs-1", "reject"));
    }
}
