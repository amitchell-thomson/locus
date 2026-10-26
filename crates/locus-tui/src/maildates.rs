//! Dates from mail (spec E6): interview bookings and rebookings for your applications, found in
//! keryx's local copy of your mail (read-only) and offered in Approve to put on the calendar,
//! linked to the application. Nothing reaches the calendar without you.
//!
//! The date is read from the mail's own words, deterministically: a day and month ("Wednesday 16
//! September", "15 Sep", "October 16th") and the time near it ("at 3:00pm", "9:30", "10:00-10:45").
//! No model is asked, so a date offered is a date the mail states (grounded or silent).

use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, NaiveTime, TimeZone, Utc};
use locus_contract::{ProjectKind, Registry};

/// A date one mail states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stated {
    pub day: NaiveDate,
    pub time: Option<NaiveTime>,
    /// "10:00-10:45" or "(45 min" near it.
    pub minutes: Option<u32>,
}

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

fn month_of(w: &str) -> Option<u32> {
    let w = w.to_lowercase();
    let w = w.trim_end_matches('.');
    if w.len() < 3 {
        return None;
    }
    let full = [
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
    ];
    let i = MONTHS.iter().position(|m| w.starts_with(m))?;
    // "mar" alone, "march" whole, or "sept": a prefix of the full name.
    (full[i].starts_with(w) || (i == 8 && w == "sept")).then_some(i as u32 + 1)
}

fn day_of(w: &str) -> Option<u32> {
    let d: String = w.chars().take_while(|c| c.is_ascii_digit()).collect();
    let rest = &w[d.len()..];
    if d.is_empty() || d.len() > 2 || !matches!(rest.to_lowercase().as_str(), "" | "st" | "nd" | "rd" | "th" | ",") {
        return None;
    }
    d.parse().ok().filter(|n| (1..=31).contains(n))
}

/// "3:00pm", "3pm", "15:00", "9.30", with "pm" possibly the next word.
fn time_of(w: &str, next: Option<&str>) -> Option<NaiveTime> {
    let lw = w.to_lowercase();
    let (num, mut ampm) = match lw.find(|c: char| c.is_ascii_alphabetic()) {
        Some(i) => (lw[..i].to_string(), lw[i..].to_string()),
        None => (lw.clone(), String::new()),
    };
    if ampm.is_empty()
        && let Some(n) = next
    {
        let n = n.to_lowercase();
        if n == "am" || n == "pm" || n == "a.m." || n == "p.m." {
            ampm = n;
        }
    }
    let ampm = ampm.replace('.', "");
    if !(ampm.is_empty() || ampm == "am" || ampm == "pm") {
        return None;
    }
    let (h, m) = match num.split_once([':', '.']) {
        Some((h, m)) => (h.parse::<u32>().ok()?, m.get(..2)?.parse::<u32>().ok()?),
        // A bare number is a time only with am/pm ("3pm"): "16 September" must not read as 16:00.
        None if !ampm.is_empty() => (num.parse::<u32>().ok()?, 0),
        None => return None,
    };
    let h = match ampm.as_str() {
        "pm" if h < 12 => h + 12,
        "am" if h == 12 => 0,
        _ => h,
    };
    NaiveTime::from_hms_opt(h, m, 0)
}

/// Every date `text` states, the year taken from when it was sent (a date more than a week
/// before the mail is next year's).
pub fn stated_dates(text: &str, sent: DateTime<FixedOffset>) -> Vec<Stated> {
    let words: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | ';' | '|'))
        .filter(|w| !w.is_empty())
        .map(|w| w.trim_matches(|c: char| c == ',' || c == '.' && !w.ends_with("a.m.") && !w.ends_with("p.m.")))
        .collect();
    let mut out = Vec::new();
    for (i, w) in words.iter().enumerate() {
        let Some(month) = month_of(w) else { continue };
        // "16 September" or "September 16(th)".
        let (day, after) = match (
            i.checked_sub(1).and_then(|j| day_of(words[j])),
            words.get(i + 1).and_then(|n| day_of(n)),
        ) {
            (Some(d), _) => (d, i + 1),
            (None, Some(d)) => (d, i + 2),
            _ => continue,
        };
        // "16 Oct 2026": a year written after it is taken as given.
        let (written, after) = match words.get(after).and_then(|y| y.parse::<i32>().ok()) {
            Some(y) if (2000..2100).contains(&y) => (Some(y), after + 1),
            _ => (None, after),
        };
        let mut year = written.unwrap_or(sent.year());
        let Some(mut date) = NaiveDate::from_ymd_opt(year, month, day) else {
            continue;
        };
        if written.is_none() && date < sent.date_naive() - Duration::days(7) {
            year += 1;
            match NaiveDate::from_ymd_opt(year, month, day) {
                Some(d) => date = d,
                None => continue,
            }
        }
        // The time: within the next few words ("at 3:00pm", ", 9:30", "from 10:00-10:45").
        let mut time = None;
        let mut minutes = None;
        for k in after..(after + 5).min(words.len()) {
            let word = words[k];
            let (a, b) = match word.split_once(['-', '–']) {
                Some((a, b)) if !a.is_empty() && !b.is_empty() => (a, Some(b)),
                _ => (word, None),
            };
            if let Some(t) = time_of(a, words.get(k + 1).copied()) {
                time = Some(t);
                // The end: "10:00-10:45", "10:00 - 10:45", "4:00 PM - 4:30 PM".
                let skip = usize::from(
                    words
                        .get(k + 1)
                        .is_some_and(|n| matches!(n.to_lowercase().as_str(), "am" | "pm" | "a.m." | "p.m.")),
                );
                let end = b.and_then(|b| time_of(b, words.get(k + 1).copied())).or_else(|| {
                    match words.get(k + 1 + skip).copied() {
                        Some("-" | "–" | "to") => words
                            .get(k + 2 + skip)
                            .and_then(|e| time_of(e, words.get(k + 3 + skip).copied())),
                        _ => None,
                    }
                });
                if let Some(e) = end
                    && e > t
                {
                    minutes = Some((e - t).num_minutes() as u32);
                }
                break;
            }
            if month_of(word).is_some() {
                break;
            }
        }
        // "(45 min" or "45 minutes" nearby.
        if minutes.is_none() {
            for k in after..(after + 10).min(words.len()) {
                if let Ok(n) = words[k].parse::<u32>()
                    && words.get(k + 1).is_some_and(|u| u.to_lowercase().starts_with("min"))
                    && (10..=240).contains(&n)
                {
                    minutes = Some(n);
                    break;
                }
            }
        }
        out.push(Stated {
            day: date,
            time,
            minutes,
        });
    }
    out
}

/// Dates you said no to (`n` in Approve), kept in locus's state directory: "for_id start".
pub fn dismissed() -> Vec<String> {
    locus_contract::link::state_dir()
        .and_then(|d| std::fs::read_to_string(d.join("dates-dismissed")).ok())
        .map(|t| t.lines().map(String::from).collect())
        .unwrap_or_default()
}

/// Remember a date you said no to, so it is not offered again.
pub fn dismiss(for_id: &str, at: DateTime<FixedOffset>) -> std::io::Result<()> {
    let Some(dir) = locus_contract::link::state_dir() else {
        return Err(std::io::Error::other("no state directory"));
    };
    std::fs::create_dir_all(&dir)?;
    let mut all = dismissed();
    all.push(format!("{for_id} {}", at.to_rfc3339()));
    std::fs::write(dir.join("dates-dismissed"), all.join("\n") + "\n")
}

/// The event's title from the mail: `Millennium interview`, `Jump Trading office visit`.
pub fn title_for(company: &str, subject: &str) -> String {
    let s = subject.to_lowercase();
    let what = if s.contains("visit") {
        "office visit"
    } else if s.contains("assessment") || s.contains("hackerrank") || s.contains("test") {
        "assessment"
    } else if s.contains("call") {
        "call"
    } else if s.contains("superday") {
        "superday"
    } else {
        "interview"
    };
    format!("{company} {what}")
}

/// A booking found for an application: when, how long, from which thread and mail.
#[derive(Debug, Clone, PartialEq)]
pub struct Booking {
    pub for_id: String,
    pub company: String,
    pub subject: String,
    pub thread: String,
    pub sent: DateTime<FixedOffset>,
    pub at: DateTime<FixedOffset>,
    pub minutes: u32,
    /// Every application it could be for, when the mail does not say which (`for_id` first).
    pub choices: Vec<String>,
}

/// Words that make a mail about a date to keep (a booking, a rebooking, an invitation).
fn about_a_date(s: &str) -> bool {
    // Whole words, or the start of one ("schedul" in "scheduled"): "electronically" is no call.
    let words: Vec<String> = s
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(String::from)
        .collect();
    let starts = [
        "interview",
        "booked",
        "confirm",
        "schedul",
        "resched",
        "superday",
        "invitation",
        "visit",
    ];
    words
        .iter()
        .any(|w| starts.iter().any(|k| w.starts_with(k)) || w == "call")
}

/// Letters and digits only, lower case: "Brevan Howard" and "brevanhoward.com" meet.
fn squash(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

/// Only what the newest message itself says: its quoted history (a reply's "On … wrote", an
/// Outlook "From: / Sent:" block, a "____" rule) is dropped, or old times read as new ones.
pub fn own_text(body: &str) -> String {
    body.lines()
        .take_while(|l| {
            let t = l.trim_start();
            !(t.starts_with('>')
                || (t.starts_with("On ") && (t.contains("wrote") || t.contains(" at ")))
                || t.starts_with("From:")
                || t.starts_with("Sent:")
                || t.starts_with("-----Original")
                || t.starts_with("________"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Which of a company's applications a mail is about: the one whose year is in the subject (or,
/// failing that, the mail), or the only one. Several and nothing to tell them apart: none, and
/// the caller offers the date with every one as a choice for you to make.
fn which<'a>(apps: &[&'a locus_contract::Project], subject: &str) -> Option<&'a locus_contract::Project> {
    if apps.len() == 1 {
        return apps.first().copied();
    }
    let year = |p: &locus_contract::Project| {
        p.name
            .split_whitespace()
            .find(|w| w.len() == 4 && w.chars().all(|c| c.is_ascii_digit()))
            .map(String::from)
    };
    let hits: Vec<_> = apps
        .iter()
        .filter(|p| year(p).is_some_and(|y| subject.contains(&y)))
        .collect();
    (hits.len() == 1).then(|| *hits[0])
}

/// Bookings in the last `days` of mail for each active application, newest mail last. Reads
/// keryx's local copy only, read-only: nothing in your mail changes.
/// `past`: also bookings already over (to judge the finder on old mail).
pub fn scan(reg: &Registry, now: DateTime<Utc>, days: i64, past: bool) -> Vec<Booking> {
    let since = (now - Duration::days(days)).format("%Y-%m-%d").to_string();
    let mut companies: Vec<&str> = reg
        .projects
        .iter()
        .filter(|p| p.kind == ProjectKind::Application && p.active)
        .filter_map(|p| p.company.as_deref())
        .collect();
    companies.sort_unstable();
    companies.dedup();
    let mut out = Vec::new();
    let Some(mirror) = crate::mail::Mirror::open() else {
        return out;
    };
    for company in companies {
        let apps: Vec<&locus_contract::Project> = reg
            .projects
            .iter()
            .filter(|p| p.kind == ProjectKind::Application && p.active && p.company.as_deref() == Some(company))
            .collect();
        let q = format!("\"{company}\" after:{since}");
        for t in mirror.search(&q) {
            let subject = t.subject.clone();
            let snippet = t.snippet.as_str();
            if !about_a_date(&subject) && !about_a_date(snippet) {
                continue;
            }
            let Ok(thread) = mirror.db.thread(&t.account, &t.thread_id) else {
                continue;
            };
            // The newest message in the thread says where things stand now.
            let Some(m) = thread.last() else {
                continue;
            };
            // The firm is who it is from or what it is about, not a name in passing (a society's
            // drinks invitation lists its sponsors).
            let sender = m
                .from
                .as_ref()
                .map(|a| format!("{} {}", a.name.as_deref().unwrap_or(""), a.email))
                .unwrap_or_default();
            let firm = squash(company);
            if !squash(&subject).contains(&firm) && !squash(&sender).contains(&firm) {
                continue;
            }
            let Some(sent) =
                DateTime::from_timestamp(m.date, 0).map(|t| t.with_timezone(&mirror.cfg.tz()).fixed_offset())
            else {
                continue;
            };
            let own = own_text(&m.body_text);
            // Told apart by the subject, then by the mail itself; else you choose in Approve.
            let (app, choices) = match which(&apps, &subject).or_else(|| which(&apps, &own)) {
                Some(a) => (a, Vec::new()),
                None => (apps[0], apps.iter().map(|p| p.id.clone()).collect()),
            };
            let when = stated_dates(&format!("{subject}\n{own}"), sent)
                .into_iter()
                .filter_map(|s| {
                    let local = s.day.and_time(s.time?);
                    let at = sent.timezone().from_local_datetime(&local).single()?;
                    (past || at.with_timezone(&Utc) > now).then_some((at, s.minutes))
                })
                .next();
            let Some((at, minutes)) = when else { continue };
            out.push(Booking {
                for_id: app.id.clone(),
                company: company.to_string(),
                subject,
                thread: t.r#ref.clone(),
                sent,
                at,
                minutes: minutes.unwrap_or(60),
                choices,
            });
        }
    }
    out.sort_by_key(|b| b.sent);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(s: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(s).unwrap()
    }

    fn at(v: &[Stated]) -> Vec<String> {
        v.iter()
            .map(|s| {
                format!(
                    "{} {} {}",
                    s.day,
                    s.time.map_or("-".into(), |t| t.format("%H:%M").to_string()),
                    s.minutes.map_or("-".into(), |m| m.to_string())
                )
            })
            .collect()
    }

    #[test]
    fn the_way_bookings_are_written() {
        // As your own interview mails put it (Jump, Millennium, 14-29 Sep 2026).
        let s = sent("2026-09-14T21:50:03+01:00");
        assert_eq!(
            at(&stated_dates(
                "We are confirmed for your interview on Wednesday 16 September at 3:00pm UK time.",
                s
            )),
            ["2026-09-16 15:00 -"]
        );
        assert_eq!(
            at(&stated_dates(
                "Interview booked for 15 Sep, 9:30 (45 min, video plus live coding)",
                s
            )),
            ["2026-09-15 09:30 45"]
        );
        assert_eq!(
            at(&stated_dates("Interview settled for 23 Sep, 10:00-10:45", s)),
            ["2026-09-23 10:00 45"]
        );
        assert_eq!(
            at(&stated_dates("Moved to 1 Oct, 9:00; expect a technical interview", s)),
            ["2026-10-01 09:00 -"]
        );
        assert_eq!(
            at(&stated_dates("See you on October 16th at 10am", s)),
            ["2026-10-16 10:00 -"]
        );
        assert_eq!(at(&stated_dates("The deadline is 31 October", s)), ["2026-10-31 - -"]);
    }

    #[test]
    fn numbers_that_are_not_times_or_dates_are_left_alone() {
        let s = sent("2026-12-20T10:00:00+00:00");
        // A date more than a week before the mail is next year's.
        assert_eq!(at(&stated_dates("Your start date is 5 January", s)), ["2027-01-05 - -"]);
        assert!(stated_dates("Meeting ID: 956 3065 1608, password 687039", s).is_empty());
        assert!(
            stated_dates("We may be in touch in March or May", s).is_empty(),
            "a month with no day"
        );
    }

    #[test]
    fn a_reply_reads_only_its_own_words_and_names_the_right_application() {
        // Your Brevan Howard reply quoted your mail of 11 Aug 21:19; that is not a booking.
        let body = "Hi Alec,\n\nI've just sent across a teams invite.\n\nFrom: Alec\nSent: 11 August 2026 21:19\nSubject: Re: Catch Up Call";
        assert!(stated_dates(&own_text(body), sent("2026-08-12T09:00:00+01:00")).is_empty());
        assert!(
            !about_a_date("you electronically signed the document"),
            "no call in electronically"
        );
        assert!(about_a_date("Jump Trading | Interview Confirmation"));
        // Millennium's scheduler writes a range in 12-hour time with the year.
        let s = sent("2026-10-15T12:00:00+01:00");
        assert_eq!(
            at(&stated_dates("16 Oct 2026\n4:00 PM - 4:30 PM Europe/London (BST)", s)),
            ["2026-10-16 16:00 30"]
        );
        // Two applications with one firm: the year in the subject decides; none, nothing offered.
        let reg = locus_contract::taxonomy::load(&locus_contract::taxonomy::root())
            .unwrap()
            .registry;
        let apps: Vec<_> = reg
            .projects
            .iter()
            .filter(|p| p.company.as_deref() == Some("Millennium"))
            .collect();
        let subject = "Scheduled: Alec <> Millennium 2027 Quantitative Developer Intern, London";
        if apps.len() > 1 {
            assert_eq!(which(&apps, subject).map(|p| p.id.as_str()), Some("millennium"));
            assert_eq!(which(&apps, "Catch up"), None);
        }
    }
}
