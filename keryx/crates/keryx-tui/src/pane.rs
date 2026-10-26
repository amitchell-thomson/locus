//! A terminal inside a pane (keryx-joins-locus X6): your Neovim runs in the right half of the
//! mail view, the list still beside it.
//!
//! portable-pty runs the child on a pseudo-terminal; a reader thread feeds what it prints to a
//! vt100 parser; `tui_term` draws the parser's screen. Keys go the other way as the bytes a
//! terminal would send.
//!
//! The parser answers the few questions Neovim asks its terminal at start (device attributes,
//! cursor position, background colour). Unanswered, each one is a wait before the first frame,
//! and the background guess decides whether your colourscheme starts light or dark.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tui_term::vt100;

/// What the child asked of its terminal, answered after each read.
#[derive(Default)]
pub struct Answers {
    out: Vec<u8>,
    /// The background reported to OSC 11, as `rgb:rrrr/gggg/bbbb`.
    background: String,
}

impl vt100::Callbacks for Answers {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        _i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        let first = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        match (i1, c, first) {
            // Primary device attributes: a VT220 with ANSI colour.
            (None, 'c', 0) => self.out.extend_from_slice(b"\x1b[?62;22c"),
            // Cursor position report.
            (None, 'n', 6) => {
                let (r, col) = screen.cursor_position();
                self.out
                    .extend_from_slice(format!("\x1b[{};{}R", r + 1, col + 1).as_bytes());
            }
            // Status report: OK.
            (None, 'n', 5) => self.out.extend_from_slice(b"\x1b[0n"),
            _ => {}
        }
    }

    fn unhandled_osc(&mut self, _: &mut vt100::Screen, params: &[&[u8]]) {
        if params.first() == Some(&&b"11"[..]) && params.get(1) == Some(&&b"?"[..]) {
            self.out
                .extend_from_slice(format!("\x1b]11;{}\x1b\\", self.background).as_bytes());
        }
    }
}

type Parser = vt100::Parser<Answers>;

pub struct Pane {
    parser: Arc<Mutex<Parser>>,
    master: Box<dyn MasterPty + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Box<dyn Child + Send + Sync>,
    rows: u16,
    cols: u16,
    changed: Arc<AtomicBool>,
}

impl Pane {
    /// Run `cmd` on a `rows` × `cols` terminal. `background` is the theme's base colour as
    /// `#rrggbb`, reported to the child when it asks.
    pub fn spawn(cmd: CommandBuilder, rows: u16, cols: u16, background: &str) -> Result<Self, String> {
        let (rows, cols) = (rows.max(2), cols.max(10));
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| e.to_string())?;
        let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer = Arc::new(Mutex::new(pair.master.take_writer().map_err(|e| e.to_string())?));
        let answers = Answers {
            out: Vec::new(),
            background: osc_colour(background),
        };
        let parser = Arc::new(Mutex::new(vt100::Parser::new_with_callbacks(rows, cols, 0, answers)));
        let changed = Arc::new(AtomicBool::new(true));
        {
            let (parser, writer, changed) = (parser.clone(), writer.clone(), changed.clone());
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let reply = match parser.lock() {
                                Ok(mut p) => {
                                    p.process(&buf[..n]);
                                    std::mem::take(&mut p.callbacks_mut().out)
                                }
                                Err(_) => break,
                            };
                            if !reply.is_empty()
                                && let Ok(mut w) = writer.lock()
                            {
                                let _ = w.write_all(&reply);
                                let _ = w.flush();
                            }
                            changed.store(true, Ordering::Release);
                        }
                    }
                }
                changed.store(true, Ordering::Release);
            });
        }
        Ok(Pane {
            parser,
            master: pair.master,
            writer,
            child,
            rows,
            cols,
            changed,
        })
    }

    /// Follow the pane's size (the child gets SIGWINCH).
    pub fn resize(&mut self, rows: u16, cols: u16) {
        let (rows, cols) = (rows.max(2), cols.max(10));
        if (rows, cols) == (self.rows, self.cols) {
            return;
        }
        self.rows = rows;
        self.cols = cols;
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        if let Ok(mut p) = self.parser.lock() {
            p.screen_mut().set_size(rows, cols);
        }
    }

    pub fn send(&mut self, bytes: &[u8]) {
        if let Ok(mut w) = self.writer.lock() {
            let _ = w.write_all(bytes);
            let _ = w.flush();
        }
    }

    pub fn key(&mut self, k: KeyEvent) {
        let app_cursor = self
            .parser
            .lock()
            .map(|p| p.screen().application_cursor())
            .unwrap_or(false);
        let bytes = encode(k.code, k.modifiers, app_cursor);
        self.send(&bytes);
    }

    /// Pasted text, bracketed when the child asked for that.
    pub fn paste(&mut self, text: &str) {
        let bracketed = self
            .parser
            .lock()
            .map(|p| p.screen().bracketed_paste())
            .unwrap_or(false);
        if bracketed {
            self.send(format!("\x1b[200~{text}\x1b[201~").as_bytes());
        } else {
            self.send(text.as_bytes());
        }
    }

    /// `Some(success)` once the child has exited.
    pub fn exited(&mut self) -> Option<bool> {
        self.child.try_wait().ok().flatten().map(|s| s.success())
    }

    /// Whether the screen changed since the last call.
    pub fn take_changed(&self) -> bool {
        self.changed.swap(false, Ordering::AcqRel)
    }

    pub fn with_screen<R>(&self, f: impl FnOnce(&vt100::Screen) -> R) -> Option<R> {
        self.parser.lock().ok().map(|p| f(p.screen()))
    }
}

impl Drop for Pane {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
    }
}

/// `#1e1e2e` as `rgb:1e1e/1e1e/2e2e`.
fn osc_colour(hex: &str) -> String {
    let h = hex.trim_start_matches('#');
    let part = |i: usize| h.get(i..i + 2).unwrap_or("00");
    format!("rgb:{0}{0}/{1}{1}/{2}{2}", part(0), part(2), part(4))
}

/// The bytes a terminal sends for a key.
pub fn encode(code: KeyCode, mods: KeyModifiers, app_cursor: bool) -> Vec<u8> {
    let alt = mods.contains(KeyModifiers::ALT);
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    let shift = mods.contains(KeyModifiers::SHIFT);
    // xterm's modifier parameter: 1 + shift + 2·alt + 4·ctrl.
    let m = 1 + u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(ctrl);
    let cursor = |c: char| -> Vec<u8> {
        if m > 1 {
            format!("\x1b[1;{m}{c}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{c}").into_bytes()
        } else {
            format!("\x1b[{c}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if m > 1 {
            format!("\x1b[{n};{m}~").into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };
    let mut out = match code {
        KeyCode::Char(c) if ctrl => match c.to_ascii_lowercase() {
            c @ 'a'..='z' => vec![c as u8 & 0x1f],
            ' ' | '@' | '2' => vec![0],
            '[' | '3' => vec![0x1b],
            '\\' | '4' => vec![0x1c],
            ']' | '5' => vec![0x1d],
            '^' | '6' => vec![0x1e],
            '_' | '7' | '-' => vec![0x1f],
            '?' | '8' => vec![0x7f],
            c => c.to_string().into_bytes(),
        },
        KeyCode::Char(c) => c.to_string().into_bytes(),
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => cursor('A'),
        KeyCode::Down => cursor('B'),
        KeyCode::Right => cursor('C'),
        KeyCode::Left => cursor('D'),
        KeyCode::Home => cursor('H'),
        KeyCode::End => cursor('F'),
        KeyCode::Insert => tilde(2),
        KeyCode::Delete => tilde(3),
        KeyCode::PageUp => tilde(5),
        KeyCode::PageDown => tilde(6),
        KeyCode::F(n @ 1..=4) => format!("\x1bO{}", (b'P' + n - 1) as char).into_bytes(),
        KeyCode::F(n @ 5..=12) => tilde([15, 17, 18, 19, 20, 21, 23, 24][usize::from(n - 5)]),
        _ => vec![],
    };
    // Alt is ESC first, for the keys that carry no modifier parameter.
    if alt
        && matches!(
            code,
            KeyCode::Char(_) | KeyCode::Enter | KeyCode::Backspace | KeyCode::Esc
        )
    {
        out.insert(0, 0x1b);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn keys_are_the_bytes_a_terminal_sends() {
        let none = KeyModifiers::NONE;
        assert_eq!(encode(KeyCode::Char('a'), none, false), b"a");
        assert_eq!(encode(KeyCode::Char('é'), none, false), "é".as_bytes());
        assert_eq!(encode(KeyCode::Char('w'), KeyModifiers::CONTROL, false), [0x17]);
        assert_eq!(encode(KeyCode::Char('x'), KeyModifiers::ALT, false), b"\x1bx");
        assert_eq!(encode(KeyCode::Esc, none, false), [0x1b]);
        assert_eq!(encode(KeyCode::Up, none, false), b"\x1b[A");
        assert_eq!(encode(KeyCode::Up, none, true), b"\x1bOA");
        assert_eq!(encode(KeyCode::Left, KeyModifiers::CONTROL, true), b"\x1b[1;5D");
        assert_eq!(encode(KeyCode::Delete, none, false), b"\x1b[3~");
        assert_eq!(encode(KeyCode::F(5), none, false), b"\x1b[15~");
        assert_eq!(encode(KeyCode::F(1), none, false), b"\x1bOP");
    }

    #[test]
    fn the_background_is_reported_as_xterm_spells_it() {
        assert_eq!(osc_colour("#1e1e2e"), "rgb:1e1e/1e1e/2e2e");
    }

    fn wait(pane: &Pane, what: impl Fn(&str) -> bool) -> String {
        let start = Instant::now();
        loop {
            let s = pane.with_screen(|s| s.contents()).unwrap_or_default();
            if what(&s) || start.elapsed() > Duration::from_secs(5) {
                return s;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_child_draws_reads_keys_and_exits() {
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.args(["-c", "printf 'ready\\n'; read line; printf 'got %s\\n' \"$line\""]);
        let mut pane = Pane::spawn(cmd, 6, 30, "#1e1e2e").unwrap();
        assert!(wait(&pane, |s| s.contains("ready")).contains("ready"));
        for c in "hello".chars() {
            pane.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        pane.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(wait(&pane, |s| s.contains("got hello")).contains("got hello"));
        let start = Instant::now();
        while pane.exited().is_none() && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(pane.exited(), Some(true));
    }

    #[test]
    fn questions_to_the_terminal_are_answered() {
        // Ask for the cursor position, then print what came back on stdin.
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.args([
            "-c",
            "stty raw -echo; printf 'ab\\033[6n'; r=$(dd bs=1 count=6 2>/dev/null); stty sane; printf '\\n%s\\n' \"$r\" | od -c | head -1",
        ]);
        let pane = Pane::spawn(cmd, 6, 40, "#1e1e2e").unwrap();
        let s = wait(&pane, |s| {
            s.contains("[   1   ;   3   R") || s.contains("[  1  ;  3  R")
        });
        assert!(s.contains('R') && s.contains('3'), "{s}");
    }
}
