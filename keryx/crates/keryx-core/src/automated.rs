//! Automated-mail detection (SPEC §7). Deterministic, from headers and
//! labels only — never from anything a caller (Claude) claims. Risk policy
//! depends on it: Claude may triage automated threads without approval.

use crate::config::Automated as Overrides;
use crate::mime::list_header;
use crate::model::{Location, Message};
use std::collections::HashSet;

/// Sender local parts that mean "a machine sent this".
const MACHINE_LOCAL_PARTS: &[&str] = &[
    "noreply",
    "no-reply",
    "no_reply",
    "donotreply",
    "do-not-reply",
    "do_not_reply",
    "notifications",
    "notification",
    "mailer-daemon",
    "postmaster",
    "bounce",
    "bounces",
];

/// Gmail categories that count as automated (Personal account).
const GMAIL_CATEGORIES: &[&str] = &[
    "CATEGORY_PROMOTIONS",
    "CATEGORY_UPDATES",
    "CATEGORY_SOCIAL",
    "CATEGORY_FORUMS",
];

/// Why a message counts as automated (for explanations and tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    OverrideAlways,
    ListHeader,
    Precedence,
    AutoSubmitted,
    AutoResponseSuppress,
    FeedbackId,
    MachineSender,
    GmailCategory,
}

fn matches_override(list: &[String], email: &str) -> bool {
    let email = email.to_lowercase();
    let domain = email.rsplit_once('@').map(|(_, d)| d).unwrap_or("");
    list.iter().any(|e| {
        let e = e.trim().to_lowercase();
        match e.strip_prefix('@') {
            Some(d) => domain == d || domain.ends_with(&format!(".{d}")),
            None => e == email,
        }
    })
}

/// `Some(reason)` if the message is automated. `sent_to` holds every address
/// Alec has sent mail to (lowercased); `own` his own addresses.
pub fn classify(
    m: &Message,
    sent_to: &HashSet<String>,
    own: &HashSet<String>,
    overrides: &Overrides,
) -> Option<Reason> {
    let sender = m.from.as_ref().map(|a| a.key()).unwrap_or_default();
    // Alec's own messages are never automated: a reply makes a thread human.
    if own.contains(&sender) {
        return None;
    }
    if !sender.is_empty() && matches_override(&overrides.never, &sender) {
        return None;
    }
    if !sender.is_empty() && matches_override(&overrides.always, &sender) {
        return Some(Reason::OverrideAlways);
    }
    // Someone Alec writes to is a person, whatever their headers say
    // (mailing lists of real people, colleagues using a ticket system).
    if sent_to.contains(&sender) {
        return None;
    }
    let h = &m.list_headers;
    if list_header(h, "list-id").is_some() || list_header(h, "list-unsubscribe").is_some() {
        return Some(Reason::ListHeader);
    }
    if let Some(p) = list_header(h, "precedence")
        && matches!(p.trim().to_ascii_lowercase().as_str(), "bulk" | "list" | "junk")
    {
        return Some(Reason::Precedence);
    }
    if let Some(a) = list_header(h, "auto-submitted")
        && !a.trim().eq_ignore_ascii_case("no")
    {
        return Some(Reason::AutoSubmitted);
    }
    if let Some(a) = list_header(h, "x-auto-response-suppress")
        && a.split(',').any(|v| v.trim().eq_ignore_ascii_case("all"))
    {
        return Some(Reason::AutoResponseSuppress);
    }
    if list_header(h, "feedback-id").is_some() {
        return Some(Reason::FeedbackId);
    }
    let local = sender.split('@').next().unwrap_or("");
    if MACHINE_LOCAL_PARTS.contains(&local) {
        return Some(Reason::MachineSender);
    }
    if let Location::Labels { labels } = &m.location
        && labels.iter().any(|l| GMAIL_CATEGORIES.contains(&l.as_str()))
    {
        return Some(Reason::GmailCategory);
    }
    None
}

pub fn is_automated(m: &Message, sent_to: &HashSet<String>, own: &HashSet<String>, o: &Overrides) -> bool {
    classify(m, sent_to, own, o).is_some()
}

/// A thread is automated only if every message in it is.
pub fn thread_is_automated(msgs: &[Message], sent_to: &HashSet<String>, own: &HashSet<String>, o: &Overrides) -> bool {
    !msgs.is_empty() && msgs.iter().all(|m| is_automated(m, sent_to, own, o))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Address;

    fn msg(from: &str, headers: &[(&str, &str)]) -> Message {
        Message {
            account: "personal".into(),
            source_id: "1".into(),
            message_id: "x".into(),
            thread_id: "t".into(),
            from: Some(Address::new(None, from)),
            to: vec![],
            cc: vec![],
            bcc: vec![],
            reply_to: vec![],
            subject: String::new(),
            date: 0,
            location: Location::Labels {
                labels: vec!["INBOX".into()],
            },
            in_inbox: true,
            read: false,
            flagged: false,
            size: 0,
            attachments: vec![],
            body_text: String::new(),
            hidden_text_removed: false,
            snippet: String::new(),
            in_reply_to: vec![],
            references: vec![],
            list_headers: headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        }
    }

    fn none() -> HashSet<String> {
        HashSet::new()
    }

    fn c(m: &Message) -> Option<Reason> {
        classify(m, &none(), &none(), &Overrides::default())
    }

    #[test]
    fn each_header_rule() {
        type Case<'a> = (&'a [(&'a str, &'a str)], Option<Reason>);
        let cases: &[Case] = &[
            (&[("list-id", "<x.example.com>")], Some(Reason::ListHeader)),
            (&[("list-unsubscribe", "<mailto:u@x.com>")], Some(Reason::ListHeader)),
            (&[("precedence", "bulk")], Some(Reason::Precedence)),
            (&[("precedence", "List")], Some(Reason::Precedence)),
            (&[("precedence", "first-class")], None),
            (&[("auto-submitted", "auto-generated")], Some(Reason::AutoSubmitted)),
            (&[("auto-submitted", "no")], None),
            (
                &[("x-auto-response-suppress", "DR, OOF, AutoReply, All")],
                Some(Reason::AutoResponseSuppress),
            ),
            (&[("x-auto-response-suppress", "OOF")], None),
            (&[("feedback-id", "1:2:3")], Some(Reason::FeedbackId)),
            (&[], None),
        ];
        for (h, want) in cases {
            assert_eq!(c(&msg("jane@example.com", h)), *want, "{h:?}");
        }
    }

    #[test]
    fn machine_senders() {
        for s in [
            "no-reply@trains.example.co.uk",
            "NoReply@x.com",
            "notifications@github.com",
            "mailer-daemon@x.org",
        ] {
            assert_eq!(c(&msg(s, &[])), Some(Reason::MachineSender), "{s}");
        }
        assert_eq!(c(&msg("noreen@example.com", &[])), None);
    }

    #[test]
    fn gmail_categories() {
        let mut m = msg("shop@example.com", &[]);
        m.location = Location::Labels {
            labels: vec!["INBOX".into(), "CATEGORY_PROMOTIONS".into()],
        };
        assert_eq!(c(&m), Some(Reason::GmailCategory));
        m.location = Location::Labels {
            labels: vec!["CATEGORY_PERSONAL".into()],
        };
        assert_eq!(c(&m), None);
    }

    #[test]
    fn someone_alec_writes_to_is_never_automated() {
        let m = msg("Club@Sports.example.org", &[("list-id", "<club.sports.example.org>")]);
        let sent: HashSet<String> = ["club@sports.example.org".to_string()].into();
        assert_eq!(classify(&m, &sent, &none(), &Overrides::default()), None);
    }

    #[test]
    fn alec_himself_is_never_automated() {
        let m = msg("newc7370@ox.ac.uk", &[("precedence", "bulk")]);
        let own: HashSet<String> = ["newc7370@ox.ac.uk".to_string()].into();
        assert_eq!(classify(&m, &none(), &own, &Overrides::default()), None);
    }

    #[test]
    fn overrides() {
        let o = Overrides {
            always: vec!["@mailer.example.com".into(), "digest@ox.ac.uk".into()],
            never: vec!["@ox.ac.uk".into()],
            ..Default::default()
        };
        let cl = |from: &str, h: &[(&str, &str)]| classify(&msg(from, h), &none(), &none(), &o);
        assert_eq!(cl("x@eu.mailer.example.com", &[]), Some(Reason::OverrideAlways));
        // never wins over always, and over headers.
        assert_eq!(cl("digest@ox.ac.uk", &[]), None);
        assert_eq!(cl("it@ox.ac.uk", &[("precedence", "bulk")]), None);
        assert_eq!(cl("x@notox.ac.uk.evil.com", &[]), None);
    }

    #[test]
    fn one_human_message_makes_a_thread_people_mail() {
        let news = msg("news@x.com", &[("list-id", "x")]);
        let reply = msg("jane@example.com", &[]);
        let o = Overrides::default();
        assert!(thread_is_automated(std::slice::from_ref(&news), &none(), &none(), &o));
        assert!(!thread_is_automated(&[news.clone(), reply], &none(), &none(), &o));
        assert!(!thread_is_automated(&[], &none(), &none(), &o));
    }

    #[test]
    fn fixtures() {
        let dir = format!("{}/../../fixtures/eml", env!("CARGO_MANIFEST_DIR"));
        let load = |n: &str| {
            let p = crate::mime::parse(&std::fs::read(format!("{dir}/{n}")).unwrap()).unwrap();
            let mut m = msg("x@y.z", &[]);
            m.from = p.from;
            m.list_headers = p.list_headers;
            m
        };
        assert!(c(&load("16-newsletter.eml")).is_some());
        assert!(c(&load("17-noreply-receipt.eml")).is_some());
        assert!(c(&load("18-mailto-unsubscribe.eml")).is_some());
        assert!(c(&load("01-plain.eml")).is_none());
        assert!(c(&load("15-outlook-quote.eml")).is_none());
    }
}
