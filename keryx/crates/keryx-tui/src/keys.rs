//! keryx on locus's one key table (cleanup C4, decision D1, 13 Nov 2026).
//!
//! Every key outside a text field goes to [`locus_theme::keys::action`] first; keryx's own keys
//! ([`OWN`], [`GO`]) are only what the table does not cover (reply, forward, file, a page of a
//! thread). The help screen is [`help`], drawn from the same lists, so it cannot drift from what
//! the keys do; `tests/keys.rs` proves that every own key is one the table leaves free and that
//! every key the help or a footer hint names does something.
//!
//! What changed for keryx: `esc` is back and `h` no longer is; `q` quits only at the top (the
//! inbox, nothing searched or marked) and is back elsewhere; `u` is only undo (a send in its
//! hold is cancelled with `c`); `d` is done and filing has its own key, `M`; `x` archives after
//! asking once; `+` writes; `g` then a letter goes to locus's places (`g h a t c l`, `g m` is
//! this window) or to keryx's own lists on the letters the table leaves free.

use crate::app::Key;
use locus_theme::keys::Action;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// The shared action a key means, if any. Callers in a text field (search, a form, the snooze
/// time, the filing picker's query) must not ask.
pub fn action(k: Key) -> Option<Action> {
    let ev = match k {
        Key::Char(c) => KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
        Key::Ctrl(c) => KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL),
        Key::Esc => KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        _ => return None,
    };
    locus_theme::keys::action(ev)
}

/// keryx's own keys, as the help shows them: (keys, separated by spaces; what they do).
pub const OWN: &[(&str, &str)] = &[
    ("⏎", "read the thread; in it, open or fold the message under the bar"),
    ("j k ↑ ↓", "move in the list; scroll a thread"),
    ("space", "a page down, running on into the next message"),
    ("] [", "the next or previous message of the thread"),
    ("J K", "the next or previous thread, while reading"),
    ("⇧↑ ⇧↓", "the top or bottom"),
    ("⇥ ⇧⇥", "the next or previous inbox"),
    ("⌥⇥", "add the next inbox to the ones shown"),
    (
        "r",
        "reply to the message under the bar (Claude's draft, if there is one)",
    ),
    ("R", "reply to all"),
    ("F", "forward the message under the bar"),
    ("c", "cancel a send in its hold"),
    ("a", "archive at once"),
    ("M", "file: choose what it is for; it leaves the inbox"),
    ("m", "mark what it is for; it stays in the inbox"),
    ("f", "needs reply on or off"),
    ("U", "unread"),
    ("I", "back to the inbox"),
    ("L", "unsubscribe"),
    ("t", "a kairos task"),
    ("i", "the invite to the calendar"),
    ("w", "reply with free times"),
    ("z", "the message as received"),
    ("o", "attachments"),
    ("1…9", "open link n of the message under the bar"),
    ("b", "hide or show the list"),
    ("A", "approve, in locus"),
];

/// keryx's own lists under `g`, on letters the table's places leave free.
pub const GO: &[(&str, &str)] = &[
    ("g i", "inbox"),
    ("g r", "needs reply"),
    ("g d", "drafts"),
    ("g s", "sent"),
    ("g v", "archive"),
    ("g e", "all mail"),
    ("g f", "filed by itself"),
    ("g w", "Claude's runs"),
    ("g 1…9", "an area"),
];

/// The help screen's lines: the table, its places, then keryx's lists and keys.
pub fn help() -> Vec<(String, String)> {
    let own: Vec<(&str, &str)> = GO.iter().chain(OWN).copied().collect();
    locus_theme::keys::help(&own)
}

/// One key as shown in help or a hint (`⏎`, `space`, `⇧↑`, `^r`, `x`) as the key it is.
pub fn parse(shown: &str) -> Option<Key> {
    Some(match shown {
        "⏎" => Key::Enter,
        "esc" => Key::Esc,
        "space" => Key::Char(' '),
        "⇥" => Key::Tab,
        "⇧⇥" => Key::BackTab,
        "⌥⇥" => Key::AltTab,
        "⇧↑" => Key::Top,
        "⇧↓" => Key::Bottom,
        "↑" => Key::Up,
        "↓" => Key::Down,
        "1…9" | "1…4" => Key::Char('1'),
        s if s.starts_with('^') && s.chars().count() == 2 => Key::Ctrl(s.chars().nth(1)?),
        s if s.chars().count() == 1 => Key::Char(s.chars().next()?),
        _ => return None,
    })
}
