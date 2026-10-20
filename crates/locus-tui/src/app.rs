//! App state and keys.
//!
//! The order a key is tried in: the keys overlay (any key closes it), a mode that owns the
//! keyboard (typing a question, choosing a strike reason), the screen's own keys, then the global
//! keys (plan D: Enter opens, Esc goes back, `/` searches, `?` shows keys) and section letters.
//! A screen may claim a letter for an action that way; which ones should is for the rounds.
//!
//! Nothing leaves the process (spec 1.4 rules): an action marks its item (read, done, struck,
//! approved) and the footer says what it would have done.

use crate::fixtures::Fixtures;
use crate::follow::{self, Follow};
use crate::screen::{ScreenId, Section};
use crate::uselog::UseLog;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Instant;

/// What an action did to an item, drawn on it until `R` resets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mark {
    Read,
    Done,
    Ticked,
    Struck(&'static str),
    Approved,
    Rejected,
    Undone,
    Parked,
}

/// A mode that takes every key until it ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
    /// Typing on the Project view's ask line.
    Ask(String),
    /// A find was struck; the next key is the reason (C.7).
    Strike(String),
}

pub struct App {
    pub fixtures: Fixtures,
    /// The root (Home or Project) at the bottom, detail screens on top.
    pub stack: Vec<ScreenId>,
    /// The focused section's letter, on the top screen.
    pub focus: Option<char>,
    /// The footer's message: what an action would have done, or why a key did nothing.
    pub note: Option<String>,
    pub keys_open: bool,
    pub quit: bool,
    pub uselog: UseLog,
    /// Which project the Project view shows, following the project window (plan B.4).
    pub follow: Follow,
    pub last_key: Option<Instant>,
    /// The selected row, per screen and section (`' '` for a screen's one list).
    pub sel: HashMap<(ScreenId, char), usize>,
    /// Item key → what an action did to it.
    pub marks: HashMap<String, Mark>,
    pub mode: Mode,
    /// The project a Project view opened from Home shows (the root one follows tmux instead).
    pub viewing: Option<String>,
    /// The ink fixture Ink confirm shows.
    pub ink: Option<String>,
    pub query: String,
    pub scope_all: bool,
    pub review_step: usize,
    /// Budget: each day's bar split by job type (`t`).
    pub split: bool,
    /// The terminal's image protocol, found at startup; none means half blocks (and `--dump`).
    pub picker: Option<Picker>,
    pub image: RefCell<Option<(String, StatefulProtocol)>>,
}

impl App {
    pub fn new(fixtures: Fixtures, root: ScreenId, uselog: UseLog) -> App {
        let query = fixtures.search.query.clone();
        let scope_all = fixtures.search.scope == "everything";
        let mut app = App {
            fixtures,
            stack: vec![root],
            focus: None,
            note: None,
            keys_open: false,
            quit: false,
            uselog,
            follow: Follow::new(None),
            last_key: None,
            sel: HashMap::new(),
            marks: HashMap::new(),
            mode: Mode::Normal,
            viewing: None,
            ink: None,
            query,
            scope_all,
            review_step: 1,
            split: false,
            picker: None,
            image: RefCell::new(None),
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

    pub fn open(&mut self, screen: ScreenId) {
        self.stack.push(screen);
        self.focus = None;
        self.uselog.record(screen.name(), "open");
    }

    fn back(&mut self) {
        if self.focus.is_some() {
            self.focus = None;
        } else if self.stack.len() > 1 {
            self.stack.pop();
        }
    }

    /// The project the project screens are about: the followed one when the Project view is
    /// this window's root, otherwise the one opened from Home.
    pub fn project_id(&self) -> Option<String> {
        if self.stack.first() == Some(&ScreenId::Project) {
            let s = self.follow.session.as_deref()?;
            Some(self.fixtures.registry.by_session(s)?.id.clone())
        } else {
            self.viewing.clone()
        }
    }

    /// The selected row of a list (`' '` when the screen has one list and no sections).
    pub fn sel(&self, sec: char) -> usize {
        self.sel.get(&(self.screen(), sec)).copied().unwrap_or(0)
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
        if crate::ui::screen_key(self, key) {
            return;
        }
        match key.code {
            KeyCode::Esc => self.back(),
            KeyCode::Enter => match self.focused().and_then(|s| s.detail) {
                Some(detail) => self.open(detail),
                None if self.focus.is_none() && !self.screen().sections().is_empty() => {
                    self.note = Some("focus a section first: its letter is in its corner".into())
                }
                None => self.note = Some("nothing more to open here yet".into()),
            },
            KeyCode::Char('?') => self.keys_open = true,
            KeyCode::Char('/') => {
                if self.screen() != ScreenId::Search {
                    self.open(ScreenId::Search);
                }
            }
            KeyCode::Char('l') if self.screen() != ScreenId::AutoLog => self.open(ScreenId::AutoLog),
            KeyCode::Char('q') if self.screen().is_root() => self.quit = true,
            KeyCode::Char('R') => self.reset(),
            KeyCode::Char(ch) if self.screen().sections().iter().any(|s| s.letter == ch) => self.focus = Some(ch),
            _ => self.note = Some(format!("{name} does nothing here · ? for keys")),
        }
    }

    fn on_mode_key(&mut self, key: KeyEvent) {
        match (&mut self.mode, key.code) {
            (Mode::Ask(_), KeyCode::Esc) | (Mode::Strike(_), KeyCode::Esc) => self.mode = Mode::Normal,
            (Mode::Ask(q), KeyCode::Enter) => {
                let q = q.trim().to_string();
                self.mode = Mode::Normal;
                if !q.is_empty() {
                    self.note = Some(format!("would ask heuris: \"{q}\" · the answer lands in Answered"));
                }
            }
            (Mode::Ask(q), KeyCode::Tab) => {
                let q = q.trim().to_string();
                self.mode = Mode::Normal;
                self.note = Some(format!("would send to the writer agent for the tablet: \"{q}\""));
            }
            (Mode::Ask(q), KeyCode::Backspace) => {
                q.pop();
            }
            (Mode::Ask(q), KeyCode::Char(ch)) => q.push(ch),
            (Mode::Strike(id), KeyCode::Char(ch)) => {
                let reason = match ch {
                    'i' => Some("irrelevant"),
                    'k' => Some("knew it"),
                    'l' => Some("wrong level"),
                    't' => Some("too long"),
                    _ => None,
                };
                match reason {
                    Some(r) => {
                        let id = format!("find:{id}");
                        self.mode = Mode::Normal;
                        self.mark(
                            id,
                            Mark::Struck(r),
                            format!("struck: {r} · the scout would learn from it"),
                        );
                    }
                    None => self.note = Some("reason: i irrelevant · k knew it · l level · t too long".into()),
                }
            }
            _ => {}
        }
    }

    /// The project window moved to `session` (from the focus file), or time passed: switch the
    /// Project view if nothing holds it back.
    pub fn tick(&mut self, session: Option<String>, now: Instant) {
        let busy = follow::busy(self.last_key, now, self.stack.len() > 1) || matches!(self.mode, Mode::Ask(_));
        if let Some(s) = session {
            self.follow.offer(&s, busy);
        }
        self.follow.settle(busy);
    }

    /// Back to the fixture snapshot as it is on disk (spec: state resets on restart, or `R`).
    fn reset(&mut self) {
        match Fixtures::load(&self.fixtures.dir.clone()) {
            Ok(f) => {
                self.fixtures = f;
                self.marks.clear();
                self.query = self.fixtures.search.query.clone();
                self.image.replace(None);
                self.note = Some("fixtures reset".into());
            }
            Err(e) => self.note = Some(format!("fixtures did not reload: {e:#}")),
        }
    }
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

    #[test]
    fn letter_focuses_enter_opens_esc_backs_out() {
        let mut a = app();
        press(&mut a, KeyCode::Char('s'));
        assert_eq!(a.focus, Some('s'));
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
        assert!(a.note.as_deref().is_some_and(|n| n.contains("focus a section")));
    }

    #[test]
    fn slash_opens_search_and_question_mark_toggles_keys() {
        let mut a = app();
        press(&mut a, KeyCode::Char('?'));
        assert!(a.keys_open);
        press(&mut a, KeyCode::Char('x'));
        assert!(!a.keys_open);
        press(&mut a, KeyCode::Char('/'));
        assert_eq!(a.screen(), ScreenId::Search);
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

    #[test]
    fn a_project_opened_from_home_is_the_selected_row() {
        let mut a = app();
        press(&mut a, KeyCode::Char('p'));
        press(&mut a, KeyCode::Char('j'));
        press(&mut a, KeyCode::Enter);
        assert_eq!(a.screen(), ScreenId::Project);
        let second = crate::ui::home::project_rows(&a)[1].clone();
        assert_eq!(a.project_id(), Some(second));
    }

    #[test]
    fn striking_a_find_asks_for_a_reason() {
        let mut a = app();
        a.open(ScreenId::Explore);
        press(&mut a, KeyCode::Char('n'));
        assert!(matches!(a.mode, Mode::Strike(_)));
        press(&mut a, KeyCode::Char('l'));
        assert_eq!(a.mode, Mode::Normal);
        assert!(a.marks.values().any(|m| *m == Mark::Struck("wrong level")));
    }
}
