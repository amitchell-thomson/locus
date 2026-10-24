//! Time worked on each project, measured from your Claude Code sessions (spec K6: "project time
//! = measured from Claude sessions", drawn as faint bars after the fact, no check-in). This is the
//! stand-in for locus's own session measure (Slice 3's hooks), reading what Claude Code already
//! writes: one JSONL transcript per session under `~/.claude/projects/`, each line stamped with a
//! time and the working directory.
//!
//! A span opens at a prompt you typed and runs while the session stays active, closing after a
//! quiet gap; subagents' own lines are not you working. The project is the registry repo the
//! working directory belongs to. Reading is incremental: each file is read on from where the last
//! scan stopped (state in the cache directory), so a scan every minute costs only what is new.

use chrono::{DateTime, Duration, Utc};
use kairos_core::api::WorkedSpan;
use locus_contract::{ProjectKind, Registry};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// A gap longer than this ends a span.
pub const GAP_MIN: i64 = 10;
/// A span is at least this long (one prompt and a quick answer is still a sitting).
pub const MIN_SPAN_MIN: i64 = 5;
/// The scan's state in the cache directory (v2: spans carry their activity).
pub const STATE_FILE: &str = "worked-scan-v2.json";
/// Spans older than this are forgotten.
pub const KEEP_DAYS: i64 = 30;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct FileState {
    offset: u64,
    project: Option<String>,
    open: Option<(DateTime<Utc>, DateTime<Utc>)>,
    /// Lines per ten minutes of the open span.
    #[serde(default)]
    activity: Vec<u16>,
    spans: Vec<WorkedSpan>,
}

/// The registry repo a working directory belongs to: the deepest path segment that names one
/// (its id, its name, one of its tmux sessions, or one of its components' repositories).
pub fn project_for_cwd(cwd: &str, reg: &Registry) -> Option<String> {
    for seg in cwd.rsplit('/').filter(|s| !s.is_empty()) {
        let seg = seg.to_lowercase();
        let hit = reg.projects.iter().filter(|p| p.kind == ProjectKind::Repo).find(|p| {
            p.id.to_lowercase() == seg
                || p.name.to_lowercase() == seg
                || p.sessions.iter().any(|s| s.to_lowercase() == seg)
                || p.components
                    .iter()
                    .any(|c| c.repo.rsplit('/').next().is_some_and(|r| r.to_lowercase() == seg))
        });
        if let Some(p) = hit {
            return Some(p.id.clone());
        }
    }
    None
}

fn close(st: &mut FileState) {
    let activity = std::mem::take(&mut st.activity);
    if let (Some((s, e)), Some(p)) = (st.open.take(), st.project.clone()) {
        st.spans.push(WorkedSpan {
            project: p,
            start: s,
            end: e.max(s + Duration::minutes(MIN_SPAN_MIN)),
            activity,
        });
    }
}

/// Count a line at `ts` in the open span's activity.
fn count(st: &mut FileState, ts: DateTime<Utc>) {
    if let Some((s, _)) = st.open {
        let slot = ((ts - s).num_minutes().max(0) / 10) as usize;
        if st.activity.len() <= slot {
            st.activity.resize(slot + 1, 0);
        }
        st.activity[slot] = st.activity[slot].saturating_add(1);
    }
}

/// Read one transcript from `st.offset` on.
fn read_file(path: &Path, st: &mut FileState, reg: &Registry) -> std::io::Result<()> {
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    if len < st.offset {
        *st = FileState::default();
    }
    f.seek(SeekFrom::Start(st.offset))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    // Only whole lines: a line being written stays for the next scan.
    let Some(last_nl) = buf.iter().rposition(|b| *b == b'\n') else {
        return Ok(());
    };
    let gap = Duration::minutes(GAP_MIN);
    for line in buf[..=last_nl].split(|b| *b == b'\n') {
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        if v["isSidechain"].as_bool() == Some(true) {
            continue;
        }
        let Some(ts) = v["timestamp"].as_str().and_then(|t| t.parse::<DateTime<Utc>>().ok()) else {
            continue;
        };
        if st.project.is_none()
            && let Some(cwd) = v["cwd"].as_str()
        {
            st.project = project_for_cwd(cwd, reg);
        }
        let kind = v["type"].as_str().unwrap_or("");
        let typed = kind == "user" && v["origin"]["kind"].as_str() == Some("human");
        let active = typed || kind == "assistant" || kind == "user";
        match st.open {
            Some((s, e)) if active && ts >= e && ts - e <= gap => {
                st.open = Some((s, ts));
                count(st, ts);
            }
            // A line from inside the span (logs are not always in order): it counts there.
            Some((s, e)) if active && ts >= s && ts < e => count(st, ts),
            _ if typed => {
                close(st);
                st.open = Some((ts, ts));
                count(st, ts);
            }
            _ => {}
        }
    }
    st.offset += last_nl as u64 + 1;
    Ok(())
}

/// Every `*.jsonl` one level under each project directory of `root`, modified within `KEEP_DAYS`.
fn transcripts(root: &Path, now: DateTime<Utc>) -> Vec<PathBuf> {
    let cutoff = std::time::SystemTime::from(now - Duration::days(KEEP_DAYS));
    let mut out = Vec::new();
    let Ok(dirs) = std::fs::read_dir(root) else {
        return out;
    };
    for d in dirs.flatten() {
        let Ok(files) = std::fs::read_dir(d.path()) else {
            continue;
        };
        for f in files.flatten() {
            let p = f.path();
            let fresh = f.metadata().and_then(|m| m.modified()).is_ok_and(|m| m >= cutoff);
            if fresh && p.extension().is_some_and(|e| e == "jsonl") {
                out.push(p);
            }
        }
    }
    out
}

/// Scan `root` (`~/.claude/projects`), keeping state at `state_path`; return every span of the
/// last `KEEP_DAYS`, a session still active counted up to its last line, spans of one project
/// that overlap merged.
pub fn scan(root: &Path, state_path: &Path, reg: &Registry, now: DateTime<Utc>) -> Vec<WorkedSpan> {
    let mut state: BTreeMap<String, FileState> = std::fs::read_to_string(state_path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let files = transcripts(root, now);
    let keep = now - Duration::days(KEEP_DAYS);
    state.retain(|k, _| files.iter().any(|f| f.to_string_lossy() == *k));
    let mut all = Vec::new();
    for f in &files {
        let st = state.entry(f.to_string_lossy().into_owned()).or_default();
        let _ = read_file(f, st, reg);
        // A span quiet for longer than the gap is over.
        if st.open.is_some_and(|(_, e)| now - e > Duration::minutes(GAP_MIN)) {
            close(st);
        }
        st.spans.retain(|s| s.end >= keep);
        all.extend(st.spans.iter().cloned());
        if let (Some((s, e)), Some(p)) = (st.open, st.project.clone()) {
            all.push(WorkedSpan {
                project: p,
                start: s,
                end: e.max(s + Duration::minutes(MIN_SPAN_MIN)),
                activity: st.activity.clone(),
            });
        }
    }
    if let Some(dir) = state_path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(t) = serde_json::to_string(&state) {
        let _ = std::fs::write(state_path, t);
    }
    merge(all)
}

/// Overlapping spans of the same project become one (two sessions at once are one sitting),
/// their activity added slot by slot. A resumed session repeats its history in a new file, so
/// identical spans count once.
pub fn merge(mut v: Vec<WorkedSpan>) -> Vec<WorkedSpan> {
    v.sort_by(|a, b| (&a.project, a.start, a.end).cmp(&(&b.project, b.start, b.end)));
    v.dedup_by(|b, a| a.project == b.project && a.start == b.start && a.end == b.end);
    let mut out: Vec<WorkedSpan> = Vec::new();
    for s in v {
        match out.last_mut() {
            Some(l) if l.project == s.project && s.start <= l.end => {
                let off = ((s.start - l.start).num_minutes().max(0) / 10) as usize;
                if l.activity.len() < off + s.activity.len() {
                    l.activity.resize(off + s.activity.len(), 0);
                }
                for (i, n) in s.activity.iter().enumerate() {
                    l.activity[off + i] = l.activity[off + i].saturating_add(*n);
                }
                l.end = l.end.max(s.end);
            }
            _ => out.push(s),
        }
    }
    out.sort_by_key(|s| s.start);
    out
}

/// Measure this machine's sessions and send them to kairosd (the Mac's `kairos sync`).
pub fn push(cfg: &kairos_core::config::Config, machine: &str, now: DateTime<Utc>) -> Option<usize> {
    let (shared, _) = crate::taxonomy::get(cfg)?;
    let spans = scan(&claude_root(), &cfg.cache_dir().join(STATE_FILE), &shared.registry, now);
    let req = kairos_core::api::WorkedReq {
        machine: machine.into(),
        from: now - Duration::days(KEEP_DAYS),
        to: now + Duration::minutes(1),
        spans,
    };
    let d = crate::client::Daemon::from_config(cfg).ok()?;
    match d.post_worked(&req) {
        Ok(Ok(_)) => Some(req.spans.len()),
        _ => None,
    }
}

/// Where Claude Code keeps its transcripts on this machine.
pub fn claude_root() -> PathBuf {
    kairos_core::config::expand_home("~/.claude/projects")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn reg() -> Registry {
        locus_contract::taxonomy::load(&locus_contract::taxonomy::root())
            .unwrap()
            .registry
    }

    fn line(ts: &str, kind: &str, human: bool, cwd: &str) -> String {
        let origin = if human { r#","origin":{"kind":"human"}"# } else { "" };
        format!(r#"{{"type":"{kind}","timestamp":"{ts}","cwd":"{cwd}"{origin}}}"#) + "\n"
    }

    #[test]
    fn a_working_directory_finds_its_registry_repo() {
        let r = reg();
        let p = |c: &str| project_for_cwd(c, &r);
        assert_eq!(p("/Users/a/local-projects/locus").as_deref(), Some("locus"));
        assert_eq!(
            p("/Users/a/local-projects/locus/.claude/worktrees/x").as_deref(),
            Some("locus")
        );
        assert_eq!(p("/home/alec/server-projects/nabla").as_deref(), Some("nabla"));
        assert_eq!(p("/Users/a/local-projects/B3-Project").as_deref(), Some("3yp"));
        assert_eq!(p("/home/alec/server-projects/oqts/oxdaq-core").as_deref(), Some("oqts"));
        assert_eq!(p("/tmp"), None);
    }

    #[test]
    fn spans_open_at_your_prompt_and_close_after_a_quiet_gap() {
        let dir = tempfile::tempdir().unwrap();
        let proj = dir.path().join("projects/-x-locus");
        std::fs::create_dir_all(&proj).unwrap();
        let cwd = "/Users/a/local-projects/locus";
        let mut t = String::new();
        // Claude's own lines before you typed: not a span.
        t += &line("2026-10-24T08:00:00Z", "assistant", false, cwd);
        t += &line("2026-10-24T09:00:00Z", "user", true, cwd);
        t += &line("2026-10-24T09:05:00Z", "assistant", false, cwd);
        t += &line("2026-10-24T09:40:00Z", "assistant", false, cwd);
        t += &line("2026-10-24T09:44:00Z", "user", true, cwd);
        let f = proj.join("s.jsonl");
        std::fs::write(&f, &t).unwrap();
        let state = dir.path().join("state.json");
        let now: DateTime<Utc> = "2026-10-24T12:00:00Z".parse().unwrap();
        let v = scan(&dir.path().join("projects"), &state, &reg(), now);
        let got: Vec<_> = v
            .iter()
            .map(|s| (s.start.format("%H:%M").to_string(), s.end.format("%H:%M").to_string()))
            .collect();
        // 09:05 -> 09:40 is longer than the gap: two spans, the second at least five minutes.
        assert_eq!(
            got,
            [
                ("09:00".to_string(), "09:05".to_string()),
                ("09:44".into(), "09:49".into())
            ]
        );
        assert!(v.iter().all(|s| s.project == "locus"));
        // Appended lines are read from where the last scan stopped.
        let mut more = t.clone();
        more += &line("2026-10-24T09:50:00Z", "assistant", false, cwd);
        std::fs::write(&f, &more).unwrap();
        let v = scan(&dir.path().join("projects"), &state, &reg(), now);
        assert_eq!(v.len(), 2);
    }
}
