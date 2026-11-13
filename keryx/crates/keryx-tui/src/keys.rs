//! keryx on locus's one key table (cleanup C4, decision D1, 13 Nov 2026).
//!
//! Every key outside a text field goes to [`locus_theme::keys::action`] first; keryx's own
//! keys ([`OWN`], [`GO`]) are only what the table does not cover (reply, forward, file, a page of a
//! thread). The help screen is [`help`], drawn from the same lists, so it cannot drift from what
//! the keys do; `tests/keys.rs` proves that every own key is one the table leaves free and that
//! every key the help or a footer hint names does something.
//!
//! What changed for keryx: `esc` is back and `h` no longer is; `q` quits only at the top (the
//! inbox, nothing searched or marked) and is back elsewhere; `u` is only undo (a send in its
//! hold is cancelled with `c`); `d` is done; `x` archives after asking once; `+` writes.
//!
//! Keys of 13 Nov, Alec: `r` is needs reply, `R` reply (reply all is `a` on the confirm screen),
//! `f` for (the "what is it for" picker, which was `M`). `g` is mail's own again: `g` then a
//! letter goes to one of mail's lists, on the letters they had before C4 (`g i r d s a c l v`,
//! `g 1…9`), and never to another window ("g only makes sense in mail for going to its own
//! lists"). keryx catches `g` before asking the table ([`action`]), so it does not matter
//! whether the table still has a "go to" action: keryx never names it.

use crate::app::Key;
use locus_theme::keys::Action;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// The shared action a key means, if any. Callers in a text field (search, a form, the snooze
/// time, the filing picker's query) must not ask. `g` is keryx's own (its lists), whatever the
/// table says.
pub fn action(k: Key) -> Option<Action> {
    let ev = match k {
        Key::Char('g') => return None,
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
        "R",
        "reply to the message under the bar (Claude's draft, if there is one)",
    ),
    ("F", "forward the message under the bar"),
    ("c", "cancel a send in its hold"),
    ("a", "archive at once; sending a reply, reply or reply all"),
    ("f", "for: choose what it is for; it leaves the inbox"),
    ("m", "mark what it is for; it stays in the inbox"),
    ("r", "needs reply on or off"),
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

/// Mail's own lists under `g`.
pub const GO: &[(&str, &str)] = &[
    ("g i", "inbox"),
    ("g r", "needs reply"),
    ("g d", "drafts"),
    ("g s", "sent"),
    ("g a", "all mail"),
    ("g c", "Claude's runs"),
    ("g l", "filed by itself"),
    ("g v", "archive"),
    ("g 1…9", "an area"),
];

/// The table's keys that are keryx's here: `g` is mail's lists ([`GO`]), not locus's places.
fn shared() -> impl Iterator<Item = (String, String)> {
    locus_theme::keys::TABLE
        .iter()
        .filter(|(_, k, _)| *k != "g")
        .map(|(_, k, w)| ((*k).to_string(), (*w).to_string()))
}

/// How many of [`help`]'s lines are the table's (the rest are mail's own).
pub fn shared_len() -> usize {
    shared().count()
}

/// The help screen's lines: the table (less `g`), then keryx's lists and keys.
pub fn help() -> Vec<(String, String)> {
    shared()
        .chain(GO.iter().chain(OWN).map(|(k, w)| ((*k).to_string(), (*w).to_string())))
        .collect()
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
