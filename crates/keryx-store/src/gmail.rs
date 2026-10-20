//! Gmail API: OAuth (loopback + PKCE), token refresh, and mirroring
//! (full sync, then `history.list` increments). Scope is `gmail.modify`
//! only — never `https://mail.google.com/` (SPEC §3).
//!
//! All HTTP goes through [`Transport`] so sync is tested against recorded
//! responses (`fixtures/gmail`).

use crate::db::{Db, DbError, Raw};
use base64::Engine;
use keryx_core::build::{self, SourceFacts, is_hidden_mailbox};
use keryx_core::config::Account;
use keryx_core::mime;
use keryx_core::model::Location;
use keryx_core::text::TEXT_VERSION;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};

pub const SCOPE: &str = "https://www.googleapis.com/auth/gmail.modify";
pub const API: &str = "https://gmail.googleapis.com/gmail/v1/users/me";
pub const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
pub const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
/// Raw messages larger than this aren't stored; attachments are fetched on demand.
pub const MAX_STORED_RAW: usize = 2 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum GmailError {
    #[error("Gmail HTTP {status}: {body}")]
    Http { status: u16, body: String },
    #[error("network: {0}")]
    Network(String),
    #[error("Gmail sent something unexpected: {0}")]
    Shape(String),
    #[error("Gmail isn't authorised yet: run `keryx auth gmail`")]
    NotAuthorised,
    #[error(transparent)]
    Db(#[from] DbError),
}

impl GmailError {
    fn is_not_found(&self) -> bool {
        matches!(self, GmailError::Http { status: 404, .. })
    }
}

pub type Result<T> = std::result::Result<T, GmailError>;

/// The HTTP calls keryx makes to Google.
pub trait Transport {
    fn get(&self, url: &str, bearer: &str) -> Result<Value>;
    fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<Value>;
    fn post_json(&self, url: &str, bearer: &str, body: &Value) -> Result<Value>;
}

/// The real transport.
pub struct Http {
    client: reqwest::blocking::Client,
}

impl Http {
    pub fn new() -> Result<Http> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("keryx")
            .build()
            .map_err(|e| GmailError::Network(e.to_string()))?;
        Ok(Http { client })
    }

    fn finish(resp: reqwest::Result<reqwest::blocking::Response>) -> Result<Value> {
        let resp = resp.map_err(|e| GmailError::Network(e.to_string()))?;
        let status = resp.status().as_u16();
        let text = resp.text().map_err(|e| GmailError::Network(e.to_string()))?;
        if !(200..300).contains(&status) {
            return Err(GmailError::Http {
                status,
                body: text.chars().take(300).collect(),
            });
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).map_err(|e| GmailError::Shape(e.to_string()))
    }
}

impl Transport for Http {
    fn get(&self, url: &str, bearer: &str) -> Result<Value> {
        Http::finish(self.client.get(url).bearer_auth(bearer).send())
    }
    fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<Value> {
        Http::finish(self.client.post(url).form(form).send())
    }
    fn post_json(&self, url: &str, bearer: &str, body: &Value) -> Result<Value> {
        Http::finish(self.client.post(url).bearer_auth(bearer).json(body).send())
    }
}

/// The desktop OAuth client (the JSON Google lets you download).
#[derive(Debug, Clone, Deserialize)]
pub struct ClientSecret {
    pub client_id: String,
    pub client_secret: String,
}

impl ClientSecret {
    /// Accepts Google's download (`{"installed": {...}}`) or the bare object.
    pub fn from_json(s: &str) -> Result<ClientSecret> {
        let v: Value = serde_json::from_str(s).map_err(|e| GmailError::Shape(e.to_string()))?;
        let inner = v.get("installed").or_else(|| v.get("web")).unwrap_or(&v);
        serde_json::from_value(inner.clone()).map_err(|e| GmailError::Shape(e.to_string()))
    }
}

/// Exchange a refresh token for an access token.
pub fn access_token(t: &dyn Transport, client: &ClientSecret, refresh: &str) -> Result<String> {
    let v = t.post_form(
        TOKEN_URL,
        &[
            ("client_id", &client.client_id),
            ("client_secret", &client.client_secret),
            ("refresh_token", refresh),
            ("grant_type", "refresh_token"),
        ],
    )?;
    v.get("access_token")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| GmailError::Shape("token response without access_token".into()))
}

/// A Gmail client: a transport plus an access token.
pub struct Client<'a> {
    pub t: &'a dyn Transport,
    pub token: String,
}

impl Client<'_> {
    fn get(&self, path: &str) -> Result<Value> {
        self.t.get(&format!("{API}{path}"), &self.token)
    }

    pub fn profile_history_id(&self) -> Result<String> {
        let v = self.get("/profile")?;
        str_field(&v, "historyId")
    }

    /// Label id → name.
    pub fn labels(&self) -> Result<BTreeMap<String, String>> {
        let v = self.get("/labels")?;
        let mut out = BTreeMap::new();
        for l in v.get("labels").and_then(Value::as_array).into_iter().flatten() {
            if let (Some(id), Some(name)) = (
                l.get("id").and_then(Value::as_str),
                l.get("name").and_then(Value::as_str),
            ) {
                out.insert(id.to_string(), name.to_string());
            }
        }
        Ok(out)
    }

    /// Every message id (and thread id), excluding spam and trash.
    pub fn list_all(&self) -> Result<Vec<(String, String)>> {
        let mut out = Vec::new();
        let mut page: Option<String> = None;
        loop {
            let path = match &page {
                Some(p) => format!("/messages?maxResults=500&pageToken={p}"),
                None => "/messages?maxResults=500".to_string(),
            };
            let v = self.get(&path)?;
            for m in v.get("messages").and_then(Value::as_array).into_iter().flatten() {
                out.push((str_field(m, "id")?, str_field(m, "threadId")?));
            }
            match v.get("nextPageToken").and_then(Value::as_str) {
                Some(p) => page = Some(p.to_string()),
                None => return Ok(out),
            }
        }
    }

    pub fn get_raw(&self, id: &str) -> Result<RawMessage> {
        let v = self.get(&format!("/messages/{id}?format=raw"))?;
        let raw_b64 = str_field(&v, "raw")?;
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(raw_b64.trim_end_matches('='))
            .map_err(|e| GmailError::Shape(format!("raw isn't base64url: {e}")))?;
        Ok(RawMessage { meta: meta(&v)?, raw })
    }

    pub fn get_meta(&self, id: &str) -> Result<Meta> {
        meta(&self.get(&format!("/messages/{id}?format=minimal"))?)
    }

    /// Change a message's labels; returns its labels afterwards.
    pub fn modify(&self, id: &str, add: &[String], remove: &[String]) -> Result<Vec<String>> {
        let v = self.t.post_json(
            &format!("{API}/messages/{id}/modify"),
            &self.token,
            &serde_json::json!({"addLabelIds": add, "removeLabelIds": remove}),
        )?;
        Ok(meta(&v)?.labels)
    }

    /// Send RFC 5322 bytes, in a thread if given; returns the new message id.
    pub fn send(&self, raw: &[u8], thread_id: Option<&str>) -> Result<String> {
        let mut body = serde_json::json!({"raw": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw)});
        if let Some(t) = thread_id {
            body["threadId"] = serde_json::json!(t);
        }
        let v = self.t.post_json(&format!("{API}/messages/send"), &self.token, &body)?;
        str_field(&v, "id")
    }

    /// History since `start`, or `Err(404)` if it has expired.
    pub fn history(&self, start: &str) -> Result<History> {
        let mut h = History::default();
        let mut page: Option<String> = None;
        loop {
            let path = match &page {
                Some(p) => format!("/history?startHistoryId={start}&pageToken={p}"),
                None => format!("/history?startHistoryId={start}"),
            };
            let v = self.get(&path)?;
            h.history_id = v
                .get("historyId")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or(h.history_id.take());
            for rec in v.get("history").and_then(Value::as_array).into_iter().flatten() {
                for m in rec.get("messages").and_then(Value::as_array).into_iter().flatten() {
                    if let Some(id) = m.get("id").and_then(Value::as_str) {
                        h.touched.insert(id.to_string());
                    }
                }
                for d in rec
                    .get("messagesDeleted")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let Some(id) = d.pointer("/message/id").and_then(Value::as_str) {
                        h.deleted.insert(id.to_string());
                    }
                }
            }
            match v.get("nextPageToken").and_then(Value::as_str) {
                Some(p) => page = Some(p.to_string()),
                None => return Ok(h),
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Meta {
    pub id: String,
    pub thread_id: String,
    pub labels: Vec<String>,
    pub internal_date: i64,
    pub size: u64,
}

pub struct RawMessage {
    pub meta: Meta,
    pub raw: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct History {
    pub history_id: Option<String>,
    /// Added or re-labelled.
    pub touched: HashSet<String>,
    pub deleted: HashSet<String>,
}

fn str_field(v: &Value, k: &str) -> Result<String> {
    v.get(k)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| GmailError::Shape(format!("missing {k}")))
}

fn meta(v: &Value) -> Result<Meta> {
    Ok(Meta {
        id: str_field(v, "id")?,
        thread_id: str_field(v, "threadId")?,
        labels: v
            .get("labelIds")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        internal_date: v
            .get("internalDate")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<i64>().ok())
            .map(|ms| ms / 1000)
            .unwrap_or(0),
        size: v.get("sizeEstimate").and_then(Value::as_u64).unwrap_or(0),
    })
}

/// Sync state kept in `sync_state.state` for a Gmail account.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub history_id: Option<String>,
    /// Label id → name, refreshed every sync.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub full: bool,
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
}

/// Is a message with these labels kept out of the mirror (trash, spam, drafts)?
pub fn hidden(labels: &[String]) -> bool {
    labels.iter().any(|l| is_hidden_mailbox(l))
}

/// Where a Gmail message is: its labels minus UNREAD and STARRED, which
/// the mirror keeps as `read` / `flagged`.
pub fn location(labels: &[String]) -> Location {
    let mut l: Vec<String> = labels
        .iter()
        .filter(|l| *l != "UNREAD" && *l != "STARRED")
        .cloned()
        .collect();
    l.sort();
    Location::Labels { labels: l }
}

fn store_raw(db: &Db, account: &Account, r: &RawMessage) -> Result<()> {
    let p = mime::parse(&r.raw).map_err(|e| GmailError::Shape(format!("message {}: {e}", r.meta.id)))?;
    let labels = &r.meta.labels;
    let m = build::message(
        &p,
        SourceFacts {
            account: account.id.clone(),
            source_id: r.meta.id.clone(),
            thread_id: r.meta.thread_id.clone(),
            location: location(labels),
            in_inbox: labels.iter().any(|l| l == "INBOX"),
            read: !labels.iter().any(|l| l == "UNREAD"),
            flagged: labels.iter().any(|l| l == "STARRED"),
            size: r.meta.size.max(r.raw.len() as u64),
            received: r.meta.internal_date,
        },
    );
    let raw = if r.raw.len() <= MAX_STORED_RAW {
        Raw::Bytes(&r.raw)
    } else {
        Raw::None
    };
    db.upsert(&m, raw, TEXT_VERSION)?;
    Ok(())
}

/// Mirror one Gmail account: incremental if possible, else full.
pub fn sync(db: &mut Db, c: &Client<'_>, account: &Account, now: i64) -> Result<Report> {
    let mut state: State = db
        .sync_state(&account.id)?
        .and_then(|(s, _)| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    state.labels = c.labels()?;
    let report = match state.history_id.clone() {
        Some(h) => match incremental(db, c, account, &h) {
            Ok((r, new_h)) => {
                state.history_id = Some(new_h);
                r
            }
            Err(e) if e.is_not_found() => {
                tracing::info!(account = %account.id, "gmail history expired; full resync");
                let (r, h) = full(db, c, account)?;
                state.history_id = Some(h);
                r
            }
            Err(e) => return Err(e),
        },
        None => {
            let (r, h) = full(db, c, account)?;
            state.history_id = Some(h);
            r
        }
    };
    let s = serde_json::to_string(&state).map_err(|e| GmailError::Shape(e.to_string()))?;
    db.set_sync_state(&account.id, &s, now)?;
    Ok(report)
}

fn full(db: &mut Db, c: &Client<'_>, account: &Account) -> Result<(Report, String)> {
    // The history id is taken first so nothing between list and fetch is lost.
    let history_id = c.profile_history_id()?;
    let ids = c.list_all()?;
    let current = db.fingerprints(&account.id)?;
    let mut report = Report {
        full: true,
        ..Report::default()
    };
    let wanted: HashSet<&str> = ids.iter().map(|(id, _)| id.as_str()).collect();
    for chunk in ids.chunks(crate::ingest::BATCH) {
        let mut fetched = Vec::new();
        for (id, _) in chunk {
            match current.get(id) {
                Some(fp) if fp.text_version == TEXT_VERSION => {
                    let m = c.get_meta(id)?;
                    let loc = location(&m.labels);
                    if hidden(&m.labels) {
                        fetched.push(Fetched::Remove(id.clone()));
                    } else if fp.location != loc {
                        fetched.push(Fetched::State(m));
                    }
                }
                _ => {
                    let r = c.get_raw(id)?;
                    if hidden(&r.meta.labels) {
                        continue;
                    }
                    fetched.push(Fetched::New(Box::new(r), current.contains_key(id)));
                }
            }
        }
        apply(db, account, fetched, &mut report)?;
    }
    let gone: Vec<String> = current
        .keys()
        .filter(|k| !wanted.contains(k.as_str()))
        .cloned()
        .collect();
    apply(
        db,
        account,
        gone.into_iter().map(Fetched::Remove).collect(),
        &mut report,
    )?;
    Ok((report, history_id))
}

fn incremental(db: &mut Db, c: &Client<'_>, account: &Account, start: &str) -> Result<(Report, String)> {
    let h = c.history(start)?;
    let current = db.fingerprints(&account.id)?;
    let mut report = Report::default();
    let mut work = Vec::new();
    for id in &h.deleted {
        work.push(Fetched::Remove(id.clone()));
    }
    let mut touched: Vec<&String> = h.touched.iter().filter(|id| !h.deleted.contains(*id)).collect();
    touched.sort();
    for id in touched {
        let known = current.get(id).filter(|fp| fp.text_version == TEXT_VERSION);
        let res = match known {
            Some(_) => c.get_meta(id).map(|m| {
                if hidden(&m.labels) {
                    Fetched::Remove(id.clone())
                } else {
                    Fetched::State(m)
                }
            }),
            None => c.get_raw(id).map(|r| {
                if hidden(&r.meta.labels) {
                    Fetched::Remove(id.clone())
                } else {
                    Fetched::New(Box::new(r), current.contains_key(id))
                }
            }),
        };
        match res {
            Ok(f) => work.push(f),
            // Deleted between history and fetch.
            Err(e) if e.is_not_found() => work.push(Fetched::Remove(id.clone())),
            Err(e) => return Err(e),
        }
    }
    apply(db, account, work, &mut report)?;
    let new_h = h.history_id.unwrap_or_else(|| start.to_string());
    Ok((report, new_h))
}

enum Fetched {
    New(Box<RawMessage>, bool),
    State(Meta),
    Remove(String),
}

fn apply(db: &mut Db, account: &Account, work: Vec<Fetched>, report: &mut Report) -> Result<()> {
    if work.is_empty() {
        return Ok(());
    }
    let mut err = None;
    let res = db.transaction(|db| {
        for w in &work {
            match w {
                Fetched::New(r, existed) => {
                    if let Err(e) = store_raw(db, account, r) {
                        err = Some(e);
                        return Err(DbError::Aborted);
                    }
                    if *existed {
                        report.updated += 1;
                    } else {
                        report.added += 1;
                    }
                }
                Fetched::State(m) => {
                    let l = &m.labels;
                    if db.update_state(
                        &account.id,
                        &m.id,
                        &location(l),
                        l.iter().any(|x| x == "INBOX"),
                        !l.iter().any(|x| x == "UNREAD"),
                        l.iter().any(|x| x == "STARRED"),
                    )? {
                        report.updated += 1;
                    }
                }
                Fetched::Remove(id) => {
                    if db.delete(&account.id, id)? {
                        report.removed += 1;
                    }
                }
            }
        }
        Ok(())
    });
    match (res, err) {
        (_, Some(e)) => Err(e),
        (Err(e), None) => Err(e.into()),
        (Ok(()), None) => Ok(()),
    }
}

// ---------- OAuth loopback flow (used by `keryx auth gmail`) ----------

/// PKCE verifier and S256 challenge.
pub fn pkce(random: &[u8; 32]) -> (String, String) {
    use sha2::Digest;
    let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random);
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

/// The consent URL for the loopback flow.
pub fn auth_url(client: &ClientSecret, redirect: &str, challenge: &str, state: &str) -> String {
    let q = [
        ("client_id", client.client_id.as_str()),
        ("redirect_uri", redirect),
        ("response_type", "code"),
        ("scope", SCOPE),
        ("access_type", "offline"),
        ("prompt", "consent"),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
        ("state", state),
    ];
    let qs: Vec<String> = q.iter().map(|(k, v)| format!("{k}={}", urlencode(v))).collect();
    format!("{AUTH_URL}?{}", qs.join("&"))
}

pub fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Pull `code` out of the redirect's request line, checking `state`.
pub fn code_from_request_line(line: &str, state: &str) -> std::result::Result<String, String> {
    let target = line.split_whitespace().nth(1).ok_or("not an HTTP request")?;
    let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
    let mut code = None;
    let mut got_state = None;
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        match k {
            "code" => code = Some(v.to_string()),
            "state" => got_state = Some(v.to_string()),
            "error" => return Err(format!("Google said: {v}")),
            _ => {}
        }
    }
    if got_state.as_deref() != Some(state) {
        return Err("state mismatch: ignoring this response".into());
    }
    code.map(|c| percent_decode(&c))
        .ok_or_else(|| "no code in the redirect".into())
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = ((b[i + 1] as char).to_digit(16), (b[i + 2] as char).to_digit(16))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(if b[i] == b'+' { b' ' } else { b[i] });
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Exchange the authorisation code for tokens; returns the refresh token.
pub fn exchange_code(
    t: &dyn Transport,
    client: &ClientSecret,
    code: &str,
    verifier: &str,
    redirect: &str,
) -> Result<String> {
    let v = t.post_form(
        TOKEN_URL,
        &[
            ("client_id", &client.client_id),
            ("client_secret", &client.client_secret),
            ("code", code),
            ("code_verifier", verifier),
            ("grant_type", "authorization_code"),
            ("redirect_uri", redirect),
        ],
    )?;
    if let Some(scope) = v.get("scope").and_then(Value::as_str)
        && scope.split_whitespace().any(|s| s == "https://mail.google.com/")
    {
        return Err(GmailError::Shape(
            "Google granted the full mail.google.com scope; keryx only accepts gmail.modify".into(),
        ));
    }
    v.get("refresh_token")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| GmailError::Shape("no refresh_token (was prompt=consent dropped?)".into()))
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::path::PathBuf;

    pub use crate::testkit::Fixtures;

    fn account() -> Account {
        let cfg = keryx_core::config::Config::load(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config/config.test.toml"),
        )
        .unwrap();
        cfg.account("personal").unwrap().clone()
    }

    #[test]
    fn full_sync() {
        let f = Fixtures::new();
        let c = Client {
            t: &f,
            token: "tok".into(),
        };
        let mut db = Db::open_in_memory().unwrap();
        let r = sync(&mut db, &c, &account(), 100).unwrap();
        assert!(r.full);
        assert_eq!(r.added, 5);
        let m = db.message("personal", "m01").unwrap().unwrap();
        assert_eq!(m.subject, "Café on Sunday");
        assert!(m.in_inbox && !m.read && !m.flagged);
        let m5 = db.message("personal", "m05").unwrap().unwrap();
        assert!(m5.flagged, "STARRED = flagged");
        let m4 = db.message("personal", "m04").unwrap().unwrap();
        assert!(!m4.in_inbox, "Admin & Receipts label only: archived");
        assert!(db.raw("personal", "m01").unwrap().0.is_some());
        let (state, at) = db.sync_state("personal").unwrap().unwrap();
        let state: State = serde_json::from_str(&state).unwrap();
        assert_eq!(state.history_id.as_deref(), Some("1200"));
        assert_eq!(state.labels["Label_1"], "Admin & Receipts");
        assert_eq!(at, 100);
    }

    #[test]
    fn incremental_sync_follows_history() {
        let f = Fixtures::new();
        let c = Client {
            t: &f,
            token: "tok".into(),
        };
        let mut db = Db::open_in_memory().unwrap();
        sync(&mut db, &c, &account(), 100).unwrap();
        let r = sync(&mut db, &c, &account(), 200).unwrap();
        assert!(!r.full);
        // m06 added (reply in thread t01), m02 archived, m03 read, m04 deleted.
        assert_eq!((r.added, r.updated, r.removed), (1, 2, 1), "{r:?}");
        let m6 = db.message("personal", "m06").unwrap().unwrap();
        assert_eq!(m6.thread_id, "t01");
        assert!(!db.message("personal", "m02").unwrap().unwrap().in_inbox);
        assert!(db.message("personal", "m03").unwrap().unwrap().read);
        assert!(db.message("personal", "m04").unwrap().is_none());
        let (state, _) = db.sync_state("personal").unwrap().unwrap();
        assert!(state.contains("\"history_id\":\"1204\""));
    }

    #[test]
    fn expired_history_triggers_full_resync() {
        let mut f = Fixtures::new();
        let mut db = Db::open_in_memory().unwrap();
        {
            let c = Client {
                t: &f,
                token: "tok".into(),
            };
            sync(&mut db, &c, &account(), 100).unwrap();
        }
        f.history_expired = true;
        let c = Client {
            t: &f,
            token: "tok".into(),
        };
        let r = sync(&mut db, &c, &account(), 200).unwrap();
        assert!(r.full);
        assert_eq!(db.count(Some("personal")).unwrap(), 5);
        // Already-mirrored messages are checked with format=minimal, not re-downloaded.
        let raw_fetches = f.calls.borrow().iter().filter(|u| u.contains("format=raw")).count();
        assert_eq!(raw_fetches, 5, "only the first full sync downloads raw");
    }

    #[test]
    fn tokens_and_pkce() {
        let f = Fixtures::new();
        let cs = ClientSecret::from_json(
            r#"{"installed":{"client_id":"id","client_secret":"sec","redirect_uris":["http://localhost"]}}"#,
        )
        .unwrap();
        assert_eq!(cs.client_id, "id");
        assert_eq!(access_token(&f, &cs, "ref").unwrap(), "tok");
        assert_eq!(exchange_code(&f, &cs, "c", "v", "http://127.0.0.1:1").unwrap(), "ref");
        let (v, c) = pkce(&[7u8; 32]);
        assert_eq!(v.len(), 43);
        assert_ne!(v, c);
        let url = auth_url(&cs, "http://127.0.0.1:5555", &c, "st8");
        assert!(url.contains("scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fgmail.modify"));
        assert!(url.contains("access_type=offline") && url.contains("code_challenge_method=S256"));
        assert!(!url.contains("mail.google.com"));
    }

    #[test]
    fn redirect_parsing() {
        assert_eq!(
            code_from_request_line("GET /?state=s1&code=4%2F0Abc HTTP/1.1", "s1").unwrap(),
            "4/0Abc"
        );
        assert!(code_from_request_line("GET /?state=other&code=x HTTP/1.1", "s1").is_err());
        assert!(code_from_request_line("GET /?error=access_denied&state=s1 HTTP/1.1", "s1").is_err());
        assert!(code_from_request_line("GET /favicon.ico HTTP/1.1", "s1").is_err());
    }

    #[test]
    fn hidden_labels() {
        assert!(hidden(&["TRASH".into()]));
        assert!(hidden(&["INBOX".into(), "SPAM".into()]));
        assert!(!hidden(&["INBOX".into(), "UNREAD".into()]));
    }
}
