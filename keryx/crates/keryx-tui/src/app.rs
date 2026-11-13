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
use locus_theme::keys::Action;
use std::collections::{BTreeMap, BTreeSet};
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
    /// ⇧↑ (or Home): the top of the list or thread.
    Top,
    /// ⇧↓ (or End): the bottom.
    Bottom,
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

/// A thread opened for reading, read like a document (cleanup C6, decision D4, 13 Nov 2026).
///
/// There is ONE scroll and no separate selection. Before this, ↑↓ moved a selection (a tint on
/// one header line) while space scrolled, so the tint scrolled out of sight and `⏎`, `z` and `r`
/// acted on a message you could not see. Now the message under the top line of the view is the
/// one they act on ([`Open::target`]), and the screen marks it with a bar down its whole left
/// edge. `]` `[` jump between messages; near the end of a thread, where the last messages can
/// never reach the top line, the jump keeps the bar on the message it went to (`focus`).
#[derive(Debug, Clone)]
pub struct Open {
    pub account: String,
    pub thread_id: String,
    pub msgs: Vec<Message>,
    /// Parsed invites, per message (same order as `msgs`).
    pub invites: Vec<Option<Invite>>,
    /// Messages (indices into `msgs`) open when the thread opened: the newest new one and up to
    /// two more new ones, or the newest when nothing is new. Everything else folds to one line.
    pub first_open: std::collections::HashSet<usize>,
    /// Messages whose fold ⏎ flipped from how they started.
    pub toggled: std::collections::HashSet<usize>,
    /// Messages shown exactly as received (`z`), quoted history and all.
    pub raw: std::collections::HashSet<usize>,
    /// When you last read this thread here (the "new since" mark, spec keryx-thread-view T3).
    pub last_read: Option<i64>,
    /// The first body line shown (the header stays above it).
    pub scroll: std::cell::Cell<u16>,
    /// Where the view is, as last drawn or scrolled: (a message, lines below its lead-in). Each
    /// draw puts that line back at the top, so the view holds while the thread's lines move
    /// under it (new mail above, a draft added, a resize rewrapping every message). None above
    /// the first message (your sends and drafts).
    pub anchor: std::cell::Cell<Option<(usize, usize)>>,
    /// Put this message (index into `msgs`) at the top on the next draw: on opening, `]` `[`,
    /// `⏎` and `z`, so the message you act on stays where you are looking.
    pub jump: std::cell::Cell<Option<usize>>,
    /// The message a jump went to when it could not reach the top line (the end of the thread):
    /// the bar stays on it until you scroll.
    pub focus: std::cell::Cell<Option<usize>>,
    /// As last drawn: each message's block, in the order shown, as (index into `msgs`, the line
    /// its lead-in starts on (day band or blank), its first line, the line after its last).
    pub blocks: std::cell::RefCell<Vec<Block>>,
    /// The body pane's height and how far it can scroll, as last drawn.
    pub height: std::cell::Cell<u16>,
    pub max_scroll: std::cell::Cell<u16>,
    /// Messages that were on screen while you read the thread: "read" is stamped for these, and
    /// only these, when you leave it (D4: it was stamped on opening, so opening a thread and
    /// moving on lost its new messages).
    pub seen: std::cell::RefCell<BTreeSet<usize>>,
}

/// The anchor for a view whose top line is `top`: the last message whose lead-in starts at or
/// above it, and how far below. None above the first message.
pub fn anchor_at(blocks: &[Block], top: usize) -> Option<(usize, usize)> {
    blocks
        .iter()
        .rev()
        .find(|b| b.lead <= top)
        .map(|b| (b.msg, top - b.lead))
}

/// The top line an anchor stands for in freshly drawn `blocks`, kept inside its message.
pub fn anchor_line(blocks: &[Block], (msg, off): (usize, usize)) -> Option<usize> {
    let b = blocks.iter().find(|b| b.msg == msg)?;
    Some(b.lead + off.min(b.end.saturating_sub(b.lead + 1)))
}

/// Where one message sits in the drawn thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Block {
    pub msg: usize,
    pub lead: usize,
    pub start: usize,
    pub end: usize,
}

impl Open {
    /// Message indices in the order shown: newest first (11 Nov: "most recent should always be
    /// at the top").
    pub fn order(&self) -> Vec<usize> {
        let mut v: Vec<usize> = (0..self.msgs.len()).collect();
        v.sort_by_key(|&i| std::cmp::Reverse(self.msgs[i].date));
        v
    }

    /// The message `⏎`, `z`, `r` and the link digits act on: the one under the top line of the
    /// view (the first whose block ends below it), or the one a jump went to.
    pub fn target(&self) -> Option<usize> {
        if let Some(i) = self.jump.get().or(self.focus.get()) {
            return Some(i);
        }
        let top = usize::from(self.scroll.get());
        let blocks = self.blocks.borrow();
        if blocks.is_empty() {
            // Not drawn yet: the message the thread opens on.
            return self.first_target();
        }
        blocks.iter().find(|b| b.end > top).or(blocks.last()).map(|b| b.msg)
    }

    /// The message the thread opens on: the newest of those open at first, else the newest.
    pub fn first_target(&self) -> Option<usize> {
        let order = self.order();
        order
            .iter()
            .copied()
            .find(|i| self.first_open.contains(i))
            .or(order.first().copied())
    }

    /// Whether message `i` is shown whole (else folded to one line).
    pub fn is_open(&self, i: usize) -> bool {
        self.first_open.contains(&i) != self.toggled.contains(&i) || self.raw.contains(&i)
    }

    /// Scroll by `d` lines; the bar goes back to following the top line.
    pub fn scroll_by(&mut self, d: i32) {
        self.jump.set(None);
        self.focus.set(None);
        let to = (i32::from(self.scroll.get()) + d).clamp(0, i32::from(self.max_scroll.get()));
        self.scroll.set(to as u16);
        self.anchor.set(anchor_at(&self.blocks.borrow(), to as usize));
    }

    /// A page: the pane's height less two lines of overlap.
    pub fn page(&self) -> i32 {
        i32::from(self.height.get()).saturating_sub(2).max(1)
    }

    /// `]` / `[`: the next (older) or previous (newer) message to the top.
    pub fn step(&mut self, d: isize) {
        let order = self.order();
        let Some(at) = self.target().and_then(|t| order.iter().position(|&i| i == t)) else {
            return;
        };
        let to = (at as isize + d).clamp(0, order.len() as isize - 1) as usize;
        self.focus.set(None);
        self.jump.set(Some(order[to]));
    }

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
    /// Threads marked with `v` for a batch.
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
    /// Where `?` came from: help closes back to it (a thread stays open).
    pub help_back: Option<Mode>,
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
    /// When each thread was last opened here (`thread_read`); None in tests.
    pub thread_read_file: Option<PathBuf>,
    /// `x` on a kept draft: the one the next `x` throws away.
    /// The draft `x` asked about: `file:<path>` (yours) or `claude:<changeset>`.
    pub drop_draft: Option<String>,
    /// Threads taken out of the inbox here, and when: one that comes back with nothing newer
    /// in it was put back by Mail, and the list says so (round 11).
    pub gone: BTreeMap<String, i64>,
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
    /// The worker that asks keryxd for tasks' live state (the real TUI); `None` asks in place
    /// (tests). Opening a thread asked on the screen's own time, and keryxd answers one request
    /// at a time: behind an archive (10 s of Mail) the screen froze (31 Oct).
    pub status_worker: Option<std::sync::mpsc::Sender<Vec<String>>>,
    /// Tasks already asked about, so reopening a thread does not ask again while it waits.
    pub status_asked: BTreeSet<String>,
    /// How far the keys overlay is scrolled.
    pub help_scroll: u16,
    /// `x` asked about these threads: the next `x` archives them.
    pub remove_asked: Option<Vec<String>>,
    /// Your sends from this session, shown in their thread until the mirror has them (D4): in
    /// the hold, sent, cancelled, or not sent and why (kept until you deal with it).
    pub outbox: Vec<crate::send::Outbox>,
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
            thread_read_file: None,
            drop_draft: None,
            gone: BTreeMap::new(),
            att_sel: 0,
            help_back: None,
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
            status_worker: None,
            status_asked: BTreeSet::new(),
            remove_asked: None,
            help_scroll: 0,
            outbox: Vec::new(),
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
            if res.is_err() {
                // Said already, as an error: nothing to notice later.
                self.gone.remove(r);
            }
        }
        match res {
            Ok(Some(m)) => self.message = Some((m, false)),
            Ok(None) => {}
            Err(e) => self.message = Some((e, true)),
        }
        self.reload();
    }

    fn open_help(&mut self) {
        self.help_scroll = 0;
        self.help_back = Some(self.mode.clone());
        self.mode = Mode::Help;
    }

    /// `b`: hide or show the list; there is a list to hide only beside a thread (wide window).
    fn toggle_list(&mut self) {
        if self.split() {
            self.list_hidden = !self.list_hidden;
        } else {
            self.message = Some(("the list hides beside a thread: widen the window".into(), false));
        }
    }

    /// Digit `n` in a thread: open link n of the message under the bar.
    pub fn open_link(&mut self, n: usize) {
        let Some(m) = self
            .open
            .as_ref()
            .and_then(|o| o.target().and_then(|i| o.msgs.get(i)).or(o.msgs.last()))
        else {
            return;
        };
        let r = keryx_core::text::readable(&m.body_text, false);
        match r.links.iter().find(|(k, _, _)| *k == n) {
            Some((_, url, site)) => {
                if !self.dry_open {
                    let _ = std::process::Command::new("open").arg(url).spawn();
                }
                self.message = Some((format!("opened {site}"), false));
                self.opened.push(PathBuf::from(url));
            }
            None => self.message = Some((format!("no link {n} here"), false)),
        }
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

    /// The "<ref> <unix time>" lines of `keryx-thread-read`, in locus's state directory: when you
    /// last read each thread here. Local, and losing it only loses the "new since" mark.
    fn thread_reads(&self) -> std::collections::BTreeMap<String, i64> {
        let Some(path) = &self.thread_read_file else {
            return Default::default();
        };
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.rsplit_once(' '))
            .filter_map(|(k, v)| Some((k.to_string(), v.trim().parse().ok()?)))
            .collect()
    }

    /// When you last read this thread here.
    pub fn thread_read_at(&self, r#ref: &str) -> Option<i64> {
        self.thread_reads().get(r#ref).copied()
    }

    /// Record that you read this thread up to `t` (never moves the mark back).
    fn stamp_thread_read(&mut self, r#ref: &str, t: i64) {
        let Some(path) = self.thread_read_file.clone() else {
            return;
        };
        let mut map = self.thread_reads();
        if map.get(r#ref).is_some_and(|&was| was >= t) {
            return;
        }
        map.insert(r#ref.to_string(), t);
        let body: String = map.iter().map(|(k, v)| format!("{k} {v}\n")).collect();
        let _ = std::fs::write(&path, body);
    }

    /// Leave the open thread, stamping "read" for what was on screen (D4). Its new messages that
    /// never reached the screen stay new: the local mark moves only up to just before the
    /// oldest of them, and the mailbox's read flag is set only when none is left. Returns how many
    /// new messages you did not see.
    pub fn leave_thread(&mut self) -> usize {
        let Some(o) = self.open.take() else { return 0 };
        let r = keryx_core::model::thread_ref(&o.account, &o.thread_id);
        let seen = o.seen.borrow().clone();
        let unseen: Vec<i64> = (0..o.msgs.len())
            .filter(|&i| self.is_new(&o, i) && !seen.contains(&i))
            .map(|i| o.msgs[i].date)
            .collect();
        let up_to = unseen.iter().min().map_or(self.now.timestamp(), |t| t - 1);
        self.stamp_thread_read(&r, up_to);
        if unseen.is_empty() {
            self.read_here.insert(r.clone());
            let unread = o
                .msgs
                .iter()
                .any(|m| m.account == o.account && m.thread_id == o.thread_id && !m.read);
            if unread {
                self.deferred.push(Job::Triage {
                    req: keryx_store::api::TriageReq {
                        intent: keryx_store::api::Intent::Read { on: true },
                        refs: vec![r],
                        summary: None,
                        review_id: None,
                    },
                    quiet: true,
                });
            }
        }
        unseen.len()
    }

    /// Back out of a thread to where it was opened from.
    pub fn close_thread(&mut self) {
        let unseen = self.leave_thread();
        if unseen > 0 {
            self.message = Some((
                format!(
                    "{unseen} new message{} you did not reach stay unread",
                    if unseen == 1 { "" } else { "s" }
                ),
                false,
            ));
        }
        self.mode = self.back.take().unwrap_or(Mode::List);
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
            accounts: if self.account == 0 {
                self.cfg.default_accounts()
            } else {
                self.shown_accounts()
                    .into_iter()
                    .filter_map(|i| self.cfg.accounts.get(i - 1).map(|a| a.id.clone()))
                    .collect()
            },
            limit: LIST_LIMIT,
        }
    }

    pub fn account_label(&self) -> String {
        match self.account {
            0 => self.cfg.default_label(),
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
        // A thread that left the list (archived, filed) leaves the cursor where it was, on the
        // one that moved up into its place: back at the top, you lost your place (round 11).
        self.note_put_back();
        self.sel = keep
            .and_then(|r| self.rows.iter().position(|x| x.r#ref == r))
            .unwrap_or(self.sel)
            .min(self.rows.len().saturating_sub(1));
        self.stale = self.stale_accounts();
        self.open_proposals = keryx_store::state::open_proposals(&self.db).unwrap_or(0);
        self.pending_sends = self.pending_sends().len();
        // Marks on threads that left the list are dropped.
        let refs: BTreeSet<String> = self.rows.iter().map(|r| r.r#ref.clone()).collect();
        self.marked.retain(|r| refs.contains(r));
        self.marks = self.db.fors().unwrap_or_default();
        self.preview = None;
        self.refresh_open();
    }

    /// A thread archived here that is back in the inbox with nothing newer in it: Mail accepted
    /// the change and then took the server's word over it. On Personal (Gmail through Mail) the
    /// Inbox label came back on most archives of 27 Oct, under a new copy of the message, while
    /// keryx had said "done" (round 11). A thread that is back because a reply came is not this.
    fn note_put_back(&mut self) {
        let now = self.now.timestamp();
        self.gone.retain(|_, t| now - *t < 600);
        if self.view != View::Inbox || self.search.is_some() {
            return;
        }
        let back: Vec<(String, String, String)> = self
            .rows
            .iter()
            // A filed thread that needs a reply stays listed by design (round 10).
            .filter(|r| r.needs_reply.is_none() && self.gone.get(&r.r#ref).is_some_and(|t| r.date <= *t))
            .map(|r| (r.r#ref.clone(), r.subject.clone(), r.account.clone()))
            .collect();
        let Some((_, subject, account)) = back.first() else {
            return;
        };
        let name = self.cfg.account(account).map_or(account.as_str(), |a| a.name.as_str());
        let more = match back.len() {
            1 => String::new(),
            n => format!(" and {} more", n - 1),
        };
        self.message = Some((
            format!(
                "Mail put “{}”{more} back in the inbox: {name}'s server kept it there. a tries again",
                locus_theme::fit(subject, 40)
            ),
            true,
        ));
        for (r, _, _) in back {
            self.gone.remove(&r);
        }
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

    /// A thread's messages, oldest first, with its copies in your other accounts.
    fn thread_messages(&self, account: &str, thread_id: &str) -> Option<Vec<Message>> {
        let own = self.db.thread(account, thread_id).ok()?;
        if own.is_empty() {
            return None;
        }
        // The same conversation in his other accounts (11 Nov: the OxFORD enquiry opened from
        // Oxford showed only the copy he was cc'd on; its history was in OQTS shared). One round
        // out from this thread's ids, each copy once.
        let ids: Vec<String> = own
            .iter()
            .flat_map(|m| {
                std::iter::once(m.message_id.clone())
                    .chain(m.in_reply_to.clone())
                    .chain(m.references.clone())
            })
            .collect();
        let mut all = own.clone();
        for (a, t) in self.db.threads_with_ids(&ids).unwrap_or_default() {
            if a == account && t == thread_id {
                continue;
            }
            all.extend(self.db.thread(&a, &t).unwrap_or_default());
        }
        // A copy in the thread's own account wins, so read state and refs are this thread's.
        all.sort_by_key(|m| (m.date, m.account != account));
        Some(view::distinct(all))
    }

    pub fn load_thread(&self, account: &str, thread_id: &str) -> Option<Open> {
        let msgs = self.thread_messages(account, thread_id)?;
        let invites = msgs.iter().map(|m| self.invite_of(m)).collect();
        let mut o = Open {
            account: account.to_string(),
            thread_id: thread_id.to_string(),
            msgs,
            invites,
            first_open: Default::default(),
            toggled: Default::default(),
            raw: Default::default(),
            last_read: None,
            scroll: std::cell::Cell::new(0),
            anchor: std::cell::Cell::new(None),
            jump: std::cell::Cell::new(None),
            focus: std::cell::Cell::new(None),
            blocks: Default::default(),
            height: std::cell::Cell::new(0),
            max_scroll: std::cell::Cell::new(u16::MAX),
            seen: Default::default(),
        };
        o.last_read = self.thread_read_at(&keryx_core::model::thread_ref(account, thread_id));
        self.settle_open(&mut o);
        Some(o)
    }

    /// One of your addresses wrote it.
    pub fn is_mine(&self, m: &Message) -> bool {
        m.from
            .as_ref()
            .is_some_and(|a| self.cfg.personal_addresses().contains(&a.key()))
    }

    /// New to you: theirs, and after the last time you read this thread here; with no record of
    /// that yet, unread in the mirror.
    pub fn is_new(&self, open: &Open, i: usize) -> bool {
        let m = &open.msgs[i];
        !self.is_mine(m)
            && match open.last_read {
                Some(t) => m.date > t,
                None => !m.read,
            }
    }

    /// How a thread opens (D4): the newest new message at the top, open; up to two more new ones
    /// open; everything older folded to one line. With nothing new, the newest is open.
    pub fn settle_open(&self, o: &mut Open) {
        let order = o.order();
        let new: Vec<usize> = order.iter().copied().filter(|&i| self.is_new(o, i)).take(3).collect();
        o.first_open = if new.is_empty() {
            order.first().copied().into_iter().collect()
        } else {
            new.into_iter().collect()
        };
        o.toggled.clear();
        o.jump.set(o.first_target());
    }

    /// The open thread again from the mirror, when keryxd has changed it (a new message, a copy
    /// from another account, a body Mail finished downloading), keeping your place: what is open,
    /// folded, raw or seen follows each message, and the view stays on the line you were reading
    /// ([`Open::anchor`]). A new message that is new to you opens, above the view, and the line
    /// above the footer says so. Nothing changes when the messages are the same.
    pub fn refresh_open(&mut self) {
        let Some(o) = self.open.as_ref() else { return };
        let Some(msgs) = self.thread_messages(&o.account, &o.thread_id) else {
            return;
        };
        let key = |m: &Message| {
            (
                m.account.clone(),
                m.source_id.clone(),
                m.body_text.len(),
                m.attachments.len(),
            )
        };
        if msgs.iter().map(key).eq(o.msgs.iter().map(key)) {
            return;
        }
        let id = |m: &Message| (m.account.clone(), m.source_id.clone());
        let old_ids: Vec<(String, String)> = o.msgs.iter().map(id).collect();
        let at: Vec<Option<usize>> = old_ids.iter().map(|k| msgs.iter().position(|m| &id(m) == k)).collect();
        let map = |i: usize| at.get(i).copied().flatten();
        let set = |s: &std::collections::HashSet<usize>| s.iter().filter_map(|&i| map(i)).collect();
        let invites = msgs.iter().map(|m| self.invite_of(m)).collect();
        let arrived: Vec<usize> = (0..msgs.len()).filter(|&i| !old_ids.contains(&id(&msgs[i]))).collect();
        let Some(o) = self.open.as_mut() else { return };
        o.first_open = set(&o.first_open);
        o.toggled = set(&o.toggled);
        o.raw = set(&o.raw);
        let seen: BTreeSet<usize> = o.seen.borrow().iter().filter_map(|&i| map(i)).collect();
        *o.seen.borrow_mut() = seen;
        o.jump.set(o.jump.get().and_then(map));
        o.focus.set(o.focus.get().and_then(map));
        o.anchor.set(o.anchor.get().and_then(|(i, off)| Some((map(i)?, off))));
        let blocks: Vec<Block> = o
            .blocks
            .borrow()
            .iter()
            .filter_map(|b| Some(Block { msg: map(b.msg)?, ..*b }))
            .collect();
        *o.blocks.borrow_mut() = blocks;
        o.msgs = msgs;
        o.invites = invites;
        let new: Vec<usize> = arrived.into_iter().filter(|&i| self.is_new_in(i)).collect();
        if let Some(o) = self.open.as_mut() {
            o.first_open.extend(new.iter().copied());
        }
        if !new.is_empty() && self.mode == Mode::Thread {
            let n = new.len();
            self.message = Some((
                format!(
                    "{n} new message{} in this thread, above · ⇧↑ goes to the top",
                    if n == 1 { "" } else { "s" }
                ),
                false,
            ));
        }
    }

    /// [`App::is_new`] for message `i` of the open thread.
    fn is_new_in(&self, i: usize) -> bool {
        self.open
            .as_ref()
            .is_some_and(|o| i < o.msgs.len() && self.is_new(o, i))
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
        // ^C quits from anywhere (it used to type a "q" into searches and forms).
        if k == Key::Ctrl('c') {
            self.leave_thread();
            self.quit = true;
            return;
        }
        if self.message.is_some() && !matches!(self.mode, Mode::Search(_)) {
            self.message = None;
        }
        // x asks once; any other key forgets the question.
        if k != Key::Char('x') {
            self.remove_asked = None;
        }
        match self.mode.clone() {
            // The keys: j k scroll them, any other key closes them.
            Mode::Help => match k {
                Key::Char('j') | Key::Down | Key::Char(' ') | Key::PageDown => self.help_scroll += 3,
                Key::Char('k') | Key::Up | Key::PageUp => self.help_scroll = self.help_scroll.saturating_sub(3),
                _ => self.mode = self.help_back.take().unwrap_or(Mode::List),
            },
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
        if let Some(a) = crate::keys::action(k) {
            match a {
                Action::Back => self.list_back(),
                // q quits only at the top (D1); in a search, another list or with rows marked it
                // is back, as esc.
                Action::Quit if self.at_top() => self.quit = true,
                Action::Quit => self.list_back(),
                Action::Mark => {
                    if let Some(r) = self.selected().map(|r| r.r#ref.clone()) {
                        if !self.marked.remove(&r) {
                            self.marked.insert(r);
                        }
                        self.move_sel_by(1);
                    }
                }
                a => self.common_action(a),
            }
            return;
        }
        if self.write_key(k) {
            return;
        }
        match k {
            Key::Char('j') | Key::Down => self.move_sel(1),
            Key::Char('k') | Key::Up => self.move_sel(-1),
            Key::PageDown | Key::Ctrl('d') => self.move_sel(10),
            Key::PageUp | Key::Ctrl('u') => self.move_sel(-10),
            Key::Char('G') | Key::Bottom => self.move_sel(isize::MAX / 2),
            Key::Top => self.move_sel(isize::MIN / 2),
            Key::Enter => self.open_selected(),
            Key::Tab => self.cycle_account(1),
            Key::AltTab => self.extend_account(),
            Key::BackTab => self.cycle_account(-1),
            Key::Char('b') => self.toggle_list(),
            // g then a letter: one of mail's lists (keryx's own, 13 Nov).
            Key::Char('g') => self.mode = Mode::GoTo,
            _ => {}
        }
    }

    /// The inbox, with no search and nothing marked: where q quits.
    pub fn at_top(&self) -> bool {
        self.view == View::Inbox && self.search.is_none() && self.marked.is_empty()
    }

    /// esc in the list: unmark, then clear the search, then back to the inbox.
    fn list_back(&mut self) {
        if !self.marked.is_empty() {
            self.marked.clear();
        } else if self.search.is_some() {
            self.search = None;
            self.reload();
        } else if self.view != View::Inbox {
            // From another list (sent, the archive, an area), esc is the inbox (round 11 audit).
            self.view = View::Inbox;
            self.sel = 0;
            self.reload();
        } else {
            self.message = Some(("this is the inbox · q quits".into(), false));
        }
    }

    /// A table action that means the same in the list and in a thread (D1).
    fn common_action(&mut self, a: Action) {
        match a {
            Action::Undo => self.undo(),
            Action::Edit => self.edit(),
            Action::Yes | Action::No => {
                let k = Key::Char(if a == Action::Yes { 'y' } else { 'n' });
                if !self.kairos_key(k) {
                    self.message = Some(("nothing here to say yes or no to".into(), false));
                }
            }
            Action::New => {
                self.compose_key(Key::Char('+'));
            }
            Action::Done => self.done(),
            Action::Remove => self.remove(),
            Action::Search => self.mode = Mode::Search(self.search.clone().unwrap_or_default()),
            Action::Refresh => {
                self.deferred.push(Job::Sync);
                self.message = Some(("syncing…".into(), false));
            }
            Action::Help => self.open_help(),
            Action::Mark => {
                self.message = Some(("v marks threads in the list, for one change to several".into(), false))
            }
            // Back and quit are the caller's. Any other table action (the table's "go to", while
            // it has one: keryx catches g first, `keys::action`) does nothing here. Named by a
            // wildcard so keryx builds whether or not the table still has it.
            _ => {}
        }
    }

    /// `u`: undo the last change at once; the line then says what was undone. A move you just
    /// made is put back on screen at once (round 11). A send in its hold is cancelled with `c`,
    /// never `u` (D4).
    pub fn undo(&mut self) {
        if let Some((refs, left, cs)) = self.last_move.take() {
            self.deferred.push(Job::UndoMove {
                refs,
                back_in: left,
                cs,
            });
        } else {
            self.deferred.push(Job::UndoLast);
        }
        self.message = Some(("undoing…".into(), false));
    }

    /// `x`: archive, asking once (D1: remove always asks). Nothing is ever deleted.
    pub fn remove(&mut self) {
        let refs = self.targets();
        if refs.is_empty() {
            return;
        }
        if self.remove_asked.as_ref() == Some(&refs) {
            self.remove_asked = None;
            self.triage_intent(keryx_store::api::Intent::Archive, "archiving");
            return;
        }
        let what = match refs.as_slice() {
            [one] => {
                let subject = self
                    .rows
                    .iter()
                    .find(|r| &r.r#ref == one)
                    .map(|r| r.subject.clone())
                    .or_else(|| {
                        self.open
                            .as_ref()
                            .and_then(|o| o.msgs.last().map(|m| m.subject.clone()))
                    })
                    .unwrap_or_default();
                format!("“{}”", locus_theme::fit(&subject, 40))
            }
            v => format!("the {} marked threads", v.len()),
        };
        self.message = Some((format!("archive {what}? x again archives · nothing is deleted"), false));
        self.remove_asked = Some(refs);
    }

    /// `e`: the draft of this thread, Claude's or yours, else say how to write one.
    fn edit(&mut self) {
        if !self.open_thread_draft() {
            self.message = Some(("nothing to edit here · R writes a reply, + a new mail".into(), false));
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
        self.leave_thread();
        self.open = self.load_thread(account, thread_id);
        if self.open.is_some() {
            self.mode = Mode::Thread;
            self.queue_task_status();
        }
        self.open.is_some()
    }

    /// Read the selected thread. Read is stamped when you leave it, for what was on screen
    /// ([`App::leave_thread`]); opening it marks nothing.
    fn open_selected(&mut self) {
        let Some(r) = self.selected().cloned() else { return };
        self.leave_thread();
        self.open = self.load_thread(&r.account, &r.thread_id);
        if self.open.is_some() {
            self.mode = Mode::Thread;
            self.queue_task_status();
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

    /// Reading a thread (D4): one scroll, and the message under the top line (the bar) is what
    /// `⏎`, `z`, `r` and the link digits act on.
    fn thread_key(&mut self, k: Key) {
        if self.open.is_none() {
            self.mode = Mode::List;
            return;
        }
        if let Some(a) = crate::keys::action(k) {
            match a {
                Action::Back | Action::Quit => self.close_thread(),
                Action::Help => self.open_help(),
                // A thread opened from a run's detail reads only (round 11 audit).
                _ if self.back.is_some() => {}
                a => self.common_action(a),
            }
            return;
        }
        if self.back.is_none() && self.write_key(k) {
            return;
        }
        if k == Key::Char('g') && self.back.is_none() {
            self.mode = Mode::GoTo;
            return;
        }
        let Some(open) = self.open.as_mut() else { return };
        match k {
            Key::Char('j') | Key::Down => open.scroll_by(1),
            Key::Char('k') | Key::Up => open.scroll_by(-1),
            // A page; it runs on into the next message, as a document does.
            Key::PageDown | Key::Char(' ') | Key::Ctrl('d') => {
                let p = open.page();
                open.scroll_by(p);
            }
            Key::PageUp | Key::Ctrl('u') => {
                let p = open.page();
                open.scroll_by(-p);
            }
            Key::Top => open.scroll_by(-i32::from(u16::MAX)),
            Key::Bottom => open.scroll_by(i32::from(u16::MAX)),
            Key::Char(']') => open.step(1),
            Key::Char('[') => open.step(-1),
            Key::Char('b') => self.toggle_list(),
            // The message under the bar opened or folded, and kept at the top.
            Key::Enter => {
                if let Some(i) = open.target() {
                    if !open.toggled.remove(&i) {
                        open.toggled.insert(i);
                    }
                    open.raw.remove(&i);
                    open.focus.set(None);
                    open.jump.set(Some(i));
                }
            }
            // That message exactly as received (11 Nov: z on the whole thread was unreadable);
            // again goes back to its own words.
            Key::Char('z') => {
                if let Some(i) = open.target() {
                    if !open.raw.remove(&i) {
                        open.raw.insert(i);
                    }
                    open.focus.set(None);
                    open.jump.set(Some(i));
                }
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
                self.leave_thread();
                self.move_sel(1);
                self.open_selected();
            }
            Key::Char('K') => {
                self.leave_thread();
                self.move_sel(-1);
                self.open_selected();
            }
            Key::Char(c) if c.is_ascii_digit() && c != '0' => self.open_link(c as usize - '0' as usize),
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

    /// `g` then a letter: one of mail's own lists, on the letters they had before C4 (13 Nov,
    /// Alec: "g only makes sense in mail for going to its own lists"). It no longer reaches
    /// locus's other windows; `A` is still locus's Approve.
    fn goto_key(&mut self, k: Key) {
        self.mode = Mode::List;
        let Key::Char(c) = k else { return };
        let v = match c {
            'd' => {
                self.mode = Mode::Drafts { sel: 0 };
                return;
            }
            'l' => {
                self.mode = Mode::Filed { sel: 0 };
                return;
            }
            'c' => {
                self.mode = Mode::Runs { sel: 0 };
                return;
            }
            'i' => View::Inbox,
            's' => View::Sent,
            'r' => View::NeedsReply,
            'z' if self.cfg.snooze.enabled => View::Snoozed,
            'v' => View::Archive,
            'a' => View::All,
            c if c.is_ascii_digit() && c != '0' => match self.goto_folders().get(c as usize - '1' as usize) {
                Some((name, _)) => View::Key(name.clone()),
                None => return,
            },
            _ => return,
        };
        self.leave_thread();
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
}
