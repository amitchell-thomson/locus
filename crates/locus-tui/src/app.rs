//! App state and keys.
//!
//! The order a key is tried in: the keys overlay (any key closes it), then the global keys
//! (plan D: Enter opens, Esc goes back, `/` searches, `?` shows keys), then a section letter.
//! Per-screen action keys arrive in step 1.4 and are tried before section letters, so a screen
//! can claim a letter for an action; whether any should is a design-round question.

use crate::fixtures::Fixtures;
use crate::screen::{ScreenId, Section};
use crate::uselog::UseLog;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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
}

impl App {
    pub fn new(fixtures: Fixtures, root: ScreenId, uselog: UseLog) -> App {
        let mut app = App {
            fixtures,
            stack: vec![root],
            focus: None,
            note: None,
            keys_open: false,
            quit: false,
            uselog,
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

    pub fn on_key(&mut self, key: KeyEvent) {
        let name = key_name(key);
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
        match key.code {
            KeyCode::Esc => self.back(),
            KeyCode::Enter => match self.focused().and_then(|s| s.detail) {
                Some(detail) => self.open(detail),
                None if self.focus.is_none() => {
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
            KeyCode::Char('q') if self.screen().is_root() => self.quit = true,
            KeyCode::Char('R') => self.reset(),
            KeyCode::Char(ch) if self.screen().sections().iter().any(|s| s.letter == ch) => self.focus = Some(ch),
            _ => self.note = Some(format!("{name} does nothing here · ? for keys")),
        }
    }

    /// Back to the fixture snapshot as it is on disk (spec: state resets on restart, or `R`).
    fn reset(&mut self) {
        match Fixtures::load(&self.fixtures.dir.clone()) {
            Ok(f) => {
                self.fixtures = f;
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
        press(&mut a, KeyCode::Char('/'));
        assert_eq!(a.screen(), ScreenId::Search);
        press(&mut a, KeyCode::Char('?'));
        assert!(a.keys_open);
        press(&mut a, KeyCode::Char('x'));
        assert!(!a.keys_open);
        assert_eq!(a.screen(), ScreenId::Search, "closing the overlay does nothing else");
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
}
