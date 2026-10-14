//! kairos's HTTP API (SPEC §9): tasks, events and free time. keryx always
//! sends the true author — Alec typing (`alec` + interactive) or a script —
//! and never on Claude's behalf (Claude only *suggests*; Alec saves).

use keryx_core::freetime::Slot;
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum KairosError {
    #[error("kairos isn't reachable at {url}: {msg}")]
    Unreachable { url: String, msg: String },
    #[error("kairos said: {0}")]
    Refused(String),
    #[error("no kairos token in the Keychain ({0})")]
    Token(String),
}

pub type Result<T> = std::result::Result<T, KairosError>;

/// Who keryx says is asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum As {
    /// Alec typing in the TUI or on a terminal: his own edit.
    Alec,
    /// A script or non-interactive CLI: kairos applies its full policy.
    Script,
}

impl As {
    /// The headers kairos reads (`X-Kairos-Author`, `X-Kairos-Interactive`).
    pub fn headers(self) -> [(&'static str, &'static str); 2] {
        match self {
            As::Alec => [("x-kairos-author", "alec"), ("x-kairos-interactive", "1")],
            As::Script => [("x-kairos-author", "script"), ("x-kairos-interactive", "0")],
        }
    }
}

/// The calls keryx makes (so tests can stand in for kairos).
pub trait Kairos: Send {
    /// `POST /api/tasks` → the created task (JSON from kairos).
    fn create_task(&self, req: &Value, who: As) -> Result<Value>;
    /// `POST /api/events`.
    fn create_event(&self, req: &Value, who: As) -> Result<Value>;
    /// `GET /api/tasks/{id}`.
    fn task(&self, id: &str) -> Result<Option<Value>>;
    /// `GET /api/free`.
    fn free(&self, from: &str, to: &str, duration_min: u32) -> Result<Vec<Slot>>;
}

pub struct Http {
    base: String,
    token: String,
    http: reqwest::blocking::Client,
}

impl Http {
    pub fn new(base: &str, token_service: &str) -> Result<Http> {
        let token = crate::keychain::get(token_service).map_err(|e| KairosError::Token(e.to_string()))?;
        Http::with_token(base, &token)
    }

    pub fn with_token(base: &str, token: &str) -> Result<Http> {
        let token = token.to_string();
        let http = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(8))
            .build()
            .map_err(|e| KairosError::Unreachable {
                url: base.into(),
                msg: e.to_string(),
            })?;
        Ok(Http {
            base: base.trim_end_matches('/').to_string(),
            token,
            http,
        })
    }

    fn finish(&self, r: reqwest::Result<reqwest::blocking::Response>) -> Result<Option<Value>> {
        let resp = r.map_err(|e| KairosError::Unreachable {
            url: self.base.clone(),
            msg: e.to_string(),
        })?;
        let status = resp.status();
        let text = resp.text().unwrap_or_default();
        if status.as_u16() == 404 {
            return Ok(None);
        }
        if !status.is_success() {
            let msg = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string))
                .unwrap_or(text);
            return Err(KairosError::Refused(msg));
        }
        Ok(serde_json::from_str(&text).ok())
    }

    fn post(&self, path: &str, body: &Value, who: As) -> Result<Value> {
        let mut req = self
            .http
            .post(format!("{}{path}", self.base))
            .bearer_auth(&self.token)
            .json(body);
        for (k, v) in who.headers() {
            req = req.header(k, v);
        }
        self.finish(req.send())?
            .ok_or_else(|| KairosError::Refused("not found".into()))
    }
}

impl Kairos for Http {
    fn create_task(&self, req: &Value, who: As) -> Result<Value> {
        self.post("/api/tasks", req, who)
    }
    fn create_event(&self, req: &Value, who: As) -> Result<Value> {
        self.post("/api/events", req, who)
    }
    fn task(&self, id: &str) -> Result<Option<Value>> {
        self.finish(
            self.http
                .get(format!("{}/api/tasks/{id}", self.base))
                .bearer_auth(&self.token)
                .send(),
        )
    }
    fn free(&self, from: &str, to: &str, duration_min: u32) -> Result<Vec<Slot>> {
        let v = self
            .finish(
                self.http
                    .get(format!(
                        "{}/api/free?from={}&to={}&duration_min={duration_min}",
                        self.base,
                        enc(from),
                        enc(to)
                    ))
                    .bearer_auth(&self.token)
                    .send(),
            )?
            .unwrap_or(Value::Null);
        let ts = |s: Option<&Value>| {
            s.and_then(Value::as_str)
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.timestamp())
        };
        Ok(v.as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| {
                Some(Slot {
                    start: ts(x.get("start"))?,
                    end: ts(x.get("end"))?,
                })
            })
            .collect())
    }
}

/// Percent-encode a query value (times carry `+` and `:`).
fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn author_headers() {
        assert_eq!(
            As::Alec.headers(),
            [("x-kairos-author", "alec"), ("x-kairos-interactive", "1")]
        );
        assert_eq!(
            As::Script.headers(),
            [("x-kairos-author", "script"), ("x-kairos-interactive", "0")]
        );
    }

    /// A one-request HTTP server that hands back what it was sent.
    fn mock(reply: &'static str) -> (String, std::thread::JoinHandle<String>) {
        use std::io::{Read, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let n = s.read(&mut chunk).unwrap();
                buf.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&buf).to_string();
                if let Some(i) = text.find("\r\n\r\n") {
                    let len = text[..i]
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if buf.len() >= i + 4 + len {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            write!(s, "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}", reply.len()).unwrap();
            String::from_utf8_lossy(&buf).to_lowercase()
        });
        (base, h)
    }

    #[test]
    fn real_requests_carry_the_true_author() {
        for (who, author, interactive) in [(As::Alec, "alec", "1"), (As::Script, "script", "0")] {
            let (base, h) = mock(r#"{"task":{"id":"t-1"}}"#);
            let k = Http::with_token(&base, "tok").unwrap();
            let v = k.create_task(&serde_json::json!({"title": "x"}), who).unwrap();
            assert_eq!(v["task"]["id"], "t-1");
            let req = h.join().unwrap();
            assert!(req.starts_with("post /api/tasks "), "{req}");
            assert!(req.contains(&format!("x-kairos-author: {author}\r\n")), "{req}");
            assert!(
                req.contains(&format!("x-kairos-interactive: {interactive}\r\n")),
                "{req}"
            );
            assert!(req.contains("authorization: bearer tok\r\n"), "{req}");
        }
        let (base, h) =
            mock(r#"[{"start":"2026-10-20T14:00:00+01:00","end":"2026-10-20T15:00:00+01:00","minutes":60}]"#);
        let k = Http::with_token(&base, "tok").unwrap();
        let v = k.free("2026-10-20T00:00+01:00", "2026-10-21", 60).unwrap();
        assert_eq!(v[0].end - v[0].start, 3600);
        assert!(
            h.join()
                .unwrap()
                .starts_with("get /api/free?from=2026-10-20t00%3a00%2b01%3a00&to=2026-10-21&duration_min=60 ")
        );
    }

    #[test]
    fn unreachable_is_named() {
        let k = Http::with_token("http://127.0.0.1:1", "tok").unwrap();
        assert!(matches!(
            k.create_task(&Value::Null, As::Alec),
            Err(KairosError::Unreachable { .. })
        ));
    }

    #[test]
    fn query_values_are_encoded() {
        assert_eq!(enc("2026-10-15T09:00+01:00"), "2026-10-15T09%3A00%2B01%3A00");
    }
}
