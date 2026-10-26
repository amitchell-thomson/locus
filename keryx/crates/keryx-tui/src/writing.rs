//! Writing a mail in your Neovim, in the right-hand pane with the list beside it
//! (keryx-joins-locus X6).
//!
//! keryx opens the mail file in Neovim on a [`Pane`], with `nvim/keryx.lua` loaded for that one
//! buffer: `:w` touches the `sent` file and quits (send), `:q` on a changed buffer writes the
//! draft file (keep), `:cq` exits non-zero (throw the draft away). The decision is read from those
//! files and the exit status here, so nothing has to be parsed out of the screen.
//!
//! The Lua is compiled into the binary and written next to the mail file at each start, so an
//! installed `keryx` needs nothing beside it, and your Neovim config is never touched.

use crate::pane::Pane;
use crate::send::Purpose;
use portable_pty::CommandBuilder;
use std::path::{Path, PathBuf};

const KERYX_LUA: &str = include_str!("../nvim/keryx.lua");
const BLINK_LUA: &str = include_str!("../nvim/lua/keryx_blink.lua");

/// How a writing session ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Finished {
    /// `:w`: the file as saved, for the confirm screen.
    Send(String),
    /// `:q` on a changed mail: kept as a draft.
    Kept,
    /// `:cq`: the draft thrown away.
    Discarded,
    /// `:q` with nothing changed.
    Closed,
}

pub struct Session {
    pub purpose: Purpose,
    pub pane: Pane,
    dir: PathBuf,
    file: PathBuf,
    sent: PathBuf,
    /// Where `:q` keeps a changed mail (`None`: nowhere, it is dropped).
    draft: Option<PathBuf>,
    /// The draft's modification time when the session started: a different one at the end
    /// means `:q` kept it.
    draft_was: Option<std::time::SystemTime>,
}

fn modified(p: Option<&PathBuf>) -> Option<std::time::SystemTime> {
    p.and_then(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
}

/// The editor to embed: `$VISUAL`, else `$EDITOR`, when it is Neovim; else `nvim` when it is on
/// the PATH. `None` means the editor runs full screen as before (any other editor).
pub fn embeddable_editor() -> Option<Vec<String>> {
    let named = std::env::var("VISUAL")
        .ok()
        .or_else(|| std::env::var("EDITOR").ok())
        .filter(|s| !s.trim().is_empty());
    match named {
        Some(e) => {
            let parts: Vec<String> = e.split_whitespace().map(str::to_string).collect();
            let prog = parts
                .first()
                .map(|p| Path::new(p).file_name().unwrap_or_default().to_os_string())?;
            (prog == "nvim").then_some(parts)
        }
        None => which("nvim").map(|p| vec![p.to_string_lossy().into_owned()]),
    }
}

fn which(prog: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join(prog))
            .find(|p| p.is_file())
    })
}

impl Session {
    /// Start `editor` on `text` in a `rows` × `cols` pane. `draft` is where `:q` keeps a changed
    /// mail. `nvim` gets keryx.lua; any other program is run with the same environment (tests).
    pub fn start(
        editor: &[String],
        text: &str,
        purpose: Purpose,
        draft: Option<PathBuf>,
        accounts: &[String],
        (rows, cols): (u16, u16),
        background: &str,
    ) -> Result<Self, String> {
        let dir = std::env::temp_dir().join(format!("keryx-writing-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(dir.join("lua")).map_err(|e| e.to_string())?;
        let file = dir.join("keryx-mail.eml");
        let sent = dir.join("sent");
        std::fs::write(&file, text).map_err(|e| e.to_string())?;
        std::fs::write(dir.join("keryx.lua"), KERYX_LUA).map_err(|e| e.to_string())?;
        std::fs::write(dir.join("lua").join("keryx_blink.lua"), BLINK_LUA).map_err(|e| e.to_string())?;
        if let Some(d) = draft.as_ref().and_then(|d| d.parent()) {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let Some(prog) = editor.first() else {
            return Err("no editor".into());
        };
        let mut cmd = CommandBuilder::new(prog);
        cmd.args(&editor[1..]);
        if Path::new(prog).file_name().is_some_and(|n| n == "nvim") {
            cmd.arg("-c");
            cmd.arg(format!("luafile {}", dir.join("keryx.lua").display()));
        }
        cmd.arg(&file);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("KERYX_FILE", &file);
        cmd.env("KERYX_SENT", &sent);
        if let Some(d) = &draft {
            cmd.env("KERYX_DRAFT", d);
        }
        cmd.env("KERYX_BIN", keryx_bin());
        // Your accounts as `Name <address>`, one a line: what the From: line offers.
        cmd.env("KERYX_ACCOUNTS", accounts.join("\n"));
        if let Some(home) = std::env::var_os("HOME") {
            cmd.cwd(home);
        }
        let draft_was = modified(draft.as_ref());
        let pane = Pane::spawn(cmd, rows, cols, background)?;
        Ok(Session {
            purpose,
            pane,
            dir,
            file,
            sent,
            draft,
            draft_was,
        })
    }

    /// `Some` once the editor has exited, with what that meant.
    pub fn finished(&mut self) -> Option<Finished> {
        let ok = self.pane.exited()?;
        Some(if !ok {
            if let Some(d) = &self.draft {
                let _ = std::fs::remove_file(d);
            }
            Finished::Discarded
        } else if self.sent.exists() {
            match std::fs::read_to_string(&self.file) {
                Ok(t) => Finished::Send(t),
                Err(_) => Finished::Closed,
            }
        } else if modified(self.draft.as_ref()).is_some_and(|t| Some(t) != self.draft_was) {
            Finished::Kept
        } else {
            Finished::Closed
        })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The keryx binary, for address completion (`keryx addresses …`).
fn keryx_bin() -> PathBuf {
    std::env::current_exe()
        .ok()
        .filter(|p| p.file_name().is_some_and(|n| n == "keryx"))
        .unwrap_or_else(|| PathBuf::from("keryx"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn purpose() -> Purpose {
        Purpose::Compose {
            account: "oxford".into(),
        }
    }

    fn run(script: &str, draft: Option<PathBuf>) -> Finished {
        let editor = vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            script.to_string(),
            "sh".to_string(),
        ];
        let mut s = Session::start(&editor, "To: \n\nhello\n", purpose(), draft, &[], (10, 40), "#1e1e2e").unwrap();
        let t = Instant::now();
        loop {
            if let Some(f) = s.finished() {
                return f;
            }
            assert!(t.elapsed() < Duration::from_secs(5), "the editor never exited");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_write_means_send_with_the_saved_text() {
        let f = run(r#"printf 'To: a@example.com\n\nhi\n' > "$1"; : > "$KERYX_SENT""#, None);
        assert_eq!(f, Finished::Send("To: a@example.com\n\nhi\n".into()));
    }

    #[test]
    fn quitting_a_changed_mail_keeps_a_draft_and_cq_throws_it_away() {
        let dir = tempfile::tempdir().unwrap();
        let draft = dir.path().join("drafts").join("new-oxford.eml");
        let f = run(
            r#"printf 'To: \n\nhalf a thought\n' > "$KERYX_DRAFT""#,
            Some(draft.clone()),
        );
        assert_eq!(f, Finished::Kept);
        assert_eq!(std::fs::read_to_string(&draft).unwrap(), "To: \n\nhalf a thought\n");
        let f = run("exit 1", Some(draft.clone()));
        assert_eq!(f, Finished::Discarded);
        assert!(!draft.exists());
    }

    #[test]
    fn quitting_with_nothing_changed_is_just_closed() {
        assert_eq!(run("exit 0", None), Finished::Closed);
    }
}
