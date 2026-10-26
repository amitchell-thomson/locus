//! `config.toml` schema and validation (SPEC §10). Nothing about Alec's
//! accounts is hard-coded: accounts, keys, thresholds and times all come from
//! here. `validate` returns every problem, each as a named error.
//!
//! Copied in spirit from `kairos-core::config` (same load/validate shape).

use crate::colour::Rgb;
use chrono::NaiveTime;
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// Where an account's mail comes from and how keryx acts on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    /// Gmail API (OAuth, `gmail.modify`).
    Gmail,
    /// Mac Mail: read from `~/Library/Mail`, act through AppleScript.
    Mailapp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Account {
    /// Short lowercase id used in refs (`oxford:1234`), config and the CLI.
    pub id: String,
    /// Shown in the TUI (`Tab` filter, account tags).
    pub name: String,
    pub address: String,
    /// Other addresses that reach this account (count as Alec's for reply-all).
    #[serde(default)]
    pub aliases: Vec<String>,
    /// The address shown for this account on the `From:` line, when it is not the one Mail
    /// sends with (Oxford: your college address shown, your SSO address sent; your call of
    /// 26 Oct). Sending is unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shown_address: Option<String>,
    pub backend: Backend,
    /// Mailapp: Mail's account UUID (`keryx doctor --discover` lists them).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mailapp_account_id: Option<String>,
    /// Mailapp: mailbox names (path below the account), e.g. `Inbox`, `Archive`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inbox_mailbox: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_mailbox: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_mailbox: Option<String>,
    #[serde(default = "yes")]
    pub writable: bool,
    /// Used by other people too: every Claude write needs Alec (SPEC §6.2).
    #[serde(default)]
    pub shared: bool,
    pub colour: Rgb,
    /// Plain text added to replies and drafts from this account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

impl Account {
    /// Every address that is Alec's on this account, lowercased.
    pub fn addresses(&self) -> impl Iterator<Item = String> + '_ {
        std::iter::once(&self.address)
            .chain(self.aliases.iter())
            .chain(self.shown_address.iter())
            .map(|a| a.trim().to_lowercase())
    }
}

/// A shared filing destination across accounts ("Admin is Admin").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyDef {
    pub name: String,
    pub colour: Rgb,
    /// Default kairos calendar for tasks made from mail filed here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kairos_key: Option<String>,
    /// Account id → label (Gmail) or mailbox (Mail.app).
    pub folders: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Risk {
    /// Claude's automated-only triage batches above this need Alec.
    #[serde(default = "d_batch")]
    pub batch_threshold: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Send {
    #[serde(default = "d_hold")]
    pub hold_seconds: u32,
    #[serde(default = "d_attach_mb")]
    pub max_attachments_mb: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snooze {
    /// Off (Alec, 2026-10-20): no snoozing for now. Wake-ups only run on
    /// the Mac, so snoozed mail could stay hidden on the phone.
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "d_later")]
    pub later_today_hours: u32,
    #[serde(default = "d_morning")]
    pub morning: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gmail {
    #[serde(default = "d_poll")]
    pub poll_seconds: u32,
}

/// Overrides for automated-mail detection, by address or `@domain`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Automated {
    #[serde(default)]
    pub always: Vec<String>,
    #[serde(default)]
    pub never: Vec<String>,
    /// File automated mail for what it is for as it arrives (keryx-joins-locus X-d, your call
    /// of 26 Oct: "Automated mail filed away straight away"). Only mail that arrives after
    /// keryxd first runs with this on; never a person's mail; logged and undoable.
    #[serde(default = "yes")]
    pub file_on_arrival: bool,
}

impl Default for Automated {
    fn default() -> Self {
        Automated {
            always: vec![],
            never: vec![],
            file_on_arrival: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cleanup {
    /// "New" = arrived within this many days.
    #[serde(default = "d_new_days")]
    pub new_days: u32,
    /// "Needed soon" = within this many days; later → snooze.
    #[serde(default = "d_soon_days")]
    pub soon_days: u32,
    /// Free-text habits for Claude ("keep JCR Treasurer mail").
    #[serde(default)]
    pub notes: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Runs {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "d_run_morning")]
    pub morning: String,
    #[serde(default = "d_run_evening")]
    pub evening: String,
    #[serde(default = "d_max_threads")]
    pub max_threads: u32,
    #[serde(default = "d_timeout")]
    pub timeout_minutes: u32,
    #[serde(default = "d_claude")]
    pub claude_bin: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Kairos {
    #[serde(default = "d_kairos_url")]
    pub url: String,
    /// Keychain service holding kairos's bearer token.
    #[serde(default = "d_kairos_token")]
    pub token_service: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Notify {
    /// Read and ignored: the ntfy pushes and the phone pages were removed on 26 Oct 2026 (plan
    /// B.4, one approve place); kept so a config written before still loads.
    #[serde(default = "d_ntfy")]
    pub ntfy_server: String,
    /// Read and ignored, as `ntfy_server`.
    #[serde(default)]
    pub approvals: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Server {
    /// The Mac's Tailscale IP (keryxd binds only this).
    #[serde(default = "d_bind")]
    pub bind: String,
    #[serde(default = "d_port")]
    pub port: u16,
    /// Base URL clients use to reach keryxd; default `http://<bind>:<port>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Paths {
    #[serde(default = "d_data_dir")]
    pub data_dir: PathBuf,
    #[serde(default = "d_mail_dir")]
    pub mail_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// `false`: writes only to `[keryx-test]` messages, sends only to Alec.
    #[serde(default)]
    pub live: bool,
    #[serde(default = "d_tz")]
    pub timezone: String,
    pub accounts: Vec<Account>,
    #[serde(default)]
    pub keys: Vec<KeyDef>,
    #[serde(default = "Risk::default_")]
    pub risk: Risk,
    #[serde(default = "Send::default_")]
    pub send: Send,
    #[serde(default = "Snooze::default_")]
    pub snooze: Snooze,
    #[serde(default = "Gmail::default_")]
    pub gmail: Gmail,
    #[serde(default)]
    pub automated: Automated,
    #[serde(default = "Cleanup::default_")]
    pub cleanup: Cleanup,
    #[serde(default = "Runs::default_")]
    pub runs: Runs,
    #[serde(default = "Kairos::default_")]
    pub kairos: Kairos,
    #[serde(default = "Notify::default_")]
    pub notify: Notify,
    #[serde(default = "Server::default_")]
    pub server: Server,
    #[serde(default = "Paths::default_")]
    pub paths: Paths,
}

fn yes() -> bool {
    true
}
fn d_batch() -> u32 {
    100
}
fn d_hold() -> u32 {
    10
}
fn d_attach_mb() -> u32 {
    25
}
fn d_later() -> u32 {
    3
}
fn d_morning() -> String {
    "08:00".into()
}
fn d_poll() -> u32 {
    60
}
fn d_new_days() -> u32 {
    3
}
fn d_soon_days() -> u32 {
    7
}
fn d_run_morning() -> String {
    "08:00".into()
}
fn d_run_evening() -> String {
    "18:00".into()
}
fn d_max_threads() -> u32 {
    200
}
fn d_timeout() -> u32 {
    15
}
fn d_claude() -> String {
    "claude".into()
}
fn d_kairos_url() -> String {
    "http://compute-node:7420".into()
}
fn d_kairos_token() -> String {
    "kairos-api-token".into()
}
fn d_ntfy() -> String {
    "https://ntfy.sh".into()
}
fn d_bind() -> String {
    "127.0.0.1".into()
}
fn d_port() -> u16 {
    7421
}
fn d_tz() -> String {
    "Europe/London".into()
}
fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}
fn d_data_dir() -> PathBuf {
    home().join(".local/share/keryx")
}
fn d_mail_dir() -> PathBuf {
    home().join("Library/Mail/V10")
}

impl Risk {
    fn default_() -> Self {
        Risk {
            batch_threshold: d_batch(),
        }
    }
}
impl Send {
    fn default_() -> Self {
        Send {
            hold_seconds: d_hold(),
            max_attachments_mb: d_attach_mb(),
        }
    }
}
impl Snooze {
    fn default_() -> Self {
        Snooze {
            enabled: false,
            later_today_hours: d_later(),
            morning: d_morning(),
        }
    }
}
impl Gmail {
    fn default_() -> Self {
        Gmail { poll_seconds: d_poll() }
    }
}
impl Cleanup {
    fn default_() -> Self {
        Cleanup {
            new_days: d_new_days(),
            soon_days: d_soon_days(),
            notes: String::new(),
        }
    }
}
impl Runs {
    fn default_() -> Self {
        Runs {
            enabled: true,
            morning: d_run_morning(),
            evening: d_run_evening(),
            max_threads: d_max_threads(),
            timeout_minutes: d_timeout(),
            claude_bin: d_claude(),
        }
    }
}
impl Kairos {
    fn default_() -> Self {
        Kairos {
            url: d_kairos_url(),
            token_service: d_kairos_token(),
        }
    }
}
impl Notify {
    fn default_() -> Self {
        Notify {
            ntfy_server: d_ntfy(),
            approvals: false,
        }
    }
}
impl Server {
    fn default_() -> Self {
        Server {
            bind: d_bind(),
            port: d_port(),
            url: None,
        }
    }
}
impl Paths {
    fn default_() -> Self {
        Paths {
            data_dir: d_data_dir(),
            mail_dir: d_mail_dir(),
        }
    }
}

/// Keys are numbered 1–9 in the TUI.
pub const MAX_KEYS: usize = 9;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("can't read {path}: {msg}")]
    Read { path: String, msg: String },
    #[error("config isn't valid TOML: {0}")]
    Toml(String),
    #[error("unknown timezone {0:?}")]
    Timezone(String),
    #[error("no accounts configured")]
    NoAccounts,
    #[error("account id {0:?}: use lowercase letters, digits and '-' only")]
    AccountId(String),
    #[error("duplicate account id {0:?}")]
    DuplicateAccount(String),
    #[error("{account:?}: {address:?} isn't an email address")]
    BadAddress { account: String, address: String },
    #[error("address {address:?} is configured on more than one account")]
    DuplicateAddress { address: String },
    #[error("account {0:?}: mailapp accounts need mailapp_account_id, inbox_mailbox, archive_mailbox and sent_mailbox")]
    MailappIncomplete(String),
    #[error("account {0:?}: gmail accounts don't take mailapp_* or *_mailbox settings")]
    GmailHasMailapp(String),
    #[error("duplicate mailapp_account_id {0:?}")]
    DuplicateMailappId(String),
    #[error("more than {MAX_KEYS} keys ({0}); keys are numbered 1–{MAX_KEYS}")]
    TooManyKeys(usize),
    #[error("duplicate key name {0:?}")]
    DuplicateKey(String),
    #[error("key {key:?} maps unknown account {account:?}")]
    KeyUnknownAccount { key: String, account: String },
    #[error("key {0:?} has no folders: map it to a label/mailbox on at least one account")]
    KeyNoFolders(String),
    #[error("{field}: {value:?} isn't a time like 08:00")]
    BadTime { field: &'static str, value: String },
    #[error("{field} must be between {min} and {max} (got {value})")]
    OutOfRange {
        field: &'static str,
        value: u64,
        min: u64,
        max: u64,
    },
    #[error("{field}: {value:?} isn't an http(s) URL")]
    BadUrl { field: &'static str, value: String },
    #[error("automated.{list}: {value:?} must be an address or @domain")]
    BadAutomatedEntry { list: &'static str, value: String },
}

/// A loose but useful address check: one `@`, something either side, a dot
/// in the domain, no whitespace or angle brackets.
pub fn is_address(s: &str) -> bool {
    let s = s.trim();
    let Some((local, domain)) = s.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains('@')
        && !s.chars().any(|c| c.is_whitespace() || c == '<' || c == '>' || c == ',')
}

/// `HH:MM` (24 h).
pub fn parse_hhmm(s: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(s.trim(), "%H:%M").ok()
}

impl Config {
    pub fn from_toml(s: &str) -> Result<Config, ConfigError> {
        toml::from_str(s).map_err(|e| ConfigError::Toml(e.to_string()))
    }

    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        let s = std::fs::read_to_string(path).map_err(|e| ConfigError::Read {
            path: path.display().to_string(),
            msg: e.to_string(),
        })?;
        Config::from_toml(&s)
    }

    /// `$KERYX_CONFIG`, else `~/.config/keryx/config.toml`.
    pub fn default_path() -> PathBuf {
        std::env::var_os("KERYX_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".config/keryx/config.toml"))
    }

    /// All problems, not just the first.
    pub fn validate(&self) -> Vec<ConfigError> {
        let mut errs = Vec::new();
        if self.timezone.parse::<Tz>().is_err() {
            errs.push(ConfigError::Timezone(self.timezone.clone()));
        }
        self.validate_accounts(&mut errs);
        self.validate_keys(&mut errs);

        let times = [
            ("snooze.morning", &self.snooze.morning),
            ("runs.morning", &self.runs.morning),
            ("runs.evening", &self.runs.evening),
        ];
        for (field, value) in times {
            if parse_hhmm(value).is_none() {
                errs.push(ConfigError::BadTime {
                    field,
                    value: value.clone(),
                });
            }
        }
        let ranges: [(&'static str, u64, u64, u64); 10] = [
            ("risk.batch_threshold", self.risk.batch_threshold.into(), 1, 1000),
            ("send.hold_seconds", self.send.hold_seconds.into(), 1, 60),
            ("send.max_attachments_mb", self.send.max_attachments_mb.into(), 1, 25),
            ("snooze.later_today_hours", self.snooze.later_today_hours.into(), 1, 12),
            ("gmail.poll_seconds", self.gmail.poll_seconds.into(), 15, 3600),
            ("cleanup.new_days", self.cleanup.new_days.into(), 1, 30),
            ("cleanup.soon_days", self.cleanup.soon_days.into(), 1, 60),
            ("runs.max_threads", self.runs.max_threads.into(), 1, 2000),
            ("runs.timeout_minutes", self.runs.timeout_minutes.into(), 1, 60),
            ("server.port", self.server.port.into(), 1024, 65535),
        ];
        for (field, value, min, max) in ranges {
            if !(min..=max).contains(&value) {
                errs.push(ConfigError::OutOfRange { field, value, min, max });
            }
        }
        let urls = [
            ("kairos.url", &self.kairos.url),
            ("notify.ntfy_server", &self.notify.ntfy_server),
        ];
        for (field, value) in urls {
            if !(value.starts_with("http://") || value.starts_with("https://")) {
                errs.push(ConfigError::BadUrl {
                    field,
                    value: value.clone(),
                });
            }
        }
        for (list, entries) in [("always", &self.automated.always), ("never", &self.automated.never)] {
            for e in entries {
                let ok = match e.strip_prefix('@') {
                    Some(domain) => domain.contains('.') && !domain.contains('@'),
                    None => is_address(e),
                };
                if !ok {
                    errs.push(ConfigError::BadAutomatedEntry { list, value: e.clone() });
                }
            }
        }
        errs
    }

    fn validate_accounts(&self, errs: &mut Vec<ConfigError>) {
        if self.accounts.is_empty() {
            errs.push(ConfigError::NoAccounts);
        }
        let mut ids = HashSet::new();
        let mut addrs = HashSet::new();
        let mut mail_ids = HashSet::new();
        for a in &self.accounts {
            let id_ok = !a.id.is_empty()
                && a.id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
            if !id_ok {
                errs.push(ConfigError::AccountId(a.id.clone()));
            }
            if !ids.insert(a.id.as_str()) {
                errs.push(ConfigError::DuplicateAccount(a.id.clone()));
            }
            for addr in std::iter::once(&a.address).chain(a.aliases.iter()) {
                if !is_address(addr) {
                    errs.push(ConfigError::BadAddress {
                        account: a.id.clone(),
                        address: addr.clone(),
                    });
                } else if !addrs.insert(addr.trim().to_lowercase()) {
                    errs.push(ConfigError::DuplicateAddress { address: addr.clone() });
                }
            }
            let mailboxes = [&a.inbox_mailbox, &a.archive_mailbox, &a.sent_mailbox];
            match a.backend {
                Backend::Mailapp => {
                    if a.mailapp_account_id.is_none() || mailboxes.iter().any(|m| m.is_none()) {
                        errs.push(ConfigError::MailappIncomplete(a.id.clone()));
                    }
                    if let Some(m) = &a.mailapp_account_id
                        && !mail_ids.insert(m.to_uppercase())
                    {
                        errs.push(ConfigError::DuplicateMailappId(m.clone()));
                    }
                }
                Backend::Gmail => {
                    if a.mailapp_account_id.is_some() || mailboxes.iter().any(|m| m.is_some()) {
                        errs.push(ConfigError::GmailHasMailapp(a.id.clone()));
                    }
                }
            }
        }
    }

    fn validate_keys(&self, errs: &mut Vec<ConfigError>) {
        if self.keys.len() > MAX_KEYS {
            errs.push(ConfigError::TooManyKeys(self.keys.len()));
        }
        let mut names = HashSet::new();
        for k in &self.keys {
            if !names.insert(k.name.to_lowercase()) {
                errs.push(ConfigError::DuplicateKey(k.name.clone()));
            }
            if k.folders.is_empty() {
                errs.push(ConfigError::KeyNoFolders(k.name.clone()));
            }
            for acc in k.folders.keys() {
                if self.account(acc).is_none() {
                    errs.push(ConfigError::KeyUnknownAccount {
                        key: k.name.clone(),
                        account: acc.clone(),
                    });
                }
            }
        }
    }

    pub fn tz(&self) -> Tz {
        self.timezone.parse().unwrap_or(chrono_tz::Europe::London)
    }

    pub fn account(&self, id: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.id == id)
    }

    /// Case-insensitive key lookup.
    pub fn key(&self, name: &str) -> Option<&KeyDef> {
        self.keys.iter().find(|k| k.name.eq_ignore_ascii_case(name))
    }

    /// Every address of Alec's, across all accounts, lowercased.
    pub fn own_addresses(&self) -> HashSet<String> {
        self.accounts.iter().flat_map(|a| a.addresses()).collect()
    }

    /// keryxd's base URL for clients.
    pub fn server_url(&self) -> String {
        self.server
            .url
            .clone()
            .unwrap_or_else(|| format!("http://{}:{}", self.server.bind, self.server.port))
    }

    pub fn db_path(&self) -> PathBuf {
        self.paths.data_dir.join("keryx.db")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = include_str!("../../../config/config.test.toml");

    fn example() -> Config {
        Config::from_toml(EXAMPLE).unwrap()
    }

    fn errs_after(edit: impl FnOnce(&mut Config)) -> Vec<ConfigError> {
        let mut c = example();
        edit(&mut c);
        c.validate()
    }

    #[test]
    fn example_config_is_valid() {
        let c = example();
        assert_eq!(c.validate(), vec![]);
        assert_eq!(c.accounts.len(), 4);
        assert!(!c.live, "the example must default to live = false");
        assert!(c.account("oqts-shared").unwrap().shared);
        assert_eq!(c.key("admin").unwrap().folders.len(), 2);
        assert!(c.own_addresses().contains("newc7370@ox.ac.uk"));
    }

    #[test]
    fn defaults_match_spec() {
        let c = Config::from_toml(
            r##"
            [[accounts]]
            id = "personal"
            name = "Personal"
            address = "a@example.com"
            backend = "gmail"
            colour = "lavender"
            "##,
        )
        .unwrap();
        assert_eq!(c.validate(), vec![]);
        assert!(!c.live);
        assert_eq!(c.risk.batch_threshold, 100);
        assert_eq!(c.send.hold_seconds, 10);
        assert_eq!(c.send.max_attachments_mb, 25);
        assert_eq!(c.cleanup.new_days, 3);
        assert_eq!(c.cleanup.soon_days, 7);
        assert_eq!((c.runs.morning.as_str(), c.runs.evening.as_str()), ("08:00", "18:00"));
        assert_eq!(c.server.port, 7421);
        assert!(c.accounts[0].writable);
        assert!(!c.accounts[0].shared);
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let bad = EXAMPLE.replace("live = false", "live = false\nliev = true");
        assert!(matches!(Config::from_toml(&bad), Err(ConfigError::Toml(_))));
    }

    #[test]
    fn timezone() {
        let e = errs_after(|c| c.timezone = "Europe/Londn".into());
        assert_eq!(e, vec![ConfigError::Timezone("Europe/Londn".into())]);
    }

    #[test]
    fn no_accounts() {
        let e = errs_after(|c| {
            c.accounts.clear();
            c.keys.clear();
        });
        assert_eq!(e, vec![ConfigError::NoAccounts]);
    }

    #[test]
    fn account_ids() {
        let e = errs_after(|c| c.accounts[0].id = "Personal".into());
        assert!(e.contains(&ConfigError::AccountId("Personal".into())), "{e:?}");
        let e = errs_after(|c| c.accounts[1].id = "personal".into());
        assert!(e.contains(&ConfigError::DuplicateAccount("personal".into())), "{e:?}");
    }

    #[test]
    fn addresses() {
        let e = errs_after(|c| c.accounts[0].address = "not an address".into());
        assert!(e.iter().any(|x| matches!(x, ConfigError::BadAddress { .. })), "{e:?}");
        let e = errs_after(|c| {
            let a = c.accounts[0].address.to_uppercase();
            c.accounts[1].aliases.push(a);
        });
        assert!(
            e.iter().any(|x| matches!(x, ConfigError::DuplicateAddress { .. })),
            "{e:?}"
        );
        for bad in [
            "a@b",
            "@b.com",
            "a@.com",
            "a@b.com.",
            "a b@c.com",
            "<a@b.com>",
            "a@b@c.com",
        ] {
            assert!(!is_address(bad), "{bad}");
        }
        for good in ["a@b.com", "newc7370@ox.ac.uk", "first.last+tag@sub.example.org"] {
            assert!(is_address(good), "{good}");
        }
    }

    #[test]
    fn mailapp_accounts_need_their_mailboxes() {
        let e = errs_after(|c| c.accounts[1].archive_mailbox = None);
        assert_eq!(e, vec![ConfigError::MailappIncomplete("oxford".into())]);
        let e = errs_after(|c| c.accounts[1].mailapp_account_id = None);
        assert_eq!(e, vec![ConfigError::MailappIncomplete("oxford".into())]);
        let e = errs_after(|c| {
            let id = c.accounts[1].mailapp_account_id.clone();
            c.accounts[2].mailapp_account_id = id.map(|s| s.to_lowercase());
        });
        assert!(
            e.iter().any(|x| matches!(x, ConfigError::DuplicateMailappId(_))),
            "{e:?}"
        );
    }

    #[test]
    fn gmail_accounts_take_no_mailapp_settings() {
        let e = errs_after(|c| c.accounts[0].inbox_mailbox = Some("INBOX".into()));
        assert_eq!(e, vec![ConfigError::GmailHasMailapp("personal".into())]);
    }

    #[test]
    fn keys() {
        let e = errs_after(|c| {
            let k = c.keys[0].clone();
            c.keys.push(KeyDef {
                name: k.name.to_uppercase(),
                ..k
            });
        });
        assert!(e.iter().any(|x| matches!(x, ConfigError::DuplicateKey(_))), "{e:?}");
        let e = errs_after(|c| {
            c.keys[0].folders.insert("nowhere".into(), "X".into());
        });
        assert!(
            e.iter().any(|x| matches!(x, ConfigError::KeyUnknownAccount { .. })),
            "{e:?}"
        );
        let e = errs_after(|c| c.keys[0].folders.clear());
        assert!(e.iter().any(|x| matches!(x, ConfigError::KeyNoFolders(_))), "{e:?}");
        let e = errs_after(|c| {
            let k = c.keys[0].clone();
            for i in 0..MAX_KEYS {
                c.keys.push(KeyDef {
                    name: format!("k{i}"),
                    ..k.clone()
                });
            }
        });
        assert!(e.iter().any(|x| matches!(x, ConfigError::TooManyKeys(_))), "{e:?}");
    }

    #[test]
    fn times() {
        let e = errs_after(|c| c.runs.evening = "6pm".into());
        assert_eq!(
            e,
            vec![ConfigError::BadTime {
                field: "runs.evening",
                value: "6pm".into()
            }]
        );
        assert!(parse_hhmm("08:00").is_some());
        assert!(parse_hhmm("24:00").is_none());
    }

    #[test]
    fn ranges() {
        let e = errs_after(|c| c.send.hold_seconds = 0);
        assert!(
            e.iter().any(|x| matches!(
                x,
                ConfigError::OutOfRange {
                    field: "send.hold_seconds",
                    ..
                }
            )),
            "{e:?}"
        );
        let e = errs_after(|c| c.send.max_attachments_mb = 26);
        assert!(
            e.iter().any(|x| matches!(
                x,
                ConfigError::OutOfRange {
                    field: "send.max_attachments_mb",
                    ..
                }
            )),
            "{e:?}"
        );
        let e = errs_after(|c| c.risk.batch_threshold = 0);
        assert_eq!(e.len(), 1, "{e:?}");
    }

    #[test]
    fn urls_and_overrides() {
        let e = errs_after(|c| c.kairos.url = "compute-node:7420".into());
        assert!(
            e.iter().any(|x| matches!(
                x,
                ConfigError::BadUrl {
                    field: "kairos.url",
                    ..
                }
            )),
            "{e:?}"
        );
        let e = errs_after(|c| {
            c.automated.never = vec!["@ox.ac.uk".into(), "jane@example.com".into(), "oops".into()];
        });
        assert_eq!(
            e,
            vec![ConfigError::BadAutomatedEntry {
                list: "never",
                value: "oops".into()
            }]
        );
    }

    #[test]
    fn errors_name_the_problem() {
        let msg = ConfigError::KeyUnknownAccount {
            key: "Admin".into(),
            account: "nowhere".into(),
        }
        .to_string();
        assert!(msg.contains("Admin") && msg.contains("nowhere"), "{msg}");
    }
}

#[cfg(test)]
mod example_tests {
    #[test]
    fn the_example_config_is_valid() {
        let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config/config.example.toml");
        let c = super::Config::load(&p).unwrap_or_else(|e| panic!("{e}"));
        assert!(c.validate().is_empty(), "{:?}", c.validate());
        assert!(c.accounts.iter().all(|a| a.backend == super::Backend::Mailapp));
    }
}
