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
    // Only what they wrote this time, one level deep (11 Nov: replies arrived with ">>>>>"
    // stacked five deep, "odd for people I am replying to to see"). Their message's own quoted
    // history and signature are left out, as `compact` does for summaries; the thread below is
    // still in every mail client's view of the conversation. A message that is ALL quote (some
    // phones send a reply that way) is quoted whole rather than as nothing.
    let own = crate::text::compact(&m.body_text);
    let own = own.strip_suffix("…[truncated]").unwrap_or(&own);
    let body = if own.trim().is_empty() {
        m.body_text.as_str()
    } else {
        own
    };
    let mut q = format!("On {when}, {who} wrote:\n");
    for line in body.lines() {
        if line.trim().is_empty() {
            q.push_str(">\n");
        } else {
            q.push_str("> ");
            q.push_str(line.trim_start_matches('>').trim_start());
            q.push('\n');
        }
    }
    q
}

/// The text you wrote, without a plain `> ` quote keryx used to append (11 Nov: that quote reached
/// people as literal `>` lines). Cut at the LAST "On … wrote:" line that only quoted lines follow,
/// so your own words, even a line starting "On", are never cut.
pub fn without_plain_quote(body: &str) -> &str {
    let lines: Vec<&str> = body.lines().collect();
    let mut cut = None;
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim();
        if t.starts_with("On ")
            && t.ends_with("wrote:")
            && lines[i + 1..]
                .iter()
                .all(|n| n.trim().is_empty() || n.trim_start().starts_with('>'))
        {
            cut = Some(i);
            break;
        }
    }
    match cut {
        Some(i) => {
            let at: usize = lines[..i].iter().map(|l| l.len() + 1).sum();
            body[..at.min(body.len())].trim_end()
        }
        None => body.trim_end(),
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Plain text as Mail writes it in HTML: a `<div>` a line, `<div><br></div>` for a blank one.
fn text_html(text: &str) -> String {
    text.lines()
        .map(|l| {
            if l.trim().is_empty() {
                "<div><br></div>".to_string()
            } else {
                format!("<div>{}</div>", escape(l))
            }
        })
        .collect()
}

/// What is inside a document's `<body>`, or the whole of it when it has none.
fn body_of(html: &str) -> &str {
    let lower = html.to_ascii_lowercase();
    let start = lower
        .find("<body")
        .and_then(|b| lower[b..].find('>').map(|e| b + e + 1));
    let end = lower.rfind("</body>");
    match (start, end) {
        (Some(s), Some(e)) if s <= e => &html[s..e],
        (Some(s), None) => &html[s..],
        _ => html,
    }
}

/// A reply in Mail's own format (spec keryx-thread-view R1; Mail's untouched reply, test A, 11 Nov):
/// your text, then "On 8 Nov 2026, at 22:30, Matilda Borthwick <…> wrote:" and their message, as
/// they sent it, in `<blockquote type="cite">`. Their history is inside their message, so it nests
/// the way every client folds it. `original_html` is their HTML part; without one their text.
pub fn reply_html(cfg: &Config, own: &str, to: &Message, original_html: Option<&str>) -> String {
    let when = cfg
        .tz()
        .timestamp_opt(to.date, 0)
        .single()
        .map(|d| d.format("On %-d %b %Y, at %H:%M").to_string())
        .unwrap_or_else(|| "On an unknown date".into());
    let who = match &to.from {
        Some(a) if a.name.as_deref().is_some_and(|n| !n.trim().is_empty()) => format!(
            "{} &lt;{}&gt;",
            escape(a.name.as_deref().unwrap_or_default().trim()),
            escape(&a.email)
        ),
        Some(a) => format!("&lt;{}&gt;", escape(&a.email)),
        None => "someone".into(),
    };
    let theirs = match original_html {
        Some(h) if !h.trim().is_empty() => body_of(h).to_string(),
        _ => text_html(&to.body_text),
    };
    format!(
        "<html><head><meta http-equiv=\"content-type\" content=\"text/html; charset=utf-8\"></head>\
<body style=\"overflow-wrap: break-word; -webkit-nbsp-mode: space; line-break: after-white-space;\">\
{mine}<div><br><blockquote type=\"cite\"><div>{when}, {who} wrote:</div><br class=\"Apple-interchange-newline\">\
<div>{theirs}</div></blockquote></div><br></body></html>",
        mine = text_html(without_plain_quote(own)),
    )
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
    fn a_plain_quote_keryx_appended_comes_off_and_your_words_stay() {
        let body = "Thanks, see you then.\n\nOn a related note, I wrote:\nnothing\n\nAlec\n\nOn Mon 12 Oct 2026 at 09:00, Jane Doe wrote:\n> Let's split it.\n>\n> Thanks\n";
        assert_eq!(
            without_plain_quote(body),
            "Thanks, see you then.\n\nOn a related note, I wrote:\nnothing\n\nAlec"
        );
        assert_eq!(without_plain_quote("Just this.\n"), "Just this.");
    }

    #[test]
    fn a_reply_is_written_in_mails_own_format() {
        let mut m = msg("oxford", "Jane Doe <jane.doe@example.com>", &["newc7370@ox.ac.uk"], &[]);
        m.body_text = "Plain & simple".into();
        let h = reply_html(
            &cfg(),
            "Yes, part two.\n\nAlec\n\nOn Mon 12 Oct 2026 at 09:00, Jane Doe wrote:\n> old\n",
            &m,
            Some("<html><head><style>x</style></head><body class=\"b\"><p>Her <b>HTML</b></p></body></html>"),
        );
        assert!(
            h.contains("<div>Yes, part two.</div><div><br></div><div>Alec</div><div><br><blockquote type=\"cite\">"),
            "{h}"
        );
        assert!(
            h.contains("<div>On 12 Oct 2026, at 09:00, Jane Doe &lt;jane.doe@example.com&gt; wrote:</div>"),
            "{h}"
        );
        assert!(h.contains("<div><p>Her <b>HTML</b></p></div></blockquote>"), "{h}");
        assert!(!h.contains("&gt; old") && !h.contains("<style>"), "{h}");
        // No HTML part: her text, escaped, a div a line.
        let h = reply_html(&cfg(), "Ok", &m, None);
        assert!(
            h.contains("<div><div>Plain &amp; simple</div></div></blockquote>"),
            "{h}"
        );
    }

    #[test]
    fn quote_format() {
        let m = msg("oxford", "Jane Doe <jane.doe@example.com>", &["newc7370@ox.ac.uk"], &[]);
        assert_eq!(
            quote(&cfg(), &m),
            "On Mon 12 Oct 2026 at 09:00, Jane Doe wrote:\n> Let's split it.\n>\n> Thanks\n",
            "one level only: her own quoted history is left out (11 Nov)"
        );
        // A long thread: her reply quotes his, which quotes hers. Only her new text is quoted.
        let mut deep = m.clone();
        deep.body_text =
            "Tuesday works.\n\nOn Mon, Alec wrote:\n> Could we meet?\n>> On Sun, Jane wrote:\n>>> hi".into();
        let q = quote(&cfg(), &deep);
        assert_eq!(q.matches('>').count(), 1, "{q}");
        assert!(q.contains("> Tuesday works.") && !q.contains("Could we meet"));
        // All quote (a phone reply): quoted whole, one level, not as nothing.
        let mut all = m.clone();
        all.body_text = "> I plan to renew my passport\n> next month".into();
        assert!(quote(&cfg(), &all).contains("> I plan to renew my passport"));
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
