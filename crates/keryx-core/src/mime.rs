//! RFC 5322 / MIME parsing into the pieces keryx uses. Text normalisation
//! (HTML hygiene, sanitising) lives in [`crate::text`]; this module only
//! decodes. Built on `mail-parser`.

use crate::model::{Address, Attachment};
use mail_parser::{HeaderValue, MessageParser, MimeHeaders};

/// Headers keryx reasons about for automated detection and unsubscribe.
pub const LIST_HEADERS: &[&str] = &[
    "list-id",
    "list-unsubscribe",
    "list-unsubscribe-post",
    "precedence",
    "auto-submitted",
    "feedback-id",
    "x-auto-response-suppress",
];

/// A decoded message, before text normalisation.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Parsed {
    pub from: Option<Address>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub bcc: Vec<Address>,
    pub reply_to: Vec<Address>,
    pub subject: String,
    /// Unix seconds (0 if the Date header is missing or unreadable).
    pub date: i64,
    pub message_id: String,
    pub in_reply_to: Vec<String>,
    pub references: Vec<String>,
    pub list_headers: Vec<(String, String)>,
    /// The first `text/plain` body part, decoded, if any.
    pub text_plain: Option<String>,
    /// The first `text/html` body part, decoded, if any.
    pub text_html: Option<String>,
    /// Calendar parts (`text/calendar`), decoded.
    pub calendars: Vec<String>,
    pub attachments: Vec<Attachment>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a parseable email message")]
pub struct ParseError;

fn addrs(a: Option<&mail_parser::Address>) -> Vec<Address> {
    a.map(|a| {
        a.iter()
            .filter_map(|x| x.address().map(|e| Address::new(x.name(), e)))
            .collect()
    })
    .unwrap_or_default()
}

fn id_list(v: &HeaderValue) -> Vec<String> {
    match v {
        HeaderValue::Text(t) => vec![t.to_string()],
        HeaderValue::TextList(l) => l.iter().map(|t| t.to_string()).collect(),
        _ => Vec::new(),
    }
}

/// Parse raw RFC 5322 bytes.
pub fn parse(raw: &[u8]) -> Result<Parsed, ParseError> {
    let msg = MessageParser::default().parse(raw).ok_or(ParseError)?;
    let mut p = Parsed {
        from: addrs(msg.from()).into_iter().next(),
        to: addrs(msg.to()),
        cc: addrs(msg.cc()),
        bcc: addrs(msg.bcc()),
        reply_to: addrs(msg.reply_to()),
        subject: msg.subject().unwrap_or_default().to_string(),
        date: msg.date().map(|d| d.to_timestamp()).unwrap_or(0),
        message_id: msg.message_id().unwrap_or_default().to_string(),
        in_reply_to: id_list(msg.in_reply_to()),
        references: id_list(msg.references()),
        ..Parsed::default()
    };
    for (name, value) in msg.headers_raw() {
        let lname = name.to_ascii_lowercase();
        if LIST_HEADERS.contains(&lname.as_str()) {
            p.list_headers.push((lname, value.trim().to_string()));
        }
    }

    // Body parts: only the top-level message's own text/html bodies, not
    // those inside attached messages (a forward's inner text is shown via
    // the text body mail-parser builds for it).
    for &id in &msg.text_body {
        if let Some(part) = msg.parts.get(id as usize)
            && !part.is_text_html()
            && p.text_plain.is_none()
        {
            p.text_plain = part.text_contents().map(str::to_string);
        }
    }
    for &id in &msg.html_body {
        if let Some(part) = msg.parts.get(id as usize)
            && part.is_text_html()
            && p.text_html.is_none()
        {
            p.text_html = part.text_contents().map(str::to_string);
        }
    }
    // A forwarded message/rfc822 part: append its text so it's readable.
    for part in &msg.parts {
        if let Some(inner) = part.message() {
            let header = format!(
                "\n---------- Forwarded message ----------\nFrom: {}\nSubject: {}\n\n",
                addrs(inner.from()).first().map(Address::display).unwrap_or_default(),
                inner.subject().unwrap_or_default()
            );
            let body = inner.body_text(0).map(|b| b.to_string()).unwrap_or_default();
            let t = p.text_plain.get_or_insert_with(String::new);
            t.push_str(&header);
            t.push_str(&body);
        }
    }

    for (i, part) in msg.parts.iter().enumerate() {
        let ctype = part
            .content_type()
            .map(|c| match c.subtype() {
                Some(s) => format!("{}/{}", c.ctype(), s).to_ascii_lowercase(),
                None => c.ctype().to_ascii_lowercase(),
            })
            .unwrap_or_default();
        if ctype == "text/calendar" {
            if let Some(t) = part.text_contents() {
                p.calendars.push(t.to_string());
            }
            continue;
        }
        let is_inline_image = part.content_id().is_some() && ctype.starts_with("image/");
        if let Some(name) = part.attachment_name()
            && !part.is_message()
        {
            p.attachments.push(Attachment {
                id: i as u32,
                name: name.to_string(),
                mime: if ctype.is_empty() {
                    "application/octet-stream".into()
                } else {
                    ctype
                },
                size: part.contents().len() as u64,
            });
        } else if is_inline_image {
            // Inline images without a filename aren't listed.
        }
    }
    Ok(p)
}

/// The decoded bytes of attachment part `id`, if it exists and is non-empty.
/// For `.partial.emlx` sources the store supplies the bytes from disk instead.
pub fn attachment_bytes(raw: &[u8], id: u32) -> Option<Vec<u8>> {
    let msg = MessageParser::default().parse(raw)?;
    let part = msg.parts.get(id as usize)?;
    part.attachment_name()?;
    let c = part.contents();
    (!c.is_empty()).then(|| c.to_vec())
}

/// The value of a list header (lowercased name).
pub fn list_header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        let path = format!("{}/../../fixtures/eml/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read(&path).unwrap()
    }

    #[test]
    fn plain() {
        let p = parse(&fixture("01-plain.eml")).unwrap();
        assert_eq!(p.from.unwrap().display(), "Jane Doe <jane.doe@example.com>");
        assert_eq!(p.to[0].email, "newc7370@ox.ac.uk");
        assert_eq!(p.subject, "Lab partner for Thursday?");
        assert_eq!(p.message_id, "plain-01@example.com");
        assert!(p.text_plain.unwrap().contains("B4 lab on Thursday"));
        assert!(p.text_html.is_none());
        assert_eq!(p.date, 1_791_879_120); // Tue 13 Oct 2026 09:12 +0100
    }

    #[test]
    fn alternative_has_both_parts() {
        let p = parse(&fixture("02-multipart-alternative.eml")).unwrap();
        assert!(p.text_plain.unwrap().contains("confirmed for Wed 21 Oct"));
        assert!(p.text_html.unwrap().contains("HTML ONLY TEXT"));
    }

    #[test]
    fn html_only_has_no_plain_part() {
        let p = parse(&fixture("03-html-only.eml")).unwrap();
        assert!(p.text_plain.is_none());
        assert!(p.text_html.unwrap().contains("portal.example.ac.uk"));
    }

    #[test]
    fn charsets_and_encodings() {
        let p = parse(&fixture("04-quoted-printable.eml")).unwrap();
        assert_eq!(p.subject, "Café on Sunday");
        assert!(
            p.text_plain
                .unwrap()
                .contains("café at 11? It's €5 for a coffee there, apparently.")
        );
        let p = parse(&fixture("05-base64-utf8.eml")).unwrap();
        assert!(p.text_plain.unwrap().contains("München was great!"));
        let p = parse(&fixture("06-latin1.eml")).unwrap();
        assert_eq!(p.from.unwrap().name.as_deref(), Some("Jürgen"));
        assert!(p.text_plain.unwrap().contains("Viele Grüße aus München."));
        let p = parse(&fixture("07-windows-1252.eml")).unwrap();
        assert!(
            p.text_plain
                .unwrap()
                .contains("Total: €12.50 – thanks for your “order”.")
        );
        let p = parse(&fixture("08-encoded-words.eml")).unwrap();
        assert_eq!(p.from.unwrap().name.as_deref(), Some("Élodie Martin"));
        assert_eq!(p.to[0].name.as_deref(), Some("Alec Mitchell-Thomson"));
        assert_eq!(p.subject, "Réunion de l’équipe — jeudi");
    }

    #[test]
    fn forwarded_message_is_readable() {
        let p = parse(&fixture("09-forward-rfc822.eml")).unwrap();
        let t = p.text_plain.unwrap();
        assert!(t.contains("See below."), "{t}");
        assert!(t.contains("Seminar room 2 is booked"), "{t}");
        assert!(t.contains("From: Porters <porters@example.ac.uk>"), "{t}");
        assert!(p.attachments.is_empty(), "{:?}", p.attachments);
    }

    #[test]
    fn inline_image_and_attachments() {
        let p = parse(&fixture("10-inline-image.eml")).unwrap();
        assert_eq!(p.attachments.len(), 1);
        assert_eq!(p.attachments[0].name, "poster.png");
        let p = parse(&fixture("23-pdf-attachment.eml")).unwrap();
        assert_eq!(p.attachments.len(), 1);
        let a = &p.attachments[0];
        assert_eq!((a.name.as_str(), a.mime.as_str()), ("sheet3.pdf", "application/pdf"));
        let bytes = attachment_bytes(&fixture("23-pdf-attachment.eml"), a.id).unwrap();
        assert!(bytes.starts_with(b"%PDF-1.4"));
        assert_eq!(bytes.len() as u64, a.size);
    }

    #[test]
    fn calendar_parts() {
        let p = parse(&fixture("11-invite-request.eml")).unwrap();
        assert_eq!(p.calendars.len(), 1);
        assert!(p.calendars[0].contains("METHOD:REQUEST"));
        assert!(p.attachments.is_empty());
    }

    #[test]
    fn list_headers_and_threading() {
        let p = parse(&fixture("16-newsletter.eml")).unwrap();
        assert_eq!(list_header(&p.list_headers, "precedence"), Some("bulk"));
        assert_eq!(
            list_header(&p.list_headers, "list-unsubscribe-post"),
            Some("List-Unsubscribe=One-Click")
        );
        assert!(list_header(&p.list_headers, "list-id").is_some());
        let p = parse(&fixture("20-thread-b.eml")).unwrap();
        assert_eq!(p.in_reply_to, vec!["thread-a@example.com"]);
        assert_eq!(p.references, vec!["thread-a@example.com"]);
        let p = parse(&fixture("19-thread-a.eml")).unwrap();
        assert_eq!(p.reply_to[0].email, "jane@personal.example.com");
        assert_eq!(p.cc[0].email, "team@example.com");
    }

    #[test]
    fn every_fixture_parses() {
        let dir = format!("{}/../../fixtures/eml", env!("CARGO_MANIFEST_DIR"));
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let raw = std::fs::read(&path).unwrap();
            let p = parse(&raw).unwrap_or_else(|_| panic!("{}", path.display()));
            assert!(!p.message_id.is_empty(), "{}", path.display());
            assert!(p.date > 0, "{}", path.display());
        }
    }
}
