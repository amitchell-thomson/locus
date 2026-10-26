//! What a thread is for, guessed (keryx-joins-locus X7): `d` opens with this already chosen,
//! and automated mail is filed by it as it arrives. Deterministic and explained, never a model:
//! the guess is one keypress from being corrected, and its reason is shown beside it.
//!
//! In order, the first that answers wins:
//! 1. the item this thread was already filed for;
//! 2. a registry item named in the subject or by the sender: a course's code (`B14`), a repo's
//!    name, an application's company (the rest of the application's name and being active
//!    settle between several with one company), with the opening text as a weaker signal;
//! 3. what you filed this sender's mail for before, when one answer dominates;
//! 4. receipts, bookings and orders to Errands.
//!
//! A tie gives no item rather than a coin toss. Text matching comes before sender history
//! because one sender (a careers service) writes about many items; history only breaks silence.

use crate::db::Db;
use keryx_core::model::Message;
use locus_contract::fors::{For, area_fors, fors};
use locus_contract::taxonomy::Shared;
use locus_contract::{Project, ProjectKind};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq)]
pub struct Guess {
    /// The option's id (`b14`, `errands`, `careers`).
    pub id: String,
    /// Why, in a few words: "B14 in the subject".
    pub why: String,
}

/// Every option keryx files for: registry items and Personal's streams, then the plain areas.
pub fn options(shared: &Shared) -> Vec<For> {
    let mut v = fors(shared);
    v.extend(area_fors(shared));
    v
}

/// Words that mark mail as a receipt, a booking or an order: Errands.
const ERRANDS: &[&str] = &[
    "receipt",
    "invoice",
    "your order",
    "order confirmation",
    "booking",
    "reservation",
    "payment",
    "delivery",
    "dispatched",
    "refund",
    "e-ticket",
    "your ticket",
];

/// Whether `needle` occurs in `hay` as a whole word (case ignored).
pub fn has_word(hay: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let h = hay.to_lowercase();
    let n = needle.to_lowercase();
    let mut from = 0;
    while let Some(i) = h[from..].find(&n) {
        let at = from + i;
        let before = h[..at].chars().next_back();
        let after = h[at + n.len()..].chars().next();
        let edge = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric());
        if edge(before) && edge(after) {
            return true;
        }
        from = at + n.len().max(1);
        if from >= h.len() {
            break;
        }
    }
    false
}

/// The words that name `p` in mail.
fn names(p: &Project) -> Vec<String> {
    match p.kind {
        ProjectKind::Course => vec![p.id.clone()],
        ProjectKind::Application => p.company.clone().into_iter().collect(),
        // Short repo ids (`cv`) are ordinary words in mail; a repo counts only by a real name.
        ProjectKind::Repo => [p.id.clone(), p.name.clone()]
            .into_iter()
            .filter(|n| n.chars().count() >= 4)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect(),
    }
}

/// How strongly the thread names `p`, and where.
fn score(p: &Project, subject: &str, sender: &str, body: &str) -> (f32, &'static str) {
    let mut best = (0.0f32, "");
    for n in names(p) {
        if has_word(subject, &n) {
            best = (best.0.max(3.0), "in the subject");
        } else if has_word(sender, &n) && best.0 < 3.0 {
            best = (3.0, "from the sender");
        } else if has_word(body, &n) && best.0 < 1.0 {
            best = (1.0, "in the text");
        }
    }
    if best.0 > 0.0 && p.kind == ProjectKind::Application {
        // Between applications of one company: the rest of the name, then being active.
        let company = p.company.clone().unwrap_or_default().to_lowercase();
        let extra = p
            .name
            .split_whitespace()
            .filter(|w| !company.contains(&w.to_lowercase()) && (w.len() >= 4 || w.chars().all(|c| c.is_ascii_digit())))
            .filter(|w| has_word(subject, w) || has_word(body, w))
            .count();
        best.0 += extra as f32 * 0.5 + if p.active { 0.25 } else { 0.0 };
    }
    best
}

/// The guess for a thread. `own` is your addresses (the sender is the latest other person).
pub fn guess(db: &Db, shared: &Shared, msgs: &[Message], own: &HashSet<String>, automated: bool) -> Option<Guess> {
    let first = msgs.first()?;
    if let Ok(Some(id)) = db.item_of(&first.account, &first.thread_id) {
        return Some(Guess {
            id,
            why: "filed for it before".into(),
        });
    }
    let other = msgs
        .iter()
        .rev()
        .find(|m| m.from.as_ref().is_some_and(|f| !own.contains(&f.key())))
        .or(msgs.last())?;
    let subject = &first.subject;
    let sender = other
        .from
        .as_ref()
        .map(|f| format!("{} {}", f.name.clone().unwrap_or_default(), f.email))
        .unwrap_or_default();
    let body: String = other.body_text.chars().take(3000).collect();

    let mut scored: Vec<(f32, &Project, &'static str)> = shared
        .registry
        .projects
        .iter()
        .map(|p| {
            let (s, w) = score(p, subject, &sender, &body);
            (s, p, w)
        })
        .filter(|(s, _, _)| *s >= 1.0)
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    if let Some((top, p, w)) = scored.first() {
        let tied = scored.get(1).is_some_and(|(s, _, _)| (s - top).abs() < f32::EPSILON);
        // The text alone (no subject, no sender) is too weak to file by.
        if *top >= 3.0 && !tied {
            let named = names(p).into_iter().next().unwrap_or_else(|| p.id.clone());
            return Some(Guess {
                id: p.id.clone(),
                why: format!("{} {w}", named),
            });
        }
    }

    if let Some(email) = other.from.as_ref().map(|f| f.key())
        && let Some((id, n)) = sender_history(db, &email)
    {
        return Some(Guess {
            id,
            why: format!("you filed {n} from this sender for it"),
        });
    }

    let low = subject.to_lowercase();
    // A bounce says "delivery" and is about your own mail, not an errand (30 of the 85 Errands
    // guesses on the real inbox, 26 Oct, before this line).
    let bounce = ["delivery status notification", "undeliverable", "mail delivery"]
        .iter()
        .any(|w| low.contains(w));
    if automated && !bounce && ERRANDS.iter().any(|w| low.contains(w)) {
        return Some(Guess {
            id: "errands".into(),
            why: "a receipt or a booking".into(),
        });
    }
    None
}

/// The item you filed this sender's threads for, when one answer is most of them.
fn sender_history(db: &Db, email: &str) -> Option<(String, usize)> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for (account, thread, item) in db.items().ok()? {
        let msgs = db.thread(&account, &thread).ok()?;
        if msgs.iter().any(|m| m.from.as_ref().is_some_and(|f| f.key() == email)) {
            *counts.entry(item).or_default() += 1;
        }
    }
    let total: usize = counts.values().sum();
    let (id, n) = counts.into_iter().max_by_key(|(_, n)| *n)?;
    (n * 3 >= total * 2).then_some((id, n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "testkit")]
    #[test]
    fn a_bounce_is_not_an_errand() {
        let (cfg, db) = crate::testkit::fixture_db();
        let s = shared();
        let mut m = db.thread("oxford", &db_first_thread(&db)).unwrap();
        m.truncate(1);
        m[0].subject = "Delivery Status Notification (Failure)".into();
        assert_eq!(guess(&db, &s, &m, &cfg.own_addresses(), true), None);
        m[0].subject = "Your parcel is out for delivery".into();
        assert_eq!(
            guess(&db, &s, &m, &cfg.own_addresses(), true).map(|g| g.id),
            Some("errands".into())
        );
    }

    #[cfg(feature = "testkit")]
    fn db_first_thread(db: &Db) -> String {
        db.conn()
            .query_row(
                "SELECT thread_id FROM messages WHERE account = 'oxford' LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap()
    }

    #[test]
    fn whole_words_only() {
        assert!(has_word("B14 lecture moved", "b14"));
        assert!(has_word("Re: [quast] build failed", "quast"));
        assert!(!has_word("B145 lecture", "b14"));
        assert!(!has_word("ab14", "b14"));
        assert!(has_word("Hello from G-Research!", "G-Research"));
    }

    fn shared() -> Shared {
        locus_contract::taxonomy::load(&locus_contract::taxonomy::root()).unwrap()
    }

    #[test]
    fn short_repo_ids_never_count_and_applications_settle_by_their_name() {
        let s = shared();
        let p = |id: &str| s.registry.get(id).unwrap();
        assert!(names(p("cv")).is_empty(), "\"send your CV\" is not the cv repo");
        let subject = "Meet Millennium 2026: your place is confirmed";
        let a = score(p("meet-millennium"), subject, "", "").0;
        let b = score(p("millennium"), subject, "", "").0;
        assert!(a > b, "{a} vs {b}");
    }
}
