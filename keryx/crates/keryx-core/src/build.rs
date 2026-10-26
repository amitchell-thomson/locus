//! Turning a parsed source message into a mirror [`Message`].

use crate::mime::Parsed;
use crate::model::{Location, Message};
use crate::text;

/// Mailboxes / labels keryx never mirrors: trash, spam, drafts, outbox.
/// Compared on the last path segment, case-insensitively.
pub fn is_hidden_mailbox(path: &str) -> bool {
    let last = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    matches!(
        last.as_str(),
        "trash"
            | "bin"
            | "deleted items"
            | "deleted messages"
            | "spam"
            | "junk"
            | "junk email"
            | "junk e-mail"
            | "drafts"
            | "outbox"
            | "conversation history"
            | "sync issues"
            | "conflicts"
            | "local failures"
            | "server failures"
    ) || matches!(path, "TRASH" | "SPAM" | "DRAFT")
}

/// Source facts that aren't in the RFC 5322 bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceFacts {
    pub account: String,
    pub source_id: String,
    pub thread_id: String,
    pub location: Location,
    pub in_inbox: bool,
    pub read: bool,
    pub flagged: bool,
    pub size: u64,
    /// Used when the Date header is missing (Gmail internalDate, Mail date_received).
    pub received: i64,
}

pub fn message(p: &Parsed, f: SourceFacts) -> Message {
    let (body_text, hidden_text_removed) = text::body_text(p);
    let snippet = text::snippet(&body_text);
    Message {
        account: f.account,
        source_id: f.source_id,
        message_id: p.message_id.clone(),
        thread_id: f.thread_id,
        from: p.from.clone(),
        to: p.to.clone(),
        cc: p.cc.clone(),
        bcc: p.bcc.clone(),
        reply_to: p.reply_to.clone(),
        subject: p.subject.clone(),
        date: if p.date > 0 { p.date } else { f.received },
        location: f.location,
        in_inbox: f.in_inbox,
        read: f.read,
        flagged: f.flagged,
        size: f.size,
        attachments: p.attachments.clone(),
        body_text,
        hidden_text_removed,
        snippet,
        in_reply_to: p.in_reply_to.clone(),
        references: p.references.clone(),
        list_headers: p.list_headers.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_mailboxes() {
        for h in [
            "[Gmail]/Bin",
            "Deleted Items",
            "Junk Email",
            "[Gmail]/Spam",
            "Drafts",
            "TRASH",
            "SPAM",
        ] {
            assert!(is_hidden_mailbox(h), "{h}");
        }
        for v in [
            "INBOX",
            "Inbox",
            "Archive",
            "[Gmail]/All Mail",
            "Sent Items",
            "Admin",
            "Label_1",
            "Trash talk",
        ] {
            assert!(!is_hidden_mailbox(v), "{v}");
        }
    }

    #[test]
    fn date_falls_back_to_received() {
        let p = Parsed::default();
        let m = message(
            &p,
            SourceFacts {
                account: "a".into(),
                source_id: "1".into(),
                thread_id: "t".into(),
                location: Location::Labels { labels: vec![] },
                in_inbox: false,
                read: true,
                flagged: false,
                size: 1,
                received: 42,
            },
        );
        assert_eq!(m.date, 42);
    }
}
