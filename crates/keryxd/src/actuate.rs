//! Acting on the sources: Gmail API label changes, and Mail.app through
//! AppleScript (SPEC §3). Values reach AppleScript only as `osascript`
//! arguments, never as script text.
//!
//! Gmail over IMAP (the OQTS accounts) needs care: messages live in
//! `[Gmail]/All Mail` and moving one *out of All Mail* deletes it in Gmail.
//! So labels are added by **copying** to the label's mailbox and removed by
//! moving **out of that label** (INBOX → All Mail) — never out of All Mail.

use crate::engine::Actuator;
use keryx_core::changeset::{MsgRef, MsgState};
use keryx_core::config::{Account, Backend, Config};
use keryx_core::location::{self, is_label_style};
use keryx_core::model::Location;
use keryx_store::db::Db;
use keryx_store::gmail::{self, Client, ClientSecret, Http};
use keryx_store::ingest;
use keryx_store::mailapp::MailStore;
use std::process::Command;
use std::time::{Duration, Instant};

/// Gmail label changes for a state change: (add, remove).
pub fn gmail_modify(before: &MsgState, after: &MsgState) -> (Vec<String>, Vec<String>) {
    let (mut add, mut remove) = location::label_diff(&before.location, &after.location);
    if before.read != after.read {
        if after.read {
            remove.push("UNREAD".into())
        } else {
            add.push("UNREAD".into())
        }
    }
    if before.flagged != after.flagged {
        if after.flagged {
            add.push("STARRED".into())
        } else {
            remove.push("STARRED".into())
        }
    }
    (add, remove)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MailStep {
    Flag(bool),
    Read(bool),
    /// Move from one mailbox to another.
    Move {
        from: String,
        to: String,
    },
    /// Copy into a mailbox (adds a Gmail label over IMAP).
    Copy {
        from: String,
        to: String,
    },
}

/// The mailbox to find the message in before any step runs.
fn current_box(acc: &Account, loc: &Location) -> String {
    match loc {
        Location::Mailboxes { physical, labels } => {
            let inbox = acc.inbox_mailbox.as_deref().unwrap_or("INBOX");
            if is_label_style(acc) && labels.iter().any(|l| l == inbox) {
                inbox.to_string()
            } else {
                physical.clone()
            }
        }
        Location::Labels { .. } => String::new(),
    }
}

/// AppleScript steps for a Mail.app state change, in order. Each step
/// names the mailbox the message is in when it runs.
pub fn mail_steps(acc: &Account, before: &MsgState, after: &MsgState) -> Vec<MailStep> {
    let mut steps = Vec::new();
    if before.flagged != after.flagged {
        steps.push(MailStep::Flag(after.flagged));
    }
    if before.read != after.read {
        steps.push(MailStep::Read(after.read));
    }
    if let (
        Location::Mailboxes {
            physical: p1,
            labels: l1,
        },
        Location::Mailboxes {
            physical: p2,
            labels: l2,
        },
    ) = (&before.location, &after.location)
    {
        if is_label_style(acc) {
            // Add labels first (copy from All Mail), then remove (move out of the label).
            for l in l2.iter().filter(|l| !l1.contains(l)) {
                steps.push(MailStep::Copy {
                    from: p1.clone(),
                    to: l.clone(),
                });
            }
            for l in l1.iter().filter(|l| !l2.contains(l)) {
                steps.push(MailStep::Move {
                    from: l.clone(),
                    to: p2.clone(),
                });
            }
        } else if p1 != p2 {
            steps.push(MailStep::Move {
                from: p1.clone(),
                to: p2.clone(),
            });
        }
    }
    steps
}

/// One AppleScript, values only via argv.
const SCRIPT: &str = r#"
on run argv
  set {acctId, acctAddr, boxName, mid, action, arg1} to argv
  tell application "Mail"
    set acc to missing value
    try
      set acc to first account whose id is acctId
    end try
    if acc is missing value then
      try
        set acc to first account whose email addresses contains acctAddr
      end try
    end if
    if acc is missing value then return "noaccount"
    set ms to (messages of mailbox boxName of acc whose message id is mid)
    if (count of ms) is 0 then return "notfound"
    set m to item 1 of ms
    if action is "flag" then set flagged status of m to (arg1 is "true")
    if action is "read" then set read status of m to (arg1 is "true")
    if action is "move" then move m to mailbox arg1 of acc
    if action is "copy" then duplicate m to mailbox arg1 of acc
    return "ok"
  end tell
end run
"#;

/// The argv for one step (the box is where the message is now).
pub fn step_args(acc: &Account, mid: &str, current: &str, step: &MailStep) -> Vec<String> {
    let (box_name, action, arg) = match step {
        MailStep::Flag(on) => (current.to_string(), "flag", on.to_string()),
        MailStep::Read(on) => (current.to_string(), "read", on.to_string()),
        MailStep::Move { from, to } => (from.clone(), "move", to.clone()),
        MailStep::Copy { from, to } => (from.clone(), "copy", to.clone()),
    };
    vec![
        acc.mailapp_account_id.clone().unwrap_or_default(),
        acc.address.clone(),
        box_name,
        mid.to_string(),
        action.to_string(),
        arg,
    ]
}

pub struct Real {
    cfg: Config,
}

impl Real {
    pub fn new(cfg: Config) -> Real {
        Real { cfg }
    }

    fn gmail(&self, msg: &MsgRef, before: &MsgState, after: &MsgState) -> Result<Option<MsgState>, String> {
        let (add, remove) = gmail_modify(before, after);
        let client = keryx_store::keychain::get(crate::gmail_sync::CLIENT_SERVICE)
            .map_err(|e| e.to_string())
            .and_then(|j| ClientSecret::from_json(&j).map_err(|e| e.to_string()))?;
        let refresh = keryx_store::keychain::get(crate::gmail_sync::REFRESH_SERVICE).map_err(|e| e.to_string())?;
        let http = Http::new().map_err(|e| e.to_string())?;
        let token = gmail::access_token(&http, &client, &refresh).map_err(|e| e.to_string())?;
        let c = Client { t: &http, token };
        let labels = c.modify(&msg.source_id, &add, &remove).map_err(|e| e.to_string())?;
        Ok(Some(MsgState {
            location: gmail::location(&labels),
            read: !labels.iter().any(|l| l == "UNREAD"),
            flagged: labels.iter().any(|l| l == "STARRED"),
        }))
    }

    fn ensure_mail_running() -> Result<(), String> {
        let running = || {
            Command::new("pgrep")
                .args(["-x", "Mail"])
                .output()
                .is_ok_and(|o| o.status.success())
        };
        if running() {
            return Ok(());
        }
        tracing::info!("starting Mail in the background");
        Command::new("open")
            .args(["-gja", "Mail"])
            .status()
            .map_err(|e| e.to_string())?;
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(60) {
            if running() {
                // Give Mail a moment to load its accounts.
                std::thread::sleep(Duration::from_secs(3));
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        Err("Mail didn't start within 60 s".into())
    }

    fn mail(
        &self,
        acc: &Account,
        msg: &MsgRef,
        before: &MsgState,
        after: &MsgState,
    ) -> Result<Option<MsgState>, String> {
        Self::ensure_mail_running()?;
        let mut current = current_box(acc, &before.location);
        for step in mail_steps(acc, before, after) {
            let out = Command::new("osascript")
                .arg("-")
                .args(step_args(acc, &msg.message_id, &current, &step))
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .and_then(|mut child| {
                    use std::io::Write;
                    if let Some(stdin) = child.stdin.as_mut() {
                        stdin.write_all(SCRIPT.as_bytes())?;
                    }
                    child.wait_with_output()
                })
                .map_err(|e| format!("osascript: {e}"))?;
            let said = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if said != "ok" {
                let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
                return Err(match said.as_str() {
                    "notfound" => format!("Mail can't find the message in {current}"),
                    "noaccount" => format!("Mail has no account {}", acc.address),
                    _ if err.contains("-1743") => {
                        "keryxd isn't allowed to control Mail: System Settings → Privacy & Security → Automation".into()
                    }
                    _ => format!("Mail: {said} {err}"),
                });
            }
            if let MailStep::Move { to, .. } = &step {
                current = to.clone();
            }
        }
        self.await_envelope(acc, msg, after)?;
        Ok(None)
    }

    /// Wait (≤ 30 s) until Mail's Envelope Index shows the new state.
    fn await_envelope(&self, acc: &Account, msg: &MsgRef, after: &MsgState) -> Result<(), String> {
        let uuid = acc.mailapp_account_id.as_deref().unwrap_or_default();
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(30) {
            if let Ok(store) = MailStore::open(&self.cfg.paths.mail_dir)
                && let Ok(rows) = store.scan(uuid)
                && rows.iter().any(|r| {
                    r.message_id == msg.message_id
                        && r.read == after.read
                        && r.flagged == after.flagged
                        && ingest::mailapp_location(r).as_ref() == Some(&after.location)
                })
            {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        Err("Mail accepted the change but it didn't show up within 30 s".into())
    }
}

impl Actuator for Real {
    fn apply(
        &mut self,
        account: &Account,
        msg: &MsgRef,
        before: &MsgState,
        after: &MsgState,
    ) -> Result<Option<MsgState>, String> {
        match account.backend {
            Backend::Gmail => self.gmail(msg, before, after),
            Backend::Mailapp => self.mail(account, msg, before, after),
        }
    }

    fn resync(&mut self, db: &mut Db, account: &Account) -> Result<(), String> {
        let store = MailStore::open(&self.cfg.paths.mail_dir).map_err(|e| e.to_string())?;
        ingest::mailapp_account(db, &store, account, None)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config::load(&keryx_store::testkit::root().join("config/config.example.toml")).unwrap()
    }

    fn st(loc: Location, read: bool, flagged: bool) -> MsgState {
        MsgState {
            location: loc,
            read,
            flagged,
        }
    }

    fn mb(p: &str, l: &[&str]) -> Location {
        Location::Mailboxes {
            physical: p.into(),
            labels: l.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn gmail_label_changes() {
        let l = |v: &[&str]| Location::Labels {
            labels: v.iter().map(|s| s.to_string()).collect(),
        };
        let (add, remove) = gmail_modify(&st(l(&["INBOX"]), false, false), &st(l(&["Label_1"]), true, true));
        assert_eq!(add, vec!["Label_1", "STARRED"]);
        assert_eq!(remove, vec!["INBOX", "UNREAD"]);
        let (add, remove) = gmail_modify(&st(l(&[]), true, true), &st(l(&[]), false, false));
        assert_eq!((add, remove), (vec!["UNREAD".to_string()], vec!["STARRED".to_string()]));
    }

    #[test]
    fn exchange_moves() {
        let c = cfg();
        let a = c.account("oxford").unwrap();
        assert_eq!(
            mail_steps(
                a,
                &st(mb("Inbox", &[]), true, false),
                &st(mb("Archive", &[]), true, false)
            ),
            vec![MailStep::Move {
                from: "Inbox".into(),
                to: "Archive".into()
            }]
        );
        assert_eq!(
            mail_steps(
                a,
                &st(mb("Inbox", &[]), false, false),
                &st(mb("Inbox", &[]), true, true)
            ),
            vec![MailStep::Flag(true), MailStep::Read(true)]
        );
    }

    #[test]
    fn gmail_over_imap_never_moves_out_of_all_mail() {
        let c = cfg();
        let a = c.account("oqts").unwrap();
        let all = "[Gmail]/All Mail";
        // Archive: out of INBOX.
        let s = mail_steps(a, &st(mb(all, &["INBOX"]), true, false), &st(mb(all, &[]), true, false));
        assert_eq!(
            s,
            vec![MailStep::Move {
                from: "INBOX".into(),
                to: all.into()
            }]
        );
        // Un-archive: copy into INBOX.
        let s = mail_steps(a, &st(mb(all, &[]), true, false), &st(mb(all, &["INBOX"]), true, false));
        assert_eq!(
            s,
            vec![MailStep::Copy {
                from: all.into(),
                to: "INBOX".into()
            }]
        );
        // File: copy to the label, then out of INBOX.
        let s = mail_steps(
            a,
            &st(mb(all, &["INBOX"]), true, false),
            &st(mb(all, &["Society"]), true, false),
        );
        assert_eq!(
            s,
            vec![
                MailStep::Copy {
                    from: all.into(),
                    to: "Society".into()
                },
                MailStep::Move {
                    from: "INBOX".into(),
                    to: all.into()
                }
            ]
        );
        for step in s {
            if let MailStep::Move { from, .. } = step {
                assert_ne!(from, all, "never move out of All Mail");
            }
        }
        // Flags are set where the message is found (INBOX if it's there).
        assert_eq!(current_box(a, &mb(all, &["INBOX"])), "INBOX");
        assert_eq!(current_box(a, &mb(all, &[])), all);
    }

    #[test]
    fn argv_carries_values_not_script() {
        let c = cfg();
        let a = c.account("oxford").unwrap();
        let evil = "x\" & (do shell script \"rm -rf ~\") & \"";
        let args = step_args(
            a,
            evil,
            "Inbox",
            &MailStep::Move {
                from: "Inbox".into(),
                to: "Archive".into(),
            },
        );
        assert_eq!(args[3], evil, "passed verbatim as data");
        assert!(!SCRIPT.contains("rm -rf"));
        assert_eq!(args[0], "74EA59BE-FE1D-4D49-8712-D8A2AB402605");
        assert_eq!(
            (args[2].as_str(), args[4].as_str(), args[5].as_str()),
            ("Inbox", "move", "Archive")
        );
    }
}
