//! Phase 6 in the TUI: reply / reply all / forward / compose in `$EDITOR`,
//! the confirm screen, the hold ("Sending in 7 s · u cancel"), approving
//! Claude's drafts, and unsubscribe.
//!
//! The app can't run an editor while it owns the terminal: it sets
//! `editor_request`, the run loop suspends the TUI, runs `$EDITOR`, and
//! calls [`App::editor_done`] with the saved text.

use crate::app::{App, Key, Mode};
use crate::write::Job;
use keryx_core::changeset::{Changeset, Outgoing, Status};
use keryx_core::compose::{self, ComposeError};
use keryx_core::model::{Address, parse_thread_ref};
use keryx_core::unsubscribe::{self, Unsubscribe};
use keryx_store::api::SendReq;

/// What the editor is for.
#[derive(Debug, Clone, PartialEq)]
pub enum Purpose {
    Reply {
        r#ref: String,
        account: String,
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
    fn account(&self) -> &str {
        match self {
            Purpose::Reply { account, .. }
            | Purpose::Forward { account }
            | Purpose::Compose { account }
            | Purpose::EditDraft { account, .. } => account,
        }
    }
    fn thread(&self) -> Option<String> {
        match self {
            Purpose::Reply { r#ref, .. } => Some(r#ref.clone()),
            Purpose::EditDraft { thread, .. } => thread.clone(),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EditorRequest {
    pub text: String,
    pub purpose: Purpose,
}

/// A composed message waiting for `y` on the confirm screen.
#[derive(Debug, Clone, PartialEq)]
pub struct SendDraft {
    pub req: SendReq,
    pub purpose: Purpose,
    /// The editor text (for `e` edit again).
    pub text: String,
    pub warnings: Vec<String>,
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

impl App {
    fn signature(&self, account: &str) -> Option<String> {
        self.cfg.account(account).and_then(|a| a.signature.clone())
    }

    /// The thread a compose key acts on (open thread, else selection).
    pub(crate) fn compose_thread(&self) -> Option<(String, Vec<keryx_core::model::Message>)> {
        let r = match (&self.mode, &self.open) {
            (Mode::Thread, Some(o)) => keryx_core::model::thread_ref(&o.account, &o.thread_id),
            _ => self.selected()?.r#ref.clone(),
        };
        let (a, t) = parse_thread_ref(&r)?;
        let msgs = self.db.thread(a, t).ok()?;
        (!msgs.is_empty()).then_some((r, msgs))
    }

    /// `r` / `R` / `F` / `c`. Returns whether `k` was one of them.
    pub fn compose_key(&mut self, k: Key) -> bool {
        let req = match k {
            Key::Char('r') | Key::Char('R') => {
                let Some((r, msgs)) = self.compose_thread() else {
                    return true;
                };
                match keryx_core::reply::reply(&self.cfg, &msgs, k == Key::Char('R')) {
                    Ok(d) => EditorRequest {
                        text: compose::file(
                            &d.to,
                            &d.cc,
                            &d.subject,
                            self.signature(&d.account).as_deref(),
                            &d.quote,
                        ),
                        purpose: Purpose::Reply {
                            r#ref: r,
                            account: d.account,
                        },
                    },
                    Err(e) => {
                        self.message = Some((e.to_string(), true));
                        return true;
                    }
                }
            }
            Key::Char('F') => {
                let Some((_, msgs)) = self.compose_thread() else {
                    return true;
                };
                match keryx_core::reply::forward(&self.cfg, &msgs) {
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
            Key::Char('c') => {
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
        self.editor_request = Some(req);
        true
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
                self.message = Some(("Nothing sent".into(), false));
            }
            Err(e) => {
                // Back to the editor with the problem at the top.
                self.editor_request = Some(EditorRequest {
                    text: compose::with_error(&text, &e),
                    purpose,
                });
            }
            Ok(c) => {
                let req = SendReq {
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
                }));
            }
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
        keryx_core::outgoing::warnings(&o, &sent_to, &self.cfg.own_addresses(), &thread, i64::MAX)
    }

    pub fn confirm_send_key(&mut self, k: Key, d: SendDraft) {
        match k {
            Key::Char('y') => {
                self.deferred.push(match &d.purpose {
                    Purpose::EditDraft { id, .. } => Job::Replace(id.clone(), d.req.clone()),
                    _ => Job::Send(d.req.clone()),
                });
                self.mode = self.back.take().unwrap_or(Mode::List);
                self.open = None;
            }
            Key::Char('e') => {
                self.editor_request = Some(EditorRequest {
                    text: d.text.clone(),
                    purpose: d.purpose.clone(),
                });
                self.mode = Mode::List;
            }
            Key::Char('n') | Key::Esc => {
                self.mode = Mode::List;
                self.message = Some(("Nothing sent".into(), false));
            }
            _ => self.mode = Mode::ConfirmSend(Box::new(d)),
        }
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

    pub fn approve_send_key(&mut self, k: Key, id: String) {
        let Some(cs) = self.db.changeset(&id).ok().flatten() else {
            self.mode = Mode::Approvals { sel: 0 };
            return;
        };
        match k {
            Key::Char('y') => {
                self.deferred.push(Job::Approve(id));
                self.mode = Mode::Approvals { sel: 0 };
            }
            Key::Char('n') => {
                self.deferred.push(Job::Reject(id));
                self.mode = Mode::Approvals { sel: 0 };
            }
            Key::Char('e') => {
                if let Some(o) = cs.outgoing() {
                    let mut text = compose::file(&o.to, &o.cc, &o.subject, None, "");
                    // The draft's body goes where Alec types.
                    text = text.replacen("\n\n\n", &format!("\n\n{}\n", o.body), 1);
                    self.editor_request = Some(EditorRequest {
                        text,
                        purpose: Purpose::EditDraft {
                            id,
                            account: o.account.clone(),
                            thread: o
                                .thread_id
                                .as_ref()
                                .map(|t| keryx_core::model::thread_ref(&o.account, t)),
                        },
                    });
                    self.mode = Mode::Approvals { sel: 0 };
                }
            }
            Key::Esc | Key::Char('q') => self.mode = Mode::Approvals { sel: 0 },
            _ => {}
        }
    }

    /// `L`: work out how, and ask first.
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
        self.mode = Mode::List;
        if matches!(k, Key::Char('y') | Key::Enter) {
            self.deferred.push(Job::Unsubscribe(r));
        }
    }

    /// Refresh the holding send for the status bar (and `u` → cancel).
    pub fn refresh_hold(&mut self) {
        self.hold = self
            .daemon
            .as_ref()
            .and_then(|d| d.holding().ok().flatten())
            .and_then(|c| c.hold_until.map(|t| (c.id.clone(), t)));
    }
}
