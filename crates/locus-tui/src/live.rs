//! Home live (spec `docs/redesign/slices/home-live.md`): what Home shows about your projects,
//! read from where it is true, laid over the fixtures as `calendar::apply` lays the calendar.
//!
//! - **Sessions** (H1, H2) come from Claude Code's own per-process files,
//!   `~/.claude/sessions/<pid>.json`: each names its tmux pane, its folder, its transcript and a
//!   status (`busy`, `waiting` with what for, `idle`). That is better than reading pane screens,
//!   which differ in every state (a resume picker, a question, a permission prompt) and were
//!   measured on 10 Nov to give nothing usable for half the panes. A file outlives its process,
//!   so a session counts only while its pane still runs Claude.
//! - **Last activity** (H3) is the newest commit across a repo's directories.
//! - **Careers** (H5) is `~/files/Careers/applications.toml`, kept by you and the chat until
//!   heuris H2 writes it from mail.
//! - **System** (H7) is `locus home-system` on the server, over ssh.
//!
//! Each reader is cheap and fails to "nothing found", never to a fixture: once Home is live, a
//! fixture row standing in for a real one would look true and not be (CLAUDE.md §3).

use crate::fixtures::Fixtures;
use chrono::{DateTime, FixedOffset, NaiveDate, TimeZone, Utc};
use locus_contract::{Project, ProjectKind, Registry, Session, SessionState, Status, Turn};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn at_ms(ms: i64) -> DateTime<FixedOffset> {
    Utc.timestamp_millis_opt(ms)
        .single()
        .unwrap_or_else(Utc::now)
        .fixed_offset()
}

// --- sessions --------------------------------------------------------------------------------

/// One of Claude Code's `~/.claude/sessions/<pid>.json`, the fields Home uses.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcFile {
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub cwd: String,
    /// `quast:@26.%42`: the tmux session, window and pane.
    #[serde(default)]
    pub tmux: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    /// `busy`, `waiting`, `idle`; absent in older versions.
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub waiting_for: Option<String>,
    #[serde(default)]
    pub status_updated_at: Option<i64>,
    #[serde(default)]
    pub updated_at: Option<i64>,
    #[serde(default)]
    pub started_at: Option<i64>,
}

impl ProcFile {
    fn tmux_session(&self) -> Option<&str> {
        self.tmux
            .as_deref()
            .and_then(|t| t.split(':').next())
            .filter(|s| !s.is_empty())
    }
    fn pane(&self) -> Option<&str> {
        self.tmux
            .as_deref()
            .and_then(|t| t.rsplit('.').next())
            .filter(|p| p.starts_with('%'))
    }
}

/// A live Claude session, matched to the registry or not.
#[derive(Debug, Clone, PartialEq)]
pub struct Live {
    /// The registry entry it works on; none when nothing matches (H2).
    pub id: Option<String>,
    /// Its tmux session, which names an unregistered one.
    pub tmux: String,
    pub cwd: PathBuf,
    pub session: Session,
}

/// The state a status file says, as Home draws it.
pub fn state_of(f: &ProcFile) -> SessionState {
    match f.status.as_deref() {
        Some("busy") => SessionState::Working,
        Some("waiting") => SessionState::NeedsYou,
        _ => SessionState::Idle,
    }
}

/// Panes running Claude now: pane id → its current command. A shell or an editor there means
/// the Claude that wrote the status file has exited.
fn claude_panes() -> HashMap<String, String> {
    let out = Command::new("tmux")
        .args(["list-panes", "-a", "-F", "#{pane_id} #{pane_current_command}"])
        .output();
    let Ok(out) = out else { return HashMap::new() };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split_once(' '))
        .filter(|(_, cmd)| is_claude_command(cmd))
        .map(|(p, c)| (p.to_string(), c.to_string()))
        .collect()
}

/// Claude Code shows in tmux as its version (`2.1.317`), or as `claude`/`node`.
pub fn is_claude_command(cmd: &str) -> bool {
    let cmd = cmd.trim();
    cmd == "claude"
        || cmd == "node"
        || (cmd.split('.').count() == 3 && cmd.split('.').all(|p| p.parse::<u32>().is_ok()))
}

/// Which registry entry a session works on: its tmux session named in `sessions`, else the
/// folder it runs in (`~/local-projects/<repo>` or `~/files/<Area>/<folder>`).
pub fn match_entry<'a>(reg: &'a Registry, tmux: &str, cwd: &Path, home: &Path) -> Option<&'a Project> {
    if let Some(p) = reg.projects.iter().find(|p| p.sessions.iter().any(|s| s == tmux)) {
        return Some(p);
    }
    if let Ok(rest) = cwd.strip_prefix(home.join("local-projects")) {
        let dir = rest.components().next()?.as_os_str().to_str()?;
        return reg.projects.iter().find(|p| {
            p.kind == ProjectKind::Repo
                && (p.id == dir
                    || p.folder.as_deref() == Some(dir)
                    || p.components.iter().any(|c| c.repo.split('/').next() == Some(dir)))
        });
    }
    if let Ok(rest) = cwd.strip_prefix(home.join("files")) {
        let mut parts = rest.components().filter_map(|c| c.as_os_str().to_str());
        let (area, folder) = (parts.next()?, parts.next()?);
        let hits: Vec<&Project> = reg
            .projects
            .iter()
            .filter(|p| p.area == area && p.folder.as_deref() == Some(folder))
            .collect();
        // A folder shared by a course and its repo (B3 and 3YP) goes to the repo: that is the
        // thing sessions work in.
        return hits
            .iter()
            .find(|p| p.kind == ProjectKind::Repo)
            .or(hits.first())
            .copied();
    }
    None
}

/// Where Claude Code keeps a folder's transcripts: the path with every non-alphanumeric
/// character turned into `-`.
pub fn transcript_dir(cwd: &Path, home: &Path) -> PathBuf {
    let mangled: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    home.join(".claude/projects").join(mangled)
}

/// The last thing you asked in a transcript, one line. Read backwards in chunks: a long
/// transcript is megabytes, and your last prompt can sit behind a megabyte of tool output
/// (measured 10 Nov: a fixed 512 KB tail found nothing in a working session). Stops after 16 MB.
pub fn last_prompt(path: &Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    const CHUNK: u64 = 1024 * 1024;
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let mut end = len;
    // Bytes of a line cut at the chunk's start, joined to the chunk before.
    let mut carry: Vec<u8> = Vec::new();
    while end > 0 && len - end < 16 * CHUNK {
        let start = end.saturating_sub(CHUNK);
        f.seek(SeekFrom::Start(start)).ok()?;
        let mut buf = vec![0u8; usize::try_from(end - start).ok()?];
        f.read_exact(&mut buf).ok()?;
        buf.extend_from_slice(&carry);
        // The first line may be cut, unless this chunk starts the file.
        let cut = if start == 0 {
            0
        } else {
            buf.iter().position(|&b| b == b'\n').map_or(buf.len(), |i| i + 1)
        };
        let text = String::from_utf8_lossy(&buf[cut..]);
        if let Some(p) = text.lines().rev().find_map(prompt_of) {
            return Some(p);
        }
        carry = buf[..cut].to_vec();
        end = start;
    }
    None
}

/// Your words in one transcript line, if it is a prompt you typed (not a tool result, a
/// command's output or a reminder the harness injected).
pub fn prompt_of(line: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "user" || v.get("isMeta").and_then(|m| m.as_bool()) == Some(true) {
        return None;
    }
    let content = v.get("message")?.get("content")?;
    let text = match content {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(parts) => parts
            .iter()
            .filter(|p| p.get("type").and_then(|t| t.as_str()) == Some("text"))
            .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join(" "),
        _ => return None,
    };
    let t = text.trim();
    if t.is_empty() || t.starts_with('<') || t.starts_with("Caveat:") || t.starts_with("[Request interrupted") {
        return None;
    }
    let one: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(one)
}

/// A transcript's last prompt, re-read only when the file changed.
#[derive(Default)]
pub struct Prompts(HashMap<PathBuf, (std::time::SystemTime, Option<String>)>);

impl Prompts {
    pub fn get(&mut self, path: &Path) -> Option<String> {
        let m = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
        if let Some((t, p)) = self.0.get(path)
            && *t == m
        {
            return p.clone();
        }
        let p = last_prompt(path);
        self.0.insert(path.to_path_buf(), (m, p.clone()));
        p
    }
}

/// The newest `.jsonl` in a folder that no session has claimed.
fn newest_unclaimed(dir: &Path, claimed: &std::collections::HashSet<PathBuf>) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("jsonl") && !claimed.contains(p))
        .filter_map(|p| Some((std::fs::metadata(&p).and_then(|m| m.modified()).ok()?, p)))
        .max_by_key(|(t, _)| *t)
        .map(|(_, p)| p)
}

/// Every Claude session running in tmux now.
pub fn scan_sessions(reg: &Registry, prompts: &mut Prompts) -> Vec<Live> {
    let home = home();
    let panes = claude_panes();
    let Ok(dir) = std::fs::read_dir(home.join(".claude/sessions")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    // Sessions whose own transcript exists claim it first, so a stand-in never takes one.
    let mut entries: Vec<_> = dir.flatten().map(|e| e.path()).collect();
    entries.sort();
    let mut claimed: std::collections::HashSet<PathBuf> = entries
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .filter_map(|s| serde_json::from_str::<ProcFile>(&s).ok())
        .map(|f| transcript_dir(Path::new(&f.cwd), &home).join(format!("{}.jsonl", f.session_id)))
        .filter(|t| t.exists())
        .collect();
    for path in entries {
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Some(f) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<ProcFile>(&s).ok())
        else {
            continue;
        };
        let (Some(tmux), Some(pane)) = (f.tmux_session(), f.pane()) else {
            continue;
        };
        if !panes.contains_key(pane) {
            continue;
        }
        let cwd = PathBuf::from(&f.cwd);
        let id = match_entry(reg, tmux, &cwd, &home).map(|p| p.id.clone());
        let tdir = transcript_dir(&cwd, &home);
        let mut transcript = tdir.join(format!("{}.jsonl", f.session_id));
        // Older Claude Code names the id it started with; a resumed session writes elsewhere.
        // Then the newest transcript in its folder that no other session names stands in.
        if !transcript.exists() {
            match newest_unclaimed(&tdir, &claimed) {
                Some(t) => transcript = t,
                None => transcript = PathBuf::new(),
            }
        }
        claimed.insert(transcript.clone());
        let state = state_of(&f);
        let since = f
            .status_updated_at
            .or(f.updated_at)
            .or(f.started_at)
            .map(at_ms)
            .unwrap_or_else(|| Utc::now().fixed_offset());
        out.push(Live {
            id,
            tmux: tmux.to_string(),
            cwd,
            session: Session {
                name: tmux.to_string(),
                state,
                since,
                doing: (state == SessionState::NeedsYou)
                    .then(|| f.waiting_for.clone().unwrap_or_else(|| "waiting for you".into())),
                summary: prompts.get(&transcript),
                background: f.kind.as_deref().is_some_and(|k| k != "interactive"),
            },
        });
    }
    out.sort_by(|a, b| {
        (a.session.state, std::cmp::Reverse(a.session.since))
            .cmp(&(b.session.state, std::cmp::Reverse(b.session.since)))
    });
    out
}

/// Sessions onto their projects; every fixture session goes, so a project with none says so.
/// Returns the unregistered ones by tmux session, for their own rows (H2).
pub fn apply_sessions(fx: &mut Fixtures, live: &[Live]) -> Vec<(String, Vec<Session>)> {
    for pf in fx.projects.values_mut() {
        pf.view.sessions.clear();
    }
    let mut loose: BTreeMap<String, Vec<Session>> = BTreeMap::new();
    for l in live {
        match l.id.as_ref().and_then(|id| fx.projects.get_mut(id)) {
            Some(pf) => pf.view.sessions.push(l.session.clone()),
            None if l.id.is_none() => loose.entry(l.tmux.clone()).or_default().push(l.session.clone()),
            // Registered but not active: Home does not list it, and its session is not news.
            None => {}
        }
    }
    loose.into_iter().collect()
}

// --- last activity ----------------------------------------------------------------------------

/// The directories a repo entry lives in on the laptop.
pub fn repo_dirs(p: &Project, live: &[Live], home: &Path) -> Vec<PathBuf> {
    let lp = home.join("local-projects");
    let mut dirs: Vec<PathBuf> = vec![lp.join(&p.id)];
    if let Some(f) = &p.folder {
        dirs.push(lp.join(f));
    }
    dirs.extend(p.components.iter().map(|c| lp.join(&c.repo)));
    dirs.extend(
        live.iter()
            .filter(|l| l.id.as_deref() == Some(p.id.as_str()))
            .map(|l| l.cwd.clone()),
    );
    dirs.sort();
    dirs.dedup();
    dirs.into_iter().filter(|d| d.join(".git").exists()).collect()
}

/// The newest commit in any of the directories: when, and its subject.
pub fn last_commit(dirs: &[PathBuf]) -> Option<(DateTime<FixedOffset>, String)> {
    dirs.iter()
        .filter_map(|d| {
            let out = Command::new("git")
                .arg("-C")
                .arg(d)
                .args(["log", "-1", "--format=%ct%x09%s"])
                .output()
                .ok()?;
            let s = String::from_utf8_lossy(&out.stdout);
            let (t, subject) = s.trim().split_once('\t')?;
            Some((
                Utc.timestamp_opt(t.parse().ok()?, 0).single()?.fixed_offset(),
                subject.to_string(),
            ))
        })
        .max_by_key(|(t, _)| *t)
}

/// Last commit for every active repo.
pub fn scan_commits(reg: &Registry, live: &[Live]) -> BTreeMap<String, (DateTime<FixedOffset>, String)> {
    let home = home();
    reg.projects
        .iter()
        .filter(|p| p.kind == ProjectKind::Repo && (p.active || p.state.is_some()))
        .filter_map(|p| Some((p.id.clone(), last_commit(&repo_dirs(p, live, &home))?)))
        .collect()
}

/// Git repositories in `~/local-projects` that no registry entry names (the projects view's
/// last section): each with its newest commit, newest first.
/// A folder's name and its newest commit, if it has one.
pub type Repo = (String, Option<(DateTime<FixedOffset>, String)>);

pub fn scan_unregistered_repos(reg: &Registry) -> Vec<Repo> {
    let lp = home().join("local-projects");
    let Ok(dir) = std::fs::read_dir(&lp) else {
        return Vec::new();
    };
    let known = |name: &str| {
        reg.projects.iter().any(|p| {
            p.id == name
                || p.folder.as_deref() == Some(name)
                || p.sessions.iter().any(|s| s == name)
                || p.components.iter().any(|c| c.repo.split('/').next() == Some(name))
        })
    };
    let mut out: Vec<_> = dir
        .flatten()
        .filter(|e| e.path().join(".git").exists())
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| !known(n))
        .map(|n| {
            let last = last_commit(&[lp.join(&n)]);
            (n, last)
        })
        .collect();
    out.sort_by(|a, b| b.1.as_ref().map(|x| x.0).cmp(&a.1.as_ref().map(|x| x.0)));
    out
}

// --- careers ----------------------------------------------------------------------------------

/// `~/files/Careers/applications.toml` (H5).
pub fn careers_file() -> PathBuf {
    home().join("files/Careers/applications.toml")
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CareerEntry {
    stage: String,
    /// A bare TOML date (`2026-10-22`) or the same quoted.
    since: toml::Value,
    #[serde(default)]
    turn: Option<String>,
    /// His own note on it (advice, plans; 10 Nov). Read by `context`; Home does not show it.
    #[serde(default)]
    #[allow(dead_code)]
    note: Option<String>,
}

fn date_of(v: &toml::Value) -> Option<NaiveDate> {
    let s = match v {
        toml::Value::Datetime(d) => d.to_string(),
        toml::Value::String(s) => s.clone(),
        _ => return None,
    };
    NaiveDate::parse_from_str(s.get(..10)?, "%Y-%m-%d").ok()
}

/// Each application's stage, by registry id. An unreadable file is an error Home shows, never
/// a quiet fall back to the fixture.
pub fn read_careers(path: &Path) -> Result<BTreeMap<String, Status>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let raw: BTreeMap<String, CareerEntry> =
        toml::from_str(&text).map_err(|e| format!("{}: {}", path.display(), e.message()))?;
    raw.into_iter()
        .map(|(id, e)| {
            let turn = match e.turn.as_deref().unwrap_or("") {
                "you" => Some(Turn::You),
                "them" => Some(Turn::Them),
                "" => None,
                t => return Err(format!("{id}: turn is `you`, `them` or nothing, not `{t}`")),
            };
            let since = date_of(&e.since).ok_or_else(|| format!("{id}: since is a date, 2026-10-22"))?;
            Ok((
                id,
                Status {
                    stage: e.stage,
                    since,
                    turn,
                },
            ))
        })
        .collect()
}

/// Careers' stages onto their entries; every Careers entry loses its fixture stage first.
pub fn apply_careers(fx: &mut Fixtures, careers: &BTreeMap<String, Status>) {
    let ids: Vec<String> = fx
        .registry
        .projects
        .iter()
        .filter(|p| p.area == "Careers")
        .map(|p| p.id.clone())
        .collect();
    for id in ids {
        if let Some(pf) = fx.projects.get_mut(&id) {
            pf.view.status = careers.get(&id).cloned();
        }
    }
}

// --- system ------------------------------------------------------------------------------------

/// `locus home-system`'s JSON (locus/home_system.py).
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct SystemLive {
    pub at: String,
    #[serde(default)]
    pub running: Vec<RunNow>,
    #[serde(default)]
    pub recent: Vec<RunDone>,
    pub done_today: u32,
    pub failed_today: u32,
    pub credit_used: f64,
    pub credit_since: String,
    pub calls_today: u32,
    pub backend: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct RunNow {
    pub kind: String,
    #[serde(default)]
    pub started: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct RunDone {
    pub kind: String,
    pub started: String,
    #[serde(default)]
    pub ended: Option<String>,
    pub ok: bool,
    #[serde(default)]
    pub cause: String,
}

/// Ask the server, as `locus files` does (files_sync.server_command).
pub fn fetch_system() -> Result<SystemLive, String> {
    let host = std::env::var("LOCUS_HOST").unwrap_or_else(|_| "compute-node".into());
    let dir = std::env::var("LOCUS_DIR").unwrap_or_else(|_| "/home/alec/server-projects/locus".into());
    let uv = std::env::var("LOCUS_SERVER_UV").unwrap_or_else(|_| "~/.local/bin/uv".into());
    let out = Command::new("ssh")
        .args(["-o", "ConnectTimeout=8", "-o", "BatchMode=yes", &host])
        .arg(format!("cd {dir} && {uv} run --quiet locus home-system"))
        .output()
        .map_err(|e| format!("ssh: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(err
            .lines()
            .last()
            .unwrap_or("the server did not answer")
            .trim()
            .to_string());
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("unreadable answer: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg() -> Registry {
        locus_contract::taxonomy::load(&locus_contract::taxonomy::root())
            .expect("registry")
            .registry
    }

    #[test]
    fn status_file_states() {
        let f = |s: &str| ProcFile {
            session_id: "x".into(),
            cwd: "/".into(),
            tmux: Some("quast:@26.%42".into()),
            kind: None,
            status: Some(s.into()),
            waiting_for: None,
            status_updated_at: None,
            updated_at: None,
            started_at: None,
        };
        assert_eq!(state_of(&f("busy")), SessionState::Working);
        assert_eq!(state_of(&f("waiting")), SessionState::NeedsYou);
        assert_eq!(state_of(&f("idle")), SessionState::Idle);
        assert_eq!(f("idle").tmux_session(), Some("quast"));
        assert_eq!(f("idle").pane(), Some("%42"));
    }

    #[test]
    fn claude_commands() {
        assert!(is_claude_command("2.1.317") && is_claude_command("claude"));
        assert!(!is_claude_command("zsh") && !is_claude_command("nvim") && !is_claude_command("1.2"));
    }

    #[test]
    fn sessions_match_by_name_then_folder() {
        let r = reg();
        let h = Path::new("/Users/a");
        let id = |t: &str, c: &str| match_entry(&r, t, Path::new(c), h).map(|p| p.id.clone());
        assert_eq!(id("locus-dev", "/elsewhere").as_deref(), Some("locus"));
        assert_eq!(id("x", "/Users/a/local-projects/quast/src").as_deref(), Some("quast"));
        assert_eq!(
            id("x", "/Users/a/local-projects/oqts/oxdaq-core").as_deref(),
            Some("oqts")
        );
        assert_eq!(
            id("x", "/Users/a/files/Studies/b1-engineering-computation/algos").as_deref(),
            Some("b1")
        );
        assert_eq!(id("peek", "/Users/a/local-projects/peek"), None); // aulos was registered 10 Nov
    }

    #[test]
    fn transcript_dir_mangles_like_claude_code() {
        let d = transcript_dir(Path::new("/Users/a/local-projects/locus"), Path::new("/Users/a"));
        assert!(d.ends_with(".claude/projects/-Users-a-local-projects-locus"));
    }

    #[test]
    fn prompts_are_your_words_only() {
        let p = |s: &str| prompt_of(s);
        assert_eq!(
            p(r#"{"type":"user","message":{"content":"fix the   barrier\nsweep"}}"#).as_deref(),
            Some("fix the barrier sweep")
        );
        assert_eq!(
            p(r#"{"type":"user","message":{"content":[{"type":"text","text":"go on"}]}}"#).as_deref(),
            Some("go on")
        );
        assert_eq!(
            p(r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#),
            None
        );
        assert_eq!(
            p(r#"{"type":"user","message":{"content":"<command-name>/clear</command-name>"}}"#),
            None
        );
        assert_eq!(
            p(r#"{"type":"user","isMeta":true,"message":{"content":"hidden"}}"#),
            None
        );
        assert_eq!(p(r#"{"type":"assistant","message":{"content":"hi"}}"#), None);
    }

    #[test]
    fn careers_file_reads_and_refuses_a_bad_turn() {
        let dir = tempfile::tempdir().expect("tmp");
        let f = dir.path().join("a.toml");
        std::fs::write(&f, "[jump]\nstage = \"not offered\"\nsince = 2026-10-22\n\n[millennium]\nstage = \"offer accepted\"\nsince = 2026-10-30\nturn = \"you\"\n").expect("write");
        let c = read_careers(&f).expect("reads");
        assert_eq!(c["jump"].turn, None);
        assert_eq!(c["millennium"].turn, Some(Turn::You));
        std::fs::write(&f, "[jump]\nstage = \"x\"\nsince = 2026-10-22\nturn = \"soon\"\n").expect("write");
        assert!(read_careers(&f).is_err());
    }
}
