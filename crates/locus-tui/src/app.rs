//! App state and keys.
//!
//! The order a key is tried in: the keys overlay (any key closes it), a mode that owns the
//! keyboard (typing to the chat, correcting a fact), then the shared key table
//! (`locus_theme::keys`, cleanup C4): the screen gives a table key its own meaning where it has
//! one (y on Approve), else it means what it means in every app. Then Tab and the arrows between
//! sections, the screen's own keys, and Home's (Enter opens, `i` the chat, `l` the log), and last
//! a section's letter, which focuses it (drawn in its corner).
//!
//! Section letters went with C4 for one evening, which left Tab and the arrows as the only way
//! round Home; Alec (13 Nov): "now navigating around home only works with tab and arrows". They
//! are back, on letters the table leaves free: system is `m` on Home (it was `y`, now yes), and
//! on the System screen recent is `h` (was `e`, now edit) and usage `b` (was `u`, now undo; `b`
//! opens the budget). A letter the table owns is refused by a test.
//!
//! Nothing leaves the process (spec 1.4 rules): an action marks its item (read, done, struck,
//! approved) and the footer says what it would have done.

use crate::fixtures::Fixtures;
use crate::screen::{ScreenId, Section};
use crate::uselog::UseLog;
use locus_theme::keys::{self, Action};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Instant;

/// What an action did to an item, drawn on it for the rest of the run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mark {
    Ticked,
    Approved,
    Rejected,
    Undone,
}

/// A mode that takes every key until it ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
    /// Typing to the chat (files-and-chat M2), from Home's bar or the chat view.
    Chat(String),
    /// Renaming the chat view's selected conversation.
    ChatName(String),
    /// `e` on a proposed fact in Approve: its value, being corrected before it is approved
    /// (heuris H2). The approve item (group, index) and the fact's id.
    FactEdit((usize, usize), u64, String),
}

pub struct App {
    pub fixtures: Fixtures,
    /// Home at the bottom, detail screens on top.
    pub stack: Vec<ScreenId>,
    /// The focused section's letter, on the top screen.
    pub focus: Option<char>,
    /// The focus each screen under the top one had when it opened the next, so Esc comes back
    /// to where you were (round 2o). Selections are kept per screen already; only the focus was
    /// lost.
    pub focus_below: Vec<Option<char>>,
    /// The footer's message: what an action would have done, or why a key did nothing.
    pub note: Option<String>,
    pub keys_open: bool,
    pub quit: bool,
    pub uselog: UseLog,
    pub last_key: Option<Instant>,
    /// The selected row, per screen and section (`' '` for a screen's one list).
    pub sel: HashMap<(ScreenId, char), usize>,
    /// Item key → what an action did to it.
    pub marks: HashMap<String, Mark>,
    pub mode: Mode,
    /// Budget: each day's bar split by job type (`t`).
    pub split: bool,
    /// The approve list's first shown line, kept between frames so it scrolls only when the
    /// cursor nears an edge.
    pub approve_scroll: std::cell::Cell<usize>,
    /// Draw count, for the pulse on things running right now.
    pub frame: u64,
    /// Running as a window of the locus session (set by locus-session.sh): only then does a key
    /// switch tmux (a project's session, kairos's window). Tests and dumps only say what they
    /// would do.
    pub in_session: bool,
    /// A key that asks first (drop a task): the item it was pressed on; the same key again on
    /// the same item does it, any other key forgets it.
    pub armed: Option<String>,
    /// kairosd, when the window runs live (spec K8): what locus puts on the calendar goes
    /// through it. `None` in tests and dumps.
    pub calendar: Option<std::sync::Arc<crate::calendar::Live>>,
    /// keryxd, when the window runs live (keryx-joins-locus X2): mail to decide in Approve.
    pub mail: Option<std::sync::Arc<crate::mail::Live>>,
    /// Mail decisions on their way to keryxd (the window; `--dump` decides in place).
    pub mail_queue: Option<crate::mail::Queue>,
    /// `e` on Approve: where else the selected filing could go.
    pub approve_pick: Option<crate::mail::Picker>,
    /// `v` on Approve: where the visual selection started (it runs to the cursor).
    pub approve_visual: Option<usize>,
    /// `/` on Approve: the list narrowed to lines with every word typed (10 Nov: "search
    /// should search and filter among approved, not be the general search").
    pub approve_filter: String,
    /// Typing the filter now (Enter keeps it, Esc clears it).
    pub approve_filtering: bool,
    /// For a section that draws only some of its items, the ones drawn, in order: the arrows
    /// walk these and Enter opens them, never an item off screen (round 2g). Written by the
    /// draw.
    pub shown: RefCell<HashMap<(ScreenId, char), Vec<usize>>>,
    /// The Home chat (files-and-chat M2). Runs nothing until it is given a state dir.
    pub chat: crate::chat::Chat,
    /// A chat message you were typing when you pressed Esc: Space brings it back.
    pub chat_draft: String,
    /// The chat view shows a new, empty conversation (`n`) rather than the selected one.
    pub chat_new: bool,
    /// The chat view's arrows move through the conversations list (←) rather than scrolling the
    /// open conversation (→, the default).
    pub chat_list: bool,
    /// Home live (home-live.md): Claude sessions in tmux the registry does not know, by tmux
    /// session (H2).
    pub unregistered: Vec<(String, Vec<locus_contract::Session>)>,
    /// The newest commit of each active repo (H3).
    pub last_commit: std::collections::BTreeMap<String, (locus_contract::Time, String)>,
    /// Why `~/files/Careers/applications.toml` could not be read, shown on careers (H5).
    pub careers_error: Option<String>,
    /// The server's system section (H7): none until the first answer, an error when the server
    /// could not be asked.
    pub system_live: Option<Result<crate::live::SystemLive, String>>,
    /// Home is reading live data: rows the fixtures held are gone, and sections not yet live
    /// say so. False in dumps of the fixtures and in tests.
    pub home_live: bool,
    /// Repositories in `~/local-projects` the registry does not name, for the projects view.
    pub other_repos: Vec<(String, Option<(locus_contract::Time, String)>)>,
    /// `^r` asked for the live data now; the window's loop wakes its fetching threads.
    pub refresh_requested: bool,
}

impl App {
    pub fn new(fixtures: Fixtures, root: ScreenId, uselog: UseLog) -> App {
        let mut app = App {
            fixtures,
            stack: vec![root],
            focus: None,
            focus_below: Vec::new(),
            note: None,
            keys_open: false,
            quit: false,
            uselog,
            last_key: None,
            sel: HashMap::new(),
            marks: HashMap::new(),
            mode: Mode::Normal,
            split: false,
            approve_scroll: std::cell::Cell::new(0),
            frame: 0,
            in_session: false,
            armed: None,
            calendar: None,
            mail: None,
            mail_queue: None,
            approve_pick: None,
            approve_visual: None,
            approve_filter: String::new(),
            approve_filtering: false,
            shown: RefCell::new(HashMap::new()),
            chat: crate::chat::Chat::new(None),
            chat_draft: String::new(),
            chat_new: false,
            chat_list: false,
            unregistered: Vec::new(),
            last_commit: Default::default(),
            careers_error: None,
            system_live: None,
            home_live: false,
            other_repos: Vec::new(),
            refresh_requested: false,
        };
        app.uselog.record(root.name(), "open");
        app
    }

    pub fn screen(&self) -> ScreenId {
        self.stack.last().copied().unwrap_or(ScreenId::Home)
    }

    pub fn focused(&self) -> Option<&'static Section> {
        let f = self.focus?;
        self.screen().sections().iter().find(|s| s.letter == f)
    }

    /// Enter on a registry entry, wherever it is listed (Home, the projects view, a request from
    /// kairos): an application asks the Home chat where things stand, prefilled and not sent;
    /// anything else switches this client to its own tmux session. An id the registry does not
    /// know leaves you on Home.
    pub fn open_item(&mut self, id: &str) {
        let Some(p) = self.fixtures.registry.get(id).cloned() else {
            self.home();
            return;
        };
        if p.kind == locus_contract::ProjectKind::Application {
            self.ask_home(format!("Where do things stand with {}?", p.name));
            return;
        }
        let Some(session) = p.sessions.first() else {
            self.note = Some(format!("no tmux session for {}", p.name));
            return;
        };
        if !self.in_session || std::env::var_os("TMUX").is_none() {
            self.note = Some(format!("not in tmux: {}'s session is {session}", p.name));
            return;
        }
        let target = format!("={session}");
        if !tmux(&["has-session", "-t", &target]) {
            self.note = Some(format!("no tmux session for {} ({session} is not running)", p.name));
        } else if !tmux(&["switch-client", "-t", &target]) {
            self.note = Some(format!("tmux did not switch to {session}"));
        }
    }

    /// The window is reading live data: what only the fixtures know goes, so Home never shows a
    /// sample as current (cleanup C3). The attended project marks and reorders nothing, and the
    /// mail count waits for keryx.
    pub fn go_live(&mut self) {
        self.home_live = true;
        self.fixtures.home.attended = None;
        self.fixtures.home.mail.need_reply = None;
    }

    /// Back to Home with nothing open on it.
    pub fn home(&mut self) {
        self.stack.truncate(1);
        self.focus_below.clear();
        self.focus = None;
    }

    /// The Home chat bar, open for typing with `text` in it (not sent).
    pub fn ask_home(&mut self, text: String) {
        if self.screen() != ScreenId::Home {
            self.home();
        }
        self.mode = Mode::Chat(text);
    }

    pub fn open(&mut self, screen: ScreenId) {
        self.stack.push(screen);
        self.focus_below.push(self.focus.take());
        self.uselog.record(screen.name(), "open");
    }

    /// Leave the screen on top without the rest of Esc's rules (an approve item, decided).
    pub fn close_top(&mut self) {
        if self.stack.len() > 1 {
            self.stack.pop();
            self.focus = self.focus_below.pop().flatten();
        }
    }

    fn back(&mut self) {
        if self.focus.is_some() {
            self.focus = None;
        } else if self.stack.len() > 1 {
            self.stack.pop();
            self.focus = self.focus_below.pop().flatten();
        }
    }

    /// The selected row of a list (`' '` when the screen has one list and no sections).
    pub fn sel(&self, sec: char) -> usize {
        self.sel.get(&(self.screen(), sec)).copied().unwrap_or(0)
    }

    /// A page of the approve list: what the last frame showed, roughly.
    pub fn approve_page(&self) -> usize {
        15
    }

    /// The approve list's selection, from any screen (the item screen shows it).
    pub fn approve_sel(&self) -> usize {
        self.sel.get(&(ScreenId::Approve, ' ')).copied().unwrap_or(0)
    }

    /// Move a list's selection by `d`, kept inside `len`.
    pub fn move_sel(&mut self, sec: char, len: usize, d: isize) {
        let cur = self.sel(sec) as isize;
        let next = (cur + d).clamp(0, len.saturating_sub(1) as isize) as usize;
        self.sel.insert((self.screen(), sec), next);
    }

    pub fn mark(&mut self, key: String, m: Mark, note: impl Into<String>) {
        self.marks.insert(key, m);
        self.note = Some(note.into());
    }

    pub fn marked(&self, key: &str) -> Option<&Mark> {
        self.marks.get(key)
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        let name = key_name(key);
        self.last_key = Some(Instant::now());
        self.uselog.record(self.screen().name(), &name);
        self.note = None;
        if self.keys_open {
            self.keys_open = false;
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        if self.mode != Mode::Normal {
            self.on_mode_key(key);
            return;
        }
        // A list's own text input (Approve's filter, its place picker): every key is text.
        if self.typing() {
            crate::ui::screen_key(self, key);
            return;
        }
        // The shared table first (cleanup C4, D1). A screen with a meaning of its own for one
        // of its keys (y on Approve, d on Tasks) takes it; otherwise it means the same here as
        // in every app.
        if let Some(a) = keys::action(key) {
            if !crate::ui::screen_key(self, key) {
                self.shared(a, &name);
            }
            return;
        }
        if self.section_key(key) {
            return;
        }
        if crate::ui::screen_key(self, key) {
            return;
        }
        match key.code {
            KeyCode::Enter => match self.focused().and_then(|s| s.detail) {
                Some(detail) => self.open(detail),
                None if self.focus.is_none() && !self.screen().sections().is_empty() => {
                    self.note = Some("choose a section first: its letter is in its corner, or ⇥".into())
                }
                None => self.note = Some("nothing more to open here yet".into()),
            },
            // The automatic-actions log, from anywhere (it was `g l` for one evening, C4).
            KeyCode::Char('l') if self.screen() != ScreenId::AutoLog => self.open(ScreenId::AutoLog),
            // A section's letter focuses it, as its corner says (back after C4).
            KeyCode::Char(ch) if self.screen().sections().iter().any(|s| s.letter == ch) => self.focus = Some(ch),
            // The chat's conversations, from anywhere (files-and-chat M2: its own key, never Tab).
            KeyCode::Char('i') if self.screen() != ScreenId::Chat => {
                self.chat_new = false;
                let at = self
                    .chat
                    .index
                    .current
                    .as_deref()
                    .and_then(|id| self.chat.list().iter().position(|c| c.id == id));
                self.open(ScreenId::Chat);
                self.sel.insert((ScreenId::Chat, ' '), at.unwrap_or(0));
            }
            _ => self.note = Some(format!("{name} does nothing here · ? for keys")),
        }
    }

    /// Typing into a list's own field, where no key is a command.
    fn typing(&self) -> bool {
        self.screen() == ScreenId::Approve && (self.approve_filtering || self.approve_pick.is_some())
    }

    /// What a key of the shared table does when the screen gave it no meaning of its own.
    fn shared(&mut self, a: Action, name: &str) {
        match a {
            Action::Back => self.back(),
            // q quits only at the top (Home); anywhere deeper it is back.
            Action::Quit if self.screen().is_root() => self.quit = true,
            Action::Quit => self.back(),
            Action::Help => self.keys_open = true,
            // The search screen held sample data (cleanup C3): `/` asks the chat instead.
            Action::Search => self.ask_home(String::new()),
            // ^r: the live data now, not at the next tick (main.rs wakes its threads).
            Action::Refresh => {
                self.refresh_requested = true;
                self.note = Some("refreshing: asking kairos, keryx and the server now".into());
            }
            Action::Undo => self.note = Some("nothing to undo here".into()),
            _ => {
                let what = keys::TABLE.iter().find(|(x, _, _)| *x == a).map_or("", |(_, _, w)| *w);
                self.note = Some(format!("{name} ({what}) does nothing here · ? for keys"));
            }
        }
    }

    /// Moving between a screen's sections without their letters (round 1, 21 Oct: "I definitely
    /// want tab to be able to cycle us through the different sections"; ↑ and ↓ pressed 162
    /// times on Home with nothing to move). Tab and Shift-Tab cycle the sections. On screens
    /// whose sections are lists, ↓ walks a section's rows and at its last row moves on into the
    /// next section; ↑ the same upwards.
    fn section_key(&mut self, key: KeyEvent) -> bool {
        let screen = self.screen();
        let secs = screen.sections();
        if secs.is_empty() {
            return false;
        }
        let at = self.focus.and_then(|f| secs.iter().position(|s| s.letter == f));
        let n = secs.len();
        match key.code {
            KeyCode::Tab | KeyCode::BackTab => {
                let fwd = key.code == KeyCode::Tab;
                let mut next = match (at, fwd) {
                    (None, true) => 0,
                    (None, false) => n - 1,
                    (Some(i), true) => (i + 1) % n,
                    (Some(i), false) => (i + n - 1) % n,
                };
                // A section with nothing in it is not drawn, so Tab passes over it (round 2q).
                for _ in 0..n {
                    if crate::ui::section_len(self, secs[next].letter) > 0 {
                        break;
                    }
                    next = if fwd { (next + 1) % n } else { (next + n - 1) % n };
                }
                self.focus = Some(secs[next].letter);
                true
            }
            _ if !crate::ui::flows(screen) => false,
            _ => {
                let Some(d) = nav(&key) else { return false };
                let Some(i) = at else {
                    let i = if d > 0 { 0 } else { n - 1 };
                    self.focus = Some(secs[i].letter);
                    return true;
                };
                let sec = secs[i].letter;
                let len = crate::ui::section_len(self, sec).max(1);
                let cur = self.sel(sec);
                if d > 0 && cur + 1 < len {
                    self.sel.insert((screen, sec), cur + 1);
                } else if d < 0 && cur > 0 {
                    self.sel.insert((screen, sec), cur - 1);
                } else if d > 0
                    && let Some(j) = (i + 1..n).find(|&j| crate::ui::section_len(self, secs[j].letter) > 0)
                {
                    let next = secs[j].letter;
                    self.focus = Some(next);
                    self.sel.insert((screen, next), 0);
                } else if d < 0
                    && let Some(j) = (0..i).rev().find(|&j| crate::ui::section_len(self, secs[j].letter) > 0)
                {
                    let prev = secs[j].letter;
                    self.focus = Some(prev);
                    let last = crate::ui::section_len(self, prev).max(1) - 1;
                    self.sel.insert((screen, prev), last);
                }
                true
            }
        }
    }

    fn on_mode_key(&mut self, key: KeyEvent) {
        match (&mut self.mode, key.code) {
            // Esc keeps a half-typed message; Space brings it back.
            (Mode::Chat(q), KeyCode::Esc) => {
                if !q.trim().is_empty() {
                    self.chat_draft = std::mem::take(q);
                    self.note = Some("draft kept · ␣ brings it back".into());
                }
                self.mode = Mode::Normal;
            }
            // A line break inside a message.
            (Mode::Chat(q), KeyCode::Enter) if key.modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) => {
                q.push('\n')
            }
            (Mode::Chat(q), KeyCode::Enter) => {
                let text = std::mem::take(q).trim().to_string();
                self.mode = Mode::Normal;
                if !text.is_empty() {
                    let into = if self.screen() == ScreenId::Chat {
                        crate::ui::chat::viewed(self)
                    } else {
                        self.chat.index.current.clone()
                    };
                    crate::ui::chat::send(self, text, into);
                }
            }
            (Mode::Chat(q) | Mode::ChatName(q), KeyCode::Backspace) => {
                q.pop();
            }
            (Mode::Chat(q) | Mode::ChatName(q), KeyCode::Char(ch)) => q.push(ch),
            (Mode::ChatName(_), KeyCode::Esc) => self.mode = Mode::Normal,
            (Mode::FactEdit(_, _, v), KeyCode::Backspace) => {
                v.pop();
            }
            (Mode::FactEdit(_, _, v), KeyCode::Char(ch)) => v.push(ch),
            (Mode::FactEdit(..), KeyCode::Esc) => {
                self.mode = Mode::Normal;
                self.note = Some("not changed: still waiting in Approve".into());
            }
            (Mode::FactEdit(at, id, v), KeyCode::Enter) => {
                let (at, id, value) = (*at, *id, std::mem::take(v));
                self.mode = Mode::Normal;
                if value.trim().is_empty() {
                    self.note = Some("a fact needs a value: n refuses it instead".into());
                } else {
                    crate::ui::approve::decide_fact(self, at, id, true, Some(value));
                }
            }
            (Mode::ChatName(n), KeyCode::Enter) => {
                let name = std::mem::take(n);
                self.mode = Mode::Normal;
                if let Some(id) = crate::ui::chat::viewed(self).filter(|_| !name.trim().is_empty()) {
                    self.chat.rename(&id, &name);
                }
            }
            _ => {}
        }
    }
}

/// Run tmux quietly; true when it succeeded.
fn tmux(args: &[&str]) -> bool {
    std::process::Command::new("tmux")
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// A key as the use log and the footer name it: `p`, `R`, `enter`, `esc`, `ctrl-c`, `alt-1`.
pub fn key_name(key: KeyEvent) -> String {
    let base = match key.code {
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "enter".into(),
        KeyCode::Esc => "esc".into(),
        KeyCode::Tab => "tab".into(),
        KeyCode::BackTab => "backtab".into(),
        KeyCode::Backspace => "backspace".into(),
        KeyCode::Up => "up".into(),
        KeyCode::Down => "down".into(),
        KeyCode::Left => "left".into(),
        KeyCode::Right => "right".into(),
        other => format!("{other:?}").to_lowercase(),
    };
    let mut out = String::new();
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        out.push_str("ctrl-");
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        out.push_str("alt-");
    }
    out.push_str(&base);
    out
}

/// Up and down for a list: j/k and the arrows. `Some(delta)` when `key` moves a selection.
pub fn nav(key: &KeyEvent) -> Option<isize> {
    match key.code {
        KeyCode::Down | KeyCode::Char('j') => Some(1),
        KeyCode::Up | KeyCode::Char('k') => Some(-1),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn app() -> App {
        let dir = crate::fixtures::sets_dir().join("default");
        let f = Fixtures::load(&dir).unwrap_or_else(|_| panic!("fixtures at {}", PathBuf::from(&dir).display()));
        App::new(f, ScreenId::Home, UseLog::off())
    }

    fn press(app: &mut App, code: KeyCode) {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    /// v on Approve, then moving, marks a run of lines; y acts on every one of them, and the
    /// selection ends (10 Nov).
    #[test]
    fn visual_mode_on_approve_acts_on_every_selected_line() {
        let mut a = app();
        a.open(ScreenId::Approve);
        press(&mut a, KeyCode::Char('j'));
        press(&mut a, KeyCode::Char('v'));
        press(&mut a, KeyCode::Char('j'));
        press(&mut a, KeyCode::Char('j'));
        assert_eq!(a.approve_visual, Some(1));
        press(&mut a, KeyCode::Char('y'));
        assert_eq!(a.approve_visual, None);
        let approved: Vec<&String> = a
            .marks
            .iter()
            .filter(|(_, m)| **m == Mark::Approved)
            .map(|(k, _)| k)
            .collect();
        assert_eq!(approved.len(), 3, "{approved:?}");
        assert!(
            !a.marks.contains_key("approve:0:0"),
            "the line before the selection is untouched"
        );
        // Esc leaves visual mode without leaving the screen.
        press(&mut a, KeyCode::Char('v'));
        press(&mut a, KeyCode::Esc);
        assert_eq!((a.approve_visual, a.screen()), (None, ScreenId::Approve));
    }

    #[test]
    fn tab_focuses_enter_opens_esc_backs_out() {
        let mut a = app();
        // Shift-Tab from nothing focused: the last section, system.
        a.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(a.focus, Some('m'));
        press(&mut a, KeyCode::Enter);
        assert_eq!(a.screen(), ScreenId::System);
        assert_eq!(a.focus, None);
        press(&mut a, KeyCode::Esc);
        assert_eq!(a.screen(), ScreenId::Home);
        press(&mut a, KeyCode::Esc);
        assert_eq!(a.screen(), ScreenId::Home, "Esc stops at the root");
    }

    #[test]
    fn enter_without_focus_says_why() {
        let mut a = app();
        press(&mut a, KeyCode::Enter);
        assert!(a.note.as_deref().is_some_and(|n| n.contains("choose a section")));
    }

    #[test]
    fn slash_asks_the_home_chat_and_question_mark_toggles_keys() {
        let mut a = app();
        press(&mut a, KeyCode::Char('?'));
        assert!(a.keys_open);
        press(&mut a, KeyCode::Char('x'));
        assert!(!a.keys_open);
        a.open(ScreenId::System);
        press(&mut a, KeyCode::Char('/'));
        assert_eq!(
            (a.screen(), a.mode.clone()),
            (ScreenId::Home, Mode::Chat(String::new()))
        );
    }

    #[test]
    fn q_quits_only_from_a_root() {
        let mut a = app();
        a.open(ScreenId::System);
        press(&mut a, KeyCode::Char('q'));
        assert!(!a.quit);
        press(&mut a, KeyCode::Esc);
        press(&mut a, KeyCode::Char('q'));
        assert!(a.quit);
    }

    /// Enter on a project row goes to its own tmux session; outside the locus session (a
    /// test) it only says which, and touches no tmux (cleanup C3).
    #[test]
    fn a_project_row_names_its_tmux_session_outside_tmux() {
        let mut a = app();
        a.focus = Some('p');
        let rows = crate::ui::home::project_rows(&a);
        let i = rows
            .iter()
            .position(|id| a.fixtures.registry.get(id).is_some_and(|p| !p.sessions.is_empty()))
            .expect("a live project with a session");
        a.sel.insert((ScreenId::Home, 'p'), i);
        press(&mut a, KeyCode::Enter);
        let p = a.fixtures.registry.get(&rows[i]).cloned().unwrap();
        assert_eq!(a.screen(), ScreenId::Home);
        assert_eq!(
            a.note.as_deref(),
            Some(format!("not in tmux: {}'s session is {}", p.name, p.sessions[0]).as_str())
        );
    }

    /// Enter on an application asks the Home chat about it, prefilled and not sent.
    #[test]
    fn a_careers_row_prefills_the_chat() {
        let mut a = app();
        a.focus = Some('c');
        press(&mut a, KeyCode::Enter);
        let id = crate::ui::home::career_rows(&a)[0].clone();
        let name = a.fixtures.registry.get(&id).unwrap().name.clone();
        assert_eq!(a.mode, Mode::Chat(format!("Where do things stand with {name}?")));
        assert!(a.chat.busy.is_none(), "not sent");
    }

    /// A name kairos or keryx sends that is no registry entry (an old screen's name) opens
    /// Home, never a panic.
    #[test]
    fn an_unknown_item_opens_home() {
        let mut a = app();
        a.open(ScreenId::Tasks);
        for name in ["project", "application", "search", "review", ""] {
            a.open_item(name);
            assert_eq!(a.screen(), ScreenId::Home, "{name}");
        }
    }

    #[test]
    fn tab_cycles_sections_and_arrows_flow_between_them() {
        let mut a = app();
        let secs: Vec<char> = ScreenId::Home.sections().iter().map(|s| s.letter).collect();
        press(&mut a, KeyCode::Tab);
        assert_eq!(a.focus, Some(secs[0]));
        press(&mut a, KeyCode::Tab);
        assert_eq!(a.focus, Some(secs[1]));
        a.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(a.focus, Some(secs[0]));
        // Down through projects, then on into the next section.
        a.focus = Some('p');
        let n = crate::ui::section_len(&a, 'p');
        for _ in 0..n {
            press(&mut a, KeyCode::Down);
        }
        assert_eq!(a.focus, Some('c'), "past the last project, into careers");
        press(&mut a, KeyCode::Up);
        assert_eq!(
            (a.focus, a.sel('p')),
            (Some('p'), n - 1),
            "and back to the last project"
        );
    }

    #[test]
    fn arrows_with_nothing_focused_focus_the_first_section() {
        let mut a = app();
        press(&mut a, KeyCode::Down);
        assert_eq!(a.focus, Some(ScreenId::Home.sections()[0].letter));
        assert!(a.note.is_none(), "no 'does nothing here'");
    }

    /// Esc from a screen comes back to the section that opened it (round 2o).
    #[test]
    fn esc_returns_to_the_section_you_left() {
        let mut a = app();
        a.focus = Some('t');
        a.open(ScreenId::Tasks);
        assert_eq!(a.focus, None);
        press(&mut a, KeyCode::Esc);
        assert_eq!(a.screen(), ScreenId::Home);
        assert_eq!(a.focus, Some('t'));
    }

    // ------------------------------------------------------------ the one key table (C4)

    /// The presses a key as the help shows it stands for: `↑ ↓` is two keys.
    fn presses(shown: &str) -> Vec<Vec<KeyEvent>> {
        let plain = |c| KeyEvent::new(c, KeyModifiers::NONE);
        shown
            .split_whitespace()
            .map(|tok| match tok {
                "esc" => plain(KeyCode::Esc),
                "^r" => KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
                "^d" => KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
                "^u" => KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
                "⇥" => plain(KeyCode::Tab),
                "⇧⇥" => KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
                "⏎" => plain(KeyCode::Enter),
                "␣" => plain(KeyCode::Char(' ')),
                "←" => plain(KeyCode::Left),
                "→" => plain(KeyCode::Right),
                "↑" => plain(KeyCode::Up),
                "↓" => plain(KeyCode::Down),
                "PgUp" | "⇞" => plain(KeyCode::PageUp),
                "PgDn" | "⇟" => plain(KeyCode::PageDown),
                "Home" => plain(KeyCode::Home),
                "End" => plain(KeyCode::End),
                t if t.chars().count() == 1 => plain(KeyCode::Char(t.chars().next().unwrap())),
                t => panic!("help names a key the test cannot press: {t:?}"),
            })
            .map(|e| vec![e])
            .collect()
    }

    /// The places a key may need: nothing focused or each section focused, on each of the first
    /// rows. Built one at a time, as a key is tried in each until one handles it.
    fn contexts(fx: &Fixtures, screen: ScreenId) -> impl Iterator<Item = App> + '_ {
        let focuses: Vec<Option<char>> = std::iter::once(None)
            .chain(screen.sections().iter().map(|s| Some(s.letter)))
            .collect();
        (0..8).flat_map(move |row| {
            focuses.clone().into_iter().map(move |focus| {
                let mut a = App::new(fx.clone(), ScreenId::Home, UseLog::off());
                if screen != ScreenId::Home {
                    a.open(screen);
                }
                a.focus = focus;
                a.sel.insert((screen, focus.unwrap_or(' ')), row);
                // A first draw, so sections that record what they drew know it.
                crate::dump::render(&a, 95, 38).unwrap();
                a
            })
        })
    }

    #[test]
    fn no_screen_binds_a_table_key_to_something_else() {
        for screen in ScreenId::ALL {
            for (shown, what) in crate::ui::help_keys(screen) {
                for tok in shown.split_whitespace() {
                    assert!(
                        keys::app_key_is_free(tok),
                        "{screen}: {tok:?} ({what}) is the table's key"
                    );
                }
            }
        }
    }

    /// Every key the help names on a screen is handled there: the screen's own keys never end in
    /// "does nothing here" (in some selection), and no table key panics anywhere.
    #[test]
    fn every_key_the_help_names_works_on_its_screen() {
        let fx = app().fixtures;
        for screen in ScreenId::ALL {
            let own: Vec<String> = crate::ui::help_keys(screen)
                .iter()
                .map(|(k, _)| k.to_string())
                .collect();
            for (shown, what) in keys::help(&crate::ui::help_keys(screen)) {
                for k in presses(&shown) {
                    let mut handled = false;
                    for mut a in contexts(&fx, screen) {
                        for e in &k {
                            a.on_key(*e);
                        }
                        crate::dump::render(&a, 95, 38).unwrap();
                        if !a.note.as_deref().is_some_and(|n| n.contains("does nothing here")) {
                            handled = true;
                            break;
                        }
                    }
                    if own.contains(&shown) {
                        assert!(handled, "{screen}: {shown} ({what}): {k:?} does nothing anywhere");
                    }
                }
            }
        }
    }

    #[test]
    fn table_keys_mean_the_same_on_home() {
        let mut a = app();
        // g is no shared key any more (13 Nov): on Home it does nothing.
        press(&mut a, KeyCode::Char('g'));
        assert_eq!(a.screen(), ScreenId::Home);
        assert!(a.note.as_deref().is_some_and(|n| n.contains("does nothing here")));
        // q is back below the top.
        a.open(ScreenId::Tasks);
        press(&mut a, KeyCode::Char('q'));
        assert_eq!((a.screen(), a.quit), (ScreenId::Home, false));
        // ^r asks the window for its live data now.
        a.on_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert!(a.refresh_requested);
        // u where there is nothing to undo says so.
        press(&mut a, KeyCode::Char('u'));
        assert_eq!(a.note.as_deref(), Some("nothing to undo here"));
    }

    /// Direct navigation is back (13 Nov: "now navigating around home only works with tab and
    /// arrows"): every section letter focuses its section, and from there Enter reaches its
    /// screen; the screen keys reach theirs in one press.
    #[test]
    fn home_letters_reach_each_section_and_screen() {
        for screen in [ScreenId::Home, ScreenId::System, ScreenId::Learning] {
            let own = crate::ui::screen_keys(screen);
            for sec in screen.sections() {
                // A letter the screen binds itself (Home's p, System's b) opens its screen.
                if own.iter().any(|(k, _)| k.chars().eq([sec.letter])) {
                    continue;
                }
                assert!(
                    keys::app_key_is_free(&sec.letter.to_string()),
                    "{screen}: {}",
                    sec.letter
                );
                let mut a = app();
                if screen != ScreenId::Home {
                    a.open(screen);
                }
                press(&mut a, KeyCode::Char(sec.letter));
                assert_eq!(
                    (a.screen(), a.focus),
                    (screen, Some(sec.letter)),
                    "{screen}: {}",
                    sec.letter
                );
                if let Some(detail) = sec.detail {
                    press(&mut a, KeyCode::Enter);
                    assert_eq!(a.screen(), detail, "{screen}: {} then Enter", sec.letter);
                }
            }
        }
        // Home: a approve, t tasks, w work, m system, each then Enter; p, L, l and i at once.
        for (letter, want) in [
            ('a', ScreenId::Approve),
            ('t', ScreenId::Tasks),
            ('w', ScreenId::WorkChart),
            ('m', ScreenId::System),
        ] {
            let mut a = app();
            press(&mut a, KeyCode::Char(letter));
            press(&mut a, KeyCode::Enter);
            assert_eq!(a.screen(), want, "{letter} then Enter");
        }
        for (letter, want) in [
            ('p', ScreenId::Projects),
            ('L', ScreenId::Learning),
            ('l', ScreenId::AutoLog),
            ('i', ScreenId::Chat),
        ] {
            let mut a = app();
            press(&mut a, KeyCode::Char(letter));
            assert_eq!(a.screen(), want, "{letter}");
        }
        // The log from a screen below Home too, and the budget from System in one key.
        let mut a = app();
        a.open(ScreenId::Tasks);
        press(&mut a, KeyCode::Char('l'));
        assert_eq!(a.screen(), ScreenId::AutoLog);
        let mut a = app();
        a.open(ScreenId::System);
        press(&mut a, KeyCode::Char('b'));
        assert_eq!(a.screen(), ScreenId::Budget);
    }
}
