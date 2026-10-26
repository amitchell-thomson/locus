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
    /// A decision reopened for review (round 2q).
    Reopened,
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
    /// The focus each screen under the top one had when it opened the next, so Esc comes back
    /// to where you were (round 2o: Esc from Answered landed on a Project view with nothing
    /// focused). Selections are kept per screen already; only the focus was lost.
    pub focus_below: Vec<Option<char>>,
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
    /// Questions asked this session about a decision, keyed `project:id`, oldest first: the
    /// prototype's stand-in for an answer that has not come back yet (round 2v).
    pub asked: Vec<(String, String)>,
    /// A question you were typing when you pressed Esc: Space brings it back (round 2zd: "Esc
    /// while typing keeps the draft").
    pub draft: String,
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
    /// Draw count, for the pulse on things running right now.
    pub frame: u64,
    /// Running as a window of the locus session (set by locus-session.sh), so Home can hand a
    /// project to the Project window instead of opening it in place.
    pub in_session: bool,
    /// A key that asks first (drop a task): the item it was pressed on; the same key again on
    /// the same item does it, any other key forgets it.
    pub armed: Option<String>,
    /// kairosd, when the window runs live (spec K8): what locus puts on the calendar goes
    /// through it. `None` in tests and dumps.
    pub calendar: Option<std::sync::Arc<crate::calendar::Live>>,
    /// keryxd, when the window runs live (keryx-joins-locus X2): mail to decide in Approve.
    pub mail: Option<std::sync::Arc<crate::mail::Live>>,
    /// The project Home asked the Project window to show, until Esc.
    pub peek: Option<String>,
    /// For a section that draws only some of its items, the ones drawn, in order: the arrows
    /// walk these and Enter opens them, never an item off screen (round 2g: Explore's selection
    /// moved over finds that were not shown). Written by the draw.
    pub shown: RefCell<HashMap<(ScreenId, char), Vec<usize>>>,
    /// The Project view's selected branch is open, its change records listed under the graph;
    /// `records_at` is how far they are scrolled (round 2h).
    pub lane_open: bool,
    pub records_at: usize,
    pub peek_file: Option<crate::follow::PeekFile>,
}

impl App {
    pub fn new(fixtures: Fixtures, root: ScreenId, uselog: UseLog) -> App {
        let query = fixtures.search.query.clone();
        let scope_all = fixtures.search.scope == "everything";
        let mut app = App {
            fixtures,
            stack: vec![root],
            focus: None,
            focus_below: Vec::new(),
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
            asked: Vec::new(),
            draft: String::new(),
            ink: None,
            query,
            scope_all,
            review_step: 1,
            split: false,
            picker: None,
            image: RefCell::new(None),
            frame: 0,
            in_session: false,
            armed: None,
            calendar: None,
            mail: None,
            peek: None,
            shown: RefCell::new(HashMap::new()),
            lane_open: false,
            records_at: 0,
            peek_file: None,
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

    /// Show a project: in the Project window when running in the locus session, else here.
    pub fn open_project(&mut self, id: String) {
        // Only projects go to the Project window, which follows tmux; an application opens here
        // (ADR-006: the Project window never shows anything but a project).
        let project = self
            .fixtures
            .registry
            .get(&id)
            .is_some_and(|p| p.kind == locus_contract::ProjectKind::Repo);
        if project
            && self.in_session
            && let Some(f) = &self.peek_file
            && f.write(Some(&id)).is_ok()
            && crate::follow::select_window("project")
        {
            return;
        }
        // An application has a page of its own (round 5), never the Project view's.
        let application = self
            .fixtures
            .registry
            .get(&id)
            .is_some_and(|p| p.kind == locus_contract::ProjectKind::Application);
        self.viewing = Some(id);
        self.open(if application {
            ScreenId::Application
        } else {
            ScreenId::Project
        });
    }

    pub fn open(&mut self, screen: ScreenId) {
        self.stack.push(screen);
        self.focus_below.push(self.focus.take());
        self.uselog.record(screen.name(), "open");
    }

    fn back(&mut self) {
        if self.stack.len() == 1 && self.peek.is_some() && self.focus.is_none() {
            // A project Home handed over: give the window back to following, and go home.
            self.peek = None;
            if let Some(f) = &self.peek_file {
                let _ = f.write(None);
            }
            crate::follow::select_window("home");
        } else if self.focus.is_some() {
            self.focus = None;
        } else if self.stack.len() > 1 {
            self.stack.pop();
            self.focus = self.focus_below.pop().flatten();
        }
    }

    /// The project the project screens are about: the followed one when the Project view is
    /// this window's root, otherwise the one opened from Home.
    pub fn project_id(&self) -> Option<String> {
        if self.stack.first() == Some(&ScreenId::Project) {
            if let Some(p) = &self.peek {
                return Some(p.clone());
            }
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

    /// Open the ask line, with any draft you left.
    pub fn start_ask(&mut self) {
        self.mode = Mode::Ask(std::mem::take(&mut self.draft));
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
        if self.section_key(key) {
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
        // An open branch takes ↑ and ↓ to scroll its change records.
        if self.lane_open && self.focus == Some('g') && nav(&key).is_some() {
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
                } else if crate::ui::holds(screen, sec) {
                    // A pane you read stays put at its ends rather than flowing on.
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
            // Esc keeps what you typed; Esc on an empty line forgets it.
            (Mode::Ask(q), KeyCode::Esc) => {
                if q.trim().is_empty() {
                    self.draft.clear();
                } else {
                    self.draft = std::mem::take(q);
                    self.note = Some("draft kept · ␣ brings it back".into());
                }
                self.mode = Mode::Normal;
            }
            (Mode::Strike(_), KeyCode::Esc) => self.mode = Mode::Normal,
            (Mode::Ask(q), KeyCode::Enter) => {
                let q = q.trim().to_string();
                self.mode = Mode::Normal;
                if q.is_empty() {
                } else if self.screen() == crate::screen::ScreenId::Decisions {
                    crate::ui::decisions::ask(self, q);
                } else if self.screen() == crate::screen::ScreenId::Answered {
                    crate::ui::projectdetail::follow_up(self, q);
                } else {
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
                self.note = Some("prototype reset: ticks and strikes cleared (fixture data only)".into());
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
    fn the_application_page_reaches_the_calendar_only_when_live() {
        // Spec K8: `o` opens an event's day in kairos, `c` puts the next date on the calendar.
        // Outside the live window (this test) neither touches tmux, a file or kairosd.
        let mut a = app();
        a.open_project("millennium".into());
        assert_eq!(a.screen(), ScreenId::Application);
        press(&mut a, KeyCode::Char('o'));
        assert!(
            a.note.as_deref().is_some_and(|n| n.starts_with("would open kairos at")),
            "{:?}",
            a.note
        );
        press(&mut a, KeyCode::Char('c'));
        let n = a.note.clone().unwrap_or_default();
        assert!(n.starts_with("would put") || n.starts_with("no next date"), "{n}");
    }

    #[test]
    fn letter_focuses_enter_opens_esc_backs_out() {
        let mut a = app();
        press(&mut a, KeyCode::Char('y'));
        assert_eq!(a.focus, Some('y'));
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
        press(&mut a, KeyCode::Char('e'));
        a.open(ScreenId::Explore);
        assert_eq!(a.focus, None);
        press(&mut a, KeyCode::Esc);
        assert_eq!(a.screen(), ScreenId::Home);
        assert_eq!(a.focus, Some('e'));
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
