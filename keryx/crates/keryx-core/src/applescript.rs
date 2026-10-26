//! AppleScript safety (SPEC §5.5). keryxd passes values to scripts as
//! `osascript` arguments (`on run argv`), never by building script text, and
//! where a literal is unavoidable it goes through [`quote`] — the only way
//! email-derived text may enter a script.

/// An AppleScript string literal for `s`.
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Other controls have no meaning in mail text; drop them.
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Lex an AppleScript string literal at the start of `src`, returning the
/// value and the rest. Mirrors AppleScript's escapes; used to prove that
/// [`quote`] round-trips and can't end early.
pub fn lex_literal(src: &str) -> Option<(String, &str)> {
    let mut chars = src.char_indices();
    if chars.next()?.1 != '"' {
        return None;
    }
    let mut out = String::new();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Some((out, &src[i + 1..])),
            '\\' => match chars.next()?.1 {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                o => out.push(o),
            },
            c => out.push(c),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn cases() {
        assert_eq!(quote("plain"), "\"plain\"");
        assert_eq!(quote("a\"b"), "\"a\\\"b\"");
        assert_eq!(quote("a\\b"), "\"a\\\\b\"");
        assert_eq!(quote("line\nnext"), "\"line\\nnext\"");
        assert_eq!(quote("x\u{0}y\u{1b}z"), "\"xyz\"");
        let evil = "\" & (do shell script \"rm -rf ~\") & \"";
        let q = quote(evil);
        let (v, rest) = lex_literal(&q).unwrap();
        assert_eq!(v, evil);
        assert_eq!(rest, "");
    }

    proptest! {
        #[test]
        fn quoted_strings_round_trip_and_never_end_early(s in "\\PC*|[\"\\\\\n\r\ta-z&()]{0,40}") {
            let q = quote(&s);
            let (v, rest) = lex_literal(&q).unwrap();
            prop_assert_eq!(rest, "", "literal ended early: {:?}", q);
            let expected: String = s.chars().filter(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t')).collect();
            prop_assert_eq!(v, expected);
        }
    }
}
