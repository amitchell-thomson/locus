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
    /// ⌥⇥: add the next inbox to the ones shown.
    AltTab,
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
    /// `d` / `m`: what the thread is for (X7), the guess chosen.
    File(Box<crate::file_for::ForPicker>),
    /// Alec's composed message: `y` sends (after the hold), `e` edits, `n` drops.
    ConfirmSend(Box<crate::send::SendDraft>),
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
    /// `g` → `l`: what keryx filed by itself as it arrived (X-d).
    Filed {
        sel: usize,
    },
    /// `g` → `d`: the drafts you kept.
    Drafts {
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
    /// Where Esc returns from a thread opened elsewhere (a run's detail).
    pub back: Option<Mode>,
    /// Open proposals waiting for Alec (status bar).
    pub open_proposals: usize,
    /// Drafts waiting for Alec's approval (status bar).
    pub pending_sends: usize,
    /// The run loop opens `$EDITOR` for this, then calls `editor_done`.
    pub editor_request: Option<crate::send::EditorRequest>,
    /// Writing a mail in the right pane (the run loop owns the editor itself).
    pub writing: Option<crate::send::Writing>,
    /// locus's taxonomy and registry: what mail can be filed for (X7). `None`: `d` archives.
    pub shared: Option<locus_contract::taxonomy::Shared>,
    /// The highlighted row of the attachment list (`o`).
    pub att_sel: usize,
    /// The worker that takes changes to mail off the screen's hands (the real TUI); `None`
    /// runs them in place (tests).
    pub background: Option<std::sync::mpsc::Sender<crate::write::Job>>,
    /// Threads a change in the worker takes out of the list: gone at once, until it answers.
    pub hidden: BTreeSet<String>,
    /// The last move, for an instant `u`: (threads, whether it took them out of the list).
    pub last_move: Option<(Vec<String>, bool, Option<String>)>,
    /// Threads a change in the worker puts back in the inbox: there at once, until it answers.
    pub shown: BTreeSet<String>,
    /// A send in its hold: (changeset id, the draft kept until it has gone).
    pub sending: Option<(String, Option<PathBuf>)>,
    /// Inboxes added to the highlighted one with ⌥⇥.
    pub also: BTreeSet<usize>,
    /// Threads opened in this session: shown as read whatever the mirror says yet.
    pub read_here: BTreeSet<String>,
    /// What each thread is marked for (`m`): (account, thread) → (kind, target).
    pub marks: keryx_store::state::Marks,
    /// Where `:q` keeps a changed mail (set by the running TUI; `None` in tests unless set).
    pub drafts_dir: Option<PathBuf>,
    /// A send in its hold: (changeset id, sends at).
    pub hold: Option<(String, i64)>,
    /// `b`: the list hidden, the message taking the width.
    pub list_hidden: bool,
    /// Mail newer than this (Unix seconds) is new since you last looked: when you last pressed
    /// a key before a break of half an hour or more (kept across restarts).
    pub seen_since: Option<i64>,
    /// When you last pressed a key (Unix seconds).
    pub last_key: i64,
    /// Where `last_key` is kept: set only by [`App::load_seen`], so an App made in a test never
    /// writes your real file (a fixture "now" once moved your marker back twelve days).
    pub seen_file: Option<PathBuf>,
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
            back: None,
            open_proposals: 0,
            pending_sends: 0,
            editor_request: None,
            writing: None,
            drafts_dir: None,
            att_sel: 0,
            also: BTreeSet::new(),
            sending: None,
            background: None,
            hidden: BTreeSet::new(),
            shown: BTreeSet::new(),
            last_move: None,
            read_here: BTreeSet::new(),
            marks: std::collections::HashMap::new(),
            shared: None,
            hold: None,
            task_status: std::collections::HashMap::new(),
            list_hidden: false,
            seen_since: None,
            last_key: 0,
            seen_file: None,
        };
        app.reload();
        app
    }

    /// Whether the list opens on a needs-reply section: the inbox, not a search or another list.
    pub fn reply_first(&self) -> bool {
        self.view == View::Inbox && self.search.is_none()
    }

    /// How one of your addresses is shown: its account's shown address (Oxford: your college
    /// address, never the SSO Mail sends with), else the address itself.
    pub fn my_address(&self, email: &str) -> String {
        let key = email.to_lowercase();
        self.cfg
            .accounts
            .iter()
            .find(|a| a.addresses().any(|x| x == key))
            .map_or_else(
                || email.to_string(),
                |a| a.shown_address.clone().unwrap_or_else(|| a.address.clone()),
            )
    }

    /// The worker finished a change: the threads it hid come back if it failed (reload shows
    /// them where they really are), and it says how it went.
    /// The worker's answer for a move: the change it applied, so `u` undoes exactly that one.
    pub fn move_applied(&mut self, touched: &[String], cs: Option<String>) {
        if let Some((refs, _, slot)) = &mut self.last_move
            && refs.as_slice() == touched
            && cs.is_some()
        {
            *slot = cs;
        }
    }

    pub fn background_done(&mut self, hid: Vec<String>, res: Result<Option<String>, String>) {
        for r in &hid {
            self.hidden.remove(r);
            self.shown.remove(r);
        }
        match res {
            Ok(Some(m)) => self.message = Some((m, false)),
            Ok(None) => {}
            Err(e) => self.message = Some((e, true)),
        }
        self.reload();
    }

    /// Whether a row reads as unread: unread in the mirror and not opened here since.
    pub fn is_unread(&self, r: &ThreadRow) -> bool {
        r.unread && !self.read_here.contains(&r.r#ref)
    }

    /// Threads in the list that came since you last looked.
    pub fn new_count(&self) -> usize {
        match self.seen_since {
            Some(s) => self.rows.iter().filter(|r| r.date > s).count(),
            None => 0,
        }
    }

    /// Where the time you last looked is kept (locus's state directory).
    fn seen_path() -> Option<PathBuf> {
        locus_contract::link::state_dir().map(|d| d.join("keryx-seen"))
    }

    /// Read when you last looked (the TUI's start).
    pub fn load_seen(&mut self) {
        self.seen_file = Self::seen_path();
        if let Some(t) = self
            .seen_file
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| s.trim().parse::<i64>().ok())
        {
            self.seen_since = Some(t);
            self.last_key = t;
        }
    }

    /// A key: after a break of 30 minutes or more, what you last saw moves up to then.
    fn note_key(&mut self) {
        let now = self.now.timestamp();
        if self.last_key > 0 && now - self.last_key >= 30 * 60 {
            self.seen_since = Some(self.last_key);
        }
        let write = now - self.last_key >= 60;
        self.last_key = now;
        if write && let Some(p) = &self.seen_file {
            let _ = std::fs::create_dir_all(p.parent().unwrap_or(std::path::Path::new(".")));
            let _ = std::fs::write(p, now.to_string());
        }
    }

    pub fn split(&self) -> bool {
        self.width >= SPLIT_WIDTH
    }

    /// The inboxes shown: the highlighted one and any added with ⌥⇥ (1 + index into
    /// `cfg.accounts`); empty means all of them.
    pub fn shown_accounts(&self) -> Vec<usize> {
        if self.account == 0 {
            return vec![];
        }
        let mut v: Vec<usize> = self.also.iter().copied().collect();
        v.push(self.account);
        v.sort_unstable();
        v.dedup();
        v
    }

    pub fn filter(&self) -> Filter {
        Filter {
            accounts: self
                .shown_accounts()
                .into_iter()
                .filter_map(|i| self.cfg.accounts.get(i - 1).map(|a| a.id.clone()))
                .collect(),
            limit: LIST_LIMIT,
        }
    }

    pub fn account_label(&self) -> String {
        match self.account {
            0 => "All accounts".into(),
            _ => self
                .shown_accounts()
                .into_iter()
                .filter_map(|i| self.cfg.accounts.get(i - 1).map(|a| a.name.clone()))
                .collect::<Vec<_>>()
                .join(" + "),
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
            Ok(mut rows) => {
                rows.retain(|r| !self.hidden.contains(&r.r#ref));
                // Put back in the inbox: listed at once (round 9), however long Mail takes.
                if self.view == View::Inbox && self.search.is_none() {
                    for r in &self.shown {
                        if rows.iter().any(|x| &x.r#ref == r) {
                            continue;
                        }
                        if let Some((a, t)) = keryx_core::model::parse_thread_ref(r)
                            && let Some(mut row) = view::thread_row(&self.db, &self.cfg, a, t)
                        {
                            row.in_inbox = true;
                            rows.push(row);
                        }
                    }
                    rows.sort_by(|a, b| b.date.cmp(&a.date));
                }
                // The inbox opens on what needs a reply, as a section of its own, then the days
                // (round 4 and 5): it is what you act on. Other lists are by date alone.
                if self.reply_first() {
                    rows.sort_by(|a, b| {
                        b.needs_reply
                            .is_some()
                            .cmp(&a.needs_reply.is_some())
                            .then(b.date.cmp(&a.date))
                    });
                }
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
        self.marks = self.db.fors().unwrap_or_default();
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
        let msgs = view::distinct(self.db.thread(account, thread_id).ok()?);
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
        let target = self.selected().map(|r| (r.account.clone(), r.thread_id.clone()));
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
        self.note_key();
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
            Mode::File(p) => self.for_key(k, *p),
            Mode::ConfirmSend(d) => self.confirm_send_key(k, *d),
            Mode::ConfirmUnsubscribe { r#ref, .. } => self.confirm_unsubscribe_key(k, r#ref),
            Mode::KairosForm(f) => self.form_key(k, *f),
            Mode::Runs { sel } => self.runs_key(k, sel),
            Mode::RunDetail { id, sel } => self.run_detail_key(k, id, sel),
            Mode::Filed { sel } => self.filed_key(k, sel),
            Mode::Drafts { sel } => self.drafts_key(k, sel),
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
            Key::AltTab => self.extend_account(),
            Key::BackTab => self.cycle_account(-1),
            Key::Char('g') => self.mode = Mode::GoTo,
            Key::Char('/') => self.mode = Mode::Search(self.search.clone().unwrap_or_default()),
            Key::Char('?') => self.mode = Mode::Help,
            Key::Char('b') => self.list_hidden = !self.list_hidden,
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
            // Opening a thread marks it read (Alec, 2026-10-14). The row stops showing unread at
            // once (round 6), whether or not the change reaches the mailbox (live = false).
            self.read_here.insert(r.r#ref.clone());
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
        self.also.clear();
        self.sel = 0;
        self.reload();
    }

    /// ⌥⇥ (round 7): keep the inboxes shown and add the next one (personal + oxford, without
    /// OQTS). From *all* it starts at the first; once every inbox is in, it goes back to one.
    fn extend_account(&mut self) {
        let n = self.cfg.accounts.len();
        if n == 0 {
            return;
        }
        if self.account == 0 {
            self.account = 1;
        } else if self.also.len() + 1 >= n {
            self.also.clear();
        } else {
            self.also.insert(self.account);
            let mut next = self.account % n + 1;
            while self.also.contains(&next) {
                next = next % n + 1;
            }
            self.account = next;
        }
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
            Key::Char('b') => self.list_hidden = !self.list_hidden,
            Key::Char('z') => {
                open.show_quotes = !open.show_quotes;
                open.expand_all = open.show_quotes;
            }
            Key::Char('o') => match open.attachments().len() {
                0 => self.message = Some(("no attachments in this thread".into(), false)),
                1 => self.open_attachment(0),
                _ => {
                    self.att_sel = 0;
                    self.mode = Mode::Attachments;
                }
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

    /// The attachment list (`o`): arrows (or `j` `k`) move, `⏎` opens the highlighted one, a
    /// digit opens that one; the list stays open so you can check several. `esc` goes back.
    fn attachment_key(&mut self, k: Key) {
        let n = self.open.as_ref().map_or(0, |o| o.attachments().len());
        match k {
            Key::Down | Key::Char('j') | Key::Tab => self.att_sel = (self.att_sel + 1).min(n.saturating_sub(1)),
            Key::Up | Key::Char('k') | Key::BackTab => self.att_sel = self.att_sel.saturating_sub(1),
            Key::Enter => self.open_attachment(self.att_sel),
            Key::Char(c) if c.is_ascii_digit() && c != '0' && (c as usize - '1' as usize) < n => {
                self.att_sel = c as usize - '1' as usize;
                self.open_attachment(self.att_sel);
            }
            Key::Esc | Key::Char('q') | Key::Char('o') => self.mode = Mode::Thread,
            _ => {}
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
            // Mail has not downloaded it (27 of 2,820 on the real store, 26 Oct): open the
            // message in Mail, where it downloads, rather than stop at an error.
            let id = m.message_id.trim_matches(|c| c == '<' || c == '>');
            let url = format!("message://%3c{}%3e", id.replace('%', "%25"));
            if !self.dry_open {
                let _ = std::process::Command::new("open").arg(&url).spawn();
            }
            self.message = Some((
                format!("Mail hasn't downloaded {} yet: opened the message in Mail", a.name),
                false,
            ));
            return;
        };
        let dir = std::env::temp_dir().join("keryx-attachments").join(&m.source_id);
        // Only the file name: never a path the sender chose.
        let name = std::path::Path::new(&a.name)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "attachment".into());
        let path = dir.join(&name);
        // Written beside and renamed into place, so a viewer (or a second `o`) never reads a
        // half-written file.
        let part = dir.join(format!(".{name}.{}", uuid::Uuid::new_v4().simple()));
        if std::fs::create_dir_all(&dir)
            .and_then(|_| std::fs::write(&part, &bytes))
            .and_then(|_| std::fs::rename(&part, &path))
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

    /// What `g` then a digit opens: the taxonomy's folders (X7), with their colours; your
    /// config's keys when there is no taxonomy. Older keys are a search away (`key:Admin`).
    pub fn goto_folders(&self) -> Vec<(String, ratatui::style::Color)> {
        match &self.shared {
            Some(s) => locus_contract::fors::folders(&s.taxonomy)
                .into_iter()
                .take(9)
                .map(|f| {
                    let col = s
                        .taxonomy
                        .colour_of(&f)
                        .map_or(crate::theme::c("text"), crate::theme::c);
                    (f, col)
                })
                .collect(),
            None => self
                .cfg
                .keys
                .iter()
                .take(9)
                .map(|k| (k.name.clone(), crate::theme::rgb(k.colour)))
                .collect(),
        }
    }

    fn goto_key(&mut self, k: Key) {
        self.mode = Mode::List;
        if k == Key::Char('c') {
            self.mode = Mode::Runs { sel: 0 };
            return;
        }
        if k == Key::Char('d') {
            self.mode = Mode::Drafts { sel: 0 };
            return;
        }
        if k == Key::Char('l') {
            self.mode = Mode::Filed { sel: 0 };
            return;
        }
        let v = match k {
            Key::Char('i') => View::Inbox,
            Key::Char('s') => View::Sent,
            Key::Char('r') => View::NeedsReply,
            Key::Char('z') if self.cfg.snooze.enabled => View::Snoozed,
            Key::Char('v') => View::Archive,
            Key::Char('a') => View::All,
            Key::Char(c) if c.is_ascii_digit() && c != '0' => {
                match self.goto_folders().get(c as usize - '1' as usize) {
                    Some((name, _)) => View::Key(name.clone()),
                    None => return,
                }
            }
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
                    "{} marked · d archive · m file · f flag · Esc unmark",
                    self.marked.len()
                )
            }
            Mode::List => "d archive · m file · r reply · t task · x mark · ? help".into(),
            Mode::Search(_) => "Enter keep · Esc clear · ↑↓ move".into(),
            _ => String::new(),
        }
    }
}
