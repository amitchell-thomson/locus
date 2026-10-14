//! Test support shared by the store, TUI and CLI tests: a mirror built from
//! `fixtures/` (the synthetic Mail store + recorded Gmail responses).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::db::Db;
use crate::gmail::{API, GmailError, Result, SCOPE, TOKEN_URL, Transport};
use crate::ingest;
use crate::mailapp::MailStore;
use keryx_core::config::Config;
use serde_json::Value;
use std::cell::RefCell;
use std::path::PathBuf;

/// The repository root (for fixtures and the example config).
pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The example config with every account mirrored from fixtures.
pub fn fixture_db() -> (Config, Db) {
    let cfg = Config::load(&root().join("config/config.example.toml")).unwrap();
    let store = MailStore::open(&root().join("fixtures/mailapp/V10")).unwrap();
    let mut db = Db::open_in_memory().unwrap();
    for id in ["oxford", "oqts", "oqts-shared"] {
        ingest::mailapp_account(&mut db, &store, cfg.account(id).unwrap(), None).unwrap();
    }
    let f = Fixtures::new();
    let c = crate::gmail::Client {
        t: &f,
        token: "tok".into(),
    };
    crate::gmail::sync(&mut db, &c, cfg.account("personal").unwrap(), 0).unwrap();
    (cfg, db)
}

/// Write the fixture mirror to a file (for read-only opens and CLI tests).
pub fn fixture_db_file(dir: &std::path::Path) -> (Config, PathBuf) {
    let (mut cfg, mem) = fixture_db();
    cfg.paths.data_dir = dir.to_path_buf();
    let path = cfg.db_path();
    std::fs::create_dir_all(dir).unwrap();
    mem.conn().execute("VACUUM INTO ?1", [path.to_string_lossy()]).unwrap();
    (cfg, path)
}

/// Serves `fixtures/gmail`, with overrides for specific URLs.
pub struct Fixtures {
    pub dir: PathBuf,
    pub history_expired: bool,
    pub calls: RefCell<Vec<String>>,
}

impl Default for Fixtures {
    fn default() -> Self {
        Self::new()
    }
}

impl Fixtures {
    pub fn new() -> Fixtures {
        Fixtures {
            dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/gmail"),
            history_expired: false,
            calls: RefCell::new(Vec::new()),
        }
    }
    fn file(&self, name: &str) -> Result<Value> {
        let s = std::fs::read_to_string(self.dir.join(name)).map_err(|_| GmailError::Http {
            status: 404,
            body: name.into(),
        })?;
        Ok(serde_json::from_str(&s).unwrap())
    }
}

impl Transport for Fixtures {
    fn get(&self, url: &str, bearer: &str) -> Result<Value> {
        assert_eq!(bearer, "tok");
        self.calls.borrow_mut().push(url.to_string());
        let path = url.strip_prefix(API).unwrap();
        if path == "/profile" {
            return self.file("profile.json");
        }
        if path == "/labels" {
            return self.file("labels.json");
        }
        if path.starts_with("/messages?") {
            return self.file(if path.contains("pageToken=page2") {
                "messages.list.page2.json"
            } else {
                "messages.list.page1.json"
            });
        }
        if let Some(rest) = path.strip_prefix("/messages/") {
            let (id, q) = rest.split_once('?').unwrap();
            let mut v = self.file(&format!("message.{id}.json"))?;
            if q == "format=minimal" {
                v.as_object_mut().unwrap().remove("raw");
                // Simulate label changes from history.json.
                if id == "m02" {
                    v["labelIds"] = serde_json::json!(["CATEGORY_UPDATES"]);
                }
                if id == "m03" {
                    v["labelIds"] = serde_json::json!(["INBOX", "CATEGORY_UPDATES"]);
                }
            }
            return Ok(v);
        }
        if path.starts_with("/history?") {
            if self.history_expired {
                return Err(GmailError::Http {
                    status: 404,
                    body: "expired".into(),
                });
            }
            return self.file("history.json");
        }
        panic!("unexpected GET {url}");
    }
    fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<Value> {
        assert_eq!(url, TOKEN_URL);
        let grant = form.iter().find(|(k, _)| *k == "grant_type").unwrap().1;
        Ok(match grant {
            "refresh_token" => serde_json::json!({"access_token": "tok", "expires_in": 3599}),
            _ => serde_json::json!({"access_token": "tok", "refresh_token": "ref", "scope": SCOPE}),
        })
    }
    fn post_json(&self, _: &str, _: &str, _: &Value) -> Result<Value> {
        unimplemented!()
    }
}
