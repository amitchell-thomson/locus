//! One set of keys for Home, kairos and keryx (cleanup C4, decision D1, 13 Nov 2026).
//!
//! Before this, the three apps had about 370 bindings between them and disagreed on undo, edit,
//! `v`, `m`, `s`, `i`, `g` and refresh: `q` was "back" on some screens and "quit" on others, `h`
//! was back in keryx, and help was a hand-written list that was wrong in places. Now every app
//! asks [`action`] first and only falls back to its own keys when the table has nothing to say,
//! and help is drawn from the same table ([`help`]), so the two cannot drift.
//!
//! An app's own keys are allowed only for what the table does not cover: day and week movement in
//! kairos, reply in keryx, a section's own verbs. [`app_key_is_free`] is what their tests use to
//! prove an app key never shadows a table key.
//!
//! `g` is not in the table. For one evening it was "go to" (`g h` home, `g a` approve, `g c` the
//! calendar...) in every app; Alec, after trying it (13 Nov): "the g - something keys feel weird,
//! across the board, it only really made sense in mail to take you to inboxes, drafts etc..., but
//! in home and calendar it is just odd." So `g` is an app's own key again: keryx's lists, nothing
//! in Home or the calendar. Home's section letters and screen keys came back, and the calendar's
//! capitals (`A` approve, `T` tasks, `P` projects) open Home's screens.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthStr;

/// What a shared key means, the same in every app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    /// `esc`: back one level (close a popup, leave a thread, clear a filter).
    Back,
    /// `q`: quit, but only at the top level; anywhere deeper it is [`Action::Back`].
    Quit,
    /// `u`: undo the last change at once, with a line saying what was undone.
    Undo,
    /// `e`: edit the thing under the cursor (opens its form; Tab reaches every field).
    Edit,
    /// `y`: approve, accept, yes.
    Yes,
    /// `n`: refuse, dismiss, no.
    No,
    /// `+`: new.
    New,
    /// `d`: done.
    Done,
    /// `x`: remove, skip or delete; always asks once.
    Remove,
    /// `v`: mark this row, for an action on several.
    Mark,
    /// `/`: search or filter.
    Search,
    /// `ctrl-r`: refresh from the source now.
    Refresh,
    /// Deprecated: never returned by [`action`] and not in [`TABLE`] (`g` is an app's own key
    /// again, 13 Nov). Kept only so keryx, which still names it, compiles until it handles `g`
    /// itself; then remove it.
    GoTo,
    /// `?`: the keys.
    Help,
}

/// The table: action, the key as shown, and what it does in a help line.
pub const TABLE: &[(Action, &str, &str)] = &[
    (Action::Back, "esc", "back"),
    (Action::Quit, "q", "quit; back when not at the top"),
    (Action::Undo, "u", "undo"),
    (Action::Edit, "e", "edit"),
    (Action::Yes, "y", "yes / approve"),
    (Action::No, "n", "no / refuse"),
    (Action::New, "+", "new"),
    (Action::Done, "d", "done"),
    (Action::Remove, "x", "remove (asks)"),
    (Action::Mark, "v", "mark several"),
    (Action::Search, "/", "search"),
    (Action::Refresh, "^r", "refresh"),
    (Action::Help, "?", "keys"),
];

/// The shared action a key press means, if any. Text fields must not call this: inside a form
/// every printable key is text.
pub fn action(key: KeyEvent) -> Option<Action> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    if alt {
        return None;
    }
    match key.code {
        KeyCode::Esc => Some(Action::Back),
        KeyCode::Char('r') if ctrl => Some(Action::Refresh),
        _ if ctrl => None,
        KeyCode::Char('q') => Some(Action::Quit),
        KeyCode::Char('u') => Some(Action::Undo),
        KeyCode::Char('e') => Some(Action::Edit),
        KeyCode::Char('y') => Some(Action::Yes),
        KeyCode::Char('n') => Some(Action::No),
        KeyCode::Char('+') => Some(Action::New),
        KeyCode::Char('d') => Some(Action::Done),
        KeyCode::Char('x') => Some(Action::Remove),
        KeyCode::Char('v') => Some(Action::Mark),
        KeyCode::Char('/') => Some(Action::Search),
        KeyCode::Char('?') => Some(Action::Help),
        _ => None,
    }
}

/// True when an app may bind `key` (as shown in its help, e.g. "r", "^l", "J") to its own
/// action: the table does not already own it.
pub fn app_key_is_free(key: &str) -> bool {
    !TABLE.iter().any(|(_, k, _)| *k == key)
}

/// Deprecated shim: the places `g` reached while it was a shared key (13 Nov, one evening).
/// Nothing in Home or the calendar uses it; keryx still names it for its own `g` until it
/// handles `g` itself. Not listed by [`help`]. Remove with [`Action::GoTo`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Place {
    Home,
    Approve,
    Tasks,
    Calendar,
    Mail,
    Log,
}

/// Deprecated shim, see [`Place`].
pub const PLACES: &[(char, Place, &str)] = &[
    ('h', Place::Home, "home"),
    ('a', Place::Approve, "approve"),
    ('t', Place::Tasks, "tasks"),
    ('c', Place::Calendar, "calendar"),
    ('m', Place::Mail, "mail"),
    ('l', Place::Log, "log"),
];

/// Deprecated shim, see [`Place`].
pub fn place(c: char) -> Option<Place> {
    PLACES.iter().find(|(k, _, _)| *k == c).map(|(_, p, _)| *p)
}

impl Place {
    /// The `locus` session's window that shows this place.
    pub fn view(self) -> &'static str {
        match self {
            Place::Home | Place::Approve | Place::Tasks | Place::Log => "home",
            Place::Calendar => "calendar",
            Place::Mail => "mail",
        }
    }
}

/// Show the `locus` session's window for `view` (home, calendar, mail) on this client, as its Alt key would. Outside tmux it does nothing and says so: tests and dumps never
/// touch tmux. A window that is missing is made by `locus-session.sh select`, found through the
/// `@locus-tmux-dir` option `locus.tmux` sets.
pub fn show_window(view: &str) -> Result<(), String> {
    if std::env::var_os("TMUX").is_none() {
        return Err(format!("not in tmux: the {view} window is Alt-{}", alt_of(view)));
    }
    let target = format!("=locus:={view}");
    if !tmux(&["select-window", "-t", &target]) {
        let script = format!("#{{@locus-tmux-dir}}/locus-session.sh select {view}");
        if !tmux(&["run-shell", &script]) {
            return Err(format!("no {view} window in the locus session"));
        }
    }
    if tmux(&["switch-client", "-t", &target]) {
        Ok(())
    } else {
        Err(format!("tmux did not switch to the {view} window"))
    }
}

/// The Alt key that shows a view's window (`locus.tmux`).
pub fn alt_of(view: &str) -> u8 {
    match view {
        "home" => 1,
        "calendar" => 2,
        "mail" => 3,
        _ => 4,
    }
}

fn tmux(args: &[&str]) -> bool {
    std::process::Command::new("tmux")
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// The help screen's lines: the shared table, then the app's own keys.
pub fn help(app_keys: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = TABLE
        .iter()
        .map(|(_, k, w)| ((*k).to_string(), (*w).to_string()))
        .collect();
    for (k, w) in app_keys {
        out.push(((*k).to_string(), (*w).to_string()));
    }
    out
}

/// [`help`] drawn for the `?` overlay, in two columns so it fits a window: on the left the
/// shared keys under "everywhere", on the right the app's own under
/// `app_title`. `width` is the overlay's inner width; descriptions are cut to fit.
pub fn help_lines(app_title: &str, app_keys: &[(&str, &str)], width: u16) -> Vec<ratatui::text::Line<'static>> {
    use ratatui::text::{Line, Span};
    let all = help(app_keys);
    let (left, right) = all.split_at(TABLE.len().min(all.len()));
    let col = usize::from(width) / 2;
    let kw = all.iter().map(|(k, _)| k.width()).max().unwrap_or(0).clamp(4, 14) + 2;
    let cell = |k: &str, w: &str| -> Vec<Span<'static>> {
        let key = crate::fit(k, kw);
        let text = crate::fit(w, col.saturating_sub(kw + 3));
        let used = 2 + kw + text.width();
        vec![
            Span::styled(
                format!("  {key}{}", " ".repeat(kw.saturating_sub(key.width()))),
                crate::keyst(),
            ),
            Span::styled(text, crate::st("text")),
            Span::raw(" ".repeat(col.saturating_sub(used))),
        ]
    };
    let head = |t: &str| Span::styled(format!(" {t:<w$}", w = col.saturating_sub(1)), crate::dim());
    let mut out = vec![Line::from(vec![head("everywhere"), head(app_title)])];
    for i in 0..left.len().max(right.len()) {
        let mut spans = match left.get(i) {
            Some((k, w)) => cell(k, w),
            None => vec![Span::raw(" ".repeat(col))],
        };
        if let Some((k, w)) = right.get(i) {
            spans.extend(cell(k, w));
        }
        out.push(Line::from(spans));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn every_table_key_parses_to_its_own_action() {
        for (a, key, _) in TABLE {
            let ev = match *key {
                "esc" => KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                "^r" => KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
                s => k(s.chars().next().unwrap()),
            };
            assert_eq!(action(ev), Some(*a), "{key}");
        }
    }

    #[test]
    fn plain_r_and_alt_keys_are_the_apps_own() {
        assert_eq!(action(k('r')), None);
        assert_eq!(action(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::ALT)), None);
        assert!(app_key_is_free("r") && !app_key_is_free("u"));
    }

    #[test]
    fn g_is_an_apps_own_key_and_help_lists_no_places() {
        assert_eq!(action(k('g')), None);
        assert!(app_key_is_free("g"));
        assert!(!TABLE.iter().any(|(a, _, _)| *a == Action::GoTo));
        let h = help(&[("r", "reply")]);
        assert_eq!(h.len(), TABLE.len() + 1);
        assert!(!h.iter().any(|(k, _)| k.starts_with("g ")) && h.last().unwrap().0 == "r");
    }
}
