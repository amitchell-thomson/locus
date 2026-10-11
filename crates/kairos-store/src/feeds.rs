//! Read-only `.ics` feeds (Google secret iCal addresses, holidays):
//! conditional GET, validate, atomic replace. A bad response never wipes the
//! last good copy.

use crate::{Result, StoreError, io_err, run_secret_cmd, vdir};
use chrono::{DateTime, Utc};
use kairos_core::config::{Config, Source, SourceKind};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct FeedState {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub fetched_at: Option<DateTime<Utc>>,
    pub checked_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchOutcome {
    Updated { bytes: usize, events: usize },
    NotModified,
}

fn state_path(cfg: &Config, id: &str) -> PathBuf {
    cfg.data_dir().join("feeds").join(format!("{id}.state.json"))
}

pub fn state(cfg: &Config, id: &str) -> FeedState {
    fs::read_to_string(state_path(cfg, id))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_state(cfg: &Config, id: &str, st: &FeedState) -> Result<()> {
    let p = state_path(cfg, id);
    let dir = p.parent().map(PathBuf::from).unwrap_or_default();
    fs::create_dir_all(&dir).map_err(io_err(&dir))?;
    vdir::write_item(&dir, &format!("{id}.state.json"), &serde_json::to_string_pretty(st)?)
}

fn url_of(src: &Source) -> Result<String> {
    match (&src.url, &src.url_cmd) {
        (Some(u), _) => Ok(u.clone()),
        (None, Some(cmd)) => run_secret_cmd(cmd),
        (None, None) => Err(StoreError::Missing(format!("feed {} has no url/url_cmd", src.id))),
    }
}

/// Normalise `webcal://` to `https://`.
fn http_url(u: &str) -> String {
    match u.strip_prefix("webcal://") {
        Some(rest) => format!("https://{rest}"),
        None => u.to_string(),
    }
}

/// Fetch one feed.
pub fn fetch(cfg: &Config, src: &Source, client: &reqwest::blocking::Client) -> Result<FetchOutcome> {
    if src.kind != SourceKind::IcsReadonly {
        return Err(StoreError::Missing(format!("{} is not an ics feed", src.id)));
    }
    let mut st = state(cfg, &src.id);
    let url = http_url(&url_of(src)?);
    let dest = vdir::feed_path(cfg, &src.id);
    let mut req = client.get(&url);
    if dest.exists() {
        if let Some(e) = &st.etag {
            req = req.header("If-None-Match", e);
        }
        if let Some(m) = &st.last_modified {
            req = req.header("If-Modified-Since", m);
        }
    }
    st.checked_at = Some(Utc::now());
    // Don't leak the secret URL into errors/logs.
    let redact = |e: reqwest::Error| StoreError::Http(format!("{}: {}", src.id, e.without_url()));
    let result = (|| {
        let resp = req.send().map_err(redact)?;
        if resp.status() == reqwest::StatusCode::NOT_MODIFIED {
            return Ok(FetchOutcome::NotModified);
        }
        if !resp.status().is_success() {
            return Err(StoreError::Http(format!("{}: HTTP {}", src.id, resp.status())));
        }
        let etag = resp
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        let lm = resp
            .headers()
            .get("last-modified")
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        let body = resp.text().map_err(redact)?;
        let cals = kairos_core::ics::parse(&body)?;
        if !cals.iter().any(|c| c.name == "VCALENDAR") {
            return Err(StoreError::Http(format!("{}: response is not a VCALENDAR", src.id)));
        }
        let events = cals.iter().map(|c| c.children_named("VEVENT").count()).sum();
        let dir = dest.parent().map(PathBuf::from).unwrap_or_default();
        vdir::write_item(&dir, &format!("{}.ics", src.id), &body)?;
        st.etag = etag;
        st.last_modified = lm;
        st.fetched_at = Some(Utc::now());
        Ok(FetchOutcome::Updated {
            bytes: body.len(),
            events,
        })
    })();
    st.last_error = result.as_ref().err().map(ToString::to_string);
    save_state(cfg, &src.id, &st)?;
    result
}

pub fn client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("kairos/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| StoreError::Http(e.to_string()))
}

/// Fetch all feeds; per-feed results.
pub fn fetch_all(cfg: &Config) -> Vec<(String, Result<FetchOutcome>)> {
    let client = match client() {
        Ok(c) => c,
        Err(e) => return vec![("*".into(), Err(e))],
    };
    cfg.sources
        .iter()
        .filter(|s| s.kind == SourceKind::IcsReadonly)
        .map(|s| (s.id.clone(), fetch(cfg, s, &client)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webcal_rewritten() {
        assert_eq!(http_url("webcal://x.com/a.ics"), "https://x.com/a.ics");
        assert_eq!(http_url("https://x.com/a.ics"), "https://x.com/a.ics");
    }
}
