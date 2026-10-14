//! `List-Unsubscribe` (RFC 2369) and one-click (RFC 8058), SPEC §7.
//! Unsubscribing contacts the sender, so it's Alec-only and always confirmed.

use crate::mime::list_header;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unsubscribe {
    /// RFC 8058: POST `List-Unsubscribe=One-Click` to this https URL.
    OneClick { url: String },
    /// Send an email (through the normal confirm screen).
    Mailto { to: String, subject: String, body: String },
    /// Only a web page: keryx never visits it; Alec opens it himself.
    Link { url: String },
}

/// The best available method: one-click, else mailto, else a link.
pub fn parse(headers: &[(String, String)]) -> Option<Unsubscribe> {
    let raw = list_header(headers, "list-unsubscribe")?;
    let targets: Vec<&str> = raw
        .split(',')
        .filter_map(|t| {
            let t = t.trim();
            t.strip_prefix('<').and_then(|t| t.strip_suffix('>')).map(str::trim)
        })
        .collect();
    let one_click = list_header(headers, "list-unsubscribe-post")
        .is_some_and(|v| v.replace(' ', "").eq_ignore_ascii_case("List-Unsubscribe=One-Click"));
    let https = targets.iter().find(|t| t.starts_with("https://"));
    if one_click && let Some(u) = https {
        return Some(Unsubscribe::OneClick { url: u.to_string() });
    }
    if let Some(m) = targets.iter().find_map(|t| t.strip_prefix("mailto:")) {
        let (to, query) = m.split_once('?').unwrap_or((m, ""));
        let mut subject = "unsubscribe".to_string();
        let mut body = String::new();
        for pair in query.split('&') {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            match k.to_ascii_lowercase().as_str() {
                "subject" => subject = percent_decode(v),
                "body" => body = percent_decode(v),
                _ => {}
            }
        }
        if crate::config::is_address(&percent_decode(to)) {
            return Some(Unsubscribe::Mailto {
                to: percent_decode(to),
                subject,
                body,
            });
        }
    }
    targets
        .iter()
        .find(|t| t.starts_with("https://") || t.starts_with("http://"))
        .map(|u| Unsubscribe::Link { url: u.to_string() })
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = ((b[i + 1] as char).to_digit(16), (b[i + 2] as char).to_digit(16))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn one_click_beats_mailto() {
        let u = parse(&h(&[
            (
                "list-unsubscribe",
                "<https://weekly.example.com/unsub?u=abc>, <mailto:unsub@weekly.example.com?subject=unsubscribe>",
            ),
            ("list-unsubscribe-post", "List-Unsubscribe=One-Click"),
        ]));
        assert_eq!(
            u,
            Some(Unsubscribe::OneClick {
                url: "https://weekly.example.com/unsub?u=abc".into()
            })
        );
    }

    #[test]
    fn mailto_without_one_click() {
        let u = parse(&h(&[(
            "list-unsubscribe",
            "<https://x.example/u>, <mailto:leave%2Dclub@sports.example.org?subject=please%20remove%20me&body=bye>",
        )]));
        assert_eq!(
            u,
            Some(Unsubscribe::Mailto {
                to: "leave-club@sports.example.org".into(),
                subject: "please remove me".into(),
                body: "bye".into()
            })
        );
    }

    #[test]
    fn link_only_and_none() {
        assert_eq!(
            parse(&h(&[("list-unsubscribe", "<http://x.example/u>")])),
            Some(Unsubscribe::Link {
                url: "http://x.example/u".into()
            })
        );
        // One-click over plain http isn't allowed by RFC 8058: falls back to a link.
        assert_eq!(
            parse(&h(&[
                ("list-unsubscribe", "<http://x.example/u>"),
                ("list-unsubscribe-post", "List-Unsubscribe=One-Click")
            ])),
            Some(Unsubscribe::Link {
                url: "http://x.example/u".into()
            })
        );
        assert_eq!(parse(&h(&[])), None);
        assert_eq!(parse(&h(&[("list-unsubscribe", "garbage")])), None);
        assert_eq!(parse(&h(&[("list-unsubscribe", "<mailto:not an address>")])), None);
    }

    #[test]
    fn fixtures() {
        let dir = format!("{}/../../fixtures/eml", env!("CARGO_MANIFEST_DIR"));
        let p = |n: &str| crate::mime::parse(&std::fs::read(format!("{dir}/{n}")).unwrap()).unwrap();
        assert!(matches!(
            parse(&p("16-newsletter.eml").list_headers),
            Some(Unsubscribe::OneClick { .. })
        ));
        assert!(matches!(
            parse(&p("18-mailto-unsubscribe.eml").list_headers),
            Some(Unsubscribe::Mailto { .. })
        ));
        assert_eq!(parse(&p("01-plain.eml").list_headers), None);
    }
}
