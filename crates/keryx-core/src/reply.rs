//! Replies and forwards (SPEC §7): recipients, the From account, subjects,
//! threading headers and the quote.

use crate::config::Config;
use crate::model::{Address, Message};
use chrono::TimeZone;
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    /// The account id that sends it.
    pub account: String,
    pub from: Address,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub subject: String,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    /// Quoted text (or the forwarded message), kept below the marker.
    pub quote: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReplyError {
    #[error("nothing to reply to: the thread is empty")]
    EmptyThread,
    #[error("account {0:?} isn't configured")]
    UnknownAccount(String),
}

/// Strip any number of leading reply/forward prefixes (`Re:`, `RE:`, `AW:`,
/// `Fwd:`, `FW:`, `WG:`, `SV:`, `Antw:`, `Re[2]:`), returning the bare subject.
pub fn bare_subject(s: &str) -> &str {
    let mut s = s.trim();
    loop {
        let lower = s.to_ascii_lowercase();
        let mut stripped = None;
        for p in ["re", "aw", "fwd", "fw", "wg", "sv", "antw", "tr", "rv"] {
            if let Some(rest) = lower.strip_prefix(p) {
                // Optional "[n]" or "(n)" counter, then ':'.
                let rest = rest
                    .trim_start_matches(|c: char| c == '[' || c == '(' || c.is_ascii_digit() || c == ']' || c == ')');
                if let Some(after) = rest.strip_prefix(':') {
                    stripped = Some(s.len() - after.len());
                    break;
                }
            }
        }
        match stripped {
            Some(i) => s = s[i..].trim_start(),
            None => return s,
        }
    }
}

pub fn reply_subject(s: &str) -> String {
    format!("Re: {}", bare_subject(s))
}

pub fn forward_subject(s: &str) -> String {
    format!("Fwd: {}", bare_subject(s))
}

fn push_unique(list: &mut Vec<Address>, seen: &mut HashSet<String>, a: &Address) {
    if seen.insert(a.key()) {
        list.push(a.clone());
    }
}

/// Which account sends: the one whose address received `m`, else the holder.
pub fn sending_account<'a>(cfg: &'a Config, m: &Message) -> Option<&'a crate::config::Account> {
    let rcpts: Vec<String> = m.to.iter().chain(&m.cc).chain(&m.bcc).map(Address::key).collect();
    let from = m.from.as_ref().map(Address::key);
    // If Alec sent it, the account he sent from.
    if let Some(f) = &from
        && let Some(a) = cfg.accounts.iter().find(|a| a.addresses().any(|x| &x == f))
    {
        return Some(a);
    }
    // Prefer the holding account if it was addressed; else any addressed account.
    let holder = cfg.account(&m.account);
    if let Some(h) = holder
        && h.addresses().any(|x| rcpts.contains(&x))
    {
        return Some(h);
    }
    cfg.accounts
        .iter()
        .find(|a| a.addresses().any(|x| rcpts.contains(&x)))
        .or(holder)
}

/// Reply (or reply-all) to the last message of a thread.
pub fn reply(cfg: &Config, thread: &[Message], all: bool) -> Result<Draft, ReplyError> {
    let m = thread.last().ok_or(ReplyError::EmptyThread)?;
    let acc = sending_account(cfg, m).ok_or_else(|| ReplyError::UnknownAccount(m.account.clone()))?;
    let own = cfg.own_addresses();
    let from_me = m.from.as_ref().is_some_and(|f| own.contains(&f.key()));

    let mut seen: HashSet<String> = own.clone();
    let mut to = Vec::new();
    let mut cc = Vec::new();
    if from_me {
        // Following up on his own message: same recipients again.
        for a in &m.to {
            push_unique(&mut to, &mut seen, a);
        }
        if all {
            for a in &m.cc {
                push_unique(&mut cc, &mut seen, a);
            }
        }
    } else {
        let primary: Vec<&Address> = if m.reply_to.is_empty() {
            m.from.iter().collect()
        } else {
            m.reply_to.iter().collect()
        };
        for a in primary {
            push_unique(&mut to, &mut seen, a);
        }
        if all {
            for a in m.to.iter().chain(&m.cc) {
                push_unique(&mut cc, &mut seen, a);
            }
        }
    }

    let mut references = m.references.clone();
    if !m.message_id.is_empty() && !references.contains(&m.message_id) {
        references.push(m.message_id.clone());
    }
    Ok(Draft {
        account: acc.id.clone(),
        from: Address::new(None, &acc.address),
        to,
        cc,
        subject: reply_subject(&m.subject),
        in_reply_to: (!m.message_id.is_empty()).then(|| m.message_id.clone()),
        references,
        quote: quote(cfg, m),
    })
}

/// Forward the last message of a thread (recipients left empty).
pub fn forward(cfg: &Config, thread: &[Message]) -> Result<Draft, ReplyError> {
    let m = thread.last().ok_or(ReplyError::EmptyThread)?;
    let acc = sending_account(cfg, m).ok_or_else(|| ReplyError::UnknownAccount(m.account.clone()))?;
    let tz = cfg.tz();
    let date = tz
        .timestamp_opt(m.date, 0)
        .single()
        .map(|d| d.format("%a %-d %b %Y at %H:%M").to_string())
        .unwrap_or_default();
    let list = |v: &[Address]| v.iter().map(Address::display).collect::<Vec<_>>().join(", ");
    let mut q = String::from("---------- Forwarded message ----------\n");
    q.push_str(&format!(
        "From: {}\n",
        m.from.as_ref().map(Address::display).unwrap_or_default()
    ));
    q.push_str(&format!("Date: {date}\nSubject: {}\nTo: {}\n", m.subject, list(&m.to)));
    if !m.cc.is_empty() {
        q.push_str(&format!("Cc: {}\n", list(&m.cc)));
    }
    q.push('\n');
    q.push_str(&m.body_text);
    Ok(Draft {
        account: acc.id.clone(),
        from: Address::new(None, &acc.address),
        to: vec![],
        cc: vec![],
        subject: forward_subject(&m.subject),
        in_reply_to: None,
        references: vec![],
        quote: q,
    })
}

/// `On Tue 14 Oct 2026 at 14:03, Jane Doe wrote:` + `> ` lines.
pub fn quote(cfg: &Config, m: &Message) -> String {
    let when = cfg
        .tz()
        .timestamp_opt(m.date, 0)
        .single()
        .map(|d| d.format("%a %-d %b %Y at %H:%M").to_string())
        .unwrap_or_else(|| "an unknown date".into());
    let who = m
        .from
        .as_ref()
        .map(|a| a.short().to_string())
        .unwrap_or_else(|| "someone".into());
    let mut q = format!("On {when}, {who} wrote:\n");
    for line in m.body_text.lines() {
        if line.is_empty() {
            q.push_str(">\n");
        } else if line.starts_with('>') {
            q.push('>');
            q.push_str(line);
            q.push('\n');
        } else {
            q.push_str("> ");
            q.push_str(line);
            q.push('\n');
        }
    }
    q
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Location;
    use std::path::PathBuf;

    fn cfg() -> Config {
        let mut c =
            Config::load(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config/config.test.toml")).unwrap();
        c.accounts[1].aliases.push("alec.mitchell-thomson@new.ox.ac.uk".into());
        c
    }

    fn a(s: &str) -> Address {
        match s.split_once(" <") {
            Some((n, e)) => Address::new(Some(n), e.trim_end_matches('>')),
            None => Address::new(None, s),
        }
    }

    fn msg(account: &str, from: &str, to: &[&str], cc: &[&str]) -> Message {
        Message {
            account: account.into(),
            source_id: "1".into(),
            message_id: "orig@example.com".into(),
            thread_id: "t".into(),
            from: Some(a(from)),
            to: to.iter().map(|s| a(s)).collect(),
            cc: cc.iter().map(|s| a(s)).collect(),
            bcc: vec![],
            reply_to: vec![],
            subject: "Group project plan".into(),
            date: 1_791_792_000, // Mon 12 Oct 2026 09:00 BST
            location: Location::Labels { labels: vec![] },
            in_inbox: true,
            read: true,
            flagged: false,
            size: 0,
            attachments: vec![],
            body_text: "Let's split it.\n\n> earlier\nThanks".into(),
            hidden_text_removed: false,
            snippet: String::new(),
            in_reply_to: vec![],
            references: vec!["root@example.com".into()],
            list_headers: vec![],
        }
    }

    #[test]
    fn subjects_are_not_doubled() {
        for (s, want) in [
            ("Hello", "Re: Hello"),
            ("Re: Hello", "Re: Hello"),
            ("RE: re: Hello", "Re: Hello"),
            ("AW: Hello", "Re: Hello"),
            ("Re[2]: Hello", "Re: Hello"),
            ("Fwd: Re: Hello", "Re: Hello"),
            ("Return policy", "Re: Return policy"),
            ("Re:Hello", "Re: Hello"),
        ] {
            assert_eq!(reply_subject(s), want, "{s}");
        }
        assert_eq!(forward_subject("Re: Budget"), "Fwd: Budget");
        assert_eq!(forward_subject("FW: Budget"), "Fwd: Budget");
    }

    #[test]
    fn reply_goes_to_sender_and_reply_all_drops_all_of_alecs_addresses() {
        let m = msg(
            "oxford",
            "Jane Doe <jane.doe@example.com>",
            &["Alec <newc7370@ox.ac.uk>", "Sam <sam@example.com>"],
            &[
                "Team <team@example.com>",
                "alec.mitchellthomson@gmail.com",
                "SAM@example.com",
                "alec.mitchell-thomson@new.ox.ac.uk",
            ],
        );
        let r = reply(&cfg(), std::slice::from_ref(&m), false).unwrap();
        assert_eq!(
            r.to.iter().map(Address::display).collect::<Vec<_>>(),
            vec!["Jane Doe <jane.doe@example.com>"]
        );
        assert!(r.cc.is_empty());
        assert_eq!(r.account, "oxford");
        assert_eq!(r.subject, "Re: Group project plan");
        assert_eq!(r.in_reply_to.as_deref(), Some("orig@example.com"));
        assert_eq!(r.references, vec!["root@example.com", "orig@example.com"]);

        let all = reply(&cfg(), &[m], true).unwrap();
        assert_eq!(all.to.len(), 1);
        assert_eq!(
            all.cc.iter().map(Address::display).collect::<Vec<_>>(),
            vec!["Sam <sam@example.com>", "Team <team@example.com>"],
            "Alec's own addresses (incl. other accounts and aliases) and the duplicate Sam are gone"
        );
    }

    #[test]
    fn reply_to_header_wins() {
        let mut m = msg("oxford", "Jane Doe <jane.doe@example.com>", &["newc7370@ox.ac.uk"], &[]);
        m.reply_to = vec![a("Jane Personal <jane@personal.example.com>")];
        let r = reply(&cfg(), &[m], true).unwrap();
        assert_eq!(r.to[0].email, "jane@personal.example.com");
        assert!(r.cc.is_empty(), "Alec was the only other recipient");
    }

    #[test]
    fn following_up_on_alecs_own_message() {
        let m = msg(
            "oxford",
            "Alec <newc7370@ox.ac.uk>",
            &["Jane Doe <jane.doe@example.com>", "Sam <sam@example.com>"],
            &["Team <team@example.com>", "alec.mitchellthomson@gmail.com"],
        );
        let r = reply(&cfg(), std::slice::from_ref(&m), false).unwrap();
        assert_eq!(r.to.len(), 2);
        assert!(r.cc.is_empty());
        let all = reply(&cfg(), &[m], true).unwrap();
        assert_eq!(
            all.cc.iter().map(|x| x.email.as_str()).collect::<Vec<_>>(),
            vec!["team@example.com"]
        );
        assert_eq!(all.account, "oxford");
    }

    #[test]
    fn from_account_is_the_one_that_was_addressed() {
        // Held in Oxford's mailbox but addressed to the OQTS address.
        let m = msg("oxford", "x@example.com", &["alec@oqts.org"], &[]);
        assert_eq!(reply(&cfg(), &[m], false).unwrap().account, "oqts");
        // Addressed to nobody of Alec's (e.g. Bcc / list): the holding account.
        let m = msg("oqts-shared", "x@example.com", &["list@example.com"], &[]);
        let r = reply(&cfg(), &[m], false).unwrap();
        assert_eq!(
            (r.account.as_str(), r.from.email.as_str()),
            ("oqts-shared", "oqts@oqts.org")
        );
    }

    #[test]
    fn quote_format() {
        let m = msg("oxford", "Jane Doe <jane.doe@example.com>", &["newc7370@ox.ac.uk"], &[]);
        assert_eq!(
            quote(&cfg(), &m),
            "On Mon 12 Oct 2026 at 09:00, Jane Doe wrote:\n> Let's split it.\n>\n>> earlier\n> Thanks\n"
        );
    }

    #[test]
    fn forward_has_header_block_and_no_recipients() {
        let m = msg(
            "oxford",
            "Jane Doe <jane.doe@example.com>",
            &["newc7370@ox.ac.uk"],
            &["sam@example.com"],
        );
        let f = forward(&cfg(), &[m]).unwrap();
        assert!(f.to.is_empty() && f.in_reply_to.is_none());
        assert_eq!(f.subject, "Fwd: Group project plan");
        assert!(f.quote.starts_with("---------- Forwarded message ----------\nFrom: Jane Doe <jane.doe@example.com>\nDate: Mon 12 Oct 2026 at 09:00\n"));
        assert!(f.quote.contains("Cc: sam@example.com\n"));
        assert!(f.quote.ends_with("Thanks"));
    }

    #[test]
    fn empty_thread() {
        assert_eq!(reply(&cfg(), &[], false), Err(ReplyError::EmptyThread));
    }
}
