//! `^a` while writing: choose a file to attach by typing part of its path (keryx-joins-locus
//! X6). The files are the ones you are likely to send: Documents, Downloads and Desktop, the most
//! recently changed first.

use crate::app::Key;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Folders searched, below your home.
pub const ROOTS: &[&str] = &["Downloads", "Desktop", "Documents"];
const MAX_FILES: usize = 20_000;
const MAX_DEPTH: usize = 5;

#[derive(Debug, Clone, PartialEq)]
pub struct File {
    pub path: PathBuf,
    pub size: u64,
    pub modified: SystemTime,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Picker {
    pub query: String,
    pub sel: usize,
    files: Vec<File>,
}

/// What a key did.
#[derive(Debug, Clone, PartialEq)]
pub enum Picked {
    Still,
    Chosen(PathBuf),
    /// `^o`: open it to check, and stay in the picker.
    Open(PathBuf),
    Cancelled,
}

impl Picker {
    pub fn new(mut files: Vec<File>) -> Self {
        files.sort_by(|a, b| b.modified.cmp(&a.modified));
        Picker {
            query: String::new(),
            sel: 0,
            files,
        }
    }

    /// Every file under `ROOTS` in `home` (hidden ones and packages skipped).
    pub fn scan(home: &Path) -> Self {
        let mut out = Vec::new();
        for r in ROOTS {
            walk(&home.join(r), 0, &mut out);
        }
        Picker::new(out)
    }

    /// The files matching the query: every word is in the path, case ignored.
    pub fn matches(&self) -> Vec<&File> {
        let words: Vec<String> = self.query.split_whitespace().map(str::to_lowercase).collect();
        self.files
            .iter()
            .filter(|f| {
                let p = f.path.to_string_lossy().to_lowercase();
                words.iter().all(|w| p.contains(w.as_str()))
            })
            .take(200)
            .collect()
    }

    pub fn key(&mut self, k: Key) -> Picked {
        match k {
            Key::Esc => return Picked::Cancelled,
            Key::Enter => {
                return match self.matches().get(self.sel) {
                    Some(f) => Picked::Chosen(f.path.clone()),
                    None => Picked::Still,
                };
            }
            Key::Ctrl('o') => {
                if let Some(f) = self.matches().get(self.sel) {
                    return Picked::Open(f.path.clone());
                }
            }
            Key::Down | Key::Ctrl('n') | Key::Tab => {
                self.sel = (self.sel + 1).min(self.matches().len().saturating_sub(1))
            }
            Key::Up | Key::Ctrl('p') | Key::BackTab => self.sel = self.sel.saturating_sub(1),
            Key::Backspace => {
                self.query.pop();
                self.sel = 0;
            }
            Key::Ctrl('u') => {
                self.query.clear();
                self.sel = 0;
            }
            Key::Char(c) => {
                self.query.push(c);
                self.sel = 0;
            }
            _ => {}
        }
        Picked::Still
    }
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<File>) {
    if depth > MAX_DEPTH || out.len() >= MAX_FILES {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        let Ok(meta) = e.metadata() else { continue };
        let path = e.path();
        if meta.is_dir() {
            // An app bundle or a library is a folder to the system and one thing to you.
            if !name.ends_with(".app") && !name.ends_with(".photoslibrary") && name != "node_modules" {
                walk(&path, depth + 1, out);
            }
        } else if meta.is_file() {
            out.push(File {
                path,
                size: meta.len(),
                modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            });
            if out.len() >= MAX_FILES {
                return;
            }
        }
    }
}

/// `~/Documents/x.pdf` for a path under `home`.
pub fn tilde(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|h| path.strip_prefix(h).ok()) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn file(p: &str, age_s: u64) -> File {
        File {
            path: PathBuf::from(p),
            size: 1000,
            modified: SystemTime::now() - Duration::from_secs(age_s),
        }
    }

    #[test]
    fn newest_first_and_every_word_must_match() {
        let mut p = Picker::new(vec![
            file("/h/Documents/cv.pdf", 500),
            file("/h/Downloads/form.pdf", 10),
            file("/h/Documents/old form.docx", 5000),
        ]);
        assert_eq!(p.matches()[0].path, PathBuf::from("/h/Downloads/form.pdf"));
        for c in "FORM doc".chars() {
            p.key(Key::Char(c));
        }
        assert_eq!(p.matches().len(), 1);
        assert_eq!(
            p.key(Key::Enter),
            Picked::Chosen(PathBuf::from("/h/Documents/old form.docx"))
        );
        assert_eq!(p.key(Key::Esc), Picked::Cancelled);
    }

    #[test]
    fn scan_skips_hidden_files_and_bundles() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        std::fs::create_dir_all(h.join("Documents/sub")).unwrap();
        std::fs::create_dir_all(h.join("Downloads/Thing.app/Contents")).unwrap();
        std::fs::write(h.join("Documents/sub/a.pdf"), b"x").unwrap();
        std::fs::write(h.join("Documents/.hidden"), b"x").unwrap();
        std::fs::write(h.join("Downloads/Thing.app/Contents/bin"), b"x").unwrap();
        let p = Picker::scan(h);
        let names: Vec<String> = p.matches().iter().map(|f| tilde(&f.path, Some(h))).collect();
        assert_eq!(names, vec!["~/Documents/sub/a.pdf"]);
    }
}
