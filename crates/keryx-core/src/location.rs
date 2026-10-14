//! Where archive / un-archive / filing puts a message, per backend (SPEC §7):
//!
//! - **Gmail API** (labels): archive removes `INBOX`; filing adds the key's
//!   label id and removes `INBOX`.
//! - **Mail.app, Exchange** (real folders): the message moves to the
//!   archive / key mailbox, or back to the inbox.
//! - **Mail.app, Gmail over IMAP** (OQTS): messages live in `[Gmail]/All
//!   Mail`; INBOX and labels are virtual mailboxes, so archive removes the
//!   INBOX label and filing adds the key's label.

use crate::config::{Account, Backend};
use crate::model::Location;

/// Gmail over IMAP: the archive mailbox is Gmail's All Mail.
pub fn is_label_style(a: &Account) -> bool {
    a.backend == Backend::Gmail
        || a.archive_mailbox
            .as_deref()
            .is_some_and(|m| m.starts_with("[Gmail]/") || m.starts_with("[Google Mail]/"))
}

fn inbox_name(a: &Account) -> &str {
    match a.backend {
        Backend::Gmail => "INBOX",
        Backend::Mailapp => a.inbox_mailbox.as_deref().unwrap_or("INBOX"),
    }
}

fn without(labels: &[String], x: &str) -> Vec<String> {
    labels.iter().filter(|l| *l != x).cloned().collect()
}

fn with(labels: &[String], x: &str) -> Vec<String> {
    let mut v = labels.to_vec();
    if !v.iter().any(|l| l == x) {
        v.push(x.to_string());
        v.sort();
    }
    v
}

pub fn in_inbox(a: &Account, loc: &Location) -> bool {
    loc.contains(inbox_name(a))
}

/// Out of the inbox, nothing else changed.
pub fn archived(a: &Account, loc: &Location) -> Location {
    let inbox = inbox_name(a);
    match loc {
        Location::Labels { labels } => Location::Labels {
            labels: without(labels, inbox),
        },
        Location::Mailboxes { physical, labels } if is_label_style(a) => Location::Mailboxes {
            physical: physical.clone(),
            labels: without(labels, inbox),
        },
        Location::Mailboxes { physical, labels } => Location::Mailboxes {
            physical: if physical == inbox {
                a.archive_mailbox.clone().unwrap_or_else(|| physical.clone())
            } else {
                physical.clone()
            },
            labels: labels.clone(),
        },
    }
}

/// Back in the inbox.
pub fn inboxed(a: &Account, loc: &Location) -> Location {
    let inbox = inbox_name(a);
    match loc {
        Location::Labels { labels } => Location::Labels {
            labels: with(labels, inbox),
        },
        Location::Mailboxes { physical, labels } if is_label_style(a) => Location::Mailboxes {
            physical: physical.clone(),
            labels: with(labels, inbox),
        },
        Location::Mailboxes { labels, .. } => Location::Mailboxes {
            physical: inbox.to_string(),
            labels: labels.clone(),
        },
    }
}

/// Filed under a key and out of the inbox. `folder` is the key's mailbox
/// (Mail.app) or label **id** (Gmail).
pub fn filed(a: &Account, loc: &Location, folder: &str) -> Location {
    let inbox = inbox_name(a);
    match loc {
        Location::Labels { labels } => Location::Labels {
            labels: with(&without(labels, inbox), folder),
        },
        Location::Mailboxes { physical, labels } if is_label_style(a) => Location::Mailboxes {
            physical: physical.clone(),
            labels: with(&without(labels, inbox), folder),
        },
        Location::Mailboxes { labels, .. } => Location::Mailboxes {
            physical: folder.to_string(),
            labels: labels.clone(),
        },
    }
}

/// Gmail label changes between two locations: (add, remove).
pub fn label_diff(before: &Location, after: &Location) -> (Vec<String>, Vec<String>) {
    let (b, a) = (before.all(), after.all());
    let add = a.iter().filter(|x| !b.contains(x)).map(|s| s.to_string()).collect();
    let remove = b.iter().filter(|x| !a.contains(x)).map(|s| s.to_string()).collect();
    (add, remove)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::path::PathBuf;

    fn cfg() -> Config {
        Config::load(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config/config.example.toml")).unwrap()
    }

    fn labels(v: &[&str]) -> Location {
        Location::Labels {
            labels: v.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn mb(p: &str, l: &[&str]) -> Location {
        Location::Mailboxes {
            physical: p.into(),
            labels: l.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn gmail_api() {
        let c = cfg();
        let a = c.account("personal").unwrap();
        let l = labels(&["INBOX", "UNREAD"]);
        assert_eq!(archived(a, &l), labels(&["UNREAD"]));
        assert_eq!(inboxed(a, &labels(&["UNREAD"])), l);
        assert_eq!(filed(a, &l, "Label_1"), labels(&["Label_1", "UNREAD"]));
        assert!(in_inbox(a, &l) && !in_inbox(a, &archived(a, &l)));
        assert_eq!(
            label_diff(&l, &filed(a, &l, "Label_1")),
            (vec!["Label_1".into()], vec!["INBOX".into()])
        );
    }

    #[test]
    fn exchange_folders() {
        let c = cfg();
        let a = c.account("oxford").unwrap();
        assert!(!is_label_style(a));
        let l = mb("Inbox", &[]);
        assert_eq!(archived(a, &l), mb("Archive", &[]));
        assert_eq!(inboxed(a, &mb("Archive", &[])), l);
        assert_eq!(filed(a, &l, "Admin"), mb("Admin", &[]));
        // A sent message stays where it is when its thread is archived.
        assert_eq!(archived(a, &mb("Sent Items", &[])), mb("Sent Items", &[]));
        assert_eq!(inboxed(a, &mb("Admin", &[])), mb("Inbox", &[]));
    }

    #[test]
    fn gmail_over_imap_labels() {
        let c = cfg();
        let a = c.account("oqts").unwrap();
        assert!(is_label_style(a));
        let l = mb("[Gmail]/All Mail", &["INBOX"]);
        assert_eq!(archived(a, &l), mb("[Gmail]/All Mail", &[]));
        assert_eq!(inboxed(a, &mb("[Gmail]/All Mail", &[])), l);
        assert_eq!(filed(a, &l, "Society"), mb("[Gmail]/All Mail", &["Society"]));
        assert!(in_inbox(a, &l));
    }

    #[test]
    fn idempotent() {
        let c = cfg();
        for id in ["personal", "oxford", "oqts"] {
            let a = c.account(id).unwrap();
            let l = if id == "personal" {
                labels(&["INBOX"])
            } else if id == "oxford" {
                mb("Inbox", &[])
            } else {
                mb("[Gmail]/All Mail", &["INBOX"])
            };
            assert_eq!(archived(a, &archived(a, &l)), archived(a, &l), "{id}");
            assert_eq!(inboxed(a, &inboxed(a, &l)), inboxed(a, &l), "{id}");
            assert_eq!(inboxed(a, &archived(a, &l)), l, "{id}");
        }
    }
}
