//! Mail, live (keryx-joins-locus X2): what keryx needs you to decide comes to locus's Approve,
//! as kairos's calendar changes do: cleanup proposals wait for your yes to file or archive.
//! Claude's drafts do not (10 Nov): they are in keryx's Drafts, where you read, edit and send. Everything goes through
//! keryxd's API, keryxd being the only writer to your mail; nothing here sends or files by
//! itself.

use crate::fixtures::Fixtures;
use keryx_core::changeset::Changeset;
use keryx_core::config::Config;
use keryx_store::api::{DecideOne, Decision, ReviewItem};
use keryx_store::client::{Client, Daemon};
use keryx_store::state::{Action, ProposalStatus};
use locus_contract::{ApproveDetail, ApproveGroup, ApproveItem, ApproveKind, MailDecision};
use std::collections::{BTreeMap, HashMap};

/// A connection to keryxd, from keryx's own config, as you (your keypresses in Approve).
pub struct Live {
    pub(crate) client: Client,
}

impl Live {
    pub fn connect() -> Option<Live> {
        let cfg = Config::load(&Config::default_path()).ok()?;
        let client = Client::from_config(&cfg).ok()?.interactive(true);
        Some(Live { client })
    }

    /// Mail drafted and waiting for your yes to send.
    pub fn drafts(&self) -> Vec<Changeset> {
        self.client
            .changesets(Some("pending"), 100)
            .unwrap_or_default()
            .into_iter()
            .filter(|c| c.outgoing().is_some())
            .collect()
    }

    /// Cleanup proposals still open, across the open reviews.
    pub fn proposals(&self) -> Vec<ReviewItem> {
        self.client
            .reviews(Some("open"))
            .unwrap_or_default()
            .into_iter()
            .flat_map(|v| v.items)
            .filter(|i| i.proposal.status == ProposalStatus::Open && !i.stale)
            .collect()
    }

    /// Approve a proposal as you changed it: somewhere else, or archived instead.
    pub fn change(&self, proposal: &str, action: &Action) -> Result<String, String> {
        self.client
            .decide(proposal, &Decision::Change(action.clone()))
            .map(|_| "done".to_string())
            .map_err(|e| e.to_string())
    }

    /// Yes or no on a mail item.
    pub fn decide(&self, d: &MailDecision, yes: bool) -> Result<String, String> {
        match d {
            MailDecision::Send(id) => {
                let r = if yes {
                    self.client.approve(id)
                } else {
                    self.client.reject(id)
                };
                r.map(|_| {
                    if yes {
                        "sending after the hold (u in keryx cancels)"
                    } else {
                        "not sent"
                    }
                    .to_string()
                })
                .map_err(|e| e.to_string())
            }
            MailDecision::Proposal(id) => self
                .client
                .decide(id, if yes { &Decision::Accept } else { &Decision::Decline })
                .map(|_| if yes { "done" } else { "left as it is" }.to_string())
                .map_err(|e| e.to_string()),
        }
    }

    /// Undo a changeset: the mail goes back where it was, and keryxd reopens the proposal.
    pub fn undo(&self, changeset: &str) -> Result<String, String> {
        self.client
            .undo(changeset)
            .map(|c| c.summary)
            .map_err(|e| e.to_string())
    }
}

/// Decisions on their way to keryxd, off the key's path (10 Nov: "very large delay between
/// pressing y and it happening"): the item leaves the list when you press, and a thread of its
/// own sends what you decided.
///
/// **In batches** (10 Nov, after a 500-item approval run took most of an hour one request at a
/// time): the worker takes everything waiting, and whatever else arrives within a third of a
/// second, and sends it as `decide-many` calls of up to 100, so keryxd carries out each group of
/// like actions as one changeset. What was sent is remembered for the session, so the minute's
/// refresh cannot bring back an item keryxd has not reached yet; a decision keryx refuses is
/// forgotten again, so its item comes back to be tried again, and the note says how many.
pub struct Queue {
    tx: std::sync::mpsc::Sender<(Job, String)>,
    sent: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    state: std::sync::Arc<std::sync::Mutex<QueueState>>,
}

/// One decision on its way to keryxd.
enum Job {
    Decide(MailDecision, bool),
    /// A proposal approved as you changed it.
    Change(String, Action),
    /// Put back what a decision did.
    Undo(String),
}

#[derive(Default)]
struct QueueState {
    waiting: usize,
    done: usize,
    /// Refusals since the last note: (what, why).
    failed: Vec<(String, String)>,
    failed_total: usize,
    changed: bool,
    /// What `u` can put back, newest last: (what, the proposals, the changeset).
    undoable: Vec<(String, Vec<String>, String)>,
    /// A note from the worker itself (an undo done).
    said: Option<String>,
}

fn decision_id(d: &MailDecision) -> &str {
    match d {
        MailDecision::Send(id) | MailDecision::Proposal(id) => id,
    }
}

const BATCH: usize = 100;

impl Queue {
    pub fn start(live: std::sync::Arc<Live>) -> Queue {
        let (tx, rx) = std::sync::mpsc::channel::<(Job, String)>();
        let state = std::sync::Arc::new(std::sync::Mutex::new(QueueState::default()));
        let sent: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>> = Default::default();
        let (st, sent2) = (state.clone(), sent.clone());
        std::thread::spawn(move || {
            while let Ok(first) = rx.recv() {
                let mut jobs = vec![first];
                // A burst of keys (y y y, or Y on a group) goes as one batch.
                std::thread::sleep(std::time::Duration::from_millis(350));
                while let Ok(j) = rx.try_recv() {
                    jobs.push(j);
                }
                let mut batch: Vec<(DecideOne, String)> = Vec::new();
                let mut others: Vec<(Job, String)> = Vec::new();
                for (job, what) in jobs {
                    match job {
                        Job::Decide(MailDecision::Proposal(id), yes) => batch.push((
                            DecideOne {
                                proposal: id,
                                decision: if yes { Decision::Accept } else { Decision::Decline },
                            },
                            what,
                        )),
                        Job::Change(id, a) => batch.push((
                            DecideOne {
                                proposal: id,
                                decision: Decision::Change(a),
                            },
                            what,
                        )),
                        other => others.push((other, what)),
                    }
                }
                for chunk in batch.chunks(BATCH) {
                    let items: Vec<DecideOne> = chunk.iter().map(|x| x.0.clone()).collect();
                    let what_of = |id: &str| {
                        chunk
                            .iter()
                            .find(|x| x.0.proposal == id)
                            .map(|x| x.1.clone())
                            .unwrap_or_default()
                    };
                    let r = live.client.decide_many(&items);
                    let Ok(mut s) = st.lock() else { return };
                    s.waiting = s.waiting.saturating_sub(chunk.len());
                    let mut back: Vec<String> = Vec::new();
                    match r {
                        Ok(out) => {
                            let mut by_cs: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
                            for d in out.done {
                                match (d.error, d.changeset) {
                                    (Some(e), _) if e.starts_with("already") => s.done += 1,
                                    (Some(e), _) => {
                                        s.failed.push((what_of(&d.proposal), e));
                                        s.failed_total += 1;
                                        back.push(d.proposal);
                                    }
                                    (None, cs) => {
                                        s.done += 1;
                                        if let Some(cs) = cs {
                                            let w = what_of(&d.proposal);
                                            by_cs.entry(cs).or_default().push((w, d.proposal));
                                        }
                                    }
                                }
                            }
                            for (cs, list) in by_cs {
                                let what = match list.as_slice() {
                                    [(w, _)] => w.clone(),
                                    many => format!("{} at once", many.len()),
                                };
                                s.undoable.push((what, list.into_iter().map(|(_, p)| p).collect(), cs));
                            }
                        }
                        Err(e) => {
                            for (d, w) in chunk {
                                s.failed.push((w.clone(), e.to_string()));
                                s.failed_total += 1;
                                back.push(d.proposal.clone());
                            }
                        }
                    }
                    s.changed = true;
                    drop(s);
                    // Refused: forgotten here, so the item comes back to Approve to try again.
                    if let Ok(mut sent) = sent2.lock() {
                        for p in back {
                            sent.remove(&p);
                        }
                    }
                }
                for (job, what) in others {
                    let r = match &job {
                        Job::Decide(d, yes) => live.decide(d, *yes).map(|_| ()),
                        Job::Undo(cs) => live.undo(cs).map(|_| ()),
                        Job::Change(..) => Ok(()),
                    };
                    let Ok(mut s) = st.lock() else { return };
                    s.waiting = s.waiting.saturating_sub(1);
                    match (r, &job) {
                        (Ok(()), Job::Undo(_)) => {
                            s.said = Some(format!(
                                "put back: “{what}” is in your inbox and back here within a minute"
                            ))
                        }
                        (Ok(()), _) => s.done += 1,
                        (Err(e), _) => {
                            s.failed.push((what, e));
                            s.failed_total += 1;
                        }
                    }
                    s.changed = true;
                }
            }
        });
        Queue { tx, sent, state }
    }

    /// Send a decision; false when the worker is gone (then decide it in place).
    pub fn push(&self, d: MailDecision, yes: bool, what: String) -> bool {
        if let Ok(mut s) = self.sent.lock() {
            s.insert(decision_id(&d).to_string());
        }
        if let Ok(mut s) = self.state.lock() {
            s.waiting += 1;
        }
        self.tx.send((Job::Decide(d, yes), what)).is_ok()
    }

    /// Send a proposal approved as you changed it.
    pub fn push_change(&self, proposal: String, action: Action, what: String) -> bool {
        if let Ok(mut s) = self.sent.lock() {
            s.insert(proposal.clone());
        }
        if let Ok(mut s) = self.state.lock() {
            s.waiting += 1;
        }
        self.tx.send((Job::Change(proposal, action), what)).is_ok()
    }

    /// `u`: put back the newest decision keryx has carried out (a batch is put back whole).
    /// Its proposals show again (keryxd reopens them). Err says why not.
    pub fn undo_last(&self) -> Result<String, String> {
        let mut s = self.state.lock().map_err(|_| "the queue is gone".to_string())?;
        if s.waiting > 0 {
            return Err(format!("keryx is still doing {}: u undoes once it has", s.waiting));
        }
        let Some((what, proposals, cs)) = s.undoable.pop() else {
            return Err("nothing to undo here yet (a refusal changes nothing, so it has no undo)".into());
        };
        s.waiting += 1;
        drop(s);
        if let Ok(mut sent) = self.sent.lock() {
            for p in &proposals {
                sent.remove(p);
            }
        }
        self.tx
            .send((Job::Undo(cs), what.clone()))
            .map_err(|_| "the queue is gone".to_string())?;
        Ok(format!("undoing “{what}”"))
    }

    /// Whether this proposal or draft was already decided here.
    pub fn was_sent(&self, id: &str) -> bool {
        self.sent.lock().is_ok_and(|s| s.contains(id))
    }

    pub fn waiting(&self) -> usize {
        self.state.lock().map_or(0, |s| s.waiting)
    }

    /// Decisions keryx refused this session (their items came back to the list).
    pub fn failed_total(&self) -> usize {
        self.state.lock().map_or(0, |s| s.failed_total)
    }

    /// A note when something finished since the last call: refusals first (all of them, as a
    /// count), else progress.
    pub fn take_note(&self) -> Option<String> {
        let mut s = self.state.lock().ok()?;
        if !s.changed {
            return None;
        }
        s.changed = false;
        if !s.failed.is_empty() {
            let failed = std::mem::take(&mut s.failed);
            return Some(match failed.as_slice() {
                [(what, why)] => format!("keryx said no to “{what}”: {why} · it is back in Approve to try again"),
                [(_, why), ..] => format!(
                    "{} failed ({why}) · they are back in Approve to try again",
                    failed.len()
                ),
                [] => String::new(),
            });
        }
        if let Some(n) = s.said.take() {
            return Some(n);
        }
        Some(if s.waiting == 0 {
            format!("keryx has done all {}", s.done)
        } else {
            format!("keryx has done {} · {} to go", s.done, s.waiting)
        })
    }
}

/// Somewhere a proposal can go instead (`c` on Approve).
#[derive(Debug, Clone, PartialEq)]
pub struct Dest {
    /// What is shown and typed against: `B15 Control Systems`, `Admin`, `archive`.
    pub label: String,
    /// Beside it, quieter: the folder it lands in.
    pub hint: String,
    pub action: Action,
}

/// Every place a filing can go: archive, each registry item, then your keys.
///
/// Which folder an item lands in is learnt from the list itself (`pairs`: item → the key its
/// filings carry now): B15 is filed under Engineering, so B15 goes there; a course with none
/// yet goes where most courses go; an application under its area (Careers); else its area.
/// Picking B15 from an Admin filing must not file it under Admin (10 Nov).
pub fn destinations(
    reg: &locus_contract::Registry,
    keys: &[(String, String)],
    pairs: &HashMap<String, String>,
) -> Vec<Dest> {
    let by_kind = |k: locus_contract::ProjectKind| -> Option<String> {
        let mut n: HashMap<&str, usize> = HashMap::new();
        for (item, key) in pairs {
            if reg.get(item).is_some_and(|p| p.kind == k) {
                *n.entry(key.as_str()).or_default() += 1;
            }
        }
        n.into_iter().max_by_key(|(_, c)| *c).map(|(k, _)| k.to_string())
    };
    let mut v = vec![Dest {
        label: "archive".into(),
        hint: "out of the inbox, nowhere in particular".into(),
        action: Action::Archive,
    }];
    // Personal's streams first (Family, Sport, Social, Errands): mail that is no project's.
    for (name, hint) in keys.iter().filter(|(_, h)| h == "Personal") {
        v.push(Dest {
            label: name.clone(),
            hint: hint.clone(),
            action: Action::File {
                key: name.clone(),
                item: None,
            },
        });
    }
    // A course or application a repo does the work of is one place with the repo, listed once
    // as the repo under both names (10 Nov: "the same thing in two different places").
    let parent = |p: &locus_contract::Project| -> Option<&locus_contract::Project> {
        p.course
            .as_deref()
            .or(p.application.as_deref())
            .and_then(|id| reg.get(id))
    };
    let absorbed: std::collections::HashSet<String> = reg
        .projects
        .iter()
        .filter_map(|p| parent(p).map(|q| q.id.clone()))
        .collect();
    let mut items: Vec<&locus_contract::Project> = reg.projects.iter().filter(|p| !absorbed.contains(&p.id)).collect();
    items.sort_by_key(|p| (!p.active, p.area.clone(), p.name.clone()));
    for p in items {
        // Where it is filed already, else: a course (or a repo doing a course's work) where
        // courses go (Engineering); an application under Careers; anything else in its own area.
        // A repo of Projects goes to Projects, never to Engineering by likeness (gr-ai-ml, 10 Nov).
        let course_like = p.kind == locus_contract::ProjectKind::Course || p.course.is_some();
        let key = if p.kind == locus_contract::ProjectKind::Application {
            p.area.clone()
        } else if course_like {
            pairs
                .get(&p.id)
                .cloned()
                .or_else(|| by_kind(locus_contract::ProjectKind::Course))
                .unwrap_or_else(|| p.area.clone())
        } else {
            // A project you build goes to a folder named after it (OQTS), else its own area:
            // never a department folder ("quast is a big project", 10 Nov).
            keys.iter()
                .find(|(n, h)| h == "a folder" && (n.eq_ignore_ascii_case(&p.name) || n.eq_ignore_ascii_case(&p.id)))
                .map(|(n, _)| n.clone())
                .unwrap_or_else(|| p.area.clone())
        };
        let label = match parent(p) {
            Some(q) => format!("{} ({})", q.name, p.name),
            None => p.name.clone(),
        };
        v.push(Dest {
            label,
            hint: format!("{key} · {}", p.id),
            action: Action::File {
                key,
                item: Some(p.id.clone()),
            },
        });
    }
    // A folder that is one item's own (the OQTS folder is where the OQTS project's mail goes)
    // is listed once, as the item: filing there and linking it to the project are one act.
    let own: Vec<String> = v
        .iter()
        .filter_map(|d| match &d.action {
            Action::File { key, item: Some(_) } if d.label.eq_ignore_ascii_case(key) => Some(key.to_lowercase()),
            _ => None,
        })
        .collect();
    for (name, hint) in keys
        .iter()
        .filter(|(n, h)| h != "Personal" && !own.contains(&n.to_lowercase()))
    {
        v.push(Dest {
            label: name.clone(),
            hint: hint.clone(),
            action: Action::File {
                key: name.clone(),
                item: None,
            },
        });
    }
    v
}

/// Item → key: from the approve list's filing groups ("file → Engineering / B15 Control
/// Systems"), then from where keryx has filed each item's threads already. The list alone was
/// not enough: once the B15 group was approved, nothing in it said where courses go (10 Nov).
pub fn pairs(fx: &Fixtures) -> HashMap<String, String> {
    let mut out = Mirror::open().map(|m| m.item_keys()).unwrap_or_default();
    for g in &fx.approve.groups {
        let Some(l) = g.label.as_deref().and_then(|l| l.strip_prefix("file → ")) else {
            continue;
        };
        if let Some((key, name)) = l.split_once(" / ")
            && let Some(p) = fx.registry.projects.iter().find(|p| p.name == name)
        {
            out.insert(p.id.clone(), key.to_string());
        }
    }
    out
}

/// Every folder a filing can name, with what it is: Personal's streams (Family, Sport, Social,
/// Errands: "is there no personal/family category", 10 Nov), the areas with no stream (Studies,
/// Projects, Careers), then your own keryx keys (Admin, Engineering, …) not already listed.
pub fn key_names() -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = Vec::new();
    if let Ok(shared) = locus_contract::taxonomy::load(&locus_contract::taxonomy::root()) {
        for f in locus_contract::fors::fors(&shared)
            .into_iter()
            .filter(|f| f.item.is_none())
        {
            v.push((f.label, "Personal".into()));
        }
        for f in locus_contract::fors::area_fors(&shared) {
            v.push((f.label, "an area".into()));
        }
    }
    if let Ok(c) = Config::load(&Config::default_path()) {
        // A retired key (files_as set, 10 Nov) is not a place of its own any more.
        for k in c.keys.into_iter().filter(|k| k.files_as.is_none()) {
            if !v.iter().any(|(n, _)| n.eq_ignore_ascii_case(&k.name)) {
                v.push((k.name, "a folder".into()));
            }
        }
    }
    v
}

/// The `c` picker over the approve list.
#[derive(Debug, Clone)]
pub struct Picker {
    /// The approve items it changes: (group, item), one or a visual selection.
    pub at: Vec<(usize, usize)>,
    pub query: String,
    pub sel: usize,
    pub dests: Vec<Dest>,
}

impl Picker {
    /// The destinations matching what is typed: every word somewhere in the label or the hint.
    pub fn matches(&self) -> Vec<&Dest> {
        let words: Vec<String> = self.query.split_whitespace().map(|w| w.to_lowercase()).collect();
        self.dests
            .iter()
            .filter(|d| {
                let hay = format!("{} {}", d.label, d.hint).to_lowercase();
                words.iter().all(|w| hay.contains(w))
            })
            .collect()
    }
}

/// keryx's mirror of your mail, opened read-only through its library (keryx-joins-locus X2: one
/// workspace, so no `keryx` command run as a process). Only keryxd ever writes it.
pub struct Mirror {
    pub cfg: keryx_core::config::Config,
    pub db: keryx_store::db::Db,
}

impl Mirror {
    /// Threads in the inbox waiting on your reply (Home's mail line).
    pub fn need_reply(&self) -> u32 {
        let f = keryx_store::view::Filter {
            accounts: vec![],
            limit: 500,
        };
        keryx_store::view::list(&self.db, &self.cfg, &keryx_store::view::View::NeedsReply, &f)
            .map_or(0, |v| v.iter().filter(|r| r.in_inbox).count() as u32)
    }

    pub fn open() -> Option<Mirror> {
        let cfg = keryx_core::config::Config::load(&keryx_core::config::Config::default_path()).ok()?;
        let db = keryx_store::db::Db::open_readonly(&cfg.db_path()).ok()?;
        Some(Mirror { cfg, db })
    }

    /// Item → the key its threads are filed under in your mailboxes now (the most common), from
    /// what keryx linked and where those threads sit: B15's mail is in Engineering.
    pub fn item_keys(&self) -> HashMap<String, String> {
        let mut n: HashMap<(String, String), usize> = HashMap::new();
        for (a, t, item) in self.db.items().unwrap_or_default() {
            let Ok(msgs) = self.db.thread(&a, &t) else { continue };
            for k in &self.cfg.keys {
                let Some(folder) = k.folders.get(&a) else { continue };
                if msgs
                    .iter()
                    .any(|m| m.location.all().iter().any(|l| l.eq_ignore_ascii_case(folder)))
                {
                    *n.entry((item.clone(), k.name.clone())).or_default() += 1;
                }
            }
        }
        let mut best: HashMap<String, (String, usize)> = HashMap::new();
        for ((item, key), c) in n {
            if best.get(&item).is_none_or(|(_, b)| c > *b) {
                best.insert(item, (key, c));
            }
        }
        best.into_iter().map(|(i, (k, _))| (i, k)).collect()
    }

    pub fn search(&self, q: &str) -> Vec<keryx_store::view::ThreadRow> {
        let Ok(q) = keryx_core::query::parse(q, chrono::Utc::now().with_timezone(&self.cfg.tz())) else {
            return Vec::new();
        };
        let f = keryx_store::view::Filter {
            accounts: vec![],
            limit: 25,
        };
        keryx_store::view::search(&self.db, &self.cfg, &q, &f).unwrap_or_default()
    }
}

/// What a proposal would do, in a few words; an item by its registry name
/// ("file → Careers / Millennium QD Internship 2027").
fn action_text(a: &Action, reg: &locus_contract::Registry) -> String {
    match a {
        Action::Archive => "archive".into(),
        Action::File { key, item: Some(i) } => {
            let name = reg.get(i).map_or(i.clone(), |p| p.name.clone());
            // "file → OQTS", not "file → OQTS / OQTS": the folder is the project's own.
            if name.eq_ignore_ascii_case(key) {
                format!("file → {key}")
            } else {
                format!("file → {key} / {name}")
            }
        }
        Action::File { key, .. } => format!("file → {key}"),
        Action::Snooze { .. } => "snooze".into(),
        Action::Keep { .. } => "keep".into(),
    }
}

/// The action as a badge: the verb in capitals, then where, as short as says it: an
/// application by its company (`FILE → Millennium`), another item by its id (`FILE → b15`), else
/// the key (`FILE → Admin`).
fn badge_of(a: &Action, reg: &locus_contract::Registry) -> String {
    match a {
        Action::Archive => "ARCHIVE".into(),
        Action::File { item: Some(i), .. } => {
            let p = reg.get(i);
            // A company with two applications (Millennium, Brevan Howard) is named by the
            // application, or the badge would not say which.
            let shared = |c: &str| reg.projects.iter().filter(|q| q.company.as_deref() == Some(c)).count() > 1;
            let short = match p {
                Some(p) => match &p.company {
                    Some(c) if !shared(c) => c.clone(),
                    // "Millennium 2027", "Brevan Howard 2026": the heading has the full name.
                    Some(c) => match p
                        .name
                        .split_whitespace()
                        .find(|w| w.len() == 4 && w.chars().all(|d| d.is_ascii_digit()))
                    {
                        Some(y) => format!("{c} {y}"),
                        None => p.name.clone(),
                    },
                    None => p.name.clone(),
                },
                None => i.to_uppercase(),
            };
            format!("FILE → {short}")
        }
        Action::File { key, .. } => format!("FILE → {key}"),
        Action::Snooze { .. } => "SNOOZE".into(),
        Action::Keep { .. } => "KEEP".into(),
    }
}

/// Home's mail line: how many threads wait on your reply.
pub fn apply_counts(fx: &mut Fixtures, need_reply: u32) {
    fx.home.mail.need_reply = Some(need_reply);
}

/// keryx's drafts and proposals onto Approve, replacing what an earlier pass put there.
/// What the detail pane shows for a proposal's thread: account and date, and the start of the
/// newest message from someone else (read from keryx's mirror, off the UI thread).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Preview {
    pub meta: String,
    pub body: String,
}

const PREVIEW_CHARS: usize = 1500;

impl Mirror {
    /// A preview for each proposal's thread, by its ref.
    pub fn previews(&self, items: &[ReviewItem]) -> HashMap<String, Preview> {
        let own = self.cfg.own_addresses();
        let mut out = HashMap::new();
        for it in items {
            let Some((a, t)) = keryx_core::model::parse_thread_ref(&it.r#ref) else {
                continue;
            };
            let Ok(msgs) = self.db.thread(a, t) else { continue };
            let Some(m) = msgs
                .iter()
                .rev()
                .find(|m| m.from.as_ref().is_some_and(|f| !own.contains(&f.key())))
                .or(msgs.last())
            else {
                continue;
            };
            let account = self.cfg.account(a).map_or(a.to_string(), |x| x.name.clone());
            let when = chrono::DateTime::from_timestamp(m.date, 0)
                .map(|d| d.with_timezone(&self.cfg.tz()).format("%a %-d %b %H:%M").to_string())
                .unwrap_or_default();
            let n = msgs.len();
            let meta = format!(
                "{account} · {when}{}",
                if n > 1 {
                    format!(" · {n} messages")
                } else {
                    String::new()
                }
            );
            let text = if m.body_text.trim().is_empty() {
                &m.snippet
            } else {
                &m.body_text
            };
            let mut body: String = text.chars().take(PREVIEW_CHARS).collect();
            if text.chars().count() > PREVIEW_CHARS {
                body.push_str(" …");
            }
            out.insert(it.r#ref.clone(), Preview { meta, body });
        }
        out
    }
}

/// keryx's cleanup proposals onto Approve, one group per action ("archive", "file → Admin"), so
/// `Y` approves one kind at a time.
///
/// Two things are NOT here (10 Nov): Claude's drafts, which wait in keryx's Drafts (`g d`) where
/// they are read, edited and sent; and keeps, which change nothing and so ask nothing (keryxd
/// settles them as they are proposed; any left open are skipped here as well).
pub fn apply(fx: &mut Fixtures, proposals: &[ReviewItem], previews: &HashMap<String, Preview>) {
    fx.approve
        .groups
        .retain(|g| !matches!(g.kind, ApproveKind::MailSend | ApproveKind::MailFiling));
    let mut groups: Vec<(String, Vec<ApproveItem>)> = Vec::new();
    for i in proposals {
        if matches!(i.proposal.action, Action::Keep { .. }) {
            continue;
        }
        let action = action_text(&i.proposal.action, &fx.registry);
        let pv = previews.get(&i.r#ref).cloned().unwrap_or_default();
        let item = ApproveItem {
            text: format!("{}: {}", i.who, i.subject),
            to: Some(action.clone()),
            from: "keryx · claude".into(),
            new: true,
            date: None,
            changeset: None,
            mail: Some(MailDecision::Proposal(i.proposal.id.clone())),
            fact: None,
            detail: Some(ApproveDetail {
                action: badge_of(&i.proposal.action, &fx.registry),
                who: i.who.clone(),
                subject: i.subject.clone(),
                meta: if i.stale {
                    format!("{} · a new message came after this was proposed", pv.meta)
                } else {
                    pv.meta
                },
                reason: i.proposal.reason.clone(),
                body: pv.body,
            }),
        };
        match groups.iter_mut().find(|(a, _)| *a == action) {
            Some((_, v)) => v.push(item),
            None => groups.push((action, vec![item])),
        }
    }
    // Filing first (each a deliberate place), the archive pile last.
    groups.sort_by_key(|(a, v)| (a == "archive", std::cmp::Reverse(v.len())));
    for (label, items) in groups.into_iter().rev() {
        fx.approve.groups.insert(
            0,
            ApproveGroup {
                kind: ApproveKind::MailFiling,
                label: Some(label),
                items,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::resolve;
    use keryx_store::state::Proposal;

    fn proposal() -> ReviewItem {
        ReviewItem {
            proposal: Proposal {
                id: "p-1".into(),
                review_id: "r-1".into(),
                position: 0,
                account: "personal".into(),
                thread_id: "t-1".into(),
                action: Action::Archive,
                reason: "a newsletter".into(),
                fingerprint: String::new(),
                status: ProposalStatus::Open,
                decided_at: None,
                changeset_id: None,
            },
            r#ref: "personal:t-1".into(),
            group: "archive".into(),
            subject: "This week".into(),
            who: "A newsletter".into(),
            date: 0,
            stale: false,
        }
    }

    fn with(action: Action, id: &str) -> ReviewItem {
        let mut p = proposal();
        p.proposal.action = action;
        p.proposal.id = id.into();
        p.r#ref = format!("personal:{id}");
        p
    }

    #[test]
    fn proposals_wait_in_approve_by_action_and_a_second_pass_replaces_them() {
        let mut fx = Fixtures::load(&resolve("default")).unwrap();
        let items = [
            proposal(),
            with(
                Action::File {
                    key: "Admin".into(),
                    item: None,
                },
                "p-2",
            ),
            with(
                Action::Keep {
                    why: keryx_store::state::Keep::New,
                    needs_reply: false,
                },
                "p-3",
            ),
        ];
        let previews: HashMap<String, Preview> = [(
            "personal:t-1".to_string(),
            Preview {
                meta: "Personal · Sun 8 Nov".into(),
                body: "Hello".into(),
            },
        )]
        .into();
        apply(&mut fx, &items, &previews);
        apply(&mut fx, &items, &previews);
        let mail: Vec<&ApproveGroup> = fx
            .approve
            .groups
            .iter()
            .filter(|g| g.kind == ApproveKind::MailFiling)
            .collect();
        // One group per action, filing before the archive pile; a keep asks nothing (10 Nov).
        let labels: Vec<_> = mail.iter().map(|g| g.label.clone().unwrap()).collect();
        assert_eq!(labels, vec!["file → Admin", "archive"]);
        assert!(
            fx.approve.groups.iter().all(|g| g.kind != ApproveKind::MailSend),
            "drafts are in keryx's Drafts"
        );
        let a = &mail[1].items[0];
        assert_eq!(a.mail, Some(MailDecision::Proposal("p-1".into())));
        let d = a.detail.as_ref().unwrap();
        assert_eq!(
            (d.action.as_str(), d.reason.as_str(), d.body.as_str()),
            ("ARCHIVE", "a newsletter", "Hello")
        );
        assert_eq!(mail[0].items[0].detail.as_ref().unwrap().action, "FILE → Admin");
        apply(&mut fx, &[], &HashMap::new());
        assert!(fx.approve.groups.iter().all(|g| g.kind != ApproveKind::MailFiling));
    }

    /// `c`: a course goes where courses are being filed (Engineering), not under the key the
    /// proposal happened to have; an application under Careers; typing narrows by every word.
    #[test]
    fn the_picker_files_an_item_where_its_kind_goes() {
        let fx = Fixtures::load(&resolve("default")).unwrap();
        let reg = &fx.registry;
        let course = reg
            .projects
            .iter()
            .find(|p| p.kind == locus_contract::ProjectKind::Course)
            .unwrap();
        let other = reg
            .projects
            .iter()
            .find(|p| p.kind == locus_contract::ProjectKind::Course && p.id != course.id)
            .unwrap();
        let app = reg
            .projects
            .iter()
            .find(|p| p.kind == locus_contract::ProjectKind::Application)
            .unwrap();
        let pairs: HashMap<String, String> = [(course.id.clone(), "Engineering".to_string())].into();
        let d = destinations(
            reg,
            &[
                ("Admin".into(), "a folder".into()),
                ("Engineering".into(), "a folder".into()),
            ],
            &pairs,
        );
        let to = |id: &str| {
            d.iter()
                .find(|x| matches!(&x.action, Action::File { item: Some(i), .. } if i == id))
                .map(|x| x.action.clone())
                .unwrap()
        };
        assert_eq!(
            to(&course.id),
            Action::File {
                key: "Engineering".into(),
                item: Some(course.id.clone())
            }
        );
        assert_eq!(
            to(&other.id),
            Action::File {
                key: "Engineering".into(),
                item: Some(other.id.clone())
            },
            "by kind"
        );
        assert_eq!(
            to(&app.id),
            Action::File {
                key: app.area.clone(),
                item: Some(app.id.clone())
            }
        );
        assert_eq!(d[0].action, Action::Archive);
        let p = Picker {
            at: vec![],
            query: course.id.clone(),
            sel: 0,
            dests: d.clone(),
        };
        assert!(p.matches().iter().any(|x| x.label == course.name));
        let p = Picker {
            at: vec![],
            query: "nothing like this".into(),
            sel: 0,
            dests: d,
        };
        assert!(p.matches().is_empty());
    }
}
