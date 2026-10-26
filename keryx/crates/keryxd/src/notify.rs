//! Phone (SPEC §8.4): ntfy pushes and signed one-shot links. A link is
//! `HMAC(key, "<id>:<action>")`, so it acts only on its own changeset (or
//! run), and only a POST from its confirmation page acts at all: link
//! previews (GET) never approve anything.

use serde_json::json;
use sha2::{Digest, Sha256};

/// HMAC-SHA256 (RFC 2104).
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    const B: usize = 64;
    let mut k = [0u8; B];
    if key.len() > B {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(k.map(|b| b ^ 0x36));
    inner.update(msg);
    let ih = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(k.map(|b| b ^ 0x5c));
    outer.update(ih);
    outer.finalize().into()
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Signs and checks phone links.
#[derive(Clone)]
pub struct Links {
    key: [u8; 32],
    /// `http://<mac tailscale ip>:7421` (config `[server] url`).
    pub base: String,
}

impl Links {
    /// The link key is derived from the API token: rotating the token
    /// invalidates every outstanding link.
    pub fn new(api_token: &str, base: &str) -> Links {
        Links {
            key: hmac_sha256(api_token.as_bytes(), b"keryx phone links v1"),
            base: base.trim_end_matches('/').to_string(),
        }
    }

    pub fn sign(&self, id: &str, action: &str) -> String {
        hex::encode(hmac_sha256(&self.key, format!("{id}:{action}").as_bytes()))
    }

    pub fn verify(&self, id: &str, action: &str, sig: &str) -> bool {
        constant_eq(self.sign(id, action).as_bytes(), sig.trim().as_bytes())
    }

    /// A changeset page: `approve` / `reject` / `cancel` / `view`.
    pub fn url(&self, id: &str, action: &str) -> String {
        format!("{}/a/{id}/{action}?s={}", self.base, self.sign(id, action))
    }

    /// A run's page (its drafts, each with approve / reject).
    pub fn run_url(&self, id: &str) -> String {
        format!("{}/r/{id}?s={}", self.base, self.sign(id, "run"))
    }
}

/// One push.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Push {
    pub title: String,
    pub message: String,
    /// Opened when the notification is tapped.
    pub click: Option<String>,
    /// Buttons: (label, url). They open the confirmation page.
    pub actions: Vec<(String, String)>,
}

pub trait Notifier: Send {
    fn push(&self, p: &Push) -> Result<(), String>;
}

/// ntfy over HTTPS (JSON publishing, so titles can be UTF-8).
pub struct Ntfy {
    server: String,
    topic: String,
    http: reqwest::blocking::Client,
}

impl Ntfy {
    pub fn new(server: &str, topic: &str) -> Result<Ntfy, String> {
        Ok(Ntfy {
            server: server.trim_end_matches('/').to_string(),
            topic: topic.to_string(),
            http: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .map_err(|e| e.to_string())?,
        })
    }

    pub fn body(&self, p: &Push) -> serde_json::Value {
        let mut v = json!({
            "topic": self.topic,
            "title": p.title,
            "message": p.message,
            "actions": p.actions.iter().map(|(label, url)| json!({
                "action": "view", "label": label, "url": url, "clear": true,
            })).collect::<Vec<_>>(),
        });
        if let Some(c) = &p.click {
            v["click"] = json!(c);
        }
        v
    }
}

impl Notifier for Ntfy {
    fn push(&self, p: &Push) -> Result<(), String> {
        let r = self
            .http
            .post(&self.server)
            .json(&self.body(p))
            .send()
            .map_err(|e| e.to_string())?;
        if r.status().is_success() {
            Ok(())
        } else {
            Err(format!("ntfy answered {}", r.status()))
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn hmac_matches_rfc_4231() {
        // Test case 2.
        assert_eq!(
            hex::encode(hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn a_link_is_bound_to_its_id_and_action() {
        let l = Links::new("token", "http://100.1.2.3:7421/");
        let s = l.sign("cs-1", "approve");
        assert!(l.verify("cs-1", "approve", &s));
        assert!(!l.verify("cs-2", "approve", &s), "another changeset");
        assert!(!l.verify("cs-1", "reject", &s), "another action");
        assert!(!Links::new("other", "x").verify("cs-1", "approve", &s), "another key");
        assert_eq!(
            l.url("cs-1", "view"),
            format!("http://100.1.2.3:7421/a/cs-1/view?s={}", l.sign("cs-1", "view"))
        );
    }

    #[test]
    fn ntfy_body_has_buttons() {
        let n = Ntfy::new("https://ntfy.sh/", "t").unwrap();
        let b = n.body(&Push {
            title: "Send to Jane?".into(),
            message: "m".into(),
            click: Some("http://x/view".into()),
            actions: vec![("Approve".into(), "http://x/a".into())],
        });
        assert_eq!(b["topic"], "t");
        assert_eq!(b["actions"][0]["action"], "view");
        assert_eq!(b["click"], "http://x/view");
    }
}
