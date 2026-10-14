//! TUI state and keys (no terminal here, so it's testable): list, thread,
//! reading pane, account filter, go-to, search, attachments, help. Writes
//! (triage, reviews) are in `write.rs`.

use crate::write::Job;
use chrono::DateTime;
use chrono_tz::Tz;
use keryx_core::config::Config;
use keryx_core::invite::{self, Invite};
use keryx_core::mime;
use keryx_core::model::Message;
use keryx_store::api::ReviewView;
use keryx_store::client::Daemon;
use keryx_store::db::Db;
use keryx_store::mailapp;
use keryx_store::view::{self, Filter, ThreadRow, View};
use std::collections::BTreeSet;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Ctrl(char),
    Enter,
    Esc,
    Tab,
    BackTab,
    Backspace,
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    List,
    /// Reading a thread (full screen when narrow; right pane when wide).
    Thread,
    /// Typing a search; results update live.
    Search(String),
    GoTo,
    Help,
    /// Choosing which attachment to open.
    Attachments,
    /// Snooze picker; `Some(text)` while typing a time for *Pick…*.
    Snooze(Option<String>),
    /// File-into-key picker.
    File,
    /// `A`: what's waiting for Alec (cleanup reviews).
    Approvals {
        sel: usize,
    },
    /// Deciding a cleanup review's proposals.
    Review {
        id: String,
        sel: usize,
    },
    /// Choosing a different action for a proposal.
    ReviewChange {
        id: String,
        sel: usize,
    },
    /// `Y`: confirm accepting a whole group.
    ConfirmGroup {
        id: String,
        sel: usize,
        group: String,
        count: usize,
    },
    /// Alec's composed message: `y` sends (after the hold), `e` edits, `n` drops.
    ConfirmSend(Box<crate::send::SendDraft>),
    /// One of Claude's drafts: `y` approve · `n` reject · `e` edit.
    ApproveSend {
        id: String,
    },
    /// `L`: how unsubscribing will happen; `y` to go ahead.
    ConfirmUnsubscribe {
        r#ref: String,
        how: String,
    },
    /// `t` / `i` / `w` (and `y` on a suggestion): a kairos form.
    KairosForm(Box<crate::form::Form>),
    /// `g` → `c`: Claude's runs.
    Runs {
        sel: usize,
    },
    /// One run: everything it made.
    RunDetail {
        id: String,
        sel: usize,
    },
}

/// A thread opened for reading.
#[derive(Debug, Clone)]
pub struct Open {
    pub account: String,
    pub thread_id: String,
    pub msgs: Vec<Message>,
    /// Parsed invites, per message (same order as `msgs`).
    pub invites: Vec<Option<Invite>>,
    pub show_quotes: bool,
    pub expand_all: bool,
    pub scroll: u16,
}

impl Open {
    /// Every attachment in the thread: (message index, attachment index).
    pub fn attachments(&self) -> Vec<(usize, usize)> {
        self.msgs
            .iter()
            .enumerate()
            .flat_map(|(i, m)| (0..m.attachments.len()).map(move |j| (i, j)))
            .collect()
    }
}

pub struct App {
    pub cfg: Config,
    pub db: Db,
    pub now: DateTime<Tz>,
    pub view: View,
    /// 0 = all accounts, else 1 + index into `cfg.accounts`.
    pub account: usize,
    pub rows: Vec<ThreadRow>,
    pub sel: usize,
    pub mode: Mode,
    /// The search shown in the list (after Enter), if any.
    pub search: Option<String>,
    pub search_error: Option<String>,
    pub open: Option<Open>,
    /// Last thread loaded for the reading pane (cached by ref).
    pub preview: Option<Open>,
    /// Status-bar message (text, is_error).
    pub message: Option<(String, bool)>,
    /// Accounts whose last sync is older than 5 minutes: (name, minutes).
    pub stale: Vec<(String, i64)>,
    pub quit: bool,
    /// Width of the last frame (decides list vs. split layout).
    pub width: u16,
    /// Files opened with `o`, for tests (the real TUI calls `open`).
    pub opened: Vec<PathBuf>,
    pub dry_open: bool,
    /// keryxd, for changes (None: read-only, e.g. keryxd not running).
    pub daemon: Option<Box<dyn Daemon>>,
    /// Threads marked with `x` for a batch.
    pub marked: BTreeSet<String>,
    /// Changes queued for keryxd (run after the frame is drawn).
    pub deferred: Vec<Job>,
    /// The review being decided (read from the mirror).
    pub review: Option<ReviewView>,
    /// Proposals marked with `x` in the review.
    pub review_marked: BTreeSet<String>,
    /// Where Esc returns from a thread opened elsewhere (the review).
    pub back: Option<Mode>,
    /// Open proposals waiting for Alec (status bar).
    pub open_proposals: usize,
    /// Drafts waiting for Alec's approval (status bar).
    pub pending_sends: usize,
    /// The run loop opens `$EDITOR` for this, then calls `editor_done`.
    pub editor_request: Option<crate::send::EditorRequest>,
    /// A send in its hold: (changeset id, sends at).
    pub hold: Option<(String, i64)>,
    /// Live kairos task states, by task id (fetched when a thread opens).
    pub task_status: std::collections::HashMap<String, keryx_store::api::TaskStatus>,
}

pub const LIST_LIMIT: u32 = 500;
pub const SPLIT_WIDTH: u16 = 120;

impl App {
    pub fn new(cfg: Config, db: Db, now: DateTime<Tz>) -> App {
        let mut app = App {
            cfg,
            db,
            now,
            view: View::Inbox,
            account: 0,
            rows: Vec::new(),
            sel: 0,
            mode: Mode::List,
            search: None,
            search_error: None,
            open: None,
            preview: None,
            message: None,
            stale: Vec::new(),
            quit: false,
            width: 100,
            opened: Vec::new(),
            dry_open: false,
            daemon: None,
            marked: BTreeSet::new(),
            deferred: Vec::new(),
            review: None,
            review_marked: BTreeSet::new(),
            back: None,
            open_proposals: 0,
            pending_sends: 0,
            editor_request: None,
            hold: None,
            task_status: std::collections::HashMap::new(),
        };
        app.reload();
        app
    }

    pub fn split(&self) -> bool {
        self.width >= SPLIT_WIDTH
    }

    pub fn filter(&self) -> Filter {
        Filter {
            accounts: match self.account {
                0 => vec![],
                i => self
                    .cfg
                    .accounts
                    .get(i - 1)
                    .map(|a| vec![a.id.clone()])
                    .unwrap_or_default(),
            },
            limit: LIST_LIMIT,
        }
    }

    pub fn account_label(&self) -> String {
        match self.account {
            0 => "All accounts".into(),
            i => self.cfg.accounts.get(i - 1).map(|a| a.name.clone()).unwrap_or_default(),
        }
    }

    pub fn title(&self) -> String {
        match &self.search {
            Some(q) => format!("Search: {q}"),
            None => self.view.title(),
        }
    }

    /// Re-read rows from the mirror, keeping the selection on the same thread.
    pub fn reload(&mut self) {
        let keep = self.rows.get(self.sel).map(|r| r.r#ref.clone());
        let f = self.filter();
        let res = match &self.search {
            Some(q) => keryx_core::query::parse(q, self.now)
                .map_err(|e| e.to_string())
                .and_then(|q| view::search(&self.db, &self.cfg, &q, &f).map_err(|e| e.to_string())),
            None => view::list(&self.db, &self.cfg, &self.view, &f).map_err(|e| e.to_string()),
        };
        match res {
            Ok(rows) => {
                self.rows = rows;
                self.search_error = None;
            }
            Err(e) => {
                self.rows.clear();
                self.search_error = Some(e);
            }
        }
        self.sel = keep
            .and_then(|r| self.rows.iter().position(|x| x.r#ref == r))
            .unwrap_or(0)
            .min(self.rows.len().saturating_sub(1));
        self.stale = self.stale_accounts();
        self.open_proposals = keryx_store::state::open_proposals(&self.db).unwrap_or(0);
        self.pending_sends = self.pending_sends().len();
        // Marks on threads that left the list are dropped.
        let refs: BTreeSet<String> = self.rows.iter().map(|r| r.r#ref.clone()).collect();
        self.marked.retain(|r| refs.contains(r));
        self.preview = None;
    }

    fn stale_accounts(&self) -> Vec<(String, i64)> {
        let now = self.now.timestamp();
        self.cfg
            .accounts
            .iter()
            .filter_map(|a| match self.db.sync_state(&a.id) {
                Ok(Some((_, at))) if now - at > 5 * 60 => Some((a.name.clone(), (now - at) / 60)),
                _ => None,
            })
            .collect()
    }

    pub fn selected(&self) -> Option<&ThreadRow> {
        self.rows.get(self.sel)
    }

    pub fn load_thread(&self, account: &str, thread_id: &str) -> Option<Open> {
        let msgs = self.db.thread(account, thread_id).ok()?;
        if msgs.is_empty() {
            return None;
        }
        let invites = msgs.iter().map(|m| self.invite_of(m)).collect();
        Some(Open {
            account: account.to_string(),
            thread_id: thread_id.to_string(),
            msgs,
            invites,
            show_quotes: false,
            expand_all: false,
            scroll: 0,
        })
    }

    /// Calendar parts aren't in the mirror's text: parse the raw message.
    pub(crate) fn invite_of(&self, m: &Message) -> Option<Invite> {
        let raw = self.raw(m)?;
        let p = mime::parse(&raw).ok()?;
        p.calendars.iter().find_map(|c| invite::parse(c, self.cfg.tz()))
    }

    fn raw(&self, m: &Message) -> Option<Vec<u8>> {
        match self.db.raw(&m.account, &m.source_id).ok()? {
            (Some(b), _) => Some(b),
            (None, Some(p)) if !p.is_empty() => mailapp::read_emlx(std::path::Path::new(&p)).ok(),
            _ => None,
        }
    }

    /// The thread shown in the reading pane (list or review selection), cached.
    pub fn ensure_preview(&mut self) {
        let target = match &self.mode {
            Mode::Review { sel, .. } | Mode::ReviewChange { sel, .. } | Mode::ConfirmGroup { sel, .. } => self
                .review_selected(*sel)
                .and_then(|i| self.review.as_ref().map(|v| v.items[i].r#ref.clone()))
                .and_then(|r| keryx_core::model::parse_thread_ref(&r).map(|(a, t)| (a.to_string(), t.to_string()))),
            _ => self.selected().map(|r| (r.account.clone(), r.thread_id.clone())),
        };
        let Some((a, t)) = target else {
            self.preview = None;
            return;
        };
        let fresh = self
            .preview
            .as_ref()
            .is_some_and(|p| p.account == a && p.thread_id == t);
        if !fresh {
            self.preview = self.load_thread(&a, &t);
        }
    }

    pub fn move_sel_by(&mut self, d: isize) {
        self.move_sel(d);
    }

    fn move_sel(&mut self, d: isize) {
        if self.rows.is_empty() {
            return;
        }
        let n = self.rows.len() as isize;
        self.sel = (self.sel as isize + d).clamp(0, n - 1) as usize;
    }

    pub fn key(&mut self, k: Key) {
        if self.message.is_some() && !matches!(self.mode, Mode::Search(_)) {
            self.message = None;
        }
        match self.mode.clone() {
            Mode::Help => self.mode = Mode::List,
            Mode::GoTo => self.goto_key(k),
            Mode::Search(q) => self.search_key(k, q),
            Mode::Attachments => self.attachment_key(k),
            Mode::Thread => self.thread_key(k),
            Mode::List => self.list_key(k),
            Mode::Snooze(input) => self.snooze_key(k, input),
            Mode::File => self.file_key(k),
            Mode::Approvals { sel } => self.approvals_key(k, sel),
            Mode::Review { id, sel } => self.review_key(k, id, sel),
            Mode::ReviewChange { id, sel } => self.review_change_key(k, id, sel),
            Mode::ConfirmGroup { id, sel, group, .. } => self.confirm_group_key(k, id, sel, group),
            Mode::ConfirmSend(d) => self.confirm_send_key(k, *d),
            Mode::ApproveSend { id } => self.approve_send_key(k, id),
            Mode::ConfirmUnsubscribe { r#ref, .. } => self.confirm_unsubscribe_key(k, r#ref),
            Mode::KairosForm(f) => self.form_key(k, *f),
            Mode::Runs { sel } => self.runs_key(k, sel),
            Mode::RunDetail { id, sel } => self.run_detail_key(k, id, sel),
        }
    }

    fn list_key(&mut self, k: Key) {
        if self.write_key(k) {
            return;
        }
        match k {
            Key::Char('q') => self.quit = true,
            Key::Char('j') | Key::Down => self.move_sel(1),
            Key::Char('k') | Key::Up => self.move_sel(-1),
            Key::PageDown | Key::Ctrl('d') => self.move_sel(10),
            Key::PageUp | Key::Ctrl('u') => self.move_sel(-10),
            Key::Char('G') => self.move_sel(isize::MAX / 2),
            Key::Enter => self.open_selected(),
            Key::Tab => self.cycle_account(1),
            Key::BackTab => self.cycle_account(-1),
            Key::Char('g') => self.mode = Mode::GoTo,
            Key::Char('/') => self.mode = Mode::Search(self.search.clone().unwrap_or_default()),
            Key::Char('?') => self.mode = Mode::Help,
            Key::Esc if !self.marked.is_empty() => self.marked.clear(),
            Key::Esc if self.search.is_some() => {
                self.search = None;
                self.reload();
            }
            _ => {}
        }
    }

    /// Open a thread by id (selecting its row when it's in the list).
    pub fn open_thread(&mut self, account: &str, thread_id: &str) -> bool {
        if let Some(i) = self
            .rows
            .iter()
            .position(|r| r.account == account && r.thread_id == thread_id)
        {
            self.sel = i;
        }
        self.open = self.load_thread(account, thread_id);
        if self.open.is_some() {
            self.mode = Mode::Thread;
            self.queue_task_status();
        }
        self.open.is_some()
    }

    fn open_selected(&mut self) {
        let Some(r) = self.selected().cloned() else { return };
        self.open = self.load_thread(&r.account, &r.thread_id);
        if self.open.is_some() {
            self.mode = Mode::Thread;
            self.queue_task_status();
            // Opening a thread marks it read (Alec, 2026-10-14).
            if r.unread {
                self.deferred.push(Job::Triage {
                    req: keryx_store::api::TriageReq {
                        intent: keryx_store::api::Intent::Read { on: true },
                        refs: vec![r.r#ref.clone()],
                        summary: None,
                        review_id: None,
                    },
                    quiet: true,
                });
            }
        }
    }

    fn cycle_account(&mut self, d: isize) {
        let n = self.cfg.accounts.len() as isize + 1;
        self.account = ((self.account as isize + d).rem_euclid(n)) as usize;
        self.sel = 0;
        self.reload();
    }

    fn thread_key(&mut self, k: Key) {
        if self.back.is_none() && self.write_key(k) {
            return;
        }
        let Some(open) = self.open.as_mut() else {
            self.mode = Mode::List;
            return;
        };
        match k {
            Key::Esc | Key::Char('q') | Key::Char('h') => {
                self.open = None;
                self.mode = self.back.take().unwrap_or(Mode::List);
            }
            Key::Char('j') | Key::Down => open.scroll = open.scroll.saturating_add(1),
            Key::Char('k') | Key::Up => open.scroll = open.scroll.saturating_sub(1),
            Key::PageDown | Key::Char(' ') | Key::Ctrl('d') => open.scroll = open.scroll.saturating_add(15),
            Key::PageUp | Key::Ctrl('u') => open.scroll = open.scroll.saturating_sub(15),
            Key::Char('g') => open.scroll = 0,
            Key::Char('z') => {
                open.show_quotes = !open.show_quotes;
                open.expand_all = open.show_quotes;
            }
            Key::Char('o') => match open.attachments().len() {
                0 => self.message = Some(("no attachments in this thread".into(), false)),
                1 => self.open_attachment(0),
                _ => self.mode = Mode::Attachments,
            },
            // Next / previous thread without going back to the list.
            Key::Char('J') => {
                self.move_sel(1);
                self.open_selected();
            }
            Key::Char('K') => {
                self.move_sel(-1);
                self.open_selected();
            }
            Key::Char('?') => self.mode = Mode::Help,
            _ => {}
        }
    }

    fn attachment_key(&mut self, k: Key) {
        match k {
            Key::Char(c) if c.is_ascii_digit() && c != '0' => {
                self.open_attachment(c as usize - '1' as usize);
                if self.mode == Mode::Attachments {
                    self.mode = Mode::Thread;
                }
            }
            _ => self.mode = Mode::Thread,
        }
    }

    /// Write attachment `n` of the open thread to a temp dir and `open` it.
    pub fn open_attachment(&mut self, n: usize) {
        let Some(open) = &self.open else { return };
        let Some(&(mi, ai)) = open.attachments().get(n) else {
            return;
        };
        let m = &open.msgs[mi];
        let a = &m.attachments[ai];
        let bytes = view::attachment_bytes(&self.db, m, a);
        let Some(bytes) = bytes else {
            self.message = Some((format!("{} isn't available locally yet", a.name), true));
            return;
        };
        let dir = std::env::temp_dir().join("keryx-attachments").join(&m.source_id);
        // Only the file name: never a path the sender chose.
        let name = std::path::Path::new(&a.name)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "attachment".into());
        let path = dir.join(&name);
        if std::fs::create_dir_all(&dir)
            .and_then(|_| std::fs::write(&path, &bytes))
            .is_err()
        {
            self.message = Some(("couldn't save the attachment".into(), true));
            return;
        }
        if !self.dry_open {
            let _ = std::process::Command::new("open").arg(&path).spawn();
        }
        self.message = Some((format!("opened {name}"), false));
        self.opened.push(path);
    }

    fn goto_key(&mut self, k: Key) {
        self.mode = Mode::List;
        if k == Key::Char('c') {
            self.mode = Mode::Runs { sel: 0 };
            return;
        }
        let v = match k {
            Key::Char('i') => View::Inbox,
            Key::Char('f') => View::Flagged,
            Key::Char('t') => View::Sent,
            Key::Char('r') => View::NeedsReply,
            Key::Char('s') => View::Snoozed,
            Key::Char('v') => View::Archive,
            Key::Char('a') => View::All,
            Key::Char(c) if c.is_ascii_digit() && c != '0' => match self.cfg.keys.get(c as usize - '1' as usize) {
                Some(k) => View::Key(k.name.clone()),
                None => return,
            },
            _ => return,
        };
        self.view = v;
        self.search = None;
        self.sel = 0;
        self.reload();
    }

    fn search_key(&mut self, k: Key, mut q: String) {
        match k {
            Key::Esc => {
                self.mode = Mode::List;
                self.search = None;
                self.reload();
                return;
            }
            Key::Enter => {
                self.mode = Mode::List;
                self.search = (!q.trim().is_empty()).then_some(q);
                self.reload();
                return;
            }
            Key::Backspace => {
                q.pop();
            }
            Key::Char(c) => q.push(c),
            Key::Down | Key::Ctrl('n') => self.move_sel(1),
            Key::Up | Key::Ctrl('p') => self.move_sel(-1),
            _ => {}
        }
        // Live results; an incomplete query (unclosed quote, "from:") keeps
        // the previous results and shows the problem.
        let parsed = keryx_core::query::parse(&q, self.now);
        match parsed {
            Ok(p) if p.is_empty() => {
                self.search = None;
                self.reload();
            }
            Ok(_) => {
                self.search = Some(q.clone());
                self.sel = 0;
                self.reload();
            }
            Err(e) => self.search_error = Some(e.to_string()),
        }
        self.mode = Mode::Search(q);
    }

    /// Hints for the bottom-right edge.
    pub fn key_hints(&self) -> String {
        match &self.mode {
            Mode::Thread if self.back.is_some() => "z quotes · Esc back to the review".into(),
            Mode::Thread => {
                let mut h = vec![];
                if self
                    .open
                    .as_ref()
                    .is_some_and(|o| self.suggestion_for(&o.account, &o.thread_id).is_some())
                {
                    h.extend(["y create", "n dismiss"]);
                }
                h.extend(["r reply", "d archive", "t task", "z quotes"]);
                if self.open.as_ref().is_some_and(|o| !o.attachments().is_empty()) {
                    h.push("o attachment");
                }
                h.extend(["Esc back", "? help"]);
                h.join(" · ")
            }
            Mode::List if self.rows.is_empty() => "g go to · / search · ? help".into(),
            Mode::List if !self.marked.is_empty() => {
                format!(
                    "{} marked · d archive · s snooze · m file · f flag · Esc unmark",
                    self.marked.len()
                )
            }
            Mode::List => "d archive · s snooze · m file · r reply · x mark · ? help".into(),
            Mode::Review { .. } => {
                "y accept · n decline · c change · x mark · Y accept group · Enter read · u undo".into()
            }
            Mode::Search(_) => "Enter keep · Esc clear · ↑↓ move".into(),
            _ => String::new(),
        }
    }
}
