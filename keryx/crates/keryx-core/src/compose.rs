//! The `$EDITOR` file for replies, forwards and new mail (SPEC §8.1):
//!
//! ```text
//! From: oxford
//! To: Jane Doe <jane@example.com>
//! Cc:
//! Bcc:
//! Subject: Re: Lab partner
//! Attach: ~/Documents/form.pdf
//!
//! body…
//!
//! signature
//! -- keryx: text above this line is sent; the quote below is kept --
//! On Tue 13 Oct 2026 at 09:12, Jane Doe wrote:
//! > …
//! ```
//!
//! Errors put Alec back in the editor with the problem at the top.

use crate::config::is_address;
use crate::model::Address;
use std::path::PathBuf;

pub const MARKER: &str = "-- keryx: text above this line is sent; the quote below is kept --";
pub const ERROR_PREFIX: &str = "# keryx: ";

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Composed {
    /// `From:` as typed: an account's id, name or address (the TUI resolves it).
    pub from: Option<String>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub bcc: Vec<Address>,
    pub subject: String,
    pub attach: Vec<PathBuf>,
    /// What's sent: the body, then the kept quote.
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ComposeError {
    #[error("Nothing sent (the message is empty)")]
    Empty,
    #[error("{0:?} isn't an email address")]
    BadAddress(String),
    #[error("no recipients: fill in To:")]
    NoRecipients,
    #[error("can't attach {path}: {why}")]
    Attachment { path: String, why: String },
    #[error("attachments total {mb} MB; the limit is {limit} MB")]
    TooBig { mb: u64, limit: u32 },
    #[error("unknown header {0:?}: use From, To, Cc, Bcc, Subject or Attach")]
    UnknownHeader(String),
    #[error("no account {0:?} to send from: use one of {1}")]
    UnknownAccount(String, String),
    #[error("a reply goes from the account it came to ({0}): forward it (F) to send from another")]
    ReplyFromOther(String),
}

fn list(v: &[Address]) -> String {
    v.iter().map(Address::display).collect::<Vec<_>>().join(", ")
}

/// The file to open in the editor.
pub fn file(to: &[Address], cc: &[Address], subject: &str, signature: Option<&str>, quote: &str) -> String {
    file_with_body(to, cc, subject, "", signature, quote)
}

/// The editor file with a body already typed (e.g. free times for `w`).
pub fn file_with_body(
    to: &[Address],
    cc: &[Address],
    subject: &str,
    body: &str,
    signature: Option<&str>,
    quote: &str,
) -> String {
    let mut s = format!(
        "To: {}\nCc: {}\nBcc: \nSubject: {subject}\n\n{body}\n",
        list(to),
        list(cc)
    );
    if let Some(sig) = signature.filter(|s| !s.trim().is_empty()) {
        s.push_str(&format!("\n{}\n", sig.trim_end()));
    }
    if !quote.is_empty() {
        s.push_str(MARKER);
        s.push('\n');
        s.push_str(quote);
    }
    s
}

/// The file with a `From:` line naming the account it goes from (keryx-joins-locus X6: change it
/// to send from another account).
pub fn with_from(text: &str, account: &str) -> String {
    format!("From: {account}\n{text}")
}

/// The file again, with the error explained at the top (and any earlier
/// error line removed).
pub fn with_error(text: &str, err: &ComposeError) -> String {
    let rest: String = text
        .lines()
        .skip_while(|l| l.starts_with(ERROR_PREFIX))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{ERROR_PREFIX}{err} — fix it and save, or delete the body to cancel\n{rest}\n")
}

/// Split "Jane Doe <jane@x.com>, bob@y.org" into addresses (commas inside
/// quotes are kept).
pub fn parse_addresses(s: &str) -> Result<Vec<Address>, ComposeError> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    let mut parts = Vec::new();
    for c in s.chars() {
        match c {
            '"' => {
                in_q = !in_q;
                cur.push(c);
            }
            ',' | ';' if !in_q => parts.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    parts.push(cur);
    for p in parts {
        let p = p.trim();
        if p.is_empty() {
            continue;
        }
        let (name, email) = match (p.rfind('<'), p.rfind('>')) {
            (Some(a), Some(b)) if a < b => (Some(p[..a].trim().trim_matches('"').trim()), p[a + 1..b].trim()),
            _ => (None, p),
        };
        if !is_address(email) {
            return Err(ComposeError::BadAddress(p.to_string()));
        }
        out.push(Address::new(name.filter(|n| !n.is_empty()), email));
    }
    Ok(out)
}

fn expand(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(rest),
        None => PathBuf::from(p),
    }
}

/// Parse the saved file. `signature` lets a body that is only the
/// signature count as empty. Attachments are checked to exist and fit.
pub fn parse(text: &str, signature: Option<&str>, max_mb: u32) -> Result<Composed, ComposeError> {
    let mut c = Composed::default();
    let mut lines = text.lines().skip_while(|l| l.starts_with(ERROR_PREFIX));
    // Headers until the first blank line. An indented line continues the header above it, as
    // in mail itself, so a long To: can put each address on its own line.
    let mut headers: Vec<String> = Vec::new();
    for line in lines.by_ref() {
        // Only an empty line ends the headers: an indented blank one is a continuation left
        // behind after the last recipient.
        if line.is_empty() || (line.trim().is_empty() && headers.is_empty()) {
            break;
        }
        if line.trim().is_empty() && !line.starts_with([' ', '\t']) {
            break;
        }
        if line.trim().is_empty() {
            continue;
        }
        match headers.last_mut() {
            Some(prev) if line.starts_with([' ', '\t']) => {
                prev.push(' ');
                prev.push_str(line.trim());
            }
            _ => headers.push(line.to_string()),
        }
    }
    for line in &headers {
        let Some((k, v)) = line.split_once(':') else {
            return Err(ComposeError::UnknownHeader(line.to_string()));
        };
        let v = v.trim();
        match k.trim().to_ascii_lowercase().as_str() {
            "from" if !v.is_empty() => c.from = Some(v.to_string()),
            "from" => {}
            "to" => c.to.extend(parse_addresses(v)?),
            "cc" => c.cc.extend(parse_addresses(v)?),
            "bcc" => c.bcc.extend(parse_addresses(v)?),
            "subject" => c.subject = v.to_string(),
            "attach" if !v.is_empty() => c.attach.push(expand(v)),
            "attach" => {}
            other => return Err(ComposeError::UnknownHeader(other.to_string())),
        }
    }
    let rest: Vec<&str> = lines.collect();
    let marker = rest.iter().position(|l| l.trim() == MARKER);
    let (body, quote) = match marker {
        Some(i) => (rest[..i].join("\n"), rest[i + 1..].join("\n")),
        None => (rest.join("\n"), String::new()),
    };
    let mut meaningful = body.trim().to_string();
    if let Some(sig) = signature
        && let Some(stripped) = meaningful.strip_suffix(sig.trim())
    {
        meaningful = stripped.trim().to_string();
    }
    if meaningful.is_empty() {
        return Err(ComposeError::Empty);
    }
    if c.to.is_empty() && c.cc.is_empty() && c.bcc.is_empty() {
        return Err(ComposeError::NoRecipients);
    }
    let mut total = 0u64;
    for p in &c.attach {
        let meta = std::fs::metadata(p).map_err(|e| ComposeError::Attachment {
            path: p.display().to_string(),
            why: e.to_string(),
        })?;
        if !meta.is_file() {
            return Err(ComposeError::Attachment {
                path: p.display().to_string(),
                why: "not a file".into(),
            });
        }
        total += meta.len();
    }
    if total > u64::from(max_mb) * 1024 * 1024 {
        return Err(ComposeError::TooBig {
            mb: total.div_ceil(1024 * 1024),
            limit: max_mb,
        });
    }
    c.body = if quote.trim().is_empty() {
        body.trim_end().to_string()
    } else {
        format!("{}\n\n{}", body.trim_end(), quote.trim_end())
    };
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(s: &str) -> Address {
        parse_addresses(s).unwrap().remove(0)
    }

    #[test]
    fn round_trip() {
        let f = file(
            &[a("Jane Doe <jane@example.com>")],
            &[a("sam@example.com")],
            "Re: Lab",
            Some("Alec"),
            "On Tue, Jane wrote:\n> hi",
        );
        let edited = f.replacen("\n\n\n", "\n\nYes, Thursday works.\n", 1);
        let c = parse(&edited, Some("Alec"), 25).unwrap();
        assert_eq!(c.to[0].display(), "Jane Doe <jane@example.com>");
        assert_eq!(c.cc[0].email, "sam@example.com");
        assert_eq!(c.subject, "Re: Lab");
        assert_eq!(c.body, "Yes, Thursday works.\n\nAlec\n\nOn Tue, Jane wrote:\n> hi");
    }

    #[test]
    fn empty_or_signature_only_is_nothing_sent() {
        let f = file(&[a("jane@example.com")], &[], "Hi", Some("Alec\nNew College"), "> q");
        assert_eq!(parse(&f, Some("Alec\nNew College"), 25), Err(ComposeError::Empty));
        assert_eq!(parse("", None, 25), Err(ComposeError::Empty));
    }

    #[test]
    fn a_header_can_run_over_indented_lines() {
        let f = "To: Jane <jane@example.com>,\n    sam@example.com,\n\tbob@example.org,\n    \nSubject: Hi\n\nbody\n";
        let c = parse(f, None, 25).unwrap();
        assert_eq!(c.to.len(), 3);
        assert_eq!(c.to[2].email, "bob@example.org");
        assert_eq!(c.subject, "Hi");
    }

    #[test]
    fn addresses() {
        let v = parse_addresses(r#""Doe, Jane" <jane@example.com>; bob@example.org,"#).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].name.as_deref(), Some("Doe, Jane"));
        assert_eq!(
            parse_addresses("jane at example"),
            Err(ComposeError::BadAddress("jane at example".into()))
        );
        let f = "To: jane@example.com, not-an-address\n\nbody\n";
        assert!(matches!(parse(f, None, 25), Err(ComposeError::BadAddress(_))));
        assert_eq!(parse("To: \n\nbody\n", None, 25), Err(ComposeError::NoRecipients));
        assert!(matches!(
            parse("Reply-To: me@x.com\n\nbody\n", None, 25),
            Err(ComposeError::UnknownHeader(_))
        ));
        let c = parse(&with_from("To: jane@example.com\n\nbody\n", "oxford"), None, 25).unwrap();
        assert_eq!(c.from.as_deref(), Some("oxford"));
    }

    #[test]
    fn attachments_must_exist_and_fit() {
        let dir = std::env::temp_dir().join(format!("keryx-compose-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("form.pdf");
        std::fs::write(&p, vec![0u8; 2 * 1024 * 1024]).unwrap();
        let f = format!("To: jane@example.com\nAttach: {}\n\nhere\n", p.display());
        assert_eq!(parse(&f, None, 25).unwrap().attach, vec![p.clone()]);
        assert!(matches!(
            parse(&f, None, 1),
            Err(ComposeError::TooBig { mb: 2, limit: 1 })
        ));
        let missing = "To: jane@example.com\nAttach: /no/such/file.pdf\n\nhere\n";
        assert!(matches!(parse(missing, None, 25), Err(ComposeError::Attachment { .. })));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn errors_are_shown_at_the_top_once() {
        let f = "To: x\n\nbody\n";
        let once = with_error(f, &ComposeError::BadAddress("x".into()));
        assert!(once.starts_with("# keryx: \"x\" isn't an email address"));
        let twice = with_error(&once, &ComposeError::NoRecipients);
        assert_eq!(twice.matches(ERROR_PREFIX).count(), 1);
        // The error line is ignored when parsing.
        assert!(matches!(parse(&twice, None, 25), Err(ComposeError::BadAddress(_))));
    }
}
