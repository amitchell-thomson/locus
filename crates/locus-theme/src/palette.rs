//! The palette `c()` draws from: Catppuccin Mocha, or, in an app that has called
//! [`follow_theme`], the current colour scheme.
//!
//! The scheme is chosen with `theme` (`~/local-projects/taxis`, one scheme written to every app:
//! Ghostty, tmux, nvim, the bar, borders and these screens). It writes
//! `~/.config/theme/palette.toml`: one `name = "#rrggbb"` line per Catppuccin colour name, the
//! names this crate already speaks. Following is opt-in, from each app's `main`, so library code
//! and tests keep Mocha and no snapshot changes with the scheme on the machine running them.
//!
//! A new scheme shows without a restart: the file's modification time is checked at most every
//! 400 ms from inside `c()`, which every frame calls anyway, so the next redraw after `theme
//! apply` is in the new colours. A missing or unreadable file means Mocha; a name the file does
//! not give keeps its Mocha value. Nothing here can panic a window.

use std::path::PathBuf;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use crate::MOCHA;

static FOLLOW: AtomicBool = AtomicBool::new(false);
static LOADED: RwLock<Option<Loaded>> = RwLock::new(None);
static FILE: RwLock<Option<PathBuf>> = RwLock::new(None);
const CHECK_EVERY: Duration = Duration::from_millis(400);

struct Loaded {
    colours: Vec<(String, (u8, u8, u8))>,
    checked: Instant,
    modified: Option<SystemTime>,
}

/// Draw from the current colour scheme (`~/.config/theme/palette.toml`) instead of the built-in
/// Mocha. Call once, early in `main`.
pub fn follow_theme() {
    follow(std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/theme/palette.toml")));
}

/// [`follow_theme`], from a palette file elsewhere (tests).
pub fn follow_theme_at(path: impl Into<PathBuf>) {
    follow(Some(path.into()));
}

fn follow(path: Option<PathBuf>) {
    if let Ok(mut f) = FILE.write() {
        *f = path;
    }
    FOLLOW.store(true, Ordering::Relaxed);
    refresh(true);
}

fn file() -> Option<PathBuf> {
    FILE.read().ok().and_then(|f| f.clone())
}

/// Read `name = "#rrggbb"` lines; anything else (comments, `mode`, unknown names) is skipped.
pub fn parse(text: &str) -> Vec<(String, (u8, u8, u8))> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        // the quoted value only: anything after it (a comment) is not part of it
        let v = v.trim();
        let v = match v.strip_prefix('"') {
            Some(rest) => rest.split('"').next().unwrap_or(""),
            None => v.split_whitespace().next().unwrap_or(""),
        };
        let k = k.trim();
        if k != "bg" && !MOCHA.iter().any(|(n, _)| *n == k) {
            continue;
        }
        let Some(hex) = v.strip_prefix('#') else { continue };
        if hex.len() != 6 {
            continue;
        }
        let Ok(n) = u32::from_str_radix(hex, 16) else { continue };
        out.push((k.to_string(), ((n >> 16) as u8, (n >> 8) as u8, n as u8)));
    }
    out
}

fn refresh(force: bool) {
    let now = Instant::now();
    if !force
        && let Ok(g) = LOADED.read()
        && let Some(l) = g.as_ref()
        && now.duration_since(l.checked) < CHECK_EVERY
    {
        return;
    }
    let path = file();
    let modified = path
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok());
    let Ok(mut g) = LOADED.write() else { return };
    if let Some(l) = g.as_mut() {
        l.checked = now;
        if !force && l.modified == modified {
            return;
        }
    }
    let colours = path
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| parse(&t))
        .unwrap_or_default();
    *g = Some(Loaded {
        colours,
        checked: now,
        modified,
    });
}

/// The terminal background every app is transparent over, when no scheme says otherwise:
/// Ghostty's own default (`ghostty +show-config --default`). Each rice sets its own, as `bg` in
/// the palette file; it is the colour `fade` blends towards (27 Nov: blending towards Mocha's
/// `base`, #1e1e2e, aimed at a colour that was never on screen).
pub const DEFAULT_BG: (u8, u8, u8) = (0x28, 0x2c, 0x34);

/// The colour for a Catppuccin name (or `bg`): the scheme's when following one, else Mocha's.
pub(crate) fn lookup(name: &str) -> Option<(u8, u8, u8)> {
    if FOLLOW.load(Ordering::Relaxed) {
        refresh(false);
        if let Ok(g) = LOADED.read()
            && let Some(rgb) = g.as_ref().and_then(|l| {
                l.colours
                    .iter()
                    .find(|(n, _)| n.eq_ignore_ascii_case(name))
                    .map(|(_, rgb)| *rgb)
            })
        {
            return Some(rgb);
        }
    }
    if name.eq_ignore_ascii_case("bg") {
        return Some(DEFAULT_BG);
    }
    MOCHA
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, rgb)| *rgb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_files_lines_and_skips_the_rest() {
        let p = parse(
            "# Generated by `theme`\nname = \"Gruvbox\"\nmode = \"dark\"\nbase = \"#282828\"\nblue = \"#83a598\"   # trailing\nbogus = \"#000000\"\nred = \"#zz0000\"\nbg = \"#101418\"\n",
        );
        assert_eq!(
            p,
            vec![
                ("base".into(), (0x28, 0x28, 0x28)),
                ("blue".into(), (0x83, 0xa5, 0x98)),
                ("bg".into(), (0x10, 0x14, 0x18))
            ]
        );
    }

    #[test]
    fn without_follow_theme_everything_is_mocha() {
        // tests never call follow_theme, so whatever scheme this machine has chosen, c() is Mocha
        assert_eq!(lookup("blue"), Some((0x89, 0xb4, 0xfa)));
        assert_eq!(lookup("nonsense"), None);
        assert_eq!(
            lookup("bg"),
            Some(DEFAULT_BG),
            "the background is Ghostty's, not Mocha's base"
        );
    }
}
