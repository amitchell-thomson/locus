//! The Home chat (files-and-chat M2): a Claude conversation with your whole life as context,
//! for short questions and locus-like tasks that belong to no one project.
//!
//! Each message is one headless turn, `claude -p --resume <id>`, so a conversation here IS an
//! ordinary Claude Code session. That is what makes `o` (the same conversation in full Claude
//! Code, in a window of the locus session) free, and why the transcript is read back from
//! Claude Code's own file rather than kept twice: a turn you take in full Claude Code shows here
//! too.
//!
//! What it may do is set by permission rules (`chat/settings.json`), not by asking it nicely:
//! read anything; kairos's tools (kairos queues its risky changes for Approve itself); keryx's
//! read and draft tools only, never triage or cleanup, and keryx never sends; edits and moves
//! inside `~/files`; edits to the registry. Everything else is refused under `dontAsk`, and
//! the brief tells it to say `o` instead. Every change it made is listed under its reply
//! (spec C2), read from the transcript's tool calls, so the list is what it DID, not what it
//! says it did.
//!
//! It runs on your subscription, as an interactive Claude Code session does; it is not one of
//! locus's background jobs, so it does not go through `agent/claude.py`'s API-credit route.

use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::SystemTime;

/// A conversation started from locus. Its transcript is Claude Code's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub name: String,
    pub created: i64,
    pub updated: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Index {
    /// The conversation the bar continues (C1: the one you used last; never started for you).
    /// None: the next message starts a new one.
    pub current: Option<String>,
    pub conversations: Vec<Conversation>,
    /// The `remember`s you answered (H2.3), by the refused call's id: true recorded, false
    /// dropped. Kept so a fact is asked about once, however often the view is redrawn.
    #[serde(default)]
    pub answered: HashMap<String, bool>,
}

/// A fact the chat offered to remember (H2.3). `remember` is off the chat's allow list, so the
/// call is refused under `dontAsk` (nobody is there to ask) and comes back carrying the exact
/// fact; the window asks instead, with one key (heuris B).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remember {
    /// The refused tool call's id: what an answer is kept against.
    pub key: String,
    pub subject: String,
    pub slot: String,
    pub value: String,
    pub as_of: Option<String>,
}

impl Remember {
    /// As the question shows it: `millennium · stage = offer accepted`.
    pub fn line(&self) -> String {
        format!("{} · {} = {}", self.subject, self.slot, self.value)
    }
}

/// One line of a conversation as the chat view shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    You(String),
    Claude(String),
    /// A change it made, with how to undo it (C2).
    Did(String),
    /// A lookup on the way (read a file, searched mail): shown faintly, never as a change.
    Looked(String),
    /// A fact it offered to remember, which waits for your y/n (H2.3).
    Remember(Remember),
}

/// What the running turn sends back.
#[derive(Debug)]
enum Event {
    Text(String),
    Tool(String),
    Done(Result<(), String>),
}

pub struct Chat {
    /// `<state>/chat`; None in tests and dumps, where nothing runs.
    dir: Option<PathBuf>,
    pub index: Index,
    rx: Option<Receiver<Event>>,
    /// The conversation the running turn belongs to.
    pub busy: Option<String>,
    /// What the running turn has said so far.
    pub live: String,
    /// What it is doing right now ("reading quast/README.md").
    pub status: String,
    pub error: Option<String>,
    /// Lines scrolled up from the bottom of the chat view's transcript.
    pub scroll: usize,
    /// How far it can scroll, as the last draw measured it: the keys stop there.
    pub max_scroll: std::cell::Cell<usize>,
    cache: RefCell<HashMap<String, (SystemTime, Vec<Entry>)>>,
    /// A `y` being recorded on the server: what it says when it is done.
    fact_rx: Option<Receiver<Result<String, String>>>,
    /// Which question that `y` answered: asked again if the server did not record it.
    fact_key: Option<String>,
    /// What the last `y` came to ("Remembered: ..." or why not), for the window's note.
    pub fact_note: Option<String>,
}

/// Where the chat runs: your projects folder, so every repository is a relative path.
pub fn workdir() -> PathBuf {
    std::env::var_os("LOCUS_CHAT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join("local-projects"))
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// `chat/` beside this crate: the brief and the permission rules, read at every turn so an edit
/// to either applies without a rebuild.
fn assets() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("chat")
}

/// Claude Code's transcript folder for a working directory: every character that is not a
/// letter, digit or hyphen becomes a hyphen (`/Users/a/local-projects` ->
/// `-Users-a-local-projects`).
pub fn transcript_dir(cwd: &Path) -> PathBuf {
    let enc: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect();
    home().join(".claude/projects").join(enc)
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

impl Chat {
    pub fn new(state: Option<PathBuf>) -> Chat {
        let dir = state.map(|s| s.join("chat"));
        let index = dir
            .as_ref()
            .and_then(|d| std::fs::read_to_string(d.join("index.json")).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Chat {
            dir,
            index,
            rx: None,
            busy: None,
            live: String::new(),
            status: String::new(),
            error: None,
            scroll: 0,
            max_scroll: std::cell::Cell::new(0),
            cache: RefCell::new(HashMap::new()),
            fact_rx: None,
            fact_key: None,
            fact_note: None,
        }
    }

    fn save(&self) {
        if let Some(d) = &self.dir {
            let _ = std::fs::create_dir_all(d);
            if let Ok(s) = serde_json::to_string_pretty(&self.index) {
                let _ = std::fs::write(d.join("index.json"), s);
            }
        }
    }

    /// Newest first.
    pub fn list(&self) -> Vec<&Conversation> {
        let mut v: Vec<_> = self.index.conversations.iter().collect();
        v.sort_by(|a, b| b.updated.cmp(&a.updated));
        v
    }

    pub fn get(&self, id: &str) -> Option<&Conversation> {
        self.index.conversations.iter().find(|c| c.id == id)
    }

    pub fn current(&self) -> Option<&Conversation> {
        self.index.current.as_deref().and_then(|id| self.get(id))
    }

    /// The bar continues this one from now on.
    pub fn select(&mut self, id: Option<String>) {
        if self.index.current != id {
            self.index.current = id;
            self.scroll = 0;
            self.save();
        }
    }

    pub fn rename(&mut self, id: &str, name: &str) {
        if let Some(c) = self.index.conversations.iter_mut().find(|c| c.id == id) {
            c.name = name.trim().to_string();
            self.save();
        }
    }

    /// Send a message in the current conversation (a new one when there is none). Refused while
    /// a turn is running: two turns on one session would interleave in its transcript.
    pub fn send(&mut self, text: &str) -> Result<(), String> {
        if self.busy.is_some() {
            return Err("still answering the last message".into());
        }
        let Some(dir) = self.dir.clone() else {
            return Err("the chat only runs in the locus window".into());
        };
        let (id, resume) = match self.index.current.clone() {
            Some(id) if self.get(&id).is_some() => (id, true),
            _ => {
                let id = uuid::Uuid::new_v4().to_string();
                self.index.conversations.push(Conversation {
                    id: id.clone(),
                    name: name_from(text),
                    created: now(),
                    updated: now(),
                });
                self.index.current = Some(id.clone());
                (id, false)
            }
        };
        if let Some(c) = self.index.conversations.iter_mut().find(|c| c.id == id) {
            c.updated = now();
        }
        self.save();
        let args = turn_args(&dir, &id, resume, text)?;
        let (tx, rx) = channel();
        self.rx = Some(rx);
        self.busy = Some(id);
        self.live.clear();
        self.status = "thinking".into();
        self.error = None;
        self.scroll = 0;
        std::thread::spawn(move || run_turn(args, tx));
        Ok(())
    }

    /// Take what the running turn has sent. True when anything changed (worth a redraw).
    pub fn pump(&mut self) -> bool {
        if let Some(r) = self.fact_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.fact_rx = None;
            let key = self.fact_key.take();
            // "Not recorded" from the server (a slot it refused) is a failure too.
            let r = r.and_then(|said| {
                if said.starts_with("Remembered")
                    || said.starts_with("Already true")
                    || said.starts_with("Kept as history")
                {
                    Ok(said)
                } else {
                    Err(said)
                }
            });
            self.fact_note = Some(match r {
                Ok(said) => said.lines().last().unwrap_or("remembered").to_string(),
                Err(e) => {
                    // Not recorded: the question comes back, so the view never claims it was.
                    if let Some(k) = key {
                        self.index.answered.remove(&k);
                        self.save();
                    }
                    format!("not remembered: {e}")
                }
            });
            return true;
        }
        let Some(rx) = &self.rx else { return false };
        let mut changed = false;
        while let Ok(ev) = rx.try_recv() {
            changed = true;
            match ev {
                Event::Text(t) => {
                    self.live.push_str(&t);
                    self.status = "writing".into();
                }
                Event::Tool(s) => self.status = s,
                Event::Done(r) => {
                    if let Err(e) = r {
                        self.error = Some(e);
                    }
                    if let Some(id) = self.busy.take() {
                        self.cache.borrow_mut().remove(&id);
                    }
                    self.rx = None;
                    self.live.clear();
                    self.status.clear();
                    return true;
                }
            }
        }
        changed
    }

    /// The conversation as the view shows it, from Claude Code's transcript (cached until the
    /// file changes).
    pub fn entries(&self, id: &str) -> Vec<Entry> {
        let path = transcript_dir(&workdir()).join(format!("{id}.jsonl"));
        let Ok(modified) = std::fs::metadata(&path).and_then(|m| m.modified()) else {
            return Vec::new();
        };
        if let Some((at, e)) = self.cache.borrow().get(id)
            && *at == modified
        {
            return e.clone();
        }
        let e = std::fs::read_to_string(&path)
            .map(|s| parse_transcript(&s))
            .unwrap_or_default();
        self.cache.borrow_mut().insert(id.to_string(), (modified, e.clone()));
        e
    }

    /// Long enough that a fresh conversation would be quicker and cheaper (C1): said, never done.
    pub fn is_long(&self, id: &str) -> bool {
        let path = transcript_dir(&workdir()).join(format!("{id}.jsonl"));
        std::fs::metadata(path).is_ok_and(|m| m.len() > 1_500_000)
            || self.entries(id).iter().filter(|e| matches!(e, Entry::You(_))).count() > 40
    }

    /// The first fact in a conversation still waiting for your y/n.
    pub fn pending(&self, id: &str) -> Option<Remember> {
        self.entries(id).into_iter().find_map(|e| match e {
            Entry::Remember(r) if !self.index.answered.contains_key(&r.key) => Some(r),
            _ => None,
        })
    }

    /// `y` records it on the server (in the background: ssh takes a second or two); `n` drops
    /// it. Either way it is never asked again.
    pub fn answer(&mut self, r: &Remember, yes: bool) {
        self.index.answered.insert(r.key.clone(), yes);
        self.save();
        if !yes || self.dir.is_none() {
            return;
        }
        let name = self.current().map(|c| c.name.clone()).unwrap_or_default();
        let today = chrono::Local::now().format("%Y-%m-%d");
        let mut args: Vec<String> = vec![
            "facts".into(),
            "remember".into(),
            r.subject.clone(),
            r.slot.clone(),
            r.value.clone(),
            "--route".into(),
            "home".into(),
            "--source".into(),
            format!("you said so in the Home chat (\"{name}\"), {today}"),
        ];
        if let Some(d) = &r.as_of {
            args.push("--as-of".into());
            args.push(d.clone());
        }
        let (tx, rx) = channel();
        self.fact_rx = Some(rx);
        self.fact_key = Some(r.key.clone());
        self.fact_note = Some(format!("remembering {}…", r.line()));
        std::thread::spawn(move || {
            let _ = tx.send(crate::live::server_locus(&args));
        });
    }

    /// The last thing it said, for the bar.
    pub fn last_reply(&self, id: &str) -> Option<String> {
        self.entries(id).into_iter().rev().find_map(|e| match e {
            Entry::Claude(t) => Some(t),
            _ => None,
        })
    }
}

/// A conversation's name from its first message: its first line, cut at a word.
pub fn name_from(text: &str) -> String {
    let line = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("conversation")
        .trim();
    if line.chars().count() <= 48 {
        return line.to_string();
    }
    let cut: String = line.chars().take(48).collect();
    match cut.rfind(' ') {
        Some(i) if i > 24 => format!("{}…", &cut[..i]),
        _ => format!("{cut}…"),
    }
}

// --- one turn ---------------------------------------------------------------------------------

fn turn_args(dir: &Path, id: &str, resume: bool, text: &str) -> Result<Vec<String>, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let brief = std::fs::read_to_string(assets().join("brief.md")).map_err(|e| format!("no chat brief: {e}"))?;
    let home = home().to_string_lossy().to_string();
    let rules = std::fs::read_to_string(assets().join("settings.json"))
        .map_err(|e| format!("no chat permissions: {e}"))?
        .replace("__HOME__", &home);
    let settings = dir.join("settings.json");
    std::fs::write(&settings, rules).map_err(|e| e.to_string())?;
    let mut a: Vec<String> = vec![
        "-p".into(),
        text.into(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--include-partial-messages".into(),
        // Anything the rules do not allow is refused, never asked about: nobody is there to ask.
        "--permission-mode".into(),
        "dontAsk".into(),
        "--settings".into(),
        settings.to_string_lossy().into(),
        "--append-system-prompt".into(),
        brief,
        "--add-dir".into(),
        format!("{home}/files"),
        "--add-dir".into(),
        format!("{home}/.claude/projects"),
    ];
    if let Some(mcp) = keryx_mcp(dir) {
        a.push("--mcp-config".into());
        a.push(mcp.to_string_lossy().into());
    }
    if resume {
        a.push("--resume".into());
    } else {
        a.push("--session-id".into());
    }
    a.push(id.into());
    Ok(a)
}

/// keryx's MCP, on keryxd's own token (the same one locus's Approve uses), written 0600 beside
/// the index. None when keryx is not set up here: the chat still runs, without mail.
fn keryx_mcp(dir: &Path) -> Option<PathBuf> {
    let cfg = keryx_core::config::Config::load(&keryx_core::config::Config::default_path()).ok()?;
    let token = keryx_store::keychain::get("keryx-api-token").ok()?;
    let json = serde_json::json!({"mcpServers": {"keryx": {
        "type": "http",
        "url": format!("{}/mcp", cfg.server_url().trim_end_matches('/')),
        "headers": {"Authorization": format!("Bearer {token}")},
    }}});
    let path = dir.join("mcp.json");
    std::fs::write(&path, json.to_string()).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Some(path)
}

fn run_turn(args: Vec<String>, tx: Sender<Event>) {
    let mut cmd = Command::new("claude");
    cmd.args(&args)
        .current_dir(workdir())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Your subscription, never the metered key (CLAUDE.md §13: mis-wired twice before), and
        // not a child of whatever Claude Code session started this window.
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT");
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(Event::Done(Err(format!("could not start claude: {e}"))));
            return;
        }
    };
    let mut failed: Option<String> = None;
    if let Some(out) = child.stdout.take() {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            match v["type"].as_str() {
                Some("stream_event") => {
                    let ev = &v["event"];
                    if ev["type"] == "content_block_delta" && ev["delta"]["type"] == "text_delta" {
                        let _ = tx.send(Event::Text(ev["delta"]["text"].as_str().unwrap_or("").into()));
                    }
                }
                Some("assistant") => {
                    for b in v["message"]["content"].as_array().into_iter().flatten() {
                        if b["type"] == "tool_use" {
                            let (s, _) = describe_tool(b["name"].as_str().unwrap_or(""), &b["input"]);
                            let _ = tx.send(Event::Tool(s));
                        } else if b["type"] == "text" {
                            // A paragraph break between one stretch of text and the next.
                            let _ = tx.send(Event::Text("\n\n".into()));
                        }
                    }
                }
                Some("result") if v["is_error"] == true || v["subtype"] != "success" => {
                    failed = Some(v["result"].as_str().unwrap_or("the turn failed").to_string());
                }
                _ => {}
            }
        }
    }
    let mut err = String::new();
    if let Some(mut e) = child.stderr.take() {
        let _ = std::io::Read::read_to_string(&mut e, &mut err);
    }
    let status = child.wait();
    let r = match (failed, status) {
        (Some(f), _) => Err(f),
        (None, Ok(s)) if s.success() => Ok(()),
        (None, _) => Err(err.lines().last().unwrap_or("claude exited with an error").to_string()),
    };
    let _ = tx.send(Event::Done(r));
}

// --- reading a transcript ---------------------------------------------------------------------

/// What a tool call was, in words, and whether it changed anything (C2). Read-only tools are
/// lookups; a kairos call is a change unless it only lists, searches, finds or gets; a keryx
/// call is a change only when it drafts.
pub fn describe_tool(name: &str, input: &serde_json::Value) -> (String, bool) {
    let home = home().to_string_lossy().to_string();
    let short = |p: &str| p.replace(&home, "~");
    let s = |k: &str| input[k].as_str().unwrap_or("").to_string();
    match name {
        "Read" => (format!("read {}", short(&s("file_path"))), false),
        "Grep" => (format!("searched for \"{}\"", s("pattern")), false),
        "Glob" => (format!("looked for {}", s("pattern")), false),
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => {
            let p = short(&s("file_path"));
            let undo = if p.ends_with("registry.toml") {
                "undo: git checkout taxonomy/registry.toml in locus"
            } else {
                "undo: ask, or edit it back"
            };
            (format!("edited {p} · {undo}"), true)
        }
        "Bash" => {
            let c = short(&s("command"));
            let first = c.split_whitespace().next().unwrap_or("");
            let changes = matches!(first, "mv" | "mkdir" | "cp" | "rm" | "touch" | "ln");
            let undo = if first == "mv" { " · undo: move it back" } else { "" };
            (format!("ran {c}{undo}"), changes)
        }
        _ if name.starts_with("mcp__kairos__") => {
            let tool = &name["mcp__kairos__".len()..];
            let looks = ["list_", "search_", "find_", "get_"]
                .iter()
                .any(|p| tool.starts_with(p));
            let what = ["title", "summary", "name", "query"]
                .iter()
                .map(|k| s(k))
                .find(|v| !v.is_empty());
            let label = format!(
                "kairos {}{}",
                tool.replace('_', " "),
                what.map(|w| format!(": {w}")).unwrap_or_default()
            );
            if looks {
                (label, false)
            } else {
                (format!("{label} · undo: u in calendar, or ask"), true)
            }
        }
        _ if name.starts_with("mcp__keryx__") => {
            let tool = &name["mcp__keryx__".len()..];
            if tool.starts_with("draft") {
                let what = s("subject");
                (
                    format!(
                        "keryx {}{} · waits in Approve; never sent by itself",
                        tool.replace('_', " "),
                        if what.is_empty() {
                            String::new()
                        } else {
                            format!(": {what}")
                        }
                    ),
                    true,
                )
            } else {
                (format!("keryx {}", tool.replace('_', " ")), false)
            }
        }
        "mcp__locus__forget" => (format!("locus forget {}", s("fact")), true),
        _ if name.starts_with("mcp__locus__") => {
            let tool = &name["mcp__locus__".len()..];
            let what = ["query", "about", "ref"].iter().map(|k| s(k)).find(|v| !v.is_empty());
            (
                format!("locus {tool}{}", what.map(|w| format!(": {w}")).unwrap_or_default()),
                false,
            )
        }
        _ => (format!("used {name}"), false),
    }
}

/// A Claude Code transcript as entries. What you typed, what it said, and its tool calls;
/// tool results, system reminders and meta lines are left out.
pub fn parse_transcript(s: &str) -> Vec<Entry> {
    // A call that was refused or failed changed nothing, so it is never listed as a change: the
    // list is what it DID (C2). The first test ran `mkdir`, was refused by the permission
    // rules, and the view still drew "✎ ran mkdir": a change claimed and not made.
    let mut failed: std::collections::HashSet<String> = std::collections::HashSet::new();
    for line in s.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        for b in v["message"]["content"].as_array().into_iter().flatten() {
            if b["type"] == "tool_result" && b["is_error"] == true {
                failed.insert(b["tool_use_id"].as_str().unwrap_or("").to_string());
            }
        }
    }
    let mut out = Vec::new();
    for line in s.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v["isMeta"] == true || v["isSidechain"] == true {
            continue;
        }
        let content = &v["message"]["content"];
        match v["type"].as_str() {
            Some("user") => {
                let text = match content {
                    serde_json::Value::String(t) => t.clone(),
                    serde_json::Value::Array(a) => a
                        .iter()
                        .filter(|b| b["type"] == "text")
                        .filter_map(|b| b["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n"),
                    _ => String::new(),
                };
                let t = text.trim();
                if !t.is_empty() && !t.starts_with('<') {
                    out.push(Entry::You(t.to_string()));
                }
            }
            Some("assistant") => {
                for b in content.as_array().into_iter().flatten() {
                    match b["type"].as_str() {
                        Some("text") => {
                            let t = b["text"].as_str().unwrap_or("").trim();
                            if !t.is_empty() {
                                out.push(Entry::Claude(t.to_string()));
                            }
                        }
                        // Claude Code finding its own tools is not a step you took part in.
                        Some("tool_use") if b["name"] == "ToolSearch" => {}
                        // Refused, as it always is here: the fact waits for your y/n (H2.3).
                        Some("tool_use")
                            if b["name"] == "mcp__locus__remember"
                                && failed.contains(b["id"].as_str().unwrap_or("-")) =>
                        {
                            let s = |k: &str| b["input"][k].as_str().unwrap_or("").trim().to_string();
                            if !s("subject").is_empty() && !s("slot").is_empty() && !s("value").is_empty() {
                                out.push(Entry::Remember(Remember {
                                    key: b["id"].as_str().unwrap_or("").to_string(),
                                    subject: s("subject"),
                                    slot: s("slot"),
                                    value: s("value"),
                                    as_of: Some(s("as_of")).filter(|d| !d.is_empty()),
                                }));
                            }
                        }
                        Some("tool_use") => {
                            let (d, changed) = describe_tool(b["name"].as_str().unwrap_or(""), &b["input"]);
                            let refused = failed.contains(b["id"].as_str().unwrap_or("-"));
                            out.push(match (changed, refused) {
                                (true, false) => Entry::Did(d),
                                (true, true) => {
                                    Entry::Looked(format!("refused: {}", d.split(" · ").next().unwrap_or(&d)))
                                }
                                _ => Entry::Looked(d),
                            });
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Open a conversation in full Claude Code, in a new window of the locus session (or of the
/// session this window is in). Its own permissions apply there, asked as usual.
pub fn open_in_claude(id: &str) -> Result<(), String> {
    let dir = workdir();
    let cmd = format!("claude --resume {id}");
    let target = if Command::new("tmux")
        .args(["has-session", "-t", "locus"])
        .status()
        .is_ok_and(|s| s.success())
    {
        "locus:"
    } else {
        ""
    };
    let mut c = Command::new("tmux");
    c.args(["new-window", "-n", "chat", "-c"]).arg(&dir);
    if !target.is_empty() {
        c.args(["-t", target]);
    }
    c.arg(cmd);
    match c.status() {
        Ok(s) if s.success() => Ok(()),
        Ok(_) => Err("tmux would not open a window".into()),
        Err(e) => Err(format!("no tmux: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_transcript_reads_as_you_claude_and_what_it_did() {
        let t = [
            r#"{"type":"user","message":{"role":"user","content":"add a task to finish the B14 sheet"}}"#,
            r#"{"type":"user","isMeta":true,"message":{"content":"<system-reminder>x</system-reminder>"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Adding it."},{"type":"tool_use","name":"mcp__kairos__list_tasks","input":{}},{"type":"tool_use","name":"mcp__kairos__create_task","input":{"title":"B14 sheet 3"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Done: due Friday."}]}}"#,
        ]
        .join("\n");
        let e = parse_transcript(&t);
        assert_eq!(e[0], Entry::You("add a task to finish the B14 sheet".into()));
        assert_eq!(e[1], Entry::Claude("Adding it.".into()));
        assert_eq!(e[2], Entry::Looked("kairos list tasks".into()));
        assert!(matches!(&e[3], Entry::Did(d) if d.starts_with("kairos create task: B14 sheet 3")));
        assert_eq!(e[4], Entry::Claude("Done: due Friday.".into()));
        assert_eq!(e.len(), 5);
    }

    #[test]
    fn a_refused_change_is_not_listed_as_done() {
        let t = [
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"mkdir ~/files/Personal/x"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":true,"content":"Permission to use Bash has been denied"}]}}"#,
        ]
        .join("\n");
        assert_eq!(
            parse_transcript(&t),
            vec![Entry::Looked("refused: ran mkdir ~/files/Personal/x".into())]
        );
    }

    #[test]
    fn a_refused_remember_waits_for_your_answer() {
        let t = [
            r#"{"type":"user","message":{"content":"Jump said no offer, reapply next year"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"r1","name":"mcp__locus__remember","input":{"subject":"jump","slot":"outcome","value":"no offer; reapply next year"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"r1","is_error":true,"content":"Permission to use mcp__locus__remember has been denied"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Noted. y to keep it."}]}}"#,
        ]
        .join("\n");
        let e = parse_transcript(&t);
        let r = e.iter().find_map(|e| match e {
            Entry::Remember(r) => Some(r.clone()),
            _ => None,
        });
        let r = r.expect("a remember entry");
        assert_eq!(
            (r.key.as_str(), r.line().as_str()),
            ("r1", "jump · outcome = no offer; reapply next year")
        );
        // Not also listed as a refused change.
        assert!(
            !e.iter()
                .any(|e| matches!(e, Entry::Looked(s) if s.contains("remember")))
        );
    }

    #[test]
    fn only_writes_count_as_changes() {
        let j = serde_json::json!({});
        assert!(!describe_tool("mcp__keryx__search", &j).1);
        assert!(describe_tool("mcp__keryx__draft_reply", &j).1);
        assert!(!describe_tool("Bash", &serde_json::json!({"command": "git log -3"})).1);
        assert!(describe_tool("Bash", &serde_json::json!({"command": "mv a b"})).1);
    }

    #[test]
    fn names_come_from_the_first_message() {
        assert_eq!(name_from("where is quast at?"), "where is quast at?");
        let n = name_from("plan my week around the B14 sheet and the Millennium prep call on Thursday please");
        assert!(n.ends_with('…') && n.chars().count() <= 49, "{n}");
    }

    #[test]
    fn transcripts_are_where_claude_code_keeps_them() {
        let d = transcript_dir(Path::new("/Users/a/local-projects"));
        assert!(d.ends_with(".claude/projects/-Users-a-local-projects"));
    }
}
