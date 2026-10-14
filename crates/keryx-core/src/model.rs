//! Mirror model shared by the store, daemon, CLI and TUI (SPEC §4).

use serde::{Deserialize, Serialize};

/// A parsed address. `email` is kept as written; compare with [`Address::key`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Address {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub email: String,
}

impl Address {
    pub fn new(name: Option<&str>, email: &str) -> Address {
        Address {
            name: name.map(str::trim).filter(|n| !n.is_empty()).map(str::to_string),
            email: email.trim().to_string(),
        }
    }

    /// Lowercased address for comparisons.
    pub fn key(&self) -> String {
        self.email.to_lowercase()
    }

    /// `Name <email>` or just `email`.
    pub fn display(&self) -> String {
        match &self.name {
            Some(n) => format!("{n} <{}>", self.email),
            None => self.email.clone(),
        }
    }

    /// Name if there is one, else the address.
    pub fn short(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.email)
    }
}

/// An attachment as listed in the mirror (bytes are fetched on demand).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    /// Stable within the message: the MIME part index.
    pub id: u32,
    pub name: String,
    pub mime: String,
    pub size: u64,
}

/// Where a message lives at its source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Location {
    /// Gmail: label ids (`INBOX`, `STARRED`, `Label_1`, …).
    Labels { labels: Vec<String> },
    /// Mail.app: the physical mailbox plus virtual label mailboxes
    /// (Gmail-over-IMAP), as paths below the account (`INBOX`, `[Gmail]/All Mail`).
    Mailboxes { physical: String, labels: Vec<String> },
}

impl Location {
    /// Every mailbox / label the message is in.
    pub fn all(&self) -> Vec<&str> {
        match self {
            Location::Labels { labels } => labels.iter().map(String::as_str).collect(),
            Location::Mailboxes { physical, labels } => std::iter::once(physical.as_str())
                .chain(labels.iter().map(String::as_str))
                .collect(),
        }
    }

    pub fn contains(&self, name: &str) -> bool {
        self.all().contains(&name)
    }
}

/// A message as stored in the mirror.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub account: String,
    /// Gmail message id, or the Mail.app ROWID as a string.
    pub source_id: String,
    /// The `Message-ID` header without angle brackets ("" if missing).
    pub message_id: String,
    pub thread_id: String,
    pub from: Option<Address>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub bcc: Vec<Address>,
    pub reply_to: Vec<Address>,
    pub subject: String,
    /// Unix seconds.
    pub date: i64,
    pub location: Location,
    pub in_inbox: bool,
    pub read: bool,
    pub flagged: bool,
    pub size: u64,
    pub attachments: Vec<Attachment>,
    pub body_text: String,
    pub hidden_text_removed: bool,
    pub snippet: String,
    /// `In-Reply-To` and `References`, without angle brackets.
    pub in_reply_to: Vec<String>,
    pub references: Vec<String>,
    /// Headers keryx reasons about (lowercased name → raw value): List-*,
    /// Precedence, Auto-Submitted, Feedback-ID, X-Auto-Response-Suppress.
    pub list_headers: Vec<(String, String)>,
}

/// `account:thread_id`, the reference used by the CLI, HTTP and MCP.
pub fn thread_ref(account: &str, thread_id: &str) -> String {
    format!("{account}:{thread_id}")
}

/// Split `account:thread_id`.
pub fn parse_thread_ref(r: &str) -> Option<(&str, &str)> {
    let (a, t) = r.split_once(':')?;
    (!a.is_empty() && !t.is_empty()).then_some((a, t))
}

/// The cross-link other apps keep (`keryx://thread/<account>/<thread_id>`).
pub fn keryx_url(account: &str, thread_id: &str) -> String {
    format!("keryx://thread/{account}/{thread_id}")
}

/// `keryx://thread/<account>/<thread_id>` → (account, thread_id).
pub fn parse_keryx_url(url: &str) -> Option<(String, String)> {
    let rest = url.trim().strip_prefix("keryx://thread/")?;
    let (a, t) = rest.split_once('/')?;
    (!a.is_empty() && !t.is_empty()).then(|| (a.to_string(), t.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keryx_urls_round_trip() {
        let u = keryx_url("oxford", "9");
        assert_eq!(u, "keryx://thread/oxford/9");
        assert_eq!(parse_keryx_url(&u), Some(("oxford".into(), "9".into())));
        assert_eq!(parse_keryx_url("keryx://thread/oxford/"), None);
        assert_eq!(parse_keryx_url("https://thread/oxford/9"), None);
    }

    #[test]
    fn address_display_and_key() {
        let a = Address::new(Some(" Jane Doe "), "Jane.Doe@Example.com ");
        assert_eq!(a.display(), "Jane Doe <Jane.Doe@Example.com>");
        assert_eq!(a.key(), "jane.doe@example.com");
        assert_eq!(Address::new(Some(""), "x@y.z").short(), "x@y.z");
    }

    #[test]
    fn thread_refs() {
        assert_eq!(thread_ref("oxford", "42"), "oxford:42");
        assert_eq!(parse_thread_ref("oxford:42"), Some(("oxford", "42")));
        assert_eq!(parse_thread_ref("personal:18f:x"), Some(("personal", "18f:x")));
        assert_eq!(parse_thread_ref(":42"), None);
        assert_eq!(parse_thread_ref("oxford"), None);
    }

    #[test]
    fn location_membership() {
        let l = Location::Mailboxes {
            physical: "[Gmail]/All Mail".into(),
            labels: vec!["INBOX".into()],
        };
        assert!(l.contains("INBOX") && l.contains("[Gmail]/All Mail"));
        assert!(!l.contains("Archive"));
    }
}
