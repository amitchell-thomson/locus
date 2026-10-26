//! Outgoing mail (SPEC §6.4): RFC 5322 bytes for the Gmail API (Mail.app
//! builds its own through AppleScript), plus the checks shown on the
//! approval screen.

use crate::changeset::Outgoing;
use crate::model::{Address, Message};
use std::collections::HashSet;

/// A new Message-ID on a keryx-owned right-hand side.
pub fn new_message_id(from: &Address) -> String {
    let domain = from.email.rsplit_once('@').map(|(_, d)| d).unwrap_or("keryx.local");
    format!("{}@{domain}", uuid::Uuid::new_v4().simple())
}

fn addr(a: &Address) -> mail_builder::headers::address::Address<'static> {
    mail_builder::headers::address::Address::new_address(a.name.clone(), a.email.clone())
}

fn list(v: &[Address]) -> mail_builder::headers::address::Address<'static> {
    mail_builder::headers::address::Address::new_list(v.iter().map(addr).collect())
}

/// RFC 5322 bytes: plain text UTF-8, threading headers, attachments.
pub fn build(o: &Outgoing, date: i64, files: &[(String, String, Vec<u8>)]) -> std::io::Result<Vec<u8>> {
    let mut b = mail_builder::MessageBuilder::new()
        .from(addr(&o.from))
        .subject(o.subject.clone())
        .message_id(o.message_id.clone())
        .date(date)
        .text_body(o.body.clone());
    if !o.to.is_empty() {
        b = b.to(list(&o.to));
    }
    if !o.cc.is_empty() {
        b = b.cc(list(&o.cc));
    }
    if !o.bcc.is_empty() {
        b = b.bcc(list(&o.bcc));
    }
    if let Some(irt) = &o.in_reply_to {
        b = b.in_reply_to(irt.clone());
    }
    if !o.references.is_empty() {
        b = b.references(o.references.clone());
    }
    for (name, mime, bytes) in files {
        b = b.attachment(mime.clone(), name.clone(), bytes.clone());
    }
    b.write_to_vec()
}

/// Warnings for the approval / confirm screen (SPEC §6.3).
pub fn warnings(
    o: &Outgoing,
    sent_to: &HashSet<String>,
    own: &HashSet<String>,
    thread: &[Message],
    drafted_at: i64,
) -> Vec<String> {
    let mut w = Vec::new();
    let participants: HashSet<String> = thread
        .iter()
        .flat_map(|m| m.from.iter().chain(&m.to).chain(&m.cc).chain(&m.reply_to))
        .map(Address::key)
        .collect();
    for a in o.to.iter().chain(&o.cc).chain(&o.bcc) {
        let k = a.key();
        if own.contains(&k) {
            continue;
        }
        if !sent_to.contains(&k) {
            w.push(format!("⚠ new: {} — you've never written to this address", a.display()));
        }
        if !thread.is_empty() && !participants.contains(&k) {
            w.push(format!("⚠ outside this thread: {}", a.display()));
        }
    }
    let newer = thread.iter().filter(|m| m.date > drafted_at).count();
    if newer > 0 {
        w.push(format!(
            "⚠ {newer} new message{} since this was drafted",
            if newer == 1 { "" } else { "s" }
        ));
    }
    w
}

/// Every URL in a body (shown on the approval screen: data can leak
/// through query strings).
pub fn urls(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    for word in body.split(|c: char| c.is_whitespace() || c == '<' || c == '>' || c == '"' || c == '(' || c == ')') {
        let w = word.trim_end_matches(['.', ',', ';', ':', '!', '?', ']']);
        if (w.starts_with("http://") || w.starts_with("https://")) && !out.iter().any(|x: &String| x == w) {
            out.push(w.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changeset::Outgoing;

    fn o() -> Outgoing {
        Outgoing {
            account: "oxford".into(),
            thread_id: Some("9".into()),
            from: Address::new(Some("Alec"), "newc7370@ox.ac.uk"),
            to: vec![Address::new(Some("Jane Doe"), "jane.doe@example.com")],
            cc: vec![Address::new(None, "stranger@evil.example")],
            bcc: vec![],
            subject: "Re: Group project plan".into(),
            body: "I'll take part two. See https://example.com/x?id=1.\n\nAlec".into(),
            in_reply_to: Some("thread-a@example.com".into()),
            references: vec!["thread-a@example.com".into()],
            message_id: "abc@ox.ac.uk".into(),
            attachments: vec![],
        }
    }

    #[test]
    fn builds_a_parseable_reply() {
        let raw = build(
            &o(),
            1_791_986_400,
            &[("a.txt".into(), "text/plain".into(), b"hello".to_vec())],
        )
        .unwrap();
        let p = crate::mime::parse(&raw).unwrap();
        assert_eq!(p.from.unwrap().email, "newc7370@ox.ac.uk");
        assert_eq!(p.to[0].display(), "Jane Doe <jane.doe@example.com>");
        assert_eq!(p.in_reply_to, vec!["thread-a@example.com"]);
        assert_eq!(p.references, vec!["thread-a@example.com"]);
        assert_eq!(p.message_id, "abc@ox.ac.uk");
        assert_eq!(p.subject, "Re: Group project plan");
        assert!(p.text_plain.unwrap().contains("part two"));
        assert_eq!(p.attachments[0].name, "a.txt");
    }

    #[test]
    fn warnings_name_new_and_outside_recipients_and_late_replies() {
        let sent_to: HashSet<String> = ["jane.doe@example.com".to_string()].into();
        let own: HashSet<String> = ["newc7370@ox.ac.uk".to_string()].into();
        let thread = vec![Message {
            account: "oxford".into(),
            source_id: "10".into(),
            message_id: "thread-a@example.com".into(),
            thread_id: "9".into(),
            from: Some(Address::new(None, "jane.doe@example.com")),
            to: vec![Address::new(None, "newc7370@ox.ac.uk")],
            cc: vec![],
            bcc: vec![],
            reply_to: vec![],
            subject: "Group project plan".into(),
            date: 200,
            location: crate::model::Location::Labels { labels: vec![] },
            in_inbox: true,
            read: true,
            flagged: false,
            size: 0,
            attachments: vec![],
            body_text: String::new(),
            hidden_text_removed: false,
            snippet: String::new(),
            in_reply_to: vec![],
            references: vec![],
            list_headers: vec![],
        }];
        let w = warnings(&o(), &sent_to, &own, &thread, 100);
        assert_eq!(
            w,
            vec![
                "⚠ new: stranger@evil.example — you've never written to this address",
                "⚠ outside this thread: stranger@evil.example",
                "⚠ 1 new message since this was drafted",
            ]
        );
        assert!(warnings(&o(), &sent_to, &own, &thread, 300).len() == 2);
    }

    #[test]
    fn url_list() {
        assert_eq!(
            urls("see https://a.example/x?q=secret, and (http://b.example/)."),
            vec!["https://a.example/x?q=secret", "http://b.example/"]
        );
    }

    #[test]
    fn message_ids_use_the_senders_domain() {
        assert!(new_message_id(&Address::new(None, "a@ox.ac.uk")).ends_with("@ox.ac.uk"));
    }
}
