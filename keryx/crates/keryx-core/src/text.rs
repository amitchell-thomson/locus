//! Message text as keryx shows and stores it (SPEC §5.3–5.4).
//!
//! - Plain text wins over HTML when both exist, unless the plain part is a generated one (mostly
//!   raw links, or "view this email in your browser"): then the HTML is used (X3, 26 Oct 2026).
//! - Invisible padding (U+034F and the zero-width family) goes; wide and non-breaking spaces
//!   become plain ones, so a newsletter's preheader leaves no rows of blanks.
//! - Links in plain text become `[n]` too, with the same numbered list at the end; a tracking
//!   redirect that carries the real address is listed as that address.
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
pub const TEXT_VERSION: u32 = 4;

/// The normalised body, and whether hidden text was removed.
pub fn body_text(p: &Parsed) -> (String, bool) {
    if let Some(t) = &p.text_plain {
        let plain = tidy(&spaces(&sanitize(t)));
        if p.text_html.is_none() || !looks_generated(&plain) {
            return (number_links(&plain), false);
        }
    }
    match &p.text_html {
        Some(h) => {
            let r = html_to_text(h);
            (r.text, r.hidden_text_removed)
        }
        None => (String::new(), false),
    }
}

/// Wide, figure and non-breaking spaces become plain spaces; a line of nothing but spaces
/// becomes empty (preheader padding).
pub fn spaces(s: &str) -> String {
    s.lines()
        .map(|l| {
            let l: String = l
                .chars()
                .map(|c| match c {
                    '\u{00A0}' | '\u{2000}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => ' ',
                    c => c,
                })
                .collect();
            if l.trim().is_empty() { String::new() } else { l }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A plain part a mailing tool generated rather than wrote: mostly raw links, or one that
/// sends you to the browser. Its HTML part reads better.
pub fn looks_generated(plain: &str) -> bool {
    let lower = plain.to_lowercase();
    if BROWSER_LINES.iter().any(|b| lower.contains(b)) {
        return true;
    }
    let url_chars: usize = plain
        .split_whitespace()
        .filter(|w| w.contains("http://") || w.contains("https://"))
        .map(str::len)
        .sum();
    plain.len() > 200 && url_chars * 4 > plain.len()
}

/// Lines that only send you to a browser copy of the mail.
const BROWSER_LINES: &[&str] = &[
    "view this email in your browser",
    "view this email in a browser",
    "view in browser",
    "view it in your browser",
    "view this email online",
    "view online version",
    "trouble viewing this email",
    "having trouble viewing",
    "if you cannot see this email",
    "if you can't see this email",
    "can't see this email",
    "web version",
];

/// Raw links in plain text become `[n]`, listed at the end as the HTML path lists them.
pub fn number_links(text: &str) -> String {
    let mut links: Vec<String> = Vec::new();
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("http://").into_iter().chain(rest.find("https://")).min() {
        let (before, from) = rest.split_at(i);
        let end = from
            .find(|c: char| c.is_whitespace() || matches!(c, '>' | ')' | ']' | '"' | '\''))
            .unwrap_or(from.len());
        let url = from[..end].trim_end_matches(['.', ',', ';', ':']);
        // `<https://…>` and `(https://…)`: the brackets go with the link.
        let before = before.strip_suffix(['<', '(']).unwrap_or(before);
        out.push_str(before);
        // "here[1]" reads as "here [1]".
        if out.chars().last().is_some_and(char::is_alphanumeric) {
            out.push(' ');
        }
        let n = match links.iter().position(|l| l == url) {
            Some(k) => k + 1,
            None => {
                links.push(url.to_string());
                links.len()
            }
        };
        out.push_str(&format!("[{n}]"));
        let mut after = &from[url.len()..];
        if before.len() < rest[..i].len() {
            after = after.strip_prefix(['>', ')']).unwrap_or(after);
        }
        rest = after;
    }
    out.push_str(rest);
    let mut text = tidy(&out);
    if !links.is_empty() {
        text.push_str("\n\nLinks:");
        for (k, l) in links.iter().enumerate() {
            text.push_str(&format!("\n[{}] {}", k + 1, unwrap_link(l)));
        }
    }
    text
}

/// A tracking redirect that carries the real address in a parameter (`?url=https%3A%2F%2F…`)
/// is its real address; anything else is itself.
pub fn unwrap_link(url: &str) -> String {
    let Some((_, query)) = url.split_once('?') else {
        return url.to_string();
    };
    for pair in query.split('&') {
        let Some((_, v)) = pair.split_once('=') else { continue };
        let v = percent_decode(v);
        if v.starts_with("http://") || v.starts_with("https://") {
            return v;
        }
    }
    url.to_string()
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(x) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(x);
            i += 3;
        } else {
            out.push(if b[i] == b'+' { b' ' } else { b[i] });
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The site a link goes to, as shown in the reading pane: its registrable domain
/// (`t.mail.example.com` → `example.com`, `www.ox.ac.uk` → `ox.ac.uk`), or `mail to …`.
pub fn site(url: &str) -> String {
    if let Some(a) = url.strip_prefix("mailto:") {
        return format!("mail to {}", a.split('?').next().unwrap_or(a));
    }
    let host = url
        .split_once("://")
        .map_or(url, |(_, r)| r)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_lowercase();
    let labels: Vec<&str> = host.split('.').filter(|l| !l.is_empty()).collect();
    let n = labels.len();
    let keep = if n >= 3
        && labels[n - 1].len() == 2
        && matches!(labels[n - 2], "co" | "ac" | "org" | "gov" | "com" | "net" | "edu")
    {
        3
    } else {
        2
    };
    labels[n.saturating_sub(keep)..].join(".")
}

/// A body as the reading pane shows it (X3): the text without browser-only lines, the footer of
/// automated mail folded away, and the links as numbers with their sites.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Readable {
    pub lines: Vec<String>,
    /// Lines folded away at the end (unsubscribe, address, legal), 0 if none.
    pub footer: usize,
    /// `(n, address, site)`.
    pub links: Vec<(usize, String, String)>,
}

/// Image placeholders a mail program left in plain text (`[cid:image003.jpg@01D…]`,
/// `[image: logo]`) go.
fn drop_placeholders(l: &str) -> String {
    let mut out = String::with_capacity(l.len());
    let mut rest = l;
    while let Some(i) = rest.find("[cid:").into_iter().chain(rest.find("[image:")).min() {
        out.push_str(&rest[..i]);
        match rest[i..].find(']') {
            Some(j) => rest = &rest[i + j + 1..],
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    if out.trim().is_empty() && !l.trim().is_empty() {
        return String::new();
    }
    out
}

/// Each raw address in `line` becomes `[n]`, reusing the number of one already listed.
fn number_raw(line: &str, links: &mut Vec<(usize, String, String)>) -> String {
    if !line.contains("http://") && !line.contains("https://") {
        return line.to_string();
    }
    line.split(' ')
        .map(|w| {
            if !(w.starts_with("http://") || w.starts_with("https://")) {
                return w.to_string();
            }
            let url = unwrap_link(w.trim_end_matches(['.', ',', ';', ':']));
            let n = match links.iter().find(|l| l.1 == url) {
                Some(l) => l.0,
                None => {
                    let n = links.iter().map(|l| l.0).max().unwrap_or(0) + 1;
                    let s = site(&url);
                    links.push((n, url, s));
                    n
                }
            };
            format!("[{n}]")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Where an automated mail's footer starts.
const FOOTER: &[&str] = &[
    "unsubscribe",
    "manage your preferences",
    "update your preferences",
    "email preferences",
    "you are receiving this",
    "you're receiving this",
    "you received this",
    "this email was sent to",
    "this message was sent to",
    "privacy policy",
    "all rights reserved",
    "©",
    "copyright",
];

/// The body of `text` for reading. `automated`: fold its footer (never for a person's mail).
pub fn readable(text: &str, automated: bool) -> Readable {
    let mut lines: Vec<&str> = text.lines().collect();
    let mut links = Vec::new();
    if let Some(at) = lines.iter().rposition(|l| l.trim() == "Links:") {
        for l in &lines[at + 1..] {
            if let Some((n, url)) = l.trim().strip_prefix('[').and_then(|r| r.split_once("] "))
                && let Ok(n) = n.parse::<usize>()
            {
                let url = unwrap_link(url);
                let s = site(&url);
                links.push((n, url, s));
            }
        }
        lines.truncate(at);
    }
    // A link whose text was its own address is still raw here: it gets a number too.
    let numbered: Vec<String> = lines.iter().map(|l| number_raw(l, &mut links)).collect();
    let lines: Vec<String> = numbered
        .iter()
        .map(String::as_str)
        .filter(|l| {
            let low = l.to_lowercase();
            !BROWSER_LINES.iter().any(|b| low.contains(b))
        })
        .map(drop_placeholders)
        // A line of nothing but link numbers was a row of image links.
        .filter(|l| !l.trim().is_empty() || l.is_empty())
        .filter(|l| {
            let t = l.trim();
            t.is_empty()
                || !t
                    .split_whitespace()
                    .all(|w| w.starts_with('[') && w.ends_with(']') && w[1..w.len() - 1].parse::<usize>().is_ok())
        })
        .collect();
    let mut lines = tidy(&lines.join("\n")).lines().map(str::to_string).collect::<Vec<_>>();
    let mut footer = 0;
    if automated && lines.len() > 6 {
        let from = lines.len() / 2;
        if let Some(i) = (from..lines.len()).find(|&i| {
            let low = lines[i].to_lowercase();
            FOOTER.iter().any(|f| low.contains(f))
        }) {
            footer = lines.len() - i;
            lines.truncate(i);
            while lines.last().is_some_and(|l| l.trim().is_empty() || l.trim() == "---") {
                lines.pop();
            }
        }
    }
    Readable { lines, footer, links }
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
            | '\u{034F}'
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

    #[test]
    fn preheader_padding_leaves_nothing_behind() {
        // A newsletter preheader: U+034F and figure spaces, row after row.
        let pad = "\u{34f}\u{2007}".repeat(40);
        let t = tidy(&spaces(&sanitize(&format!(
            "Tips for the week.\n{pad}\n{pad}\n\nRead on."
        ))));
        assert_eq!(t, "Tips for the week.\n\nRead on.");
    }

    #[test]
    fn plain_links_become_numbers_and_trackers_their_real_address() {
        let t = number_links(
            "See <https://t.example.com/r/?id=1&url=https%3A%2F%2Fexample.org%2Fpage> and (https://b.example.net/x). Again https://b.example.net/x",
        );
        assert!(t.starts_with("See [1] and [2]. Again [2]"), "{t}");
        assert!(number_links("click here<https://example.org>.").starts_with("click here [1]."));
        assert!(
            t.ends_with("Links:\n[1] https://example.org/page\n[2] https://b.example.net/x"),
            "{t}"
        );
    }

    #[test]
    fn a_generated_plain_part_gives_way_to_the_html() {
        let urls = "https://t.example.com/r/?id=abcdefghijklmnopqrstuvwxyz0123456789 ".repeat(8);
        assert!(looks_generated(&format!("Hello\n{urls}\nBye")));
        assert!(looks_generated("Trouble viewing this email? View it in your browser."));
        assert!(!looks_generated(
            "Hi Alec,\n\nThursday works. See https://example.org for the room.\n\nThanks"
        ));
    }

    #[test]
    fn reading_drops_browser_lines_folds_the_footer_and_names_sites() {
        let body = "View this email in your browser\nTitle\n\nOne\nTwo [1]\nThree\nFour\nFive\nSix\n\nUnsubscribe here [2]\n123 Street, Town\n\nLinks:\n[1] https://www.example.co.uk/a\n[2] https://t.mail.example.com/u";
        let r = readable(body, true);
        assert_eq!(r.lines.first().map(String::as_str), Some("Title"));
        assert_eq!(r.lines.last().map(String::as_str), Some("Six"));
        assert_eq!(r.footer, 2);
        assert_eq!(r.links[0].2, "example.co.uk");
        assert_eq!(r.links[1].2, "example.com");
        // A person's mail keeps everything.
        assert_eq!(readable(body, false).footer, 0);
        assert_eq!(site("mailto:a@b.org?subject=x"), "mail to a@b.org");
        let r = readable("Hello\n[cid:image003.jpg@01DD]Name here\n[3] [4]\nBye", false);
        assert_eq!(r.lines, ["Hello", "Name here", "Bye"]);
    }

    /// Every fixture's rendered body, in one reviewed snapshot.
    #[test]
    fn raw_addresses_left_in_the_text_get_numbers_and_rows_of_them_go() {
        let body = "Intro https://a.example.com/x here\nhttps://t.example.com/1 https://t.example.com/2\nEnd https://a.example.com/x\n\nLinks:\n[1] https://a.example.com/x";
        let r = readable(body, false);
        assert_eq!(r.lines, vec!["Intro [1] here", "End [1]"]);
        assert_eq!(r.links.iter().map(|l| l.0).collect::<Vec<_>>(), vec![1, 2, 3]);
    }

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
