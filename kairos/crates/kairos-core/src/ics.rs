//! A small, lossless RFC 5545 content-line parser and writer.
//!
//! We keep every component, property and parameter we don't understand so that
//! rewriting an event (e.g. moving it between calendars) never drops data such
//! as iCloud's `X-APPLE-*` properties, attendees or alarms.

use std::fmt::Write as _;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub name: String,
    pub values: Vec<String>,
    /// Whether each value was quoted in the source (kept for round-trips).
    pub quoted: Vec<bool>,
}

impl Param {
    pub fn new(name: &str, value: &str) -> Param {
        Param {
            name: name.to_ascii_uppercase(),
            values: vec![value.to_string()],
            quoted: vec![false],
        }
    }
    pub fn value(&self) -> &str {
        self.values.first().map(String::as_str).unwrap_or("")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Property {
    /// Upper-cased name.
    pub name: String,
    pub params: Vec<Param>,
    /// Raw (still escaped) value.
    pub value: String,
}

impl Property {
    pub fn new(name: &str, value: &str) -> Property {
        Property {
            name: name.to_ascii_uppercase(),
            params: Vec::new(),
            value: value.to_string(),
        }
    }

    /// A TEXT property from an unescaped string.
    pub fn text(name: &str, text: &str) -> Property {
        Property::new(name, &escape_text(text))
    }

    pub fn with_param(mut self, name: &str, value: &str) -> Property {
        self.set_param(name, value);
        self
    }

    pub fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
            .map(Param::value)
    }

    pub fn set_param(&mut self, name: &str, value: &str) {
        match self.params.iter_mut().find(|p| p.name.eq_ignore_ascii_case(name)) {
            Some(p) => *p = Param::new(name, value),
            None => self.params.push(Param::new(name, value)),
        }
    }

    pub fn remove_param(&mut self, name: &str) {
        self.params.retain(|p| !p.name.eq_ignore_ascii_case(name));
    }

    /// Unescaped TEXT value.
    pub fn text_value(&self) -> String {
        unescape_text(&self.value)
    }

    fn write_line(&self, out: &mut String) {
        let mut line = self.name.clone();
        for p in &self.params {
            line.push(';');
            line.push_str(&p.name);
            line.push('=');
            for (i, v) in p.values.iter().enumerate() {
                if i > 0 {
                    line.push(',');
                }
                let q = p.quoted.get(i).copied().unwrap_or(false) || v.contains([':', ';', ',']);
                if q {
                    line.push('"');
                    line.push_str(v);
                    line.push('"');
                } else {
                    line.push_str(v);
                }
            }
        }
        line.push(':');
        line.push_str(&self.value);
        fold_into(&line, out);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Component {
    /// Upper-cased name, e.g. `VEVENT`.
    pub name: String,
    pub props: Vec<Property>,
    pub children: Vec<Component>,
}

impl Component {
    pub fn new(name: &str) -> Component {
        Component {
            name: name.to_ascii_uppercase(),
            ..Default::default()
        }
    }

    pub fn prop(&self, name: &str) -> Option<&Property> {
        self.props.iter().find(|p| p.name.eq_ignore_ascii_case(name))
    }

    pub fn props_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Property> + 'a {
        self.props.iter().filter(move |p| p.name.eq_ignore_ascii_case(name))
    }

    pub fn value(&self, name: &str) -> Option<&str> {
        self.prop(name).map(|p| p.value.as_str())
    }

    pub fn text(&self, name: &str) -> Option<String> {
        self.prop(name).map(Property::text_value)
    }

    /// Replace all properties called `p.name` with `p`.
    pub fn set(&mut self, p: Property) {
        let pos = self.props.iter().position(|x| x.name == p.name);
        self.props.retain(|x| x.name != p.name);
        match pos {
            Some(i) if i <= self.props.len() => self.props.insert(i, p),
            _ => self.props.push(p),
        }
    }

    pub fn push(&mut self, p: Property) {
        self.props.push(p);
    }

    pub fn remove(&mut self, name: &str) {
        self.props.retain(|p| !p.name.eq_ignore_ascii_case(name));
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Component> + 'a {
        self.children.iter().filter(move |c| c.name.eq_ignore_ascii_case(name))
    }

    /// Serialise with CRLF line endings and 75-octet folding.
    pub fn to_ics(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        let _ = write!(out, "BEGIN:{}\r\n", self.name);
        for p in &self.props {
            p.write_line(out);
        }
        for c in &self.children {
            c.write(out);
        }
        let _ = write!(out, "END:{}\r\n", self.name);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IcsError {
    #[error("line {line}: no ':' in content line")]
    NoColon { line: usize },
    #[error("line {line}: END:{found} does not match BEGIN:{expected}")]
    Mismatch {
        line: usize,
        expected: String,
        found: String,
    },
    #[error("line {line}: END:{found} without BEGIN")]
    StrayEnd { line: usize, found: String },
    #[error("unterminated component {0}")]
    Unterminated(String),
    #[error("line {line}: property outside any component")]
    Orphan { line: usize },
    #[error("line {line}: bad parameter syntax")]
    BadParam { line: usize },
}

/// Unfold content lines (accepts CRLF or bare LF; ignores blank lines and a BOM).
pub fn unfold(s: &str) -> Vec<String> {
    let s = s.strip_prefix('\u{feff}').unwrap_or(s);
    let mut lines: Vec<String> = Vec::new();
    for raw in s.split('\n') {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(cont) = raw.strip_prefix([' ', '\t'])
            && let Some(last) = lines.last_mut()
        {
            last.push_str(cont);
            continue;
        }
        if raw.is_empty() {
            continue;
        }
        lines.push(raw.to_string());
    }
    lines
}

/// Parse one content line.
pub fn parse_line(line: &str, n: usize) -> Result<Property, IcsError> {
    // Split name+params from value at the first ':' outside quotes.
    let mut in_q = false;
    let mut colon = None;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_q = !in_q,
            ':' if !in_q => {
                colon = Some(i);
                break;
            }
            _ => {}
        }
    }
    let colon = colon.ok_or(IcsError::NoColon { line: n })?;
    let (head, value) = (&line[..colon], &line[colon + 1..]);
    let mut parts = split_unquoted(head, ';').into_iter();
    let name = parts.next().unwrap_or_default().to_ascii_uppercase();
    let mut params = Vec::new();
    for part in parts {
        let (pname, pval) = part.split_once('=').ok_or(IcsError::BadParam { line: n })?;
        let mut values = Vec::new();
        let mut quoted = Vec::new();
        for v in split_unquoted(pval, ',') {
            if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
                values.push(v[1..v.len() - 1].to_string());
                quoted.push(true);
            } else {
                values.push(v);
                quoted.push(false);
            }
        }
        params.push(Param {
            name: pname.to_ascii_uppercase(),
            values,
            quoted,
        });
    }
    Ok(Property {
        name,
        params,
        value: value.to_string(),
    })
}

fn split_unquoted(s: &str, sep: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    for c in s.chars() {
        if c == '"' {
            in_q = !in_q;
        }
        if c == sep && !in_q {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    out.push(cur);
    out
}

/// Parse a whole iCalendar stream into its top-level components (normally
/// one `VCALENDAR`).
pub fn parse(s: &str) -> Result<Vec<Component>, IcsError> {
    let mut stack: Vec<Component> = Vec::new();
    let mut top = Vec::new();
    for (i, line) in unfold(s).iter().enumerate() {
        let n = i + 1;
        let p = parse_line(line, n)?;
        if p.name == "BEGIN" && p.params.is_empty() {
            stack.push(Component::new(&p.value));
        } else if p.name == "END" && p.params.is_empty() {
            let c = stack.pop().ok_or_else(|| IcsError::StrayEnd {
                line: n,
                found: p.value.clone(),
            })?;
            if !c.name.eq_ignore_ascii_case(&p.value) {
                return Err(IcsError::Mismatch {
                    line: n,
                    expected: c.name,
                    found: p.value,
                });
            }
            match stack.last_mut() {
                Some(parent) => parent.children.push(c),
                None => top.push(c),
            }
        } else {
            match stack.last_mut() {
                Some(c) => c.props.push(p),
                None => return Err(IcsError::Orphan { line: n }),
            }
        }
    }
    if let Some(c) = stack.pop() {
        return Err(IcsError::Unterminated(c.name));
    }
    Ok(top)
}

/// Fold a logical line at 75 octets without splitting UTF-8 sequences.
fn fold_into(line: &str, out: &mut String) {
    let mut width = 0;
    for c in line.chars() {
        let w = c.len_utf8();
        if width + w > 75 {
            out.push_str("\r\n ");
            width = 1;
        }
        out.push(c);
        width += w;
    }
    out.push_str("\r\n");
}

pub fn escape_text(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            ';' => o.push_str("\\;"),
            ',' => o.push_str("\\,"),
            '\n' => o.push_str("\\n"),
            '\r' => {}
            c => o.push(c),
        }
    }
    o
}

pub fn unescape_text(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n' | 'N') => o.push('\n'),
                Some(x) => o.push(x),
                None => o.push('\\'),
            }
        } else {
            o.push(c);
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//test//EN\r\nBEGIN:VEVENT\r\nUID:abc@x\r\nSUMMARY:Lunch\\, with Bob\\; maybe\r\nDTSTART;TZID=Europe/London:20261023T120000\r\nATTENDEE;CN=\"Smith, J\";ROLE=REQ-PARTICIPANT:mailto:j@x.org\r\nX-APPLE-STRUCTURED-LOCATION;VALUE=URI;X-TITLE=\"Hall; A\":geo:51.7,-1.2\r\nDESCRIPTION:a very long description that will certainly need to be folded becau\r\n se it is longer than seventy five octets — with ünïcødé\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn parses_structure() {
        let cals = parse(SAMPLE).unwrap();
        assert_eq!(cals.len(), 1);
        let ev = &cals[0].children[0];
        assert_eq!(ev.name, "VEVENT");
        assert_eq!(ev.text("SUMMARY").unwrap(), "Lunch, with Bob; maybe");
        assert_eq!(ev.prop("DTSTART").unwrap().param("tzid"), Some("Europe/London"));
        assert_eq!(ev.prop("ATTENDEE").unwrap().param("CN"), Some("Smith, J"));
        assert_eq!(ev.value("X-APPLE-STRUCTURED-LOCATION"), Some("geo:51.7,-1.2"));
        assert!(ev.text("DESCRIPTION").unwrap().contains("because it is"));
        assert_eq!(ev.children[0].value("TRIGGER"), Some("-PT15M"));
    }

    #[test]
    fn round_trip_is_lossless() {
        let a = parse(SAMPLE).unwrap();
        let text = a[0].to_ics();
        let b = parse(&text).unwrap();
        assert_eq!(a, b);
        for line in text.split("\r\n") {
            assert!(line.len() <= 75, "unfolded line {line:?}");
        }
    }

    #[test]
    fn lf_only_and_tab_folding() {
        let s = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nSUMMARY:abc\n\tdef\nEND:VEVENT\nEND:VCALENDAR\n";
        let c = parse(s).unwrap();
        assert_eq!(c[0].children[0].value("SUMMARY"), Some("abcdef"));
    }

    #[test]
    fn errors() {
        assert!(matches!(
            parse("BEGIN:VCALENDAR\nEND:VEVENT\n"),
            Err(IcsError::Mismatch { .. })
        ));
        assert!(matches!(parse("BEGIN:VCALENDAR\n"), Err(IcsError::Unterminated(_))));
        assert!(matches!(parse("SUMMARY:x\n"), Err(IcsError::Orphan { .. })));
        assert!(matches!(
            parse("BEGIN:VCALENDAR\nnocolon\nEND:VCALENDAR"),
            Err(IcsError::NoColon { .. })
        ));
    }

    #[test]
    fn text_escaping_round_trip() {
        let s = "a,b;c\\d\nnew line";
        assert_eq!(unescape_text(&escape_text(s)), s);
    }

    #[test]
    fn folding_respects_utf8() {
        let mut c = Component::new("VEVENT");
        c.push(Property::text("SUMMARY", &"é".repeat(100)));
        let s = c.to_ics();
        let back = parse(&s).unwrap();
        assert_eq!(back[0].text("SUMMARY").unwrap(), "é".repeat(100));
    }

    proptest::proptest! {
        #[test]
        fn any_text_survives(s in "\\PC{0,200}") {
            let mut c = Component::new("VEVENT");
            c.push(Property::text("SUMMARY", &s));
            let back = parse(&c.to_ics()).unwrap();
            let got = back[0].text("SUMMARY").unwrap();
            proptest::prop_assert_eq!(got, s.replace('\r', ""));
        }
    }
}
