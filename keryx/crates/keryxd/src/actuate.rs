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

/// Whether the change puts the message in the inbox (`Some(true)`), takes it out (`Some(false)`),
/// or leaves the Inbox label alone.
pub fn inbox_change(acc: &Account, before: &MsgState, after: &MsgState) -> Option<bool> {
    let inbox = acc.inbox_mailbox.as_deref().unwrap_or("INBOX");
    match (&before.location, &after.location) {
        (Location::Mailboxes { labels: b, .. }, Location::Mailboxes { labels: a, .. }) => {
            let (was, is) = (b.iter().any(|l| l == inbox), a.iter().any(|l| l == inbox));
            (was != is).then_some(is)
        }
        _ => None,
    }
}

/// A Mail step that adds or takes away the Inbox label.
fn touches_inbox(acc: &Account, step: &MailStep) -> bool {
    let inbox = acc.inbox_mailbox.as_deref().unwrap_or("INBOX");
    matches!(step, MailStep::Move { from, .. } if from == inbox)
        || matches!(step, MailStep::Copy { to, .. } if to == inbox)
}

/// Ask Mail to sync an account now (it picks up a change made at the server).
fn sync_mail_account(acc: &Account) {
    let Some(id) = acc.mailapp_account_id.as_deref() else {
        return;
    };
    let _ = Command::new("osascript")
        .args([
            "-e",
            "on run argv",
            "-e",
            "tell application \"Mail\" to synchronize with (first account whose id is (item 1 of argv))",
            "-e",
            "end run",
            id,
        ])
        .output();
}

/// Whether Mail's row shows what the change was about, and only that: the mailbox it went into,
/// labels added and taken away, read and flag when they changed. Comparing the whole location
/// never matched on Gmail, which adds and drops labels of its own ("Important"), so every
/// filing waited 30 s, failed, and was undone although Mail had done it (27 Oct).
pub fn shows_change(r: &keryx_store::mailapp::MailRow, before: &MsgState, after: &MsgState) -> bool {
    if before.read != after.read && r.read != after.read {
        return false;
    }
    if before.flagged != after.flagged && r.flagged != after.flagged {
        return false;
    }
    match (&before.location, &after.location) {
        (
            Location::Mailboxes {
                physical: bp,
                labels: bl,
            },
            Location::Mailboxes {
                physical: ap,
                labels: al,
            },
        ) => {
            (bp == ap || r.physical == *ap)
                && al.iter().filter(|l| !bl.contains(l)).all(|l| r.labels.contains(l))
                && bl.iter().filter(|l| !al.contains(l)).all(|l| !r.labels.contains(l))
        }
        _ => true,
    }
}

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

/// Sending through Mail.app. A reply is made with Mail's `reply` (so Mail
/// keeps its threading headers); its recipients are then replaced by exactly
/// the approved ones. Lists arrive newline-separated in argv.
///
/// A reply's body is set as HTML (`html content`, from the file named last in argv) in Mail's own
/// reply format. Setting `content` instead loses Mail's quote of the original: Mail adds it only
/// when the message is saved, so a script sees an empty body, and what it writes is flattened.
/// Measured 11 Nov on four drafts (spec keryx-thread-view); `html content` alone kept the quote,
/// and a test reply read as an ordinary Mail reply in Gmail and Outlook.
const SEND_SCRIPT: &str = r#"
on run argv
  set {acctId, acctAddr, mid, toList, ccList, bccList, subj, body, fileList, boxList, htmlPath} to argv
  set htmlBody to ""
  if htmlPath is not "" then set htmlBody to read (POSIX file htmlPath) as «class utf8»
  -- Paths become file references here, outside Mail: inside its tell block `POSIX file` is
  -- Mail's to resolve. And never a variable called `files`: that is AppleScript's own name
  -- for "every file", so `set {…, files} to argv` failed with -10006 on every attachment.
  set fileRefs to {}
  repeat with f in paragraphs of fileList
    if (f as string) is not "" then set end of fileRefs to ((POSIX file (f as string)) as alias)
  end repeat
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
    if mid is "" then
      set msg to make new outgoing message with properties {subject:subj, content:body, visible:false}
    else
      -- Only in the mailboxes Mail's index has it in: a search of every mailbox holds Mail's
      -- main thread for minutes on a large account. Every mailbox only when the index has none.
      set found to missing value
      set boxes to {}
      repeat with bn in paragraphs of boxList
        if (bn as string) is not "" then
          try
            set end of boxes to mailbox (bn as string) of acc
          end try
        end if
      end repeat
      if boxes is {} then set boxes to every mailbox of acc
      repeat with mb in boxes
        try
          set ms to (messages of mb whose message id is mid)
          if (count of ms) > 0 then
            set found to item 1 of ms
            exit repeat
          end if
        end try
      end repeat
      if found is missing value then return "notfound"
      set msg to reply found opening window false
      set subject of msg to subj
      if htmlBody is "" then
        set content of msg to body
      else
        set html content of msg to htmlBody
      end if
    end if
    tell msg
      set sender to acctAddr
      delete every to recipient
      delete every cc recipient
      delete every bcc recipient
      repeat with a in paragraphs of toList
        if (a as string) is not "" then make new to recipient at end of to recipients with properties {address:(a as string)}
      end repeat
      repeat with a in paragraphs of ccList
        if (a as string) is not "" then make new cc recipient at end of cc recipients with properties {address:(a as string)}
      end repeat
      repeat with a in paragraphs of bccList
        if (a as string) is not "" then make new bcc recipient at end of bcc recipients with properties {address:(a as string)}
      end repeat
    end tell
    repeat with fr in fileRefs
      tell content of msg to make new attachment with properties {file name:(contents of fr)} at after last paragraph
    end repeat
    -- Adding an attachment can bring the message's window up; it stays hidden, as it was made.
    try
      set visible of msg to false
    end try
    delay 1
    send msg
    return "ok"
  end tell
end run
"#;

/// argv for SEND_SCRIPT. `boxes`: the mailboxes the message it replies to is in (Mail's index).
pub fn send_args(acc: &Account, o: &keryx_core::changeset::Outgoing, boxes: &[String]) -> Vec<String> {
    let lines = |v: &[keryx_core::model::Address]| v.iter().map(|a| a.email.clone()).collect::<Vec<_>>().join("\n");
    vec![
        acc.mailapp_account_id.clone().unwrap_or_default(),
        acc.address.clone(),
        o.in_reply_to.clone().unwrap_or_default(),
        lines(&o.to),
        lines(&o.cc),
        lines(&o.bcc),
        o.subject.clone(),
        o.body.clone(),
        o.attachments
            .iter()
            .map(|f| f.path.clone())
            .collect::<Vec<_>>()
            .join("\n"),
        boxes.join("\n"),
    ]
}

/// How long a Mail step may take. keryxd waited on osascript with no limit, so a Mail that never
/// answered (busy, showing a dialog, force-quit mid-script) stopped every later action (31 Oct).
pub const STEP_LIMIT: Duration = Duration::from_secs(60);
/// How long a send may take: making the reply, attaching, handing it to the Outbox.
pub const SEND_LIMIT: Duration = Duration::from_secs(120);

/// Why a script that ran out of time failed. A send may still have gone (the limit can fall just
/// after `send`), so it says to look before sending again.
pub fn timed_out(limit: Duration, send: bool) -> String {
    let s = limit.as_secs();
    if send {
        format!("Mail didn't answer within {s} s; it may still have sent it: look in Sent before sending again")
    } else {
        format!("Mail didn't answer within {s} s (busy, or showing a dialog)")
    }
}

/// Run `script`; any of `oks` is a success and is returned. Past `limit` osascript is killed and
/// the answer is an error.
fn osascript_answers(script: &str, args: &[String], oks: &[&str], limit: Duration) -> Result<String, String> {
    let out = run_within(Command::new("osascript").arg("-").args(args), script.as_bytes(), limit)
        .map_err(|e| format!("osascript: {e}"))?
        .ok_or_else(|| timed_out(limit, script == SEND_SCRIPT))?;
    let said = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if oks.contains(&said.as_str()) {
        return Ok(said);
    }
    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(match said.as_str() {
        "notfound" => "Mail can't find the message being replied to".into(),
        "noaccount" => "Mail doesn't have that account".into(),
        _ if err.contains("-1743") => {
            "keryxd isn't allowed to control Mail: System Settings → Privacy & Security → Automation".into()
        }
        _ => format!("Mail: {said} {err}"),
    })
}

/// Run `cmd` with `input` on stdin; `None` when it was still running after `limit` (it is
/// killed). Its output is small, so it is read after it exits.
pub fn run_within(cmd: &mut Command, input: &[u8], limit: Duration) -> std::io::Result<Option<std::process::Output>> {
    use std::io::Write;
    let mut child = cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input)?;
    }
    let t = Instant::now();
    while child.try_wait()?.is_none() {
        if t.elapsed() >= limit {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    child.wait_with_output().map(Some)
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
        let mut steps = mail_steps(acc, before, after);
        // The Inbox label goes over IMAP on accounts that say so; Mail does the rest.
        let inbox = if acc.gmail_imap {
            inbox_change(acc, before, after)
        } else {
            None
        };
        if inbox.is_some() {
            steps.retain(|s| !touches_inbox(acc, s));
        }
        // Gmail-style accounts file by adding a label, then taking the Inbox one away. The second
        // step waits until Mail shows the first on the message: a copy into a folder made a moment
        // before did nothing, the Inbox label still went, and the mail was left in All Mail alone
        // (round 8, 27 Oct). If the label never shows, nothing is taken away.
        let added: Vec<String> = steps
            .iter()
            .filter_map(|s| match s {
                MailStep::Copy { to, .. } => Some(to.clone()),
                _ => None,
            })
            .collect();
        let copies: Vec<MailStep> = steps
            .iter()
            .filter(|s| matches!(s, MailStep::Copy { .. }))
            .cloned()
            .collect();
        let rest: Vec<MailStep> = steps
            .iter()
            .filter(|s| !matches!(s, MailStep::Copy { .. }))
            .cloned()
            .collect();
        if !copies.is_empty() {
            for step in &copies {
                self.mail_step(acc, msg, &current, step)?;
            }
            if !self.await_labels(acc, msg, &added, Duration::from_secs(10)) {
                // Once more, for a folder Mail had not finished making.
                for step in &copies {
                    self.mail_step(acc, msg, &current, step)?;
                }
                if !self.await_labels(acc, msg, &added, Duration::from_secs(20)) {
                    return Err(format!(
                        "Mail didn't put it in {} (nothing was taken out of the inbox)",
                        added.join(", ")
                    ));
                }
            }
        }
        for step in rest {
            self.mail_step(acc, msg, &current, &step)?;
            if let MailStep::Move { to, .. } = &step {
                current = to.clone();
            }
        }
        if let Some(on) = inbox {
            return self.imap_inbox(acc, msg, before, after, on);
        }
        self.await_envelope(acc, msg, before, after)?;
        Ok(None)
    }

    /// The Inbox label changed at Gmail itself, then Mail asked to catch up. Gmail's answer is
    /// the confirmation; Mail only has to show it, and is nudged once if it is slow.
    fn imap_inbox(
        &self,
        acc: &Account,
        msg: &MsgRef,
        before: &MsgState,
        after: &MsgState,
        on: bool,
    ) -> Result<Option<MsgState>, String> {
        let started = Instant::now();
        let imap =
            keryx_store::gmail_imap::GmailImap::from_keychain(&acc.id, &acc.address).map_err(|e| e.to_string())?;
        let res = imap.set_inbox(&msg.message_id, on);
        tracing::info!(
            account = acc.id,
            inbox = on,
            ms = started.elapsed().as_millis() as u64,
            copies = res.as_ref().map(Vec::len).unwrap_or(0),
            error = res.as_ref().err().map(ToString::to_string).unwrap_or_default(),
            "gmail inbox label"
        );
        res.map_err(|e| e.to_string())?;
        // Mail is asked to sync at once. It used to get 5 s to notice by itself first, and it
        // never did (every archive of 30-31 Oct): 5 s more under the engine's lock, which every
        // other request to keryxd waits on, the screen's included (31 Oct).
        sync_mail_account(acc);
        if self
            .await_envelope_for(acc, msg, before, after, Duration::from_secs(30))
            .is_err()
        {
            // Done at Gmail: what Mail shows follows on its next sync, and the mirror with it.
            tracing::warn!(account = acc.id, "gmail inbox label changed; Mail hasn't shown it yet");
        }
        Ok(None)
    }

    /// Whether the message shows every label in `wanted` within `wait`.
    fn await_labels(&self, acc: &Account, msg: &MsgRef, wanted: &[String], wait: Duration) -> bool {
        let uuid = acc.mailapp_account_id.as_deref().unwrap_or_default();
        let t = Instant::now();
        while t.elapsed() < wait {
            if let Ok(store) = MailStore::open(&self.cfg.paths.mail_dir)
                && let Ok(rows) = store.scan(uuid)
                && rows
                    .iter()
                    .any(|r| r.message_id == msg.message_id && wanted.iter().all(|w| r.labels.contains(w)))
            {
                return true;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        false
    }

    /// One AppleScript step on the message, which is in `current`.
    fn mail_step(&self, acc: &Account, msg: &MsgRef, current: &str, step: &MailStep) -> Result<(), String> {
        let started = Instant::now();
        let res = self.mail_step_inner(acc, msg, current, step);
        // Every step, what Mail said and how long it took: the trail when Mail does less than
        // it answers (round 8: a label came off nowhere, and nothing recorded why).
        tracing::info!(
            account = acc.id,
            step = ?step,
            from = current,
            ms = started.elapsed().as_millis() as u64,
            ok = res.is_ok(),
            error = res.as_ref().err().map(String::as_str).unwrap_or(""),
            "mail step"
        );
        res
    }

    fn mail_step_inner(&self, acc: &Account, msg: &MsgRef, current: &str, step: &MailStep) -> Result<(), String> {
        {
            let out = run_within(
                Command::new("osascript")
                    .arg("-")
                    .args(step_args(acc, &msg.message_id, current, step)),
                SCRIPT.as_bytes(),
                STEP_LIMIT,
            )
            .map_err(|e| format!("osascript: {e}"))?
            .ok_or_else(|| timed_out(STEP_LIMIT, false))?;
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
        }
        Ok(())
    }

    /// Wait (≤ 30 s) until Mail's Envelope Index shows the new state.
    fn await_envelope(&self, acc: &Account, msg: &MsgRef, before: &MsgState, after: &MsgState) -> Result<(), String> {
        self.await_envelope_for(acc, msg, before, after, Duration::from_secs(30))
    }

    fn await_envelope_for(
        &self,
        acc: &Account,
        msg: &MsgRef,
        before: &MsgState,
        after: &MsgState,
        wait: Duration,
    ) -> Result<(), String> {
        let uuid = acc.mailapp_account_id.as_deref().unwrap_or_default();
        let t = Instant::now();
        while t.elapsed() < wait {
            if let Ok(store) = MailStore::open(&self.cfg.paths.mail_dir)
                && let Ok(rows) = store.scan(uuid)
                && rows
                    .iter()
                    .any(|r| r.message_id == msg.message_id && shows_change(r, before, after))
            {
                tracing::info!(
                    account = acc.id,
                    ms = t.elapsed().as_millis() as u64,
                    "mail change seen"
                );
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(300));
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

    fn send(
        &mut self,
        account: &Account,
        o: &keryx_core::changeset::Outgoing,
        html: Option<&str>,
    ) -> Result<(), String> {
        match account.backend {
            Backend::Gmail => {
                let mut files = Vec::new();
                for f in &o.attachments {
                    files.push((
                        f.name.clone(),
                        f.mime.clone(),
                        std::fs::read(&f.path).map_err(|e| format!("{}: {e}", f.name))?,
                    ));
                }
                let raw = keryx_core::outgoing::build(o, crate::now(), &files).map_err(|e| e.to_string())?;
                let client = keryx_store::keychain::get(crate::gmail_sync::CLIENT_SERVICE)
                    .map_err(|e| e.to_string())
                    .and_then(|j| ClientSecret::from_json(&j).map_err(|e| e.to_string()))?;
                let refresh =
                    keryx_store::keychain::get(crate::gmail_sync::REFRESH_SERVICE).map_err(|e| e.to_string())?;
                let http = Http::new().map_err(|e| e.to_string())?;
                let token = gmail::access_token(&http, &client, &refresh).map_err(|e| e.to_string())?;
                Client { t: &http, token }
                    .send(&raw, o.thread_id.as_deref())
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            }
            Backend::Mailapp => {
                Self::ensure_mail_running()?;
                // Where the message it replies to is, from Mail's index (none: Mail looks
                // everywhere, as before).
                let boxes = match (&o.in_reply_to, account.mailapp_account_id.as_deref()) {
                    (Some(mid), Some(uuid)) => MailStore::open(&self.cfg.paths.mail_dir)
                        .and_then(|st| st.mailboxes_of(uuid, mid))
                        .unwrap_or_default(),
                    _ => Vec::new(),
                };
                // The reply's HTML goes by file: argv would carry it, but a file keeps it out of
                // `ps` and has no length limit. Removed whatever the outcome.
                let file = match html {
                    Some(h) => {
                        let p = std::env::temp_dir().join(format!(
                            "keryx-reply-{}.html",
                            o.message_id.replace(['<', '>', '@', '/'], "_")
                        ));
                        std::fs::write(&p, h).map_err(|e| format!("reply html: {e}"))?;
                        Some(p)
                    }
                    None => None,
                };
                let mut args = send_args(account, o, &boxes);
                args.push(file.as_ref().map(|p| p.display().to_string()).unwrap_or_default());
                let r = osascript_answers(SEND_SCRIPT, &args, &["ok"], SEND_LIMIT).map(|_| ());
                if let Some(p) = file {
                    let _ = std::fs::remove_file(p);
                }
                r
            }
        }
    }

    fn unsubscribe(&mut self, url: &str) -> Result<(), String> {
        let resp = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| e.to_string())?
            .post(url)
            .header("content-type", "application/x-www-form-urlencoded")
            .body("List-Unsubscribe=One-Click")
            .send()
            .map_err(|e| format!("unsubscribe: {e}"))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(format!("unsubscribe: the sender answered {}", resp.status()))
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

    fn row(physical: &str, labels: &[&str], read: bool) -> keryx_store::mailapp::MailRow {
        keryx_store::mailapp::MailRow {
            rowid: 1,
            physical: physical.into(),
            labels: labels.iter().map(|l| l.to_string()).collect(),
            read,
            flagged: false,
            conversation_id: 0,
            message_id: "m".into(),
            date_received: 0,
            size: 0,
            ..Default::default()
        }
    }

    fn state_at(physical: &str, labels: &[&str], read: bool) -> MsgState {
        MsgState {
            location: Location::Mailboxes {
                physical: physical.into(),
                labels: labels.iter().map(|l| l.to_string()).collect(),
            },
            read,
            flagged: false,
        }
    }

    #[test]
    fn a_change_is_seen_by_what_it_was_about_not_the_whole_location() {
        // Gmail archive: the inbox label off; Gmail's own Important label does not matter.
        let before = state_at("[Gmail]/All Mail", &["INBOX"], true);
        let after = state_at("[Gmail]/All Mail", &[], true);
        assert!(shows_change(
            &row("[Gmail]/All Mail", &["[Gmail]/Important"], true),
            &before,
            &after
        ));
        assert!(!shows_change(
            &row("[Gmail]/All Mail", &["INBOX"], true),
            &before,
            &after
        ));
        // Read state the move did not touch is not waited on.
        assert!(shows_change(&row("[Gmail]/All Mail", &[], false), &before, &after));
        // Exchange: in the new mailbox, or not yet.
        let before = state_at("Inbox", &[], true);
        let after = state_at("Archive", &[], true);
        assert!(shows_change(&row("Archive", &[], true), &before, &after));
        assert!(!shows_change(&row("Inbox", &[], true), &before, &after));
    }

    /// Every Mail script compiles, and none names a variable after an AppleScript class
    /// (`files` is "every file": the first live attachment failed on it, 26 Oct). macOS only.
    #[cfg(target_os = "macos")]
    #[test]
    fn mail_scripts_compile_and_use_no_class_names_as_variables() {
        for (name, script) in [("SCRIPT", SCRIPT), ("SEND_SCRIPT", SEND_SCRIPT)] {
            let dir = tempfile::tempdir().unwrap();
            let src = dir.path().join("s.applescript");
            std::fs::write(&src, script).unwrap();
            let ok = Command::new("osacompile")
                .arg("-o")
                .arg(dir.path().join("s.scpt"))
                .arg(&src)
                .status()
                .is_ok_and(|s| s.success());
            assert!(ok, "{name} does not compile");
            let first = script.lines().find(|l| l.contains("to argv")).unwrap();
            for class in ["files", "file", "messages", "mailboxes", "accounts"] {
                assert!(
                    !first.contains(&format!(" {class},")) && !first.contains(&format!(" {class}}}")),
                    "{name} names a variable {class:?}"
                );
            }
        }
    }

    use super::*;

    fn cfg() -> Config {
        Config::load(&keryx_store::testkit::root().join("config/config.test.toml")).unwrap()
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
    fn send_argv() {
        let c = cfg();
        let a = c.account("oxford").unwrap();
        let o = keryx_core::changeset::Outgoing {
            account: "oxford".into(),
            thread_id: Some("9".into()),
            from: keryx_core::model::Address::new(None, "newc7370@ox.ac.uk"),
            to: vec![
                keryx_core::model::Address::new(Some("Jane"), "jane@example.com"),
                keryx_core::model::Address::new(None, "sam@example.com"),
            ],
            cc: vec![],
            bcc: vec![],
            subject: "Re: x".into(),
            body: "hi \" & quit".into(),
            in_reply_to: Some("thread-a@example.com".into()),
            references: vec![],
            message_id: "m@ox.ac.uk".into(),
            attachments: vec![],
        };
        let args = send_args(a, &o, &["Inbox".into()]);
        assert_eq!(args.len(), 10);
        assert_eq!(args[9], "Inbox", "where the message it replies to is");
        assert_eq!(args[2], "thread-a@example.com");
        assert_eq!(args[3], "jane@example.com\nsam@example.com");
        assert_eq!(args[7], "hi \" & quit", "body is data");
        assert!(!SEND_SCRIPT.contains("quit"));
    }

    #[test]
    fn a_script_that_never_answers_is_stopped_at_its_limit() {
        // 31 Oct: Mail was force-quit mid-send and keryxd waited for osascript forever.
        let t = Instant::now();
        let out = run_within(
            Command::new("sh").args(["-c", "sleep 30"]),
            b"",
            Duration::from_millis(300),
        )
        .unwrap();
        assert!(out.is_none(), "stopped");
        assert!(t.elapsed() < Duration::from_secs(5));
        let out = run_within(&mut Command::new("cat"), b"ok", Duration::from_secs(5)).unwrap();
        assert_eq!(out.map(|o| o.stdout), Some(b"ok".to_vec()), "a quick one answers");
        assert!(
            timed_out(SEND_LIMIT, true).contains("look in Sent"),
            "a send may have gone"
        );
    }

    #[test]
    fn the_reply_is_looked_for_only_where_mail_has_it() {
        assert!(SEND_SCRIPT.contains("paragraphs of boxList"));
        assert!(SEND_SCRIPT.contains("if boxes is {} then set boxes to every mailbox of acc"));
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

    #[test]
    fn the_inbox_label_is_told_apart_from_everything_else_a_change_does() {
        let c = cfg();
        let a = c.account("oqts").unwrap();
        let all = "[Gmail]/All Mail";
        let inbox = st(mb(all, &["INBOX"]), false, false);
        let out = st(mb(all, &[]), false, false);
        assert_eq!(inbox_change(a, &inbox, &out), Some(false), "archive");
        assert_eq!(inbox_change(a, &out, &inbox), Some(true), "back to the inbox");
        assert_eq!(
            inbox_change(a, &inbox, &st(mb(all, &["INBOX"]), true, true)),
            None,
            "read, flag"
        );
        // Filing under an old config key: the label copy stays with Mail, the Inbox goes over IMAP.
        let filed = st(mb(all, &["Admin"]), false, false);
        assert_eq!(inbox_change(a, &inbox, &filed), Some(false));
        let rest: Vec<MailStep> = mail_steps(a, &inbox, &filed)
            .into_iter()
            .filter(|s| !touches_inbox(a, s))
            .collect();
        assert_eq!(
            rest,
            vec![MailStep::Copy {
                from: all.into(),
                to: "Admin".into()
            }]
        );
    }
}
