//! Dev tasks.
//!
//! `cargo xtask anonymise <in.ics|dir> <out.ics|dir> [--salt S] [--keep-titles]`
//!   Replace personal text (titles, locations, descriptions, attendees, URLs…)
//!   with stable placeholders while keeping every structural property
//!   (DTSTART/TZID/RRULE/EXDATE/RECURRENCE-ID/VTIMEZONE/VALARM/X-*), so real
//!   exports can become golden fixtures. Identical inputs map to identical
//!   placeholders (dedup cases survive); UIDs are hashed consistently across
//!   files (cross-calendar same-UID cases survive).
//!
//! `cargo xtask check-fixtures`
//!   Structural sanity check of fixtures/edge and fixtures/golden.

use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Properties whose values are personal and get replaced.
const SCRUB: &[&str] = &[
    "SUMMARY",
    "LOCATION",
    "DESCRIPTION",
    "COMMENT",
    "CONTACT",
    "URL",
    "ATTACH",
    "GEO",
    "X-WR-CALNAME",
    "X-WR-CALDESC",
    "X-APPLE-STRUCTURED-LOCATION",
    "X-ALT-DESC",
    "X-GOOGLE-CONFERENCE",
    "X-MICROSOFT-SKYPETEAMSMEETINGURL",
    "X-MICROSOFT-CDO-OWNERAPPTID",
];
/// Calendar-user properties: value and CN/EMAIL params scrubbed.
const PEOPLE: &[&str] = &["ATTENDEE", "ORGANIZER"];

pub struct Anon {
    salt: String,
    keep_titles: bool,
}

impl Anon {
    fn h(&self, kind: &str, v: &str) -> String {
        let d = Sha256::digest(format!("{}\0{kind}\0{v}", self.salt).as_bytes());
        hex::encode(&d[..4])
    }

    fn placeholder(&self, name: &str, value: &str) -> String {
        let tag = match name {
            "SUMMARY" => "Event",
            "LOCATION" | "X-APPLE-STRUCTURED-LOCATION" => "Place",
            "DESCRIPTION" | "X-ALT-DESC" | "COMMENT" => "Notes",
            "URL" | "ATTACH" | "X-GOOGLE-CONFERENCE" | "X-MICROSOFT-SKYPETEAMSMEETINGURL" => {
                "https://example.invalid/x"
            }
            "GEO" => return "0.0;0.0".into(),
            "X-WR-CALNAME" => "Calendar",
            _ => "Text",
        };
        if tag.starts_with("https") {
            return format!("{tag}{}", self.h(name, value));
        }
        // Keep deadline flags and title shape (words count) so normalisation rules still apply.
        let prefix = if value.starts_with('⚑') { "⚑ " } else { "" };
        let norm = kairos_core::dedup::normalise_title(value);
        format!("{prefix}{tag}-{}", self.h("title", &norm))
    }

    /// Anonymise one unfolded content line.
    pub fn line(&self, line: &str) -> String {
        let Ok(p) = kairos_core::ics::parse_line(line, 0) else {
            return line.to_string();
        };
        let name = p.name.as_str();
        if name == "UID" || name == "RELATED-TO" {
            let mut q = p.clone();
            q.value = format!("uid-{}@anon", self.h("uid", &p.value));
            return render(&q);
        }
        if PEOPLE.contains(&name) {
            let mut q = p.clone();
            q.value = format!(
                "mailto:person-{}@example.invalid",
                self.h("person", &p.value.to_lowercase())
            );
            for prm in &mut q.params {
                if matches!(
                    prm.name.as_str(),
                    "CN" | "EMAIL" | "SENT-BY" | "DELEGATED-TO" | "DELEGATED-FROM" | "MEMBER" | "DIR"
                ) {
                    prm.values = prm
                        .values
                        .iter()
                        .map(|v| format!("person-{}", self.h("person", &v.to_lowercase())))
                        .collect();
                }
            }
            return render(&q);
        }
        if SCRUB.contains(&name) && !(self.keep_titles && name == "SUMMARY") {
            let mut q = p.clone();
            q.value = self.placeholder(name, &kairos_core::ics::unescape_text(&p.value));
            // Location-like params (X-TITLE, X-ADDRESS) carry text too.
            for prm in &mut q.params {
                if prm.name.starts_with("X-") || prm.name == "ALTREP" {
                    prm.values = prm
                        .values
                        .iter()
                        .map(|v| format!("redacted-{}", self.h("param", v)))
                        .collect();
                }
            }
            return render(&q);
        }
        line.to_string()
    }

    pub fn text(&self, input: &str) -> String {
        kairos_core::ics::unfold(input)
            .iter()
            .map(|l| fold(&self.line(l)))
            .collect()
    }
}

fn render(p: &kairos_core::ics::Property) -> String {
    let mut c = kairos_core::ics::Component::new("X");
    c.push(p.clone());
    // Component::to_ics wraps BEGIN/END; take the middle (unfolded) line.
    let s = c.to_ics();
    let lines = kairos_core::ics::unfold(&s);
    lines.get(1).cloned().unwrap_or_default()
}

fn fold(line: &str) -> String {
    let mut out = String::new();
    let mut w = 0;
    for ch in line.chars() {
        let n = ch.len_utf8();
        if w + n > 75 {
            out.push_str("\r\n ");
            w = 1;
        }
        out.push(ch);
        w += n;
    }
    out.push_str("\r\n");
    out
}

fn ics_files(p: &Path) -> Vec<PathBuf> {
    if !p.exists() {
        return Vec::new();
    }
    if p.is_dir() {
        let mut v: Vec<PathBuf> = fs::read_dir(p)
            .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).collect())
            .unwrap_or_default();
        v.retain(|f| f.extension().is_some_and(|x| x == "ics"));
        v.sort();
        v
    } else {
        vec![p.to_path_buf()]
    }
}

fn anonymise_cmd(args: &[String]) -> Result<(), String> {
    let mut pos = Vec::new();
    let mut salt = "kairos-fixtures".to_string();
    let mut keep_titles = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--salt" => salt = it.next().ok_or("--salt needs a value")?.clone(),
            "--keep-titles" => keep_titles = true,
            _ => pos.push(a.clone()),
        }
    }
    let [input, output] = pos.as_slice() else {
        return Err("usage: cargo xtask anonymise <in.ics|dir> <out.ics|dir> [--salt S] [--keep-titles]".into());
    };
    let anon = Anon { salt, keep_titles };
    let input = PathBuf::from(input);
    let output = PathBuf::from(output);
    let files = ics_files(&input);
    if files.is_empty() {
        return Err(format!("no .ics files at {}", input.display()));
    }
    for f in &files {
        let text = fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?;
        kairos_core::ics::parse(&text).map_err(|e| format!("{}: not valid iCalendar: {e}", f.display()))?;
        let out = anon.text(&text);
        // Sanity: still parses and has the same component structure.
        let a = kairos_core::ics::parse(&text).map_err(|e| e.to_string())?;
        let b = kairos_core::ics::parse(&out).map_err(|e| format!("anonymised output broke: {e}"))?;
        if shape(&a) != shape(&b) {
            return Err(format!("{}: structure changed during anonymisation", f.display()));
        }
        let dest = if input.is_dir() || output.is_dir() {
            fs::create_dir_all(&output).map_err(|e| e.to_string())?;
            output.join(f.file_name().ok_or("bad file name")?)
        } else {
            output.clone()
        };
        fs::write(&dest, out).map_err(|e| format!("{}: {e}", dest.display()))?;
        eprintln!("anonymised {} → {}", f.display(), dest.display());
    }
    eprintln!("Review the output before committing: grep it for names, emails and places you recognise.");
    Ok(())
}

fn shape(cs: &[kairos_core::ics::Component]) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(c: &kairos_core::ics::Component, depth: usize, out: &mut Vec<String>) {
        out.push(format!(
            "{}{} {}",
            " ".repeat(depth),
            c.name,
            c.props.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(",")
        ));
        for ch in &c.children {
            walk(ch, depth + 1, out);
        }
    }
    for c in cs {
        walk(c, 0, &mut out);
    }
    out
}

fn check_fixtures() -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures");
    let mut n = 0;
    for dir in ["edge", "golden"] {
        for f in ics_files(&root.join(dir)) {
            let text = fs::read_to_string(&f).map_err(|e| e.to_string())?;
            let cals = kairos_core::ics::parse(&text).map_err(|e| format!("{}: {e}", f.display()))?;
            for cal in &cals {
                if cal.name != "VCALENDAR" {
                    return Err(format!("{}: top-level {} is not VCALENDAR", f.display(), cal.name));
                }
                for ev in cal.children_named("VEVENT") {
                    for req in ["UID", "DTSTART"] {
                        if ev.prop(req).is_none() {
                            return Err(format!("{}: VEVENT without {req}", f.display()));
                        }
                    }
                }
            }
            n += 1;
        }
    }
    eprintln!("{n} fixture files OK");
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.first().map(String::as_str) {
        Some("anonymise" | "anonymize") => anonymise_cmd(&args[1..]),
        Some("check-fixtures") => check_fixtures(),
        _ => Err("tasks: anonymise <in> <out> [--salt S] [--keep-titles] | check-fixtures".into()),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nX-WR-CALNAME:Alec's calendar\r\nBEGIN:VEVENT\r\nUID:abc-123@icloud.com\r\nDTSTART;TZID=Europe/London:20261013T100000\r\nDTEND;TZID=Europe/London:20261013T110000\r\nRRULE:FREQ=WEEKLY;COUNT=8\r\nEXDATE;TZID=Europe/London:20261020T100000\r\nSUMMARY:Dentist with Dr Jones\r\nLOCATION:12 Banbury Road\\, Oxford\r\nDESCRIPTION:Bring insurance card\r\nATTENDEE;CN=\"Jones, Sam\";PARTSTAT=ACCEPTED:mailto:sam.jones@example.com\r\nORGANIZER;CN=Alec:mailto:alec.mitchellthomson@gmail.com\r\nX-APPLE-STRUCTURED-LOCATION;VALUE=URI;X-TITLE=\"12 Banbury Road\":geo:51.76,-1.26\r\nX-KAIROS-KIND:task_block\r\nBEGIN:VALARM\r\nTRIGGER:-PT15M\r\nACTION:DISPLAY\r\nDESCRIPTION:Dentist with Dr Jones\r\nEND:VALARM\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:abc-123@icloud.com\r\nRECURRENCE-ID;TZID=Europe/London:20261027T100000\r\nDTSTART;TZID=Europe/London:20261028T100000\r\nSUMMARY:Dentist with Dr Jones\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    fn anon() -> Anon {
        Anon {
            salt: "s".into(),
            keep_titles: false,
        }
    }

    #[test]
    fn removes_personal_text() {
        let out = anon().text(REAL);
        for secret in [
            "Dentist",
            "Jones",
            "Banbury",
            "insurance",
            "alec.mitchellthomson",
            "sam.jones",
            "Alec's",
            "icloud.com",
        ] {
            assert!(!out.contains(secret), "{secret} leaked:\n{out}");
        }
    }

    #[test]
    fn keeps_structure_and_semantics() {
        let out = anon().text(REAL);
        for keep in [
            "DTSTART;TZID=Europe/London:20261013T100000",
            "RRULE:FREQ=WEEKLY;COUNT=8",
            "EXDATE;TZID=Europe/London:20261020T100000",
            "RECURRENCE-ID;TZID=Europe/London:20261027T100000",
            "X-KAIROS-KIND:task_block",
            "TRIGGER:-PT15M",
        ] {
            assert!(out.contains(keep), "{keep} lost:\n{out}");
        }
        let a = kairos_core::ics::parse(REAL).unwrap();
        let b = kairos_core::ics::parse(&out).unwrap();
        assert_eq!(shape(&a), shape(&b));
    }

    #[test]
    fn deterministic_and_consistent() {
        let a = anon().text(REAL);
        let b = anon().text(REAL);
        assert_eq!(a, b);
        // The same title and UID map to the same placeholder everywhere.
        let titles: Vec<&str> = a.lines().filter(|l| l.starts_with("SUMMARY:")).collect();
        assert_eq!(titles.len(), 2);
        assert_eq!(titles[0], titles[1]);
        let uids: Vec<&str> = a.lines().filter(|l| l.starts_with("UID:")).collect();
        assert_eq!(uids[0], uids[1]);
        // A different salt gives different placeholders.
        let c = Anon {
            salt: "t".into(),
            keep_titles: false,
        }
        .text(REAL);
        assert_ne!(a, c);
    }

    #[test]
    fn normalised_title_variants_collide() {
        let x = anon();
        assert_eq!(
            x.placeholder("SUMMARY", "OQTS committee meeting!"),
            x.placeholder("SUMMARY", "OQTS Committee Meeting")
        );
        assert!(x.placeholder("SUMMARY", "⚑ B4 sheet").starts_with("⚑ "));
    }

    #[test]
    fn fixtures_are_well_formed() {
        check_fixtures().unwrap();
    }
}
