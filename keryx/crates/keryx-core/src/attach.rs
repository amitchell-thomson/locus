//! Attachment text for Claude (SPEC §5.2): text extracted by keryx from
//! PDF, DOCX, plain text, CSV and HTML, capped at [`MAX_CHARS`]. Images,
//! archives, encrypted or unparsable files give "no text available" —
//! never raw bytes. Nothing is executed or opened in another app.

use std::io::Read;
use std::time::Duration;

pub const MAX_INPUT: usize = 25 * 1024 * 1024;
pub const MAX_CHARS: usize = 20_000;
/// DOCX: refuse to inflate more than this (zip bombs).
pub const MAX_DOCX_XML: u64 = 50 * 1024 * 1024;
pub const BUDGET: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachText {
    Text {
        text: String,
        truncated: bool,
    },
    /// Why there's no text ("an image", "encrypted PDF", …).
    NoText(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Pdf,
    Docx,
    Plain,
    Html,
}

fn kind(name: &str, mime: &str) -> Option<Kind> {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    let mime = mime.to_ascii_lowercase();
    match (mime.as_str(), ext.as_str()) {
        ("application/pdf", _) | (_, "pdf") => Some(Kind::Pdf),
        ("application/vnd.openxmlformats-officedocument.wordprocessingml.document", _) | (_, "docx") => {
            Some(Kind::Docx)
        }
        ("text/html", _) | (_, "html") | (_, "htm") => Some(Kind::Html),
        (m, _) if m.starts_with("text/") => Some(Kind::Plain),
        (_, "txt" | "csv" | "md" | "tsv" | "log" | "ics" | "vcf") => Some(Kind::Plain),
        ("application/csv", _) => Some(Kind::Plain),
        _ => None,
    }
}

fn cap(text: &str) -> AttachText {
    let clean = crate::text::tidy(&crate::text::sanitize(text));
    if clean.trim().is_empty() {
        return AttachText::NoText("no text in it (scanned image?)".into());
    }
    match clean.char_indices().nth(MAX_CHARS) {
        Some((i, _)) => AttachText::Text {
            text: format!("{}…[truncated]", &clean[..i]),
            truncated: true,
        },
        None => AttachText::Text {
            text: clean,
            truncated: false,
        },
    }
}

/// Extract text synchronously (use [`extract_with_budget`] from daemons).
pub fn extract(name: &str, mime: &str, bytes: &[u8]) -> AttachText {
    if bytes.len() > MAX_INPUT {
        return AttachText::NoText(format!("too large ({} MB; limit 25 MB)", bytes.len() / 1024 / 1024));
    }
    let Some(k) = kind(name, mime) else {
        let what = if mime.starts_with("image/") {
            "an image"
        } else if mime.contains("zip") || name.ends_with(".zip") {
            "an archive"
        } else {
            "a file type keryx doesn't read"
        };
        return AttachText::NoText(what.into());
    };
    match k {
        Kind::Plain => cap(&String::from_utf8_lossy(bytes)),
        Kind::Html => cap(&crate::text::html_to_text(&String::from_utf8_lossy(bytes)).text),
        Kind::Docx => match docx_text(bytes) {
            Ok(t) => cap(&t),
            Err(e) => AttachText::NoText(e),
        },
        Kind::Pdf => {
            // pdf-extract can panic on malformed files; contain it.
            let r = std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem(bytes));
            match r {
                Ok(Ok(t)) if !t.trim().is_empty() => cap(&t),
                _ if contains(bytes, b"/Encrypt") => AttachText::NoText("encrypted PDF".into()),
                Ok(Ok(_)) => AttachText::NoText("no text in it (scanned image?)".into()),
                _ => AttachText::NoText("couldn't read this PDF".into()),
            }
        }
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn docx_text(bytes: &[u8]) -> Result<String, String> {
    let mut zip =
        zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|_| "couldn't read this DOCX".to_string())?;
    let file = zip
        .by_name("word/document.xml")
        .map_err(|_| "couldn't read this DOCX".to_string())?;
    let mut xml = Vec::new();
    file.take(MAX_DOCX_XML + 1)
        .read_to_end(&mut xml)
        .map_err(|_| "couldn't read this DOCX".to_string())?;
    if xml.len() as u64 > MAX_DOCX_XML {
        return Err("DOCX too large once unpacked".into());
    }
    let mut reader = quick_xml::Reader::from_reader(xml.as_slice());
    let mut out = String::new();
    let mut in_text = false;
    let mut buf = Vec::new();
    loop {
        use quick_xml::events::Event;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) if e.name().as_ref() == "w:t" => in_text = true,
            Ok(Event::End(e)) => match e.name().as_ref() {
                "w:t" => in_text = false,
                "w:p" => out.push('\n'),
                _ => {}
            },
            Ok(Event::Empty(e)) => match e.name().as_ref() {
                "w:tab" => out.push('\t'),
                "w:br" | "w:cr" => out.push('\n'),
                _ => {}
            },
            Ok(Event::Text(t)) if in_text => out.push_str(&t.xml10_content()),
            Ok(Event::GeneralRef(r)) if in_text => {
                if r.is_char_ref() {
                    if let Ok(Some(c)) = r.resolve_char_ref() {
                        out.push(c);
                    }
                } else {
                    out.push_str(match r.xml10_content().as_ref() {
                        "amp" => "&",
                        "lt" => "<",
                        "gt" => ">",
                        "quot" => "\"",
                        "apos" => "'",
                        _ => "",
                    });
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => return Err("couldn't read this DOCX".into()),
            _ => {}
        }
        buf.clear();
    }
    Ok(out)
}

/// [`extract`] on a worker thread with a time budget.
pub fn extract_with_budget(name: &str, mime: &str, bytes: Vec<u8>, budget: Duration) -> AttachText {
    let (tx, rx) = std::sync::mpsc::channel();
    let (name, mime) = (name.to_string(), mime.to_string());
    std::thread::spawn(move || {
        let _ = tx.send(extract(&name, &mime, &bytes));
    });
    rx.recv_timeout(budget)
        .unwrap_or_else(|_| AttachText::NoText(format!("took longer than {} s to read", budget.as_secs())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attachment(fixture: &str) -> (String, String, Vec<u8>) {
        let path = format!("{}/../../fixtures/eml/{fixture}", env!("CARGO_MANIFEST_DIR"));
        let raw = std::fs::read(path).unwrap();
        let p = crate::mime::parse(&raw).unwrap();
        let a = &p.attachments[0];
        (
            a.name.clone(),
            a.mime.clone(),
            crate::mime::attachment_bytes(&raw, a.id).unwrap(),
        )
    }

    fn text(t: AttachText) -> String {
        match t {
            AttachText::Text { text, .. } => text,
            AttachText::NoText(why) => panic!("no text: {why}"),
        }
    }

    #[test]
    fn pdf() {
        let (n, m, b) = attachment("23-pdf-attachment.eml");
        assert!(text(extract(&n, &m, &b)).contains("Problem sheet 3 is due Friday 23 October at 17:00."));
    }

    #[test]
    fn docx() {
        let (n, m, b) = attachment("24-docx-attachment.eml");
        assert_eq!(
            text(extract(&n, &m, &b)),
            "Minutes: the treasurer will circulate the budget by Monday.\nSecond paragraph."
        );
    }

    #[test]
    fn corrupt_and_encrypted_pdfs_give_no_text() {
        let (n, m, b) = attachment("25-corrupt-pdf.eml");
        assert_eq!(extract(&n, &m, &b), AttachText::NoText("couldn't read this PDF".into()));
        let (_, _, good) = attachment("23-pdf-attachment.eml");
        let enc = String::from_utf8_lossy(&good).replace(
            "/Root 1 0 R",
            "/Root 1 0 R /Encrypt << /Filter /Standard /V 2 /R 3 /O (xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx) /U (yyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyy) /P -4 /Length 128 >>",
        );
        assert_eq!(
            extract("x.pdf", "application/pdf", enc.as_bytes()),
            AttachText::NoText("encrypted PDF".into())
        );
    }

    #[test]
    fn plain_csv_html_and_unknown() {
        assert_eq!(text(extract("a.csv", "text/csv", b"a,b\n1,2\n")), "a,b\n1,2");
        assert_eq!(
            text(extract("n.txt", "application/octet-stream", b"note \x1b[31mred\x1b[0m")),
            "note red"
        );
        assert_eq!(
            text(extract(
                "p.html",
                "text/html",
                b"<p>Hi <span style='display:none'>x</span>there</p>"
            )),
            "Hi there"
        );
        assert_eq!(
            extract("poster.png", "image/png", b"\x89PNG"),
            AttachText::NoText("an image".into())
        );
        assert_eq!(
            extract("x.zip", "application/zip", b"PK"),
            AttachText::NoText("an archive".into())
        );
        assert_eq!(
            extract("x.txt", "text/plain", b"   \n"),
            AttachText::NoText("no text in it (scanned image?)".into())
        );
        assert!(matches!(extract("x.docx", "", b"not a zip"), AttachText::NoText(_)));
    }

    #[test]
    fn caps() {
        let big = "x".repeat(MAX_CHARS + 10);
        match extract("a.txt", "text/plain", big.as_bytes()) {
            AttachText::Text { text, truncated } => {
                assert!(truncated);
                assert!(text.ends_with("…[truncated]"));
            }
            other => panic!("{other:?}"),
        }
        let huge = vec![b'a'; MAX_INPUT + 1];
        assert!(matches!(extract("a.txt", "text/plain", &huge), AttachText::NoText(w) if w.contains("too large")));
    }

    #[test]
    fn budget() {
        let (n, m, b) = attachment("23-pdf-attachment.eml");
        assert!(matches!(
            extract_with_budget(&n, &m, b, BUDGET),
            AttachText::Text { .. }
        ));
    }
}
