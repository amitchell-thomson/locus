//! Search syntax (SPEC §7): free text and quoted phrases, plus operators
//! `from:` `to:` `subject:` `account:` `key:` `in:` `is:` `has:` `before:`
//! `after:`. An unknown operator is an error naming the valid ones — never
//! a silent free-text match.

use chrono::{DateTime, Duration, NaiveDate, TimeZone};
use chrono_tz::Tz;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum In {
    Inbox,
    Archive,
    Snoozed,
    Sent,
    Flagged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Is {
    Unread,
    Read,
    Flagged,
    Snoozed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Has {
    Attachment,
    Task,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Query {
    /// Free-text words and phrases (all must match: subject, people or body).
    pub text: Vec<String>,
    pub from: Vec<String>,
    pub to: Vec<String>,
    pub subject: Vec<String>,
    pub account: Vec<String>,
    pub key: Vec<String>,
    pub in_: Vec<In>,
    pub is: Vec<Is>,
    pub has: Vec<Has>,
    /// Unix seconds: messages strictly before / at-or-after.
    pub before: Option<i64>,
    pub after: Option<i64>,
}

pub const OPERATORS: &[&str] = &[
    "from", "to", "subject", "account", "key", "in", "is", "has", "before", "after",
];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QueryError {
    #[error("unknown search operator {0:?}: use one of from: to: subject: account: key: in: is: has: before: after:")]
    UnknownOperator(String),
    #[error("{op}:{value} isn't valid; use {valid}")]
    BadValue {
        op: &'static str,
        value: String,
        valid: &'static str,
    },
    #[error("{op}: needs a value")]
    Empty { op: String },
    #[error("unclosed quote")]
    UnclosedQuote,
}

/// Split into tokens, keeping `"quoted phrases"` and `op:"quoted value"`.
fn tokens(q: &str) -> Result<Vec<String>, QueryError> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    for c in q.chars() {
        match c {
            '"' => {
                in_q = !in_q;
                cur.push(c);
            }
            c if c.is_whitespace() && !in_q => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if in_q {
        return Err(QueryError::UnclosedQuote);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    Ok(out)
}

fn unquote(s: &str) -> String {
    s.strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(s)
        .to_string()
}

fn date(op: &'static str, v: &str, now: DateTime<Tz>) -> Result<i64, QueryError> {
    let bad = || QueryError::BadValue {
        op,
        value: v.to_string(),
        valid: "a date like 2026-10-14, or 7d / 2w / 3m (days, weeks, months ago)",
    };
    if let Ok(d) = NaiveDate::parse_from_str(v, "%Y-%m-%d") {
        let start = d.and_hms_opt(0, 0, 0).ok_or_else(bad)?;
        return now
            .timezone()
            .from_local_datetime(&start)
            .earliest()
            .map(|t| t.timestamp())
            .ok_or_else(bad);
    }
    let (n, unit) = v.split_at(v.len().saturating_sub(1));
    let n: i64 = n.parse().map_err(|_| bad())?;
    let days = match unit {
        "d" => n,
        "w" => n * 7,
        "m" => n * 30,
        "y" => n * 365,
        _ => return Err(bad()),
    };
    Ok((now - Duration::days(days)).timestamp())
}

/// Parse a query. `now` resolves relative dates.
pub fn parse(q: &str, now: DateTime<Tz>) -> Result<Query, QueryError> {
    let mut out = Query::default();
    for tok in tokens(q)? {
        // An operator is letters followed by ':'; URLs (x://) and times
        // (17:00) stay free text.
        let op = tok
            .split_once(':')
            .filter(|(k, v)| !k.is_empty() && k.chars().all(|c| c.is_ascii_alphabetic()) && !v.starts_with("//"));
        let Some((k, v)) = op else {
            let t = unquote(&tok);
            if !t.is_empty() {
                out.text.push(t);
            }
            continue;
        };
        let k = k.to_ascii_lowercase();
        let v = unquote(v);
        if !OPERATORS.contains(&k.as_str()) {
            return Err(QueryError::UnknownOperator(k));
        }
        if v.is_empty() {
            return Err(QueryError::Empty { op: k });
        }
        let lv = v.to_ascii_lowercase();
        match k.as_str() {
            "from" => out.from.push(v),
            "to" => out.to.push(v),
            "subject" => out.subject.push(v),
            "account" => out.account.push(lv),
            "key" => out.key.push(v),
            "in" => out.in_.push(match lv.as_str() {
                "inbox" => In::Inbox,
                "archive" | "archived" => In::Archive,
                "snoozed" => In::Snoozed,
                "sent" => In::Sent,
                "flagged" => In::Flagged,
                _ => {
                    return Err(QueryError::BadValue {
                        op: "in",
                        value: v,
                        valid: "inbox, archive, snoozed, sent or flagged",
                    });
                }
            }),
            "is" => out.is.push(match lv.as_str() {
                "unread" => Is::Unread,
                "read" => Is::Read,
                "flagged" | "starred" => Is::Flagged,
                "snoozed" => Is::Snoozed,
                _ => {
                    return Err(QueryError::BadValue {
                        op: "is",
                        value: v,
                        valid: "unread, read, flagged or snoozed",
                    });
                }
            }),
            "has" => out.has.push(match lv.as_str() {
                "attachment" | "attachments" => Has::Attachment,
                "task" => Has::Task,
                _ => {
                    return Err(QueryError::BadValue {
                        op: "has",
                        value: v,
                        valid: "attachment or task",
                    });
                }
            }),
            "before" => out.before = Some(date("before", &lv, now)?),
            "after" => out.after = Some(date("after", &lv, now)?),
            _ => return Err(QueryError::UnknownOperator(k)),
        }
    }
    Ok(out)
}

/// FTS5 string literal.
pub fn fts_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

impl Query {
    /// The FTS5 MATCH expression for the text and subject parts (None if
    /// the query has no full-text part). `from:`/`to:` are matched on the
    /// address columns by the store, not here.
    pub fn fts(&self) -> Option<String> {
        let mut parts: Vec<String> = self.text.iter().map(|t| fts_quote(t)).collect();
        parts.extend(self.subject.iter().map(|s| format!("subject:{}", fts_quote(s))));
        (!parts.is_empty()).then(|| parts.join(" AND "))
    }

    pub fn is_empty(&self) -> bool {
        *self == Query::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::Europe::London;

    fn now() -> DateTime<Tz> {
        London.with_ymd_and_hms(2026, 10, 14, 15, 0, 0).unwrap()
    }

    fn p(q: &str) -> Result<Query, QueryError> {
        parse(q, now())
    }

    #[test]
    fn free_text_and_phrases() {
        let q = p(r#"lab "problem sheet" 17:00 https://x.example/a"#).unwrap();
        assert_eq!(q.text, vec!["lab", "problem sheet", "17:00", "https://x.example/a"]);
        assert_eq!(
            q.fts().unwrap(),
            r#""lab" AND "problem sheet" AND "17:00" AND "https://x.example/a""#
        );
    }

    #[test]
    fn each_operator() {
        let q =
            p(r#"from:jane to:"Sam Smith" subject:budget account:Oxford key:Admin in:inbox is:unread has:attachment"#)
                .unwrap();
        assert_eq!(q.from, vec!["jane"]);
        assert_eq!(q.to, vec!["Sam Smith"]);
        assert_eq!(q.subject, vec!["budget"]);
        assert_eq!(q.account, vec!["oxford"]);
        assert_eq!(q.key, vec!["Admin"]);
        assert_eq!(q.in_, vec![In::Inbox]);
        assert_eq!(q.is, vec![Is::Unread]);
        assert_eq!(q.has, vec![Has::Attachment]);
        assert_eq!(q.fts().unwrap(), r#"subject:"budget""#);
        let q = p("in:archive in:snoozed in:sent in:flagged is:read is:flagged is:snoozed has:task").unwrap();
        assert_eq!(q.in_, vec![In::Archive, In::Snoozed, In::Sent, In::Flagged]);
        assert_eq!(q.is, vec![Is::Read, Is::Flagged, Is::Snoozed]);
        assert_eq!(q.has, vec![Has::Task]);
    }

    #[test]
    fn dates() {
        let q = p("after:2026-10-01 before:2026-10-14").unwrap();
        // Midnight London (BST) = 23:00 UTC the day before.
        assert_eq!(q.after, Some(1_790_809_200));
        assert_eq!(q.before, Some(1_791_932_400));
        let q = p("after:7d before:2w").unwrap();
        assert_eq!(q.after, Some((now() - Duration::days(7)).timestamp()));
        assert_eq!(q.before, Some((now() - Duration::days(14)).timestamp()));
        assert!(matches!(
            p("after:yesterday"),
            Err(QueryError::BadValue { op: "after", .. })
        ));
    }

    #[test]
    fn unknown_operators_are_errors_naming_the_valid_ones() {
        let e = p("form:jane").unwrap_err();
        assert_eq!(e, QueryError::UnknownOperator("form".into()));
        assert!(e.to_string().contains("from:") && e.to_string().contains("after:"));
        assert!(matches!(p("in:spam"), Err(QueryError::BadValue { op: "in", .. })));
        assert!(matches!(p("is:important"), Err(QueryError::BadValue { op: "is", .. })));
        assert!(matches!(p("has:pdf"), Err(QueryError::BadValue { op: "has", .. })));
        assert!(matches!(p("from:"), Err(QueryError::Empty { .. })));
        assert_eq!(p(r#""unclosed"#), Err(QueryError::UnclosedQuote));
    }

    #[test]
    fn fts_quoting_is_injection_safe() {
        assert_eq!(p(r#"a"b"#), Err(QueryError::UnclosedQuote));
        // FTS syntax in free text is just text.
        let q = p(r#""x" OR NEAR(y)"#).unwrap();
        assert_eq!(q.fts().unwrap(), r#""x" AND "OR" AND "NEAR(y)""#);
        assert_eq!(p(r#"body:"y""#), Err(QueryError::UnknownOperator("body".into())));
        assert_eq!(fts_quote(r#"x" OR body:"y"#), r#""x"" OR body:""y""#);
        assert!(p("").unwrap().is_empty());
        assert_eq!(p("").unwrap().fts(), None);
    }
}
