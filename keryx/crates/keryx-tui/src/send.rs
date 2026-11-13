//! Phase 6 in the TUI: reply / reply all / forward / compose in `$EDITOR`,
//! the confirm screen, the hold ("sending · c cancels · 8 s"), approving
//! Claude's drafts, and unsubscribe.
//!
//! The app can't run an editor while it owns the terminal: it sets
//! `editor_request`, the run loop suspends the TUI, runs `$EDITOR`, and
//! calls [`App::editor_done`] with the saved text.
//!
//! **Sending you can trust** (cleanup C6, decision D4, 13 Nov 2026). Before this:
//! - `r` answered the thread's latest message in the account's own copy, not the one you were
//!   reading; in a thread merged across accounts that could be an older message from another
//!   account. Now `r` answers the message under the bar ([`App::reply_basis`]), from the copy in
//!   the account that answers it, says so ("replying to Milly, 2 Nov 13:35"), and keryxd threads
//!   it to that message (`SendReq::in_reply_to`).
//! - The confirm screen previewed a `>` quote that is not sent (keryxd sends Mail's own quote,
//!   `reply::reply_html`) and never showed the one that is. Now it shows what goes: your words,
//!   then the real quote ([`App::sent_preview`]), the address it is shown from, the warnings
//!   wrapped, all of it scrollable.
//! - After a send you were back in the list. Now you stay in the thread and see your message,
//!   "sending · c cancels · 8 s", then "sent" ([`Outbox`]). Cancel is `c`, so `u` is only undo.
//! - A send that failed after its hold said "not sent" once. Now the failure stays in its thread
//!   and in the footer until you open the draft again.
//! - `r` on a thread with a draft (Claude's or yours) opens it rather than starting a second.

use crate::app::{App, Key, Mode};
use crate::write::Job;
use keryx_core::changeset::{Changeset, Outgoing, Status};
use keryx_core::compose::{self, ComposeError};
use keryx_core::model::{Address, Message, parse_thread_ref, thread_ref};
use keryx_core::unsubscribe::{self, Unsubscribe};
use keryx_store::api::SendReq;
use locus_theme::keys::Action;

/// The message a reply answers, and how it is named on screen.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplyTo {
    /// Its Message-ID (without angle brackets).
    pub message_id: String,
    /// "replying to Milly, 2 Nov 13:35".
    pub label: String,
}

/// What the editor is for.
#[derive(Debug, Clone, PartialEq)]
pub enum Purpose {
    Reply {
        r#ref: String,
        account: String,
        /// The message it answers; None answers the thread's latest (a reply from the list).
        to: Option<ReplyTo>,
    },
    Forward {
        account: String,
    },
    Compose {
        account: String,
    },
    /// Alec edits a pending draft (it's replaced by his version).
    EditDraft {
        id: String,
        account: String,
        thread: Option<String>,
    },
}

impl Purpose {
    pub fn account(&self) -> &str {
        match self {
            Purpose::Reply { account, .. }
            | Purpose::Forward { account }
            | Purpose::Compose { account }
            | Purpose::EditDraft { account, .. } => account,
        }
    }
    /// The same purpose from another account (new mail and forwards; a reply cannot move).
    fn moved_to(&self, account: &str) -> Purpose {
        match self {
            Purpose::Forward { .. } => Purpose::Forward {
                account: account.to_string(),
            },
            Purpose::Compose { .. } => Purpose::Compose {
                account: account.to_string(),
            },
            other => other.clone(),
        }
    }

    /// What the header says while writing.
    pub fn label(&self) -> String {
        match self {
            Purpose::Reply { to: Some(t), .. } => t.label.clone(),
            Purpose::Reply { .. } => "writing a reply".into(),
            Purpose::Forward { .. } => "forwarding".into(),
            Purpose::Compose { .. } => "writing a new mail".into(),
            Purpose::EditDraft { .. } => "editing Claude's draft".into(),
        }
    }

    /// The name a kept draft goes under: one per thread for a reply, one per account for a new
    /// mail or a forward. A draft of Claude's is not a file: it is a pending send, listed in g d beside these.
    pub fn draft_name(&self) -> Option<String> {
        let safe = |s: &str| -> String {
            s.chars()
                .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
                .collect()
        };
        match self {
            Purpose::Reply { r#ref, .. } => Some(format!("reply-{}.eml", safe(r#ref))),
            Purpose::Forward { account } => Some(format!("forward-{}.eml", safe(account))),
            Purpose::Compose { account } => Some(format!("new-{}.eml", safe(account))),
            Purpose::EditDraft { .. } => None,
        }
    }

    fn thread(&self) -> Option<String> {
        match self {
            Purpose::Reply { r#ref, .. } => Some(r#ref.clone()),
            Purpose::EditDraft { thread, .. } => thread.clone(),
            _ => None,
        }
    }

    fn in_reply_to(&self) -> Option<String> {
        match self {
            Purpose::Reply { to: Some(t), .. } if !t.message_id.is_empty() => Some(t.message_id.clone()),
            _ => None,
        }
    }
}

/// Beside a kept reply draft, the message it answers (`<draft>.to`: its Message-ID, then the
/// label), so reopening it after a restart still answers what you were reading.
pub fn reply_to_path(draft: &std::path::Path) -> std::path::PathBuf {
    let mut s = draft.as_os_str().to_os_string();
    s.push(".to");
    std::path::PathBuf::from(s)
}

/// Read what [`reply_to_path`] keeps.
pub fn read_reply_to(draft: &std::path::Path) -> Option<ReplyTo> {
    let text = std::fs::read_to_string(reply_to_path(draft)).ok()?;
    let (id, label) = text.split_once('\n')?;
    Some(ReplyTo {
        message_id: id.trim().to_string(),
        label: label.trim().to_string(),
    })
}

/// A kept draft, and what it answers, gone.
pub fn remove_draft(p: &std::path::Path) {
    let _ = std::fs::remove_file(p);
    let _ = std::fs::remove_file(reply_to_path(p));
}

#[derive(Debug, Clone, PartialEq)]
pub struct EditorRequest {
    pub text: String,
    pub purpose: Purpose,
}

/// Writing in the right pane: what for, where Esc-like endings return, and the `^a` picker.
#[derive(Debug, Clone)]
pub struct Writing {
    pub purpose: Purpose,
    pub back: Mode,
    pub picker: Option<crate::pick::Picker>,
}

/// A composed message waiting for `y` on the confirm screen.
#[derive(Debug, Clone, PartialEq)]
pub struct SendDraft {
    pub req: SendReq,
    pub purpose: Purpose,
    /// The editor text (for `e` edit again).
    pub text: String,
    pub warnings: Vec<String>,
    /// How far the confirm screen is scrolled (j k space).
    pub scroll: u16,
}

/// Where a send of yours is.
#[derive(Debug, Clone, PartialEq)]
pub enum SendState {
    /// Given to keryxd; its answer not back yet.
    Asking,
    /// In its hold (cancellable with `c`).
    Holding,
    Cancelling,
    Sent,
    Cancelled,
    /// Refused, or failed after the hold: why. Kept until you open the draft again.
    Failed(String),
}

/// One of your sends from this session, shown in its thread (D4).
#[derive(Debug, Clone, PartialEq)]
pub struct Outbox {
    /// The send's changeset, once keryxd has it.
    pub cs: Option<String>,
    /// The threads it belongs to (the one it was sent in and the one you were reading).
    pub threads: Vec<String>,
    pub subject: String,
    /// Your words (without any quote).
    pub body: String,
    /// When you pressed y.
    pub at: i64,
    pub state: SendState,
    /// Whether the draft that holds its text is still kept (g d).
    pub kept: bool,
}

/// What the confirm screen shows as sent: your words, then the quote that goes with them.
#[derive(Debug, Clone, PartialEq)]
pub struct Preview {
    pub own: String,
    /// The attribution line and their message, as keryxd's Mail-format reply quotes them.
    pub quote: Option<(String, String)>,
}

/// "Milly" for Milly Borthwick, the address when there is no name.
pub fn first_name(a: Option<&Address>) -> String {
    match a {
        Some(a) => match a.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
            Some(n) => n.split_whitespace().next().unwrap_or(n).trim_matches('"').to_string(),
            None => a.email.clone(),
        },
        None => "(no sender)".into(),
    }
}

/// Run `$VISUAL` / `$EDITOR` (default `vi`) on a `keryx-*.eml` file.
pub fn run_editor(text: &str) -> Result<String, String> {
    let path = std::env::temp_dir().join(format!("keryx-{}.eml", uuid::Uuid::new_v4().simple()));
    std::fs::write(&path, text).map_err(|e| e.to_string())?;
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".into());
    let mut parts = editor.split_whitespace();
    let prog = parts.next().unwrap_or("vi");
    let status = std::process::Command::new(prog)
        .args(parts)
        .arg(&path)
        .status()
        .map_err(|e| format!("{prog}: {e}"));
    let out = std::fs::read_to_string(&path).map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&path);
    match status? {
        s if s.success() => out,
        _ => Err("the editor quit without saving: nothing sent".into()),
    }
}

/// What a reply or forward is built from (D4): the thread it goes in, its messages up to and
/// including the one it answers, and that message.
pub struct Basis {
    pub r#ref: String,
    pub msgs: Vec<Message>,
    pub to: Option<ReplyTo>,
    /// When the account that would answer has no copy of the message: the one that has.
    pub note: Option<String>,
}

fn same_id(a: &str, b: &str) -> bool {
    let t = |s: &str| s.trim().trim_matches(|c| c == '<' || c == '>').to_string();
    !t(a).is_empty() && t(a) == t(b)
}

impl App {
    fn signature(&self, account: &str) -> Option<String> {
        self.cfg.account(account).and_then(|a| a.signature.clone())
    }

    /// The thread a compose key acts on (open thread, else selection).
    pub(crate) fn compose_thread(&self) -> Option<(String, Vec<keryx_core::model::Message>)> {
        let r = match (&self.mode, &self.open) {
            (Mode::Thread, Some(o)) => thread_ref(&o.account, &o.thread_id),
            _ => self.selected()?.r#ref.clone(),
        };
        let (a, t) = parse_thread_ref(&r)?;
        let msgs = self.db.thread(a, t).ok()?;
        (!msgs.is_empty()).then_some((r, msgs))
    }

    /// The label for a reply to `m`: "replying to Milly, 2 Nov 13:35".
    pub fn replying_label(&self, m: &Message) -> String {
        use chrono::TimeZone;
        let when = self
            .cfg
            .tz()
            .timestamp_opt(m.date, 0)
            .single()
            .map(|d| d.format("%-d %b %H:%M").to_string())
            .unwrap_or_default();
        format!("replying to {}, {when}", first_name(m.from.as_ref()))
    }

    /// What `r` / `R` / `F` act on. Reading a thread: the message under the bar, from its copy
    /// in the account that answers it (`reply::sending_account`: the one it came to). That
    /// account must hold a copy, or Mail cannot find the message to answer (it looks only in
    /// the sending account), so without one the reply goes from the account that has it, and
    /// the note says so. From the list: the selected thread's latest message, as before.
    pub fn reply_basis(&self) -> Option<Basis> {
        let reading = matches!(self.mode, Mode::Thread);
        let target = self
            .open
            .as_ref()
            .filter(|_| reading)
            .and_then(|o| o.target().and_then(|i| o.msgs.get(i)).cloned());
        let Some(m) = target else {
            let (r, msgs) = self.compose_thread()?;
            return Some(Basis {
                r#ref: r,
                msgs,
                to: None,
                note: None,
            });
        };
        let answering = keryx_core::reply::sending_account(&self.cfg, &m).map(|a| a.id.clone());
        let mut copies: Vec<(String, String)> = if m.message_id.is_empty() {
            vec![]
        } else {
            self.db
                .threads_with_ids(std::slice::from_ref(&m.message_id))
                .unwrap_or_default()
        };
        copies.push((m.account.clone(), m.thread_id.clone()));
        let holds = |a: &str, t: &str| {
            self.db.thread(a, t).unwrap_or_default().iter().any(|x| {
                same_id(&x.message_id, &m.message_id) || (x.account == m.account && x.source_id == m.source_id)
            })
        };
        let pick = copies
            .iter()
            .find(|(a, t)| answering.as_deref() == Some(a.as_str()) && holds(a, t))
            .or_else(|| copies.iter().find(|(a, t)| *a == m.account && *t == m.thread_id))
            .cloned()?;
        let msgs = self.db.thread(&pick.0, &pick.1).ok()?;
        let at = msgs.iter().position(|x| {
            same_id(&x.message_id, &m.message_id) || (x.account == m.account && x.source_id == m.source_id)
        })?;
        let note = match &answering {
            Some(a) if *a != pick.0 => Some(format!(
                "{} has no copy of this message: it goes from {}, which has",
                self.cfg.account(a).map_or(a.as_str(), |x| x.name.as_str()),
                self.cfg.account(&pick.0).map_or(pick.0.as_str(), |x| x.name.as_str()),
            )),
            _ => None,
        };
        Some(Basis {
            r#ref: thread_ref(&pick.0, &pick.1),
            msgs: msgs[..=at].to_vec(),
            to: Some(ReplyTo {
                message_id: m.message_id.clone(),
                label: self.replying_label(&m),
            }),
            note,
        })
    }

    /// Every thread the open conversation is in (merged across accounts).
    fn conversation_threads(&self) -> Vec<String> {
        let Some(o) = &self.open else {
            return self.selected().map(|r| vec![r.r#ref.clone()]).unwrap_or_default();
        };
        let mut v = vec![thread_ref(&o.account, &o.thread_id)];
        for m in &o.msgs {
            let r = thread_ref(&m.account, &m.thread_id);
            if !v.contains(&r) {
                v.push(r);
            }
        }
        v
    }

    /// Claude's waiting draft for this conversation, if there is one.
    pub fn claude_draft_here(&self) -> Option<Changeset> {
        let threads = self.conversation_threads();
        self.pending_sends().into_iter().find(|c| {
            c.outgoing().is_some_and(|o| {
                o.thread_id
                    .as_deref()
                    .is_some_and(|t| threads.contains(&thread_ref(&o.account, t)))
            })
        })
    }

    /// Your kept reply draft for this conversation, if there is one.
    pub fn kept_draft_here(&self) -> Option<crate::drafts::Kept> {
        let threads = self.conversation_threads();
        self.kept_drafts()
            .into_iter()
            .find(|k| matches!(&k.purpose, Purpose::Reply { r#ref, .. } if threads.contains(r#ref)))
    }

    /// Open this thread's draft, Claude's first, then yours. Whether there was one.
    pub fn open_thread_draft(&mut self) -> bool {
        if let Some(c) = self.claude_draft_here()
            && let Some((text, purpose)) = self.claude_draft_text(&c)
        {
            self.editor_request = Some(EditorRequest { text, purpose });
            self.message = Some((
                "Claude's draft · :wq then y sends your version · :cq leaves it waiting".into(),
                false,
            ));
            return true;
        }
        if let Some(k) = self.kept_draft_here()
            && let Ok(text) = std::fs::read_to_string(&k.path)
        {
            let label = match &k.purpose {
                Purpose::Reply { to: Some(t), .. } => format!(" ({})", t.label),
                _ => String::new(),
            };
            self.clear_failed(&k.purpose);
            self.editor_request = Some(EditorRequest {
                text,
                purpose: k.purpose.clone(),
            });
            self.message = Some((format!("your kept draft{label} · :cq throws it away"), false));
            return true;
        }
        false
    }

    /// `r` / `R` / `F` / `+`. Returns whether `k` was one of them.
    pub fn compose_key(&mut self, k: Key) -> bool {
        let req = match k {
            Key::Char('r') | Key::Char('R') => {
                // A draft for this thread already: open it rather than start a second (D4).
                if self.open_thread_draft() {
                    return true;
                }
                let Some(b) = self.reply_basis() else {
                    return true;
                };
                match keryx_core::reply::reply(&self.cfg, &b.msgs, k == Key::Char('R')) {
                    Ok(mut d) => {
                        if let Some((a, _)) = parse_thread_ref(&b.r#ref)
                            && b.note.is_some()
                        {
                            d.account = a.to_string();
                        }
                        let said = b.to.as_ref().map(|t| t.label.clone());
                        self.message = match (said, &b.note) {
                            (Some(l), Some(n)) => Some((format!("{l} · {n}"), false)),
                            (Some(l), None) => Some((l, false)),
                            _ => None,
                        };
                        EditorRequest {
                            text: compose::file(
                                &d.to,
                                &d.cc,
                                &d.subject,
                                self.signature(&d.account).as_deref(),
                                &d.quote,
                            ),
                            purpose: Purpose::Reply {
                                r#ref: b.r#ref,
                                account: d.account,
                                to: b.to,
                            },
                        }
                    }
                    Err(e) => {
                        self.message = Some((e.to_string(), true));
                        return true;
                    }
                }
            }
            Key::Char('F') => {
                let Some(b) = self.reply_basis() else {
                    return true;
                };
                match keryx_core::reply::forward(&self.cfg, &b.msgs) {
                    Ok(d) => EditorRequest {
                        text: compose::file(&[], &[], &d.subject, self.signature(&d.account).as_deref(), &d.quote),
                        purpose: Purpose::Forward { account: d.account },
                    },
                    Err(e) => {
                        self.message = Some((e.to_string(), true));
                        return true;
                    }
                }
            }
            Key::Char('+') => {
                let account = match self.account {
                    0 => self
                        .selected()
                        .map(|r| r.account.clone())
                        .or_else(|| self.cfg.accounts.first().map(|a| a.id.clone())),
                    i => self.cfg.accounts.get(i - 1).map(|a| a.id.clone()),
                };
                let Some(account) = account else { return true };
                EditorRequest {
                    text: compose::file(&[], &[], "", self.signature(&account).as_deref(), ""),
                    purpose: Purpose::Compose { account },
                }
            }
            _ => return false,
        };
        self.write(req.text, req.purpose);
        true
    }

    /// Open the editor on `text` for `purpose`: the account it goes from on a `From:` line, or
    /// the draft you kept for it.
    pub fn write(&mut self, text: String, purpose: Purpose) {
        let path = self.draft_path(&purpose);
        let kept = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .filter(|t| !t.trim().is_empty());
        let text = match kept {
            Some(t) => {
                self.message = Some(("your kept draft · :cq throws it away".into(), false));
                t
            }
            None => {
                // What the reply answers, beside the draft it may become.
                if let (Some(p), Purpose::Reply { to: Some(t), .. }) = (&path, &purpose) {
                    let _ = std::fs::write(reply_to_path(p), format!("{}\n{}\n", t.message_id, t.label));
                }
                compose::with_from(&text, &self.from_line(purpose.account()))
            }
        };
        self.editor_request = Some(EditorRequest { text, purpose });
    }

    /// How an account reads on the `From:` line: its name and its address
    /// (`Oxford <you@ox.ac.uk>`), never the bare id.
    pub fn from_line(&self, account: &str) -> String {
        match self.cfg.account(account) {
            Some(a) => format!("{} <{}>", a.name, a.shown_address.as_deref().unwrap_or(&a.address)),
            None => account.to_string(),
        }
    }

    pub fn draft_path(&self, purpose: &Purpose) -> Option<std::path::PathBuf> {
        Some(self.drafts_dir.as_ref()?.join(purpose.draft_name()?))
    }

    /// The run loop started the editor for `purpose`.
    pub fn writing_started(&mut self, purpose: Purpose) {
        self.writing = Some(Writing {
            purpose,
            back: self.mode.clone(),
            picker: None,
        });
    }

    /// The editor closed.
    pub fn writing_finished(&mut self, how: crate::writing::Finished) {
        use crate::writing::Finished;
        let Some(w) = self.writing.take() else { return };
        self.mode = w.back;
        match how {
            Finished::Send(text) => {
                // Kept until it is sent, so `n` on the confirm screen loses nothing.
                if let Some(p) = self.draft_path(&w.purpose) {
                    let _ = std::fs::write(p, &text);
                }
                self.editor_done(w.purpose, Ok(text));
            }
            Finished::Kept => {
                let again = if matches!(w.purpose, Purpose::Reply { .. }) {
                    "r"
                } else {
                    "+"
                };
                self.message = Some((format!("kept as a draft · {again} opens it again"), false));
            }
            Finished::Discarded => {
                if let Some(p) = self.draft_path(&w.purpose) {
                    let _ = std::fs::remove_file(reply_to_path(&p));
                }
                self.message = Some(("draft thrown away".into(), false))
            }
            Finished::Closed => self.message = Some(("nothing sent".into(), false)),
        }
    }

    /// `^a` while writing: the picker over your recent files.
    pub fn open_picker(&mut self, picker: crate::pick::Picker) {
        if let Some(w) = &mut self.writing {
            w.picker = Some(picker);
        }
    }

    /// A key in the picker; `Some(path)` once a file is chosen. `^o` opens the highlighted
    /// file (Preview for a PDF) and keeps the picker open.
    pub fn picker_key(&mut self, k: Key) -> Option<std::path::PathBuf> {
        let dry = self.dry_open;
        let w = self.writing.as_mut()?;
        let p = w.picker.as_mut()?;
        match p.key(k) {
            crate::pick::Picked::Still => None,
            crate::pick::Picked::Open(path) => {
                if !dry {
                    let _ = std::process::Command::new("open").arg(&path).spawn();
                }
                self.opened.push(path);
                None
            }
            crate::pick::Picked::Cancelled => {
                w.picker = None;
                None
            }
            crate::pick::Picked::Chosen(path) => {
                w.picker = None;
                Some(path)
            }
        }
    }

    /// `From:` as typed, as an account id: its id, name or one of its addresses.
    fn resolve_from(&self, purpose: &Purpose, from: Option<&str>) -> Result<String, ComposeError> {
        let Some(v) = from else {
            return Ok(purpose.account().to_string());
        };
        // `Oxford <you@ox.ac.uk>` names the account by its address; a bare name or id works too.
        let v = match (v.find('<'), v.rfind('>')) {
            (Some(i), Some(j)) if j > i => v[i + 1..j].trim(),
            _ => v.trim(),
        };
        let found = self.cfg.accounts.iter().find(|a| {
            a.id.eq_ignore_ascii_case(v)
                || a.name.eq_ignore_ascii_case(v)
                || a.address.eq_ignore_ascii_case(v)
                || a.aliases.iter().any(|x| x.eq_ignore_ascii_case(v))
                || a.shown_address.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(v))
        });
        let Some(a) = found else {
            let ids: Vec<&str> = self.cfg.accounts.iter().map(|a| a.id.as_str()).collect();
            return Err(ComposeError::UnknownAccount(v.to_string(), ids.join(", ")));
        };
        if matches!(purpose, Purpose::Reply { .. } | Purpose::EditDraft { .. }) && a.id != purpose.account() {
            return Err(ComposeError::ReplyFromOther(purpose.account().to_string()));
        }
        Ok(a.id.clone())
    }

    /// The editor closed with `result` (the saved file, or why it failed).
    pub fn editor_done(&mut self, purpose: Purpose, result: Result<String, String>) {
        let text = match result {
            Ok(t) => t,
            Err(e) => {
                self.message = Some((e, true));
                return;
            }
        };
        let account = purpose.account().to_string();
        let sig = self.signature(&account);
        match compose::parse(&text, sig.as_deref(), self.cfg.send.max_attachments_mb) {
            Err(ComposeError::Empty) => {
                self.message = Some(("nothing sent: the message is empty".into(), false));
            }
            Err(e) => {
                // Back to the editor with the problem at the top.
                self.editor_request = Some(EditorRequest {
                    text: compose::with_error(&text, &e),
                    purpose,
                });
            }
            Ok(c) => {
                let account = match self.resolve_from(&purpose, c.from.as_deref()) {
                    Ok(a) => a,
                    Err(e) => {
                        self.editor_request = Some(EditorRequest {
                            text: compose::with_error(&text, &e),
                            purpose,
                        });
                        return;
                    }
                };
                let purpose = purpose.moved_to(&account);
                let req = SendReq {
                    in_reply_to: purpose.in_reply_to(),
                    account: account.clone(),
                    thread: purpose.thread(),
                    to: c.to,
                    cc: c.cc,
                    bcc: c.bcc,
                    subject: c.subject,
                    body: c.body,
                    attachments: c.attach.iter().map(|p| p.to_string_lossy().into_owned()).collect(),
                };
                let warnings = self.local_warnings(&req);
                self.mode = Mode::ConfirmSend(Box::new(SendDraft {
                    req,
                    purpose,
                    text,
                    warnings,
                    scroll: 0,
                }));
            }
        }
    }

    /// The message a reply answers, as keryxd will find it: the one named in the thread, else
    /// the thread's latest.
    pub fn answered(&self, r: &SendReq) -> Option<Message> {
        let (a, t) = parse_thread_ref(r.thread.as_deref()?)?;
        let msgs = self.db.thread(a, t).ok()?;
        match r.in_reply_to.as_deref() {
            Some(id) => msgs.into_iter().find(|m| same_id(&m.message_id, id)),
            None => msgs.into_iter().next_back(),
        }
    }

    /// What goes, as keryxd builds it (`engine_send`): a reply in its own account's thread is
    /// sent in Mail's format, your words with the plain `>` quote left off
    /// (`reply::without_plain_quote`) and their message quoted under "On …, … wrote:"
    /// (`reply::reply_html`). Anything else (a new mail, a forward, a reply threaded from
    /// another account) goes as the text you wrote, quote and all.
    pub fn sent_preview(&self, r: &SendReq) -> Preview {
        let threaded = r
            .thread
            .as_deref()
            .and_then(parse_thread_ref)
            .is_some_and(|(a, _)| a == r.account);
        let to = if threaded { self.answered(r) } else { None };
        match to {
            Some(m) => {
                use chrono::TimeZone;
                let when = self
                    .cfg
                    .tz()
                    .timestamp_opt(m.date, 0)
                    .single()
                    .map(|d| d.format("On %-d %b %Y, at %H:%M").to_string())
                    .unwrap_or_else(|| "On an unknown date".into());
                let who = match &m.from {
                    Some(a) if a.name.as_deref().is_some_and(|n| !n.trim().is_empty()) => {
                        format!("{} <{}>", a.name.as_deref().unwrap_or_default().trim(), a.email)
                    }
                    Some(a) => format!("<{}>", a.email),
                    None => "someone".into(),
                };
                let theirs = if m.body_text.trim().is_empty() {
                    if m.attachments.is_empty() {
                        "(their message has no text)".to_string()
                    } else {
                        format!(
                            "(their message has no text: {} attachment(s), quoted as sent)",
                            m.attachments.len()
                        )
                    }
                } else {
                    m.body_text.trim_end().to_string()
                };
                Preview {
                    own: keryx_core::reply::without_plain_quote(&r.body).to_string(),
                    quote: Some((format!("{when}, {who} wrote:"), theirs)),
                }
            }
            None => Preview {
                own: r.body.trim_end().to_string(),
                quote: None,
            },
        }
    }

    /// The confirm screen's warnings, computed from the mirror.
    fn local_warnings(&self, r: &SendReq) -> Vec<String> {
        let o = Outgoing {
            account: r.account.clone(),
            thread_id: r
                .thread
                .as_deref()
                .and_then(parse_thread_ref)
                .map(|(_, t)| t.to_string()),
            from: Address::new(None, ""),
            to: r.to.clone(),
            cc: r.cc.clone(),
            bcc: r.bcc.clone(),
            subject: r.subject.clone(),
            body: r.body.clone(),
            in_reply_to: None,
            references: vec![],
            message_id: String::new(),
            attachments: vec![],
        };
        let sent_to = keryx_store::view::sent_to(&self.db, &self.cfg).unwrap_or_default();
        let thread = match r.thread.as_deref().and_then(parse_thread_ref) {
            Some((a, t)) => self.db.thread(a, t).unwrap_or_default(),
            None => vec![],
        };
        let own = self.cfg.own_addresses();
        let mut w = keryx_core::outgoing::warnings(&o, &sent_to, &own, &thread, i64::MAX);
        // A reply on a mailing list goes to everyone on it (C6 edge case): said by name.
        if let Some(m) = self.answered(r)
            && m.list_headers.iter().any(|(k, _)| k == "list-id" || k == "list-post")
        {
            let post = m
                .list_headers
                .iter()
                .find(|(k, _)| k == "list-post")
                .and_then(|(_, v)| v.split("mailto:").nth(1))
                .map(|v| v.trim_end_matches('>').trim().to_lowercase());
            let list: std::collections::HashSet<String> =
                m.to.iter().chain(&m.cc).map(Address::key).chain(post).collect();
            for a in o.recipients().iter().filter(|a| list.contains(*a) && !own.contains(*a)) {
                w.push(format!(
                    "⚠ {a} is on a mailing list's message: everyone on the list may get this"
                ));
            }
        }
        // Said before y, not after: keryxd refuses it while keryx is not live.
        if !self.cfg.live {
            for r in o.recipients().iter().filter(|r| !own.contains(*r)) {
                w.push(format!(
                    "test mode: {r} isn't one of your addresses, so this will not be sent"
                ));
            }
        }
        w
    }

    pub fn confirm_send_key(&mut self, k: Key, mut d: SendDraft) {
        match crate::keys::action(k) {
            Some(Action::Yes) => self.confirm_send(d),
            Some(Action::Edit) => {
                self.editor_request = Some(EditorRequest {
                    text: d.text.clone(),
                    purpose: d.purpose.clone(),
                });
                self.mode = if self.open.is_some() { Mode::Thread } else { Mode::List };
            }
            Some(Action::No | Action::Back | Action::Quit) => {
                // Back where you were writing from; the text waits in drafts (round 11 audit).
                self.mode = if self.open.is_some() { Mode::Thread } else { Mode::List };
                let kept = self.draft_path(&d.purpose).is_some_and(|p| p.exists());
                self.message = Some((
                    if kept {
                        "not sent · kept in drafts (g d)"
                    } else {
                        "not sent"
                    }
                    .into(),
                    false,
                ));
            }
            _ => {
                let by: i32 = match k {
                    Key::Char('j') | Key::Down => 1,
                    Key::Char('k') | Key::Up => -1,
                    Key::Char(' ') | Key::PageDown | Key::Ctrl('d') => 10,
                    Key::PageUp | Key::Ctrl('u') => -10,
                    Key::Top => -10_000,
                    Key::Bottom => 10_000,
                    _ => 0,
                };
                d.scroll = (i32::from(d.scroll) + by).clamp(0, i32::from(u16::MAX)) as u16;
                self.mode = Mode::ConfirmSend(Box::new(d));
            }
        }
    }

    /// `y` on the confirm screen: to keryxd, and you stay where you wrote it from, a thread
    /// showing your message in its hold (D4).
    fn confirm_send(&mut self, d: SendDraft) {
        let draft = self.draft_path(&d.purpose);
        let mut threads: Vec<String> = d.req.thread.iter().cloned().collect();
        if let Some(o) = &self.open {
            let r = thread_ref(&o.account, &o.thread_id);
            if !threads.contains(&r) && !threads.is_empty() {
                threads.push(r);
            }
        }
        self.clear_failed(&d.purpose);
        self.outbox.push(Outbox {
            cs: None,
            threads,
            subject: d.req.subject.clone(),
            body: keryx_core::reply::without_plain_quote(&d.req.body).to_string(),
            at: self.now.timestamp(),
            state: SendState::Asking,
            kept: draft.as_ref().is_some_and(|p| p.exists()),
        });
        self.deferred.push(match &d.purpose {
            Purpose::EditDraft { id, .. } => Job::Replace(id.clone(), d.req.clone()),
            _ => Job::Send(d.req.clone(), draft),
        });
        self.mode = match (&self.open, self.back.is_some()) {
            (Some(_), _) => Mode::Thread,
            (None, true) => self.back.take().unwrap_or(Mode::List),
            (None, false) => Mode::List,
        };
    }

    /// A failed send's entry goes once you take its draft up again or send it anew.
    fn clear_failed(&mut self, p: &Purpose) {
        let Some(t) = p.thread() else { return };
        self.outbox
            .retain(|o| !(matches!(o.state, SendState::Failed(_)) && o.threads.contains(&t)));
    }

    /// keryxd's answer to a send: in its hold (with its changeset), or refused.
    pub fn outbox_answered(&mut self, r: &SendReq, res: Result<&Changeset, &str>) {
        let Some(o) = self
            .outbox
            .iter_mut()
            .rev()
            .find(|o| o.state == SendState::Asking && o.subject == r.subject)
        else {
            return;
        };
        match res {
            Ok(c) => {
                o.cs = Some(c.id.clone());
                o.state = match c.status {
                    Status::Holding => SendState::Holding,
                    Status::Sent | Status::Applied => SendState::Sent,
                    Status::Cancelled | Status::Undone | Status::Rejected => SendState::Cancelled,
                    Status::Failed => SendState::Failed(c.error.clone().unwrap_or_default()),
                    Status::Pending => SendState::Holding,
                };
            }
            Err(e) => o.state = SendState::Failed(e.to_string()),
        }
    }

    /// Your sends shown in this thread (newest first).
    pub fn outbox_for(&self, account: &str, thread_id: &str) -> Vec<&Outbox> {
        let r = thread_ref(account, thread_id);
        self.outbox.iter().rev().filter(|o| o.threads.contains(&r)).collect()
    }

    /// A send that failed, for the footer: it stays until dealt with.
    pub fn failed_send(&self) -> Option<&Outbox> {
        self.outbox
            .iter()
            .rev()
            .find(|o| matches!(o.state, SendState::Failed(_)))
    }

    /// Claude's drafts waiting for Alec (newest first).
    pub fn pending_sends(&self) -> Vec<Changeset> {
        self.db
            .changesets(Some(Status::Pending), 100)
            .unwrap_or_default()
            .into_iter()
            .filter(|c| c.outgoing().is_some())
            .collect()
    }

    pub fn unsubscribe_key(&mut self) {
        let Some((r, msgs)) = self.compose_thread() else { return };
        let how = msgs.iter().rev().find_map(|m| unsubscribe::parse(&m.list_headers));
        let text = match how {
            None => {
                self.message = Some(("this thread has no unsubscribe link".into(), false));
                return;
            }
            Some(Unsubscribe::Link { url }) => {
                // keryx never visits it: Alec opens the page himself.
                self.message = Some((format!("open this page to unsubscribe: {url}"), false));
                return;
            }
            Some(Unsubscribe::OneClick { url }) => {
                let host = url.split('/').nth(2).unwrap_or(&url).to_string();
                format!("one-click: keryx tells {host} to stop, then archives the thread")
            }
            Some(Unsubscribe::Mailto { to, .. }) => format!("by email to {to} (you'll confirm it like any send)"),
        };
        self.mode = Mode::ConfirmUnsubscribe { r#ref: r, how: text };
    }

    pub fn confirm_unsubscribe_key(&mut self, k: Key, r: String) {
        self.mode = if self.open.is_some() { Mode::Thread } else { Mode::List };
        if matches!(k, Key::Char('y') | Key::Enter) {
            self.deferred.push(Job::Unsubscribe(r));
        }
    }

    /// Refresh the holding send for the status bar and the thread (and `c` → cancel).
    pub fn refresh_hold(&mut self) {
        self.hold = self
            .daemon
            .as_ref()
            .and_then(|d| d.holding().ok().flatten())
            .and_then(|c| c.hold_until.map(|t| (c.id.clone(), t)));
        self.settle_send();
    }

    /// Once a send's hold is over, say how it went: sent (its draft goes), or not sent and why
    /// (its draft stays, and the failure stays in its thread and the footer). Read from the
    /// mirror, where keryxd records the outcome (`Sent`; `Applied` from older daemons).
    pub fn settle_send(&mut self) {
        let Some((id, draft)) = self.sending.clone() else {
            return;
        };
        if self.hold.as_ref().is_some_and(|(h, _)| *h == id) {
            return;
        }
        let Ok(Some(cs)) = self.db.changeset(&id) else { return };
        let kept = draft.as_ref().is_some_and(|p| p.exists());
        let state = match cs.status {
            Status::Applied | Status::Sent => {
                if let Some(p) = &draft {
                    remove_draft(p);
                }
                self.message = Some(("sent".into(), false));
                SendState::Sent
            }
            Status::Failed => {
                let why = cs.error.unwrap_or_default();
                let kept = if kept {
                    " · your mail is kept in drafts (g d)"
                } else {
                    ""
                };
                self.message = Some((format!("not sent: {why}{kept}"), true));
                SendState::Failed(why)
            }
            Status::Undone | Status::Rejected | Status::Cancelled => {
                self.message = Some(("send cancelled: nothing sent".into(), false));
                SendState::Cancelled
            }
            // Still holding in the mirror (it lags keryxd by a moment): look again next time.
            _ => return,
        };
        for o in self.outbox.iter_mut().filter(|o| o.cs.as_deref() == Some(id.as_str())) {
            o.state = state.clone();
            o.kept = matches!(state, SendState::Failed(_)) && kept;
        }
        self.sending = None;
    }
}
