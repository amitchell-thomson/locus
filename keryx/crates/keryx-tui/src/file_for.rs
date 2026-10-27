//! `d` (done) files the thread for what it is for (keryx-joins-locus X7); `m` marks what it is
//! for and leaves it in the inbox (round 4: an application's mail shows on its page while it
//! still waits for you). Both open on the mark, else keryx's guess. `⏎` takes the guess, typing picks another (the same type-ahead as
//! kairos's For), and "for nothing" only archives. The folder follows from the answer.
//!
//! With no guess nothing is chosen until you type or move: `⏎` on a blank picker never files
//! mail under whatever happened to be listed first.

use crate::app::{App, Key, Mode};
use keryx_store::api::Intent;
use keryx_store::item::{self, Guess};
use locus_contract::fors::{For, matching};

#[derive(Debug, Clone, PartialEq)]
pub struct ForPicker {
    pub query: String,
    /// The highlighted row, `None` until there is something to choose.
    pub sel: Option<usize>,
    pub guess: Option<Guess>,
    /// Where Esc and a choice return to.
    pub back: Box<Mode>,
    /// `m`: mark what it is for and leave it in the inbox; `d`: file it.
    pub mark: bool,
}

/// A row of the picker.
#[derive(Debug, Clone, PartialEq)]
pub enum Choice {
    For(For),
    /// Archive only: done, and for nothing in particular.
    Nothing,
}

impl Choice {
    pub fn label(&self) -> String {
        match self {
            Choice::For(f) => f.label.clone(),
            Choice::Nothing => "for nothing: clear the mark".into(),
        }
    }
}

impl App {
    /// `d`: done. Filed for what the threads were marked for (`m`) when they all agree, with no
    /// question; else the picker, the guess chosen. No taxonomy: a plain archive.
    pub fn done(&mut self) {
        let refs = self.targets();
        if refs.is_empty() {
            return;
        }
        if self.shared.is_none() {
            self.triage_intent(Intent::Archive, "archiving");
            return;
        }
        if let Some(o) = self.marked_option(&refs) {
            let doing = format!("filing for {}:", o.label);
            self.triage_intent(
                Intent::File {
                    key: o.folder,
                    item: o.item,
                },
                &doing,
            );
            return;
        }
        self.open_for_picker(false);
    }

    /// The option every target is marked for, when they are all marked alike.
    fn marked_option(&self, refs: &[String]) -> Option<For> {
        let shared = self.shared.as_ref()?;
        let mut mark: Option<&(String, String)> = None;
        for r in refs {
            let (a, t) = keryx_core::model::parse_thread_ref(r)?;
            let m = self.marks.get(&(a.to_string(), t.to_string()))?;
            if mark.is_some_and(|x| x != m) {
                return None;
            }
            mark = Some(m);
        }
        let (kind, target) = mark?;
        item::options(shared).into_iter().find(|o| match kind.as_str() {
            "item" => o.item.as_deref() == Some(target.as_str()),
            _ => o.item.is_none() && o.folder.eq_ignore_ascii_case(target),
        })
    }

    /// The picker on the targets: `mark` keeps them in the inbox (`m`), else files them (`d`).
    pub fn open_for_picker(&mut self, mark: bool) {
        let refs = self.targets();
        if refs.is_empty() || self.shared.is_none() {
            return;
        }
        // A thread already marked opens on its mark; else on keryx's guess.
        let guess = self
            .marked_option(&refs)
            .map(|o| Guess {
                id: o.id,
                why: "marked for it".into(),
            })
            .or_else(|| self.guess_for(&refs));
        self.mode = Mode::File(Box::new(ForPicker {
            query: String::new(),
            sel: guess.as_ref().map(|_| 0),
            guess,
            back: Box::new(self.mode.clone()),
            mark,
        }));
    }

    /// One guess for all the targets, or none when they disagree.
    fn guess_for(&self, refs: &[String]) -> Option<Guess> {
        let shared = self.shared.as_ref()?;
        let own = self.cfg.own_addresses();
        let sent_to = keryx_store::view::sent_to(&self.db, &self.cfg).unwrap_or_default();
        let mut found: Option<Guess> = None;
        for r in refs {
            let (a, t) = keryx_core::model::parse_thread_ref(r)?;
            let msgs = self.db.thread(a, t).ok()?;
            let automated = keryx_core::automated::thread_is_automated(&msgs, &sent_to, &own, &self.cfg.automated);
            let g = item::guess(&self.db, shared, &msgs, &own, automated)?;
            match &found {
                Some(f) if f.id != g.id => return None,
                Some(_) => {}
                None => found = Some(g),
            }
        }
        found
    }

    /// The picker's rows for what is typed: the guess first, then the rest; "for nothing" last.
    pub fn for_choices(&self, p: &ForPicker) -> Vec<Choice> {
        let Some(shared) = &self.shared else { return vec![] };
        let all = item::options(shared);
        let mut v: Vec<Choice> = Vec::new();
        if p.query.trim().is_empty() {
            if let Some(g) = &p.guess
                && let Some(o) = all.iter().find(|o| o.id == g.id)
            {
                v.push(Choice::For(o.clone()));
            }
            v.extend(
                all.iter()
                    .filter(|o| p.guess.as_ref().is_none_or(|g| g.id != o.id))
                    .cloned()
                    .map(Choice::For),
            );
            // Only m can say "for nothing" (clear the mark): d files, and e archives outright.
            if p.mark {
                v.push(Choice::Nothing);
            }
        } else {
            v.extend(
                matching(&all, &p.query, |o| (o.id.as_str(), o.label.as_str()))
                    .into_iter()
                    .cloned()
                    .map(Choice::For),
            );
            let q = p.query.trim().to_lowercase();
            if p.mark && ("nothing".starts_with(&q) || "clear".starts_with(&q)) {
                v.push(Choice::Nothing);
            }
        }
        v
    }

    pub fn for_key(&mut self, k: Key, mut p: ForPicker) {
        let n = self.for_choices(&p).len();
        match k {
            Key::Esc => {
                self.mode = *p.back;
                return;
            }
            Key::Enter => {
                let Some(c) = p.sel.and_then(|i| self.for_choices(&p).into_iter().nth(i)) else {
                    self.message = Some(("type what it is for, or ↓ to choose".into(), false));
                    self.mode = Mode::File(Box::new(p));
                    return;
                };
                self.mode = *p.back;
                if p.mark {
                    let refs = self.targets();
                    let (id, said) = match &c {
                        Choice::For(o) => (Some(o.id.clone()), format!("marked for {}", o.label)),
                        Choice::Nothing => (None, "mark cleared".to_string()),
                    };
                    for r in &refs {
                        if let Some((a, t)) = keryx_core::model::parse_thread_ref(r) {
                            let key = (a.to_string(), t.to_string());
                            match &c {
                                // Shown at once; the mirror confirms within seconds.
                                Choice::For(o) => {
                                    let kind = if o.item.is_some() { "item" } else { "folder" };
                                    let target = o.item.clone().unwrap_or_else(|| o.folder.clone());
                                    self.marks.insert(key, (kind.to_string(), target));
                                }
                                Choice::Nothing => {
                                    self.marks.remove(&key);
                                }
                            }
                        }
                    }
                    self.deferred.push(crate::write::Job::MarkFor(refs, id));
                    self.message = Some((said, false));
                    self.marked.clear();
                    return;
                }
                match c {
                    Choice::Nothing => self.triage_intent(Intent::Archive, "archiving"),
                    Choice::For(o) => {
                        let doing = format!("filing for {}:", o.label);
                        self.triage_intent(
                            Intent::File {
                                key: o.folder,
                                item: o.item,
                            },
                            &doing,
                        );
                    }
                }
                return;
            }
            Key::Down | Key::Tab | Key::Ctrl('n') if n > 0 => {
                p.sel = Some(p.sel.map_or(0, |i| (i + 1).min(n - 1)));
            }
            Key::Up | Key::BackTab | Key::Ctrl('p') => p.sel = p.sel.map(|i| i.saturating_sub(1)),
            Key::Backspace => {
                p.query.pop();
                p.sel = (!p.query.is_empty() || p.guess.is_some()).then_some(0);
            }
            Key::Ctrl('u') => {
                p.query.clear();
                p.sel = p.guess.as_ref().map(|_| 0);
            }
            Key::Char(c) => {
                p.query.push(c);
                p.sel = Some(0);
            }
            _ => {}
        }
        if p.sel.is_some_and(|i| i >= self.for_choices(&p).len()) {
            p.sel = None;
        }
        self.mode = Mode::File(Box::new(p));
    }
}
