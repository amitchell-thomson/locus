//! Message text as keryx shows and stores it. Phase 1 version: plain text
//! wins, HTML is reduced to its text crudely. Phase 2 replaces this with the
//! full hygiene rules (SPEC §5.3–5.4) and bumps [`TEXT_VERSION`]; the mirror
//! re-derives every message whose `text_version` is older.

use crate::mime::Parsed;

/// Bump whenever [`body_text`] changes output, so the mirror re-derives.
pub const TEXT_VERSION: u32 = 1;

/// The normalised body, and whether hidden text was removed.
pub fn body_text(p: &Parsed) -> (String, bool) {
    if let Some(t) = &p.text_plain {
        return (strip_controls(t), false);
    }
    match &p.text_html {
        Some(h) => (strip_controls(&strip_tags(h)), false),
        None => (String::new(), false),
    }
}

/// The first ~140 characters, whitespace collapsed.
pub fn snippet(body: &str) -> String {
    let collapsed: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
    match collapsed.char_indices().nth(140) {
        Some((i, _)) => format!("{}…", &collapsed[..i]),
        None => collapsed,
    }
}

fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

/// Drop C0/C1 controls (keeping newline and tab) so nothing reaches a terminal.
fn strip_controls(s: &str) -> String {
    s.chars()
        .filter(|&c| c == '\n' || c == '\t' || !(c.is_control()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_truncates_and_collapses() {
        assert_eq!(snippet("a\n\n  b\tc"), "a b c");
        let long = "x".repeat(200);
        let s = snippet(&long);
        assert_eq!(s.chars().count(), 141);
        assert!(s.ends_with('…'));
    }

    #[test]
    fn plain_wins_and_controls_go() {
        let p = Parsed {
            text_plain: Some("hi\u{1b}[31m there\u{7}".into()),
            text_html: Some("<p>html</p>".into()),
            ..Parsed::default()
        };
        assert_eq!(body_text(&p).0, "hi[31m there");
    }
}
