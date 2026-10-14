//! Message text as keryx shows and stores it (SPEC §5.3–5.4).
//!
//! - Plain text wins over HTML when both exist.
//! - HTML is walked as a DOM: script/style/head/template/comments are
//!   dropped; elements hidden by inline CSS or the `hidden` attribute are
//!   dropped **and flagged** (`hidden_text_removed`) when they held text.
//!   Links become `text [n]` with a numbered URL list at the end.
//! - Everything is sanitised for terminals: ANSI/OSC/DCS escape sequences,
//!   C0/C1 controls (except newline), zero-width characters and bidi
//!   controls are removed; tabs are expanded.

use crate::mime::Parsed;
use scraper::{ElementRef, Html, Node};

/// Bump whenever the output of [`body_text`] changes; the mirror re-derives
/// older messages.
pub const TEXT_VERSION: u32 = 3;

/// The normalised body, and whether hidden text was removed.
pub fn body_text(p: &Parsed) -> (String, bool) {
    if let Some(t) = &p.text_plain {
        return (tidy(&sanitize(t)), false);
    }
    match &p.text_html {
        Some(h) => {
            let r = html_to_text(h);
            (r.text, r.hidden_text_removed)
        }
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

/// Per-message cap for compact text.
pub const COMPACT_MAX: usize = 4000;

/// A message body without quoted history or signature, for summaries
/// (`read_thread compact=true`): drops `> ` lines, everything from an
/// "On … wrote:" line, an Outlook `From:`/`Sent:` header block or a
/// `-- ` signature marker; collapses blank runs; caps at [`COMPACT_MAX`].
pub fn compact(body: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let mut out: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        let t = l.trim();
        if t == "--" && l.trim_end() == "--" || l == "-- " {
            break;
        }
        if t.starts_with("On ") && t.ends_with("wrote:") {
            break;
        }
        // "On Mon, 12 Oct 2026 at 09:00, Jane Doe <jane@x>" + "wrote:" on the next line.
        if t.starts_with("On ") && lines.get(i + 1).is_some_and(|n| n.trim() == "wrote:") {
            break;
        }
        if (t.chars().all(|c| c == '_') && t.len() >= 10) || t == "-----Original Message-----" {
            break;
        }
        if t.starts_with("From:")
            && lines[i + 1..]
                .iter()
                .take(4)
                .any(|n| n.trim_start().starts_with("Sent:"))
        {
            break;
        }
        if t.starts_with('>') {
            i += 1;
            continue;
        }
        if t.is_empty() && out.last().is_some_and(|p| p.trim().is_empty()) {
            i += 1;
            continue;
        }
        out.push(l);
        i += 1;
    }
    let joined = out.join("\n").trim().to_string();
    match joined.char_indices().nth(COMPACT_MAX) {
        Some((cut, _)) => format!("{}…[truncated]", &joined[..cut]),
        None => joined,
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct HtmlText {
    /// Text with `[n]` link markers and the numbered link list appended.
    pub text: String,
    pub links: Vec<String>,
    pub hidden_text_removed: bool,
}

/// Is a character invisible or reordering (zero-width, bidi control)?
pub fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
            | '\u{00AD}'
            | '\u{061C}'
            | '\u{180E}'
    )
}

/// Remove terminal escape sequences, control characters (newline kept),
/// zero-width and bidi characters; expand tabs; normalise line endings.
pub fn sanitize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => skip_escape(&mut chars),
            // C1 introducers: CSI, OSC, DCS, SOS, PM, APC.
            '\u{9b}' => skip_csi(&mut chars),
            '\u{9d}' | '\u{90}' | '\u{98}' | '\u{9e}' | '\u{9f}' => skip_string(&mut chars),
            '\r' => {
                if chars.peek() != Some(&'\n') {
                    out.push('\n');
                }
            }
            '\n' => out.push('\n'),
            '\t' => out.push_str("    "),
            c if c.is_control() || is_invisible(c) => {}
            c => out.push(c),
        }
    }
    out
}

type Chars<'a> = std::iter::Peekable<std::str::Chars<'a>>;

fn skip_escape(chars: &mut Chars<'_>) {
    match chars.next() {
        Some('[') => skip_csi(chars),
        Some(']') | Some('P') | Some('X') | Some('^') | Some('_') => skip_string(chars),
        // Two-character sequences (ESC c, ESC 7, ESC ( B …).
        Some('(') | Some(')') | Some('*') | Some('+') | Some('#') | Some('%') => {
            chars.next();
        }
        _ => {}
    }
}

/// CSI: parameters/intermediates until a final byte in 0x40–0x7E.
fn skip_csi(chars: &mut Chars<'_>) {
    for c in chars.by_ref() {
        if ('\u{40}'..='\u{7e}').contains(&c) {
            return;
        }
        if !('\u{20}'..='\u{3f}').contains(&c) {
            return;
        }
    }
}

/// OSC/DCS/…: until BEL or ST (`ESC \` or U+009C).
fn skip_string(chars: &mut Chars<'_>) {
    while let Some(c) = chars.next() {
        match c {
            '\u{7}' | '\u{9c}' => return,
            '\u{1b}' => {
                if chars.peek() == Some(&'\\') {
                    chars.next();
                }
                return;
            }
            _ => {}
        }
    }
}

/// Trim trailing spaces per line and collapse 3+ blank lines to 2.
pub fn tidy(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank = 0;
    for line in s.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim_matches('\n').to_string()
}

const SKIP_TAGS: &[&str] = &[
    "script", "style", "head", "title", "template", "meta", "link", "object", "embed", "iframe", "svg",
];
const BLOCK_TAGS: &[&str] = &[
    "p",
    "div",
    "section",
    "article",
    "header",
    "footer",
    "table",
    "tr",
    "ul",
    "ol",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "blockquote",
    "pre",
    "center",
    "form",
    "address",
    "dl",
    "dt",
    "dd",
];

/// Inline-CSS / attribute rules that make an element invisible.
fn is_hidden(el: &scraper::node::Element) -> bool {
    if el.attr("hidden").is_some() {
        return true;
    }
    let Some(style) = el.attr("style") else { return false };
    let mut overflow_hidden = false;
    let mut zero_box = false;
    for decl in style.split(';') {
        let Some((k, v)) = decl.split_once(':') else { continue };
        let k = k.trim().to_ascii_lowercase();
        let v = v.trim().trim_end_matches("!important").trim().to_ascii_lowercase();
        match k.as_str() {
            "display" if v == "none" => return true,
            "visibility" if v == "hidden" || v == "collapse" => return true,
            "mso-hide" if v == "all" => return true,
            "opacity" if is_zero(&v) => return true,
            "font-size" if is_zero(&v) || v == "1px" || v == "1pt" => return true,
            "overflow" if v == "hidden" => overflow_hidden = true,
            "width" | "height" | "max-height" | "max-width" if is_zero(&v) => zero_box = true,
            _ => {}
        }
    }
    overflow_hidden && zero_box
}

fn is_zero(v: &str) -> bool {
    let n = v.trim_end_matches(|c: char| c.is_ascii_alphabetic() || c == '%');
    n.parse::<f64>().is_ok_and(|x| x == 0.0)
}

struct Walk {
    out: String,
    links: Vec<String>,
    hidden: bool,
    /// The last thing written was whitespace (collapse runs).
    space: bool,
    list_depth: usize,
}

impl Walk {
    fn text(&mut self, t: &str) {
        for c in t.chars() {
            if c.is_whitespace() {
                if !self.space {
                    self.out.push(' ');
                    self.space = true;
                }
            } else {
                self.out.push(c);
                self.space = false;
            }
        }
    }
    fn newline(&mut self) {
        while self.out.ends_with(' ') {
            self.out.pop();
        }
        if !self.out.ends_with('\n') && !self.out.is_empty() {
            self.out.push('\n');
        }
        self.space = true;
    }
    fn blank_line(&mut self) {
        self.newline();
        if !self.out.ends_with("\n\n") && !self.out.is_empty() {
            self.out.push('\n');
        }
    }
    fn walk(&mut self, el: ElementRef<'_>) {
        for child in el.children() {
            match child.value() {
                Node::Text(t) => self.text(t),
                Node::Element(e) => {
                    let Some(child_el) = ElementRef::wrap(child) else {
                        continue;
                    };
                    let tag = e.name();
                    if SKIP_TAGS.contains(&tag) {
                        continue;
                    }
                    if is_hidden(e) {
                        if child_el.text().any(|t| !t.trim().is_empty()) {
                            self.hidden = true;
                        }
                        continue;
                    }
                    match tag {
                        "br" => self.newline(),
                        "hr" => {
                            self.newline();
                            self.out.push_str("---");
                            self.newline();
                        }
                        "img" => {
                            if let Some(alt) = e.attr("alt").map(str::trim).filter(|a| !a.is_empty()) {
                                self.text(&format!("[{alt}]"));
                            }
                        }
                        "a" => {
                            let href = e.attr("href").map(str::trim).unwrap_or("");
                            let before = self.out.len();
                            self.walk(child_el);
                            let shown = self.out[before..].trim().to_string();
                            if href.starts_with("http://")
                                || href.starts_with("https://")
                                || href.starts_with("mailto:")
                            {
                                if shown == href || shown.is_empty() {
                                    if shown.is_empty() {
                                        self.text(href);
                                    }
                                } else {
                                    let n = match self.links.iter().position(|l| l == href) {
                                        Some(i) => i + 1,
                                        None => {
                                            self.links.push(href.to_string());
                                            self.links.len()
                                        }
                                    };
                                    self.out.push_str(&format!(" [{n}]"));
                                    self.space = false;
                                }
                            }
                        }
                        "li" => {
                            self.newline();
                            self.out.push_str(&"  ".repeat(self.list_depth.saturating_sub(1)));
                            self.out.push_str("• ");
                            self.space = true;
                            self.walk(child_el);
                            self.newline();
                        }
                        "ul" | "ol" => {
                            self.newline();
                            self.list_depth += 1;
                            self.walk(child_el);
                            self.list_depth -= 1;
                            self.newline();
                        }
                        "td" | "th" => {
                            self.walk(child_el);
                            if !self.space {
                                self.out.push(' ');
                                self.space = true;
                            }
                        }
                        t if t.starts_with('h') && t.len() == 2 => {
                            self.blank_line();
                            self.walk(child_el);
                            self.blank_line();
                        }
                        "p" | "blockquote" => {
                            self.blank_line();
                            self.walk(child_el);
                            self.blank_line();
                        }
                        t if BLOCK_TAGS.contains(&t) => {
                            self.newline();
                            self.walk(child_el);
                            self.newline();
                        }
                        _ => self.walk(child_el),
                    }
                }
                // Comments, doctype, processing instructions: dropped.
                _ => {}
            }
        }
    }
}

/// HTML → readable text (see the module docs for the rules).
pub fn html_to_text(html: &str) -> HtmlText {
    let doc = Html::parse_document(html);
    let mut w = Walk {
        out: String::new(),
        links: Vec::new(),
        hidden: false,
        space: true,
        list_depth: 0,
    };
    let root = doc.root_element();
    // Some senders hide text in <head><style>…: only <body> is walked.
    let body = root
        .children()
        .filter_map(ElementRef::wrap)
        .find(|e| e.value().name() == "body");
    w.walk(body.unwrap_or(root));
    let mut text = tidy(&sanitize(&w.out));
    let links: Vec<String> = w.links.iter().map(|l| sanitize(l)).collect();
    if !links.is_empty() {
        text.push_str("\n\nLinks:");
        for (i, l) in links.iter().enumerate() {
            text.push_str(&format!("\n[{}] {l}", i + 1));
        }
    }
    HtmlText {
        text,
        links,
        hidden_text_removed: w.hidden,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mime::parse;
    use proptest::prelude::*;

    fn fixture(name: &str) -> (String, bool) {
        let path = format!("{}/../../fixtures/eml/{name}", env!("CARGO_MANIFEST_DIR"));
        body_text(&parse(&std::fs::read(path).unwrap()).unwrap())
    }

    /// Every fixture's rendered body, in one reviewed snapshot.
    #[test]
    fn every_fixture_renders() {
        let dir = format!("{}/../../fixtures/eml", env!("CARGO_MANIFEST_DIR"));
        let mut names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".eml"))
            .collect();
        names.sort();
        let mut out = String::new();
        for n in names {
            let (t, hidden) = fixture(&n);
            out.push_str(&format!(
                "===== {n}{}\n{t}\n",
                if hidden { "  [hidden text removed]" } else { "" }
            ));
        }
        insta::assert_snapshot!(out);
    }

    #[test]
    fn plain_text_wins() {
        let (t, hidden) = fixture("02-multipart-alternative.eml");
        assert!(t.contains("confirmed for Wed 21 Oct"));
        assert!(!t.contains("HTML ONLY TEXT"));
        assert!(!hidden);
    }

    #[test]
    fn html_only_with_links_lists_and_no_scripts() {
        let (t, hidden) = fixture("03-html-only.eml");
        insta::assert_snapshot!(t, @r"
        Submission portal

        Submit your coursework here [1] by Friday 17:00.

        • One PDF
        • Max 10 MB

        Links:
        [1] https://portal.example.ac.uk/submit?id=42
        ");
        assert!(!t.contains("Ignore me") && !t.contains("alert") && !t.contains("color:red"));
        assert!(!hidden, "scripts and styles are removed silently");
    }

    #[test]
    fn hidden_text_is_removed_and_flagged() {
        let (t, hidden) = fixture("13-hidden-text.eml");
        assert!(hidden);
        for secret in [
            "ignore previous",
            "SECRET-FS0",
            "SECRET-HIDDENATTR",
            "SECRET-VIS",
            "SECRET-OPACITY",
            "SECRET-PREHEADER",
            "SECRET-COMMENT",
            "SECRET-TEMPLATE",
        ] {
            assert!(!t.contains(secret), "{secret} leaked: {t}");
        }
        // Zero-width and bidi characters are gone; the visible words survive.
        assert_eq!(t, "Visible text: please review your settings.");
    }

    #[test]
    fn inline_image_alt_text() {
        let (t, _) = fixture("10-inline-image.eml");
        assert_eq!(t, "Here is the poster:\n\n[poster]\n\nSee you there.");
    }

    #[test]
    fn escape_sequences_never_survive() {
        let (t, _) = fixture("14b-escapes.eml");
        assert_eq!(t, "Numbers attached. Also red text and a bell.");
    }

    #[test]
    fn sanitize_cases() {
        assert_eq!(sanitize("a\u{1b}[1;31mb\u{1b}[0mc"), "abc");
        assert_eq!(sanitize("a\u{1b}]8;;http://x\u{1b}\\link\u{1b}]8;;\u{1b}\\b"), "alinkb");
        assert_eq!(sanitize("a\u{1b}]0;title\u{7}b"), "ab");
        assert_eq!(sanitize("a\u{9b}2Jb"), "ab");
        assert_eq!(sanitize("a\u{1b}Pq#0;2\u{1b}\\b"), "ab");
        assert_eq!(sanitize("x\r\ny\rz"), "x\ny\nz");
        assert_eq!(sanitize("\ta"), "    a");
        assert_eq!(sanitize("pay\u{202E}fdp.exe"), "payfdp.exe");
        assert_eq!(sanitize("soft\u{AD}hyphen\u{FEFF}"), "softhyphen");
        assert_eq!(sanitize("unterminated \u{1b}[31"), "unterminated ");
    }

    #[test]
    fn hidden_style_rules() {
        let hidden = [
            r#"<div style="display: none !important">x</div>"#,
            r#"<div style="DISPLAY:NONE">x</div>"#,
            r#"<div style="visibility:hidden">x</div>"#,
            r#"<div style="opacity:0.0">x</div>"#,
            r#"<span style="font-size:0px">x</span>"#,
            r#"<span style="font-size:1px">x</span>"#,
            r#"<div style="max-height:0;overflow:hidden">x</div>"#,
            r#"<div style="mso-hide:all">x</div>"#,
            r#"<p hidden>x</p>"#,
        ];
        for h in hidden {
            let r = html_to_text(&format!("<body>{h}<p>shown</p></body>"));
            assert_eq!(r.text, "shown", "{h}");
            assert!(r.hidden_text_removed, "{h}");
        }
        let visible = [
            r#"<div style="display:block">x</div>"#,
            r#"<div style="opacity:0.5">x</div>"#,
            r#"<div style="max-height:0">x</div>"#,
            r#"<div style="font-size:10px">x</div>"#,
            r#"<span aria-hidden="true">x</span>"#,
        ];
        for v in visible {
            let r = html_to_text(&format!("<body>{v}</body>"));
            assert_eq!(r.text, "x", "{v}");
            assert!(!r.hidden_text_removed, "{v}");
        }
        // An empty hidden spacer isn't worth a warning.
        assert!(!html_to_text(r#"<div style="display:none"> </div>"#).hidden_text_removed);
    }

    #[test]
    fn links() {
        let r = html_to_text(
            r#"<a href="https://a.example/x">Click</a> <a href="https://a.example/x">again</a>
               <a href="https://b.example">https://b.example</a> <a href="javascript:alert(1)">js</a>
               <a href="mailto:x@y.z">mail me</a>"#,
        );
        assert_eq!(r.links, vec!["https://a.example/x", "mailto:x@y.z"]);
        assert!(
            r.text
                .starts_with("Click [1] again [1] https://b.example js mail me [2]"),
            "{}",
            r.text
        );
    }

    #[test]
    fn compact_drops_quotes_signatures_and_outlook_headers() {
        let (t, _) = fixture("15-outlook-quote.eml");
        assert_eq!(compact(&t), "Approved, it will be paid on Monday.\n\nThanks,\nSam");
        let (t, _) = fixture("20-thread-b.eml");
        assert_eq!(compact(&t), "I'll take part two.");
        assert_eq!(compact("Hi\n\n\n\nthere\n-- \nJane\nPhone"), "Hi\n\nthere");
        assert_eq!(compact("Yes.\n> old\n> older\nNo."), "Yes.\nNo.");
        assert_eq!(
            compact("Fine.\nOn Mon, 12 Oct 2026 at 09:00, Jane Doe <jane@x.com>\nwrote:\n> q"),
            "Fine."
        );
        let long = "word ".repeat(2000);
        let c = compact(&long);
        assert!(c.ends_with("…[truncated]") && c.chars().count() == COMPACT_MAX + "…[truncated]".chars().count());
    }

    #[test]
    fn snippet_truncates_and_collapses() {
        assert_eq!(snippet("a\n\n  b\tc"), "a b c");
        let s = snippet(&"x".repeat(200));
        assert_eq!(s.chars().count(), 141);
        assert!(s.ends_with('…'));
    }

    proptest! {
        #[test]
        fn sanitize_output_is_terminal_safe(s in "\\PC*|[\\x00-\\x1f\\x7f-\\x9f\\u{200b}-\\u{202e}a-z\\[\\];]{0,80}") {
            let out = sanitize(&s);
            for c in out.chars() {
                prop_assert!(c == '\n' || !c.is_control(), "control {:?} in {:?}", c, out);
                prop_assert!(!is_invisible(c), "invisible {:?}", c);
            }
        }

        #[test]
        fn visible_words_always_survive(
            words in proptest::collection::vec("[a-z]{3,10}", 1..12),
            tags in proptest::collection::vec(prop_oneof!["p", "div", "span", "b", "i", "td", "li", "h2", "blockquote"], 1..12),
        ) {
            let mut html = String::from("<body>");
            for (i, w) in words.iter().enumerate() {
                let t = &tags[i % tags.len()];
                html.push_str(&format!("<{t}>{w}</{t}> "));
            }
            html.push_str("</body>");
            let r = html_to_text(&html);
            for w in &words {
                prop_assert!(r.text.contains(w.as_str()), "{} lost from {:?}", w, r.text);
            }
            prop_assert!(!r.hidden_text_removed);
        }
    }
}
