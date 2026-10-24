#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Europe::London;
use kairos_core::changeset::Changeset;
use kairos_core::config::Config;
use kairos_core::constraints::Constraints;
use kairosd::engine::{Engine, Notifier, SyncTarget, Syncer, collection_cmd};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub fn at(s: &str) -> DateTime<Utc> {
    let n = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap();
    London.from_local_datetime(&n).earliest().unwrap().with_timezone(&Utc)
}

#[derive(Clone, Default)]
pub struct Recorder {
    pub synced: Arc<Mutex<Vec<String>>>,
    pub pending: Arc<Mutex<Vec<String>>>,
}

impl Syncer for Recorder {
    fn sync_collections(&self, c: &[SyncTarget]) {
        self.synced
            .lock()
            .unwrap()
            .extend(c.iter().map(|t| collection_cmd("{collection}", t)));
    }
}
impl Notifier for Recorder {
    fn pending(&self, cs: &Changeset) {
        self.pending.lock().unwrap().push(cs.id.clone());
    }
}

pub struct Env {
    pub dir: tempfile::TempDir,
    pub now: Arc<Mutex<DateTime<Utc>>>,
    pub rec: Recorder,
}

pub const COLLECTIONS: &[(&str, &str)] = &[
    ("p-1111", "Personal"),
    ("s-2222", "Sport"),
    ("e-3333", "Engineering"),
    ("f-4444", "Family"),
    ("t-5555", "ZZ-kairos-test"),
];

impl Env {
    pub fn new(live: bool) -> Env {
        let dir = tempfile::tempdir().unwrap();
        for (d, name) in COLLECTIONS {
            let p = dir.path().join("vdir").join(d);
            fs::create_dir_all(&p).unwrap();
            fs::write(p.join("displayname"), name).unwrap();
        }
        let env = Env {
            dir,
            now: Arc::new(Mutex::new(at("2026-10-12 08:00"))),
            rec: Recorder::default(),
        };
        fs::write(env.config_path(), env.config_toml(live)).unwrap();
        env
    }

    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    pub fn config_path(&self) -> PathBuf {
        self.root().join("config.toml")
    }

    pub fn config_toml(&self, live: bool) -> String {
        format!(
            r#"
timezone = "Europe/London"
live = {live}
test_calendar = "ZZ-kairos-test"
[paths]
vdir = "{root}/vdir"
data = "{root}/data"
cache = "{root}/cache"
[sync]
collection_cmd = ""
full_cmd = ""
[[keys]]
name = "Personal"
index = 1
colour = "lavender"
calendar = "personal"
[[keys]]
name = "Sport"
index = 2
colour = "green"
calendar = "sport"
[[keys]]
name = "Engineering"
index = 3
colour = "blue"
calendar = "engineering"
[[sources]]
id = "personal"
kind = "icloud"
calendar = "Personal"
overlay = "personal"
key = "Personal"
writable = true
[[sources]]
id = "sport"
kind = "icloud"
calendar = "Sport"
overlay = "personal"
key = "Sport"
writable = true
[[sources]]
id = "engineering"
kind = "icloud"
calendar = "Engineering"
overlay = "personal"
key = "Engineering"
writable = true
[[sources]]
id = "family"
kind = "icloud"
calendar = "Family"
overlay = "family"
writable = true
[[sources]]
id = "test"
kind = "icloud"
calendar = "ZZ-kairos-test"
overlay = "personal"
key = "Personal"
writable = true
[[sources]]
id = "gmail"
kind = "ics_readonly"
url = "http://127.0.0.1:9/never.ics"
overlay = "imported"
"#,
            root = self.root().display()
        )
    }

    pub fn engine(&self) -> Engine {
        let cfg = Config::load(&self.config_path()).unwrap();
        assert_eq!(cfg.validate(), vec![]);
        let cons = Constraints::from_toml(include_str!("../../../../config/constraints.example.toml")).unwrap();
        let db = kairos_store::db::Db::open(&self.root().join("data/kairos.db")).unwrap();
        let now = self.now.clone();
        Engine {
            cfg,
            cfg_path: Some(self.config_path()),
            constraints: cons,
            db,
            clock: Arc::new(move || *now.lock().unwrap()),
            syncer: Box::new(self.rec.clone()),
            notifier: Box::new(self.rec.clone()),
            direct: false,
            replan_soon: false,
            registry: None,
        }
    }

    pub fn set_now(&self, s: &str) {
        *self.now.lock().unwrap() = at(s);
    }

    pub fn coll(&self, name: &str) -> PathBuf {
        let d = COLLECTIONS.iter().find(|(_, n)| *n == name).unwrap().0;
        self.root().join("vdir").join(d)
    }

    /// An event as if Alec had created it on his phone (no X-KAIROS-*).
    pub fn phone_event(&self, cal: &str, uid: &str, title: &str, start: &str, end: &str) {
        let f = |s: &str| at(s).format("%Y%m%dT%H%M%SZ").to_string();
        let text = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Apple Inc.//iPhone OS 26//EN\r\nBEGIN:VEVENT\r\nUID:{uid}\r\nDTSTAMP:20261001T000000Z\r\nDTSTART:{}\r\nDTEND:{}\r\nSUMMARY:{title}\r\nX-APPLE-TRAVEL-ADVISORY-BEHAVIOR:AUTOMATIC\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
            f(start),
            f(end)
        );
        fs::write(self.coll(cal).join(format!("{uid}.ics")), text).unwrap();
    }

    /// A ⚑ deadline event as kairos wrote them before due dates became task attributes (K5).
    pub fn legacy_deadline(&self, cal: &str, uid: &str, title: &str, due: &str, task_id: &str) {
        let f = |s: &str| at(s).format("%Y%m%dT%H%M%SZ").to_string();
        let text = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//kairos//EN\r\nBEGIN:VEVENT\r\nUID:{uid}\r\nDTSTAMP:20261001T000000Z\r\nDTSTART:{0}\r\nDTEND:{0}\r\nSUMMARY:⚑ {title}\r\nTRANSP:TRANSPARENT\r\nX-KAIROS-KIND:deadline\r\nX-KAIROS-CREATED-BY:claude\r\nX-KAIROS-TASK-ID:{task_id}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
            f(due)
        );
        fs::write(self.coll(cal).join(format!("{uid}.ics")), text).unwrap();
    }

    pub fn feed(&self, text: &str) {
        let d = self.root().join("data/feeds");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("gmail.ics"), text).unwrap();
    }

    pub fn items(&self, cal: &str) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(self.coll(cal))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".ics"))
            .collect();
        v.sort();
        v
    }

    pub fn read_all(&self, cal: &str) -> String {
        self.items(cal)
            .iter()
            .map(|n| fs::read_to_string(self.coll(cal).join(n)).unwrap())
            .collect()
    }
}
