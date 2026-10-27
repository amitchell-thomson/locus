//! The engine: intents → per-message ops → risk → apply / propose → undo
//! (SPEC §6). The only code that changes mailboxes, through an [`Actuator`].

use keryx_core::automated;
use keryx_core::changeset::{Author, Changeset, MsgRef, MsgState, Op, Risk, Status};
use keryx_core::config::{Account, Backend, Config};
use keryx_core::location;
use keryx_core::model::{Location, Message, parse_thread_ref};
use keryx_core::reply::bare_subject;
use keryx_core::risk::{self, Assessment, Facts, Kind, TEST_PREFIX, ThreadFacts};
use keryx_store::api::*;
use keryx_store::db::{Db, DbError};
use keryx_store::state::{self, Action, Proposal, ProposalStatus, Review, Snooze, Tag};
use keryx_store::view;
use std::collections::{BTreeMap, HashSet};

pub const NEEDS_REPLY: &str = "needs_reply";
pub const SNOOZE_OFF: &str = "snoozing is off ([snooze] enabled = false): keep, file or archive instead";

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("{0}")]
    BadRequest(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("refused: {0}")]
    Forbidden(String),
    #[error("changed since: {}", .0.join("; "))]
    Conflict(Vec<String>),
    #[error("{0}")]
    Source(String),
    #[error(transparent)]
    Db(#[from] DbError),
}

pub type Result<T> = std::result::Result<T, EngineError>;

/// Who is asking (from the HTTP headers; MCP is always Claude).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Who {
    pub author: Author,
    pub interactive: bool,
}

impl Who {
    /// MCP callers: always Claude, never interactive.
    pub fn claude() -> Who {
        Who {
            author: Author::Claude,
            interactive: false,
        }
    }
    pub fn direct(self) -> bool {
        risk::is_direct(self.author, self.interactive)
    }
    #[cfg(test)]
    pub const ALEC: Who = Who {
        author: Author::Alec,
        interactive: true,
    };
    #[cfg(test)]
    pub const CLAUDE: Who = Who {
        author: Author::Claude,
        interactive: false,
    };
    pub const SYSTEM: Who = Who {
        author: Author::System,
        interactive: false,
    };
}

/// A source that can change mail: Gmail API, Mail.app, or a fake in tests.
pub trait Actuator: Send {
    /// Make `msg` go from `before` to `after` at the source. Returns the
    /// state the source confirms (Gmail) or `None` when the account must be
    /// re-read from the source to see the result (Mail.app).
    fn apply(
        &mut self,
        account: &Account,
        msg: &MsgRef,
        before: &MsgState,
        after: &MsgState,
    ) -> std::result::Result<Option<MsgState>, String>;
    /// Re-read an account into the mirror after `apply` returned `None`.
    fn resync(&mut self, db: &mut Db, account: &Account) -> std::result::Result<(), String>;
    /// Send a message (after its hold). An error means nothing was sent.
    fn send(&mut self, account: &Account, o: &keryx_core::changeset::Outgoing) -> std::result::Result<(), String>;
    /// RFC 8058 one-click unsubscribe (POST `List-Unsubscribe=One-Click`).
    fn unsubscribe(&mut self, url: &str) -> std::result::Result<(), String>;
}

pub struct Engine {
    pub cfg: Config,
    pub db: Db,
    pub act: Box<dyn Actuator>,
    pub clock: Box<dyn Fn() -> i64 + Send>,
    /// kairos's API (`None` when it isn't set up; `kairos_note` says why).
    pub kairos: Option<Box<dyn keryx_store::kairos::Kairos>>,
    pub kairos_note: String,
    /// The run whose token made the current MCP call (set per call).
    pub current_run: Option<keryx_store::runs::Run>,
    /// locus's taxonomy and registry: where filed mail goes and what it can be for (X7).
    pub shared: Option<locus_contract::taxonomy::Shared>,
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}-{}", &uuid::Uuid::new_v4().simple().to_string()[..10])
}

fn state_of(m: &Message) -> MsgState {
    MsgState {
        location: m.location.clone(),
        read: m.read,
        flagged: m.flagged,
    }
}

fn msg_ref(m: &Message) -> MsgRef {
    MsgRef {
        account: m.account.clone(),
        thread_id: m.thread_id.clone(),
        source_id: m.source_id.clone(),
        message_id: m.message_id.clone(),
    }
}

/// Short human description of a state change, for conflicts and summaries.
fn describe(m: &Message) -> String {
    let who = m.from.as_ref().map(|a| a.short().to_string()).unwrap_or_default();
    format!("{who} · {}", m.subject)
}

impl Engine {
    pub fn new(cfg: Config, db: Db, act: Box<dyn Actuator>) -> Result<Engine> {
        db.init_state()?;
        Ok(Engine {
            cfg,
            db,
            act,
            clock: Box::new(crate::now),
            kairos: None,
            kairos_note: "no kairos client".into(),
            current_run: None,
            shared: None,
        })
    }

    pub(crate) fn now(&self) -> i64 {
        (self.clock)()
    }

    fn account(&self, id: &str) -> Result<&Account> {
        self.cfg
            .account(id)
            .ok_or_else(|| EngineError::NotFound(format!("account {id}")))
    }

    pub(crate) fn thread(&self, r: &str) -> Result<(String, String, Vec<Message>)> {
        let (a, t) =
            parse_thread_ref(r).ok_or_else(|| EngineError::BadRequest(format!("{r}: expected account:thread")))?;
        self.account(a)?;
        let msgs = self.db.thread(a, t)?;
        if msgs.is_empty() {
            return Err(EngineError::NotFound(r.to_string()));
        }
        Ok((a.to_string(), t.to_string(), msgs))
    }

    fn own(&self) -> HashSet<String> {
        self.cfg.own_addresses()
    }

    fn is_own(&self, m: &Message) -> bool {
        m.from.as_ref().is_some_and(|f| self.own().contains(&f.key()))
    }

    /// Gmail label name → id for an account.
    fn label_id(&self, account: &Account, name: &str) -> Option<String> {
        if account.backend != Backend::Gmail {
            return Some(name.to_string());
        }
        let labels = view::label_names(&self.db, &self.cfg);
        labels
            .get(&account.id)
            .and_then(|m| m.iter().find(|(_, n)| n.as_str() == name).map(|(id, _)| id.clone()))
    }

    /// The taxonomy folder `name` names (`studies` → `Studies`), if it is one.
    pub fn taxonomy_folder(&self, name: &str) -> Option<String> {
        let shared = self.shared.as_ref()?;
        locus_contract::fors::folders(&shared.taxonomy)
            .into_iter()
            .find(|f| f.eq_ignore_ascii_case(name))
    }

    /// The mailbox (or label name) filing under `key` means on `account`: the folder your
    /// config maps for it, else, for a taxonomy folder, its own name, made on first use (X-b).
    fn folder_name(&self, key: &str, account: &Account) -> Result<String> {
        let def = self.cfg.key(key);
        if let Some(f) = def.and_then(|d| d.folders.get(&account.id)) {
            return Ok(f.clone());
        }
        Err(EngineError::BadRequest(match def {
            Some(d) => format!("key {} isn't available on {} (no folder mapped)", d.name, account.name),
            None => format!("no key {key:?}"),
        }))
    }

    /// The ops an intent means for one thread.
    fn plan(&self, intent: &Intent, account: &Account, msgs: &[Message]) -> Result<Vec<Op>> {
        let mut ops = Vec::new();
        let mut state = |m: &Message, after: MsgState| {
            let before = state_of(m);
            if before != after {
                ops.push(Op::State {
                    msg: msg_ref(m),
                    before,
                    after,
                });
            }
        };
        let latest_other = msgs.iter().rev().find(|m| !self.is_own(m)).or(msgs.last());
        match intent {
            Intent::Archive | Intent::Snooze { .. } => {
                for m in msgs.iter().filter(|m| location::in_inbox(account, &m.location)) {
                    state(
                        m,
                        MsgState {
                            location: location::archived(account, &m.location),
                            ..state_of(m)
                        },
                    );
                }
            }
            Intent::Unarchive => {
                // Not your sent copies; but a mail you sent yourself and received is inbox mail
                // like any other (round 8: "1 skipped" on exactly that).
                for m in msgs.iter().filter(|m| !location::in_sent(account, &m.location)) {
                    state(
                        m,
                        MsgState {
                            location: location::inboxed(account, &m.location),
                            ..state_of(m)
                        },
                    );
                }
            }
            Intent::Unsnooze => {
                for m in msgs.iter().filter(|m| !self.is_own(m)) {
                    let read = if Some(m.source_id.as_str()) == latest_other.map(|l| l.source_id.as_str()) {
                        false
                    } else {
                        m.read
                    };
                    state(
                        m,
                        MsgState {
                            location: location::inboxed(account, &m.location),
                            read,
                            ..state_of(m)
                        },
                    );
                }
            }
            // Filing for what it is for (round 8, your call of 27 Oct: option C): keryx records
            // the item or area; on the server the mail is only archived. No folders are made or
            // moved into, so nothing on the server depends on Mail's slow, fragile label moves.
            Intent::File { key, .. } if self.taxonomy_folder(key).is_some() => {
                for m in msgs.iter().filter(|m| location::in_inbox(account, &m.location)) {
                    state(
                        m,
                        MsgState {
                            location: location::archived(account, &m.location),
                            ..state_of(m)
                        },
                    );
                }
            }
            // Your older config keys (Admin, Travel) still move into the folders they have.
            Intent::File { key, .. } => {
                let folder_name = self.folder_name(key, account)?;
                let folder = self.label_id(account, &folder_name).ok_or_else(|| {
                    EngineError::BadRequest(format!("{}: no Gmail label named {folder_name:?}", account.name))
                })?;
                let inbox: Vec<&Message> = msgs
                    .iter()
                    .filter(|m| location::in_inbox(account, &m.location))
                    .collect();
                let targets: Vec<&Message> = if inbox.is_empty() {
                    latest_other.into_iter().collect()
                } else {
                    inbox
                };
                for m in targets {
                    state(
                        m,
                        MsgState {
                            location: location::filed(account, &m.location, &folder),
                            ..state_of(m)
                        },
                    );
                }
            }
            Intent::Flag { on: true } => {
                if !msgs.iter().any(|m| m.flagged)
                    && let Some(m) = latest_other
                {
                    state(
                        m,
                        MsgState {
                            flagged: true,
                            ..state_of(m)
                        },
                    );
                }
            }
            Intent::Flag { on: false } => {
                for m in msgs.iter().filter(|m| m.flagged) {
                    state(
                        m,
                        MsgState {
                            flagged: false,
                            ..state_of(m)
                        },
                    );
                }
            }
            Intent::Read { on: true } => {
                for m in msgs.iter().filter(|m| !m.read) {
                    state(
                        m,
                        MsgState {
                            read: true,
                            ..state_of(m)
                        },
                    );
                }
            }
            Intent::Read { on: false } => {
                if let Some(m) = latest_other {
                    state(
                        m,
                        MsgState {
                            read: false,
                            ..state_of(m)
                        },
                    );
                }
            }
        }
        let thread_id = msgs.first().map(|m| m.thread_id.clone()).unwrap_or_default();
        let existing = self.db.snooze(&account.id, &thread_id)?;
        match intent {
            Intent::Snooze { until } => ops.push(Op::Snooze {
                account: account.id.clone(),
                thread_id,
                until: *until,
                previous: existing.map(|s| s.until),
            }),
            Intent::Unsnooze => {
                if let Some(s) = existing {
                    ops.push(Op::Unsnooze {
                        account: account.id.clone(),
                        thread_id,
                        until: s.until,
                    });
                }
            }
            Intent::Archive | Intent::File { .. } => {
                // Archiving/filing a snoozed thread ends the snooze.
                if let Some(s) = existing {
                    ops.push(Op::Unsnooze {
                        account: account.id.clone(),
                        thread_id,
                        until: s.until,
                    });
                }
            }
            _ => {}
        }
        Ok(ops)
    }

    fn facts_for(&self, account: &Account, msgs: &[Message], ops: &[Op], sent_to: &HashSet<String>) -> ThreadFacts {
        let touched: HashSet<&str> = ops
            .iter()
            .filter_map(|o| match o {
                Op::State { msg, .. } => Some(msg.source_id.as_str()),
                _ => None,
            })
            .collect();
        let test_message = msgs
            .iter()
            .filter(|m| touched.is_empty() || touched.contains(m.source_id.as_str()))
            .all(|m| bare_subject(&m.subject).starts_with(TEST_PREFIX));
        ThreadFacts {
            account: account.id.clone(),
            account_writable: account.writable,
            account_shared: account.shared,
            automated: automated::thread_is_automated(msgs, sent_to, &self.own(), &self.cfg.automated),
            flagged: msgs.iter().any(|m| m.flagged),
            test_message,
        }
    }

    /// Archive, file, flag, read, snooze… on threads. Low-risk threads are
    /// applied as one changeset; the rest become proposals for Alec.
    pub fn triage(&mut self, req: TriageReq, who: Who) -> Result<TriageOutcome> {
        if req.refs.is_empty() {
            return Err(EngineError::BadRequest("no threads given".into()));
        }
        self.run_room(&req.refs)?;
        if matches!(req.intent, Intent::Snooze { .. }) && !self.cfg.snooze.enabled {
            return Err(EngineError::Forbidden(SNOOZE_OFF.into()));
        }
        if let Intent::Snooze { until } = req.intent
            && until <= self.now()
        {
            return Err(EngineError::BadRequest("snooze time is in the past".into()));
        }
        // An item is checked before anything moves: a filing must not succeed and then fail.
        if let Intent::File { item: Some(id), .. } = &req.intent
            && self.shared.as_ref().and_then(|s| s.registry.get(id)).is_none()
        {
            return Err(EngineError::BadRequest(format!("no registry item {id:?}")));
        }
        let sent_to = view::sent_to(&self.db, &self.cfg)?;
        let mut out = TriageOutcome::default();
        let mut low: Vec<(String, Vec<Op>, ThreadFacts)> = Vec::new();
        let mut high: Vec<(String, String)> = Vec::new();
        let mut seen = HashSet::new();
        for r in &req.refs {
            if !seen.insert(r.clone()) {
                continue;
            }
            let (account_id, _t, msgs) = match self.thread(r) {
                Ok(x) => x,
                Err(e) => {
                    out.skipped.push((r.clone(), e.to_string()));
                    continue;
                }
            };
            let account = self.account(&account_id)?.clone();
            let ops = match self.plan(&req.intent, &account, &msgs) {
                Ok(o) => o,
                Err(e) => {
                    out.skipped.push((r.clone(), e.to_string()));
                    continue;
                }
            };
            if ops.is_empty() {
                // Already out of the inbox: filing it is only the mark (round 8, option C).
                if let Intent::File { key, item } = &req.intent
                    && self.taxonomy_folder(key).is_some()
                    && let Some((a, t)) = parse_thread_ref(r)
                {
                    self.mark_filed(a, t, key, item.as_deref())?;
                    continue;
                }
                let why = match req.intent {
                    Intent::Unarchive => "already in the inbox".to_string(),
                    Intent::Archive | Intent::File { .. } => "already out of the inbox".to_string(),
                    _ => format!("nothing to {}", req.intent.verb()),
                };
                out.skipped.push((r.clone(), why));
                continue;
            }
            let tf = self.facts_for(&account, &msgs, &ops, &sent_to);
            let one = self.assess(who, vec![tf.clone()]);
            match one {
                Assessment::Forbidden(why) => out.skipped.push((r.clone(), why)),
                Assessment::Low => low.push((r.clone(), ops, tf)),
                Assessment::High(reasons) => high.push((r.clone(), reasons.join("; "))),
            }
        }
        // The batch as a whole: too many threads → all of them need Alec.
        if !low.is_empty()
            && let Assessment::High(reasons) = self.assess(who, low.iter().map(|(_, _, f)| f.clone()).collect())
        {
            let why = reasons.join("; ");
            high.extend(low.drain(..).map(|(r, _, _)| (r, why.clone())));
        }

        let summary_base = req.summary.clone().unwrap_or_else(|| req.intent.verb());
        if !low.is_empty() {
            let mut cs = Changeset {
                id: new_id("cs"),
                author: who.author,
                interactive: who.interactive,
                created_at: self.now(),
                ops: low.iter().flat_map(|(_, o, _)| o.clone()).collect(),
                risk: Risk::Low,
                reasons: vec![],
                status: Status::Pending,
                summary: format!(
                    "{summary_base} ({} thread{})",
                    low.len(),
                    if low.len() == 1 { "" } else { "s" }
                ),
                review_id: req.review_id.clone(),
                undoes: None,
                error: None,
                hold_until: None,
                warnings: vec![],
            };
            self.apply(&mut cs)?;
            if cs.status == Status::Applied && req.intent.leaves_inbox() {
                for (r, _, _) in &low {
                    if let Some((a, t)) = parse_thread_ref(r) {
                        self.db.clear_tag(a, t, NEEDS_REPLY)?;
                        if let Intent::File { key, item } = &req.intent {
                            self.mark_filed(a, t, key, item.as_deref())?;
                        }
                    }
                }
            }
            out.applied = Some(cs);
        }
        if !high.is_empty() {
            let action = match &req.intent {
                Intent::Archive => Some(Action::Archive),
                Intent::File { key, item } => Some(Action::File {
                    key: key.clone(),
                    item: item.clone(),
                }),
                Intent::Snooze { until } => Some(Action::Snooze { until: *until }),
                _ => None,
            };
            match action {
                Some(action) => {
                    let items: Vec<ProposeItem> = high
                        .iter()
                        .map(|(r, why)| ProposeItem {
                            r#ref: r.clone(),
                            action: action.clone(),
                            reason: format!("{summary_base} (needs you: {why})"),
                        })
                        .collect();
                    let p = self.propose(
                        ProposeReq {
                            review_id: None,
                            title: summary_base.clone(),
                            items,
                        },
                        who,
                    )?;
                    out.review_id = Some(p.review_id);
                    out.proposed = p.added;
                }
                // Flag/read/unarchive changes that need Alec aren't cleanup
                // proposals: they're refused with the reason.
                None => out
                    .skipped
                    .extend(high.into_iter().map(|(r, why)| (r, format!("needs Alec: {why}")))),
            }
        }
        out.summary = self.summarise(&out, &summary_base);
        Ok(out)
    }

    fn summarise(&self, out: &TriageOutcome, what: &str) -> String {
        let mut parts = Vec::new();
        if let Some(cs) = &out.applied {
            match cs.status {
                Status::Applied => parts.push(format!("{} — done ({})", cs.summary, cs.id)),
                _ => parts.push(format!(
                    "{} — failed: {}",
                    cs.summary,
                    cs.error.clone().unwrap_or_default()
                )),
            }
        }
        if out.proposed > 0 {
            parts.push(format!(
                "{} thread{} proposed for Alec's review ({}): approve in keryx with A",
                out.proposed,
                if out.proposed == 1 { "" } else { "s" },
                out.review_id.clone().unwrap_or_default()
            ));
        }
        if !out.skipped.is_empty() {
            parts.push(format!("{} skipped", out.skipped.len()));
        }
        if parts.is_empty() {
            format!("{what}: nothing to do")
        } else {
            parts.join("; ")
        }
    }

    fn assess(&self, who: Who, threads: Vec<ThreadFacts>) -> Assessment {
        risk::assess(&Facts {
            author: who.author,
            interactive: who.interactive,
            kind: Kind::Triage,
            live: self.cfg.live,
            batch_threshold: self.cfg.risk.batch_threshold,
            threads,
            recipients_own: false,
        })
    }

    /// Find a message again: same source id (and Message-ID), else by
    /// Message-ID within the account (Mail renumbers on moves).
    fn locate(&self, m: &MsgRef) -> Result<Option<Message>> {
        if let Some(x) = self.db.message(&m.account, &m.source_id)?
            && (m.message_id.is_empty() || x.message_id == m.message_id)
        {
            return Ok(Some(x));
        }
        if m.message_id.is_empty() {
            return Ok(None);
        }
        let id: Option<String> = self
            .db
            .conn()
            .query_row(
                "SELECT source_id FROM messages WHERE account = ?1 AND message_id = ?2 ORDER BY date DESC LIMIT 1",
                rusqlite::params![m.account, m.message_id],
                |r| r.get(0),
            )
            .ok();
        Ok(match id {
            Some(id) => self.db.message(&m.account, &id)?,
            None => None,
        })
    }

    /// Record what a filed thread is for: its item, else the area or stream it was filed under
    /// (a config key such as Admin is a server folder, not a mark).
    pub(crate) fn mark_filed(&mut self, account: &str, thread_id: &str, key: &str, item: Option<&str>) -> Result<()> {
        match (item, self.taxonomy_folder(key)) {
            (Some(_), _) => self.link_item(account, thread_id, item),
            (None, Some(folder)) => {
                let now = self.now();
                self.db.set_for(
                    account,
                    thread_id,
                    Some(("folder", folder.as_str(), folder.as_str())),
                    now,
                )?;
                Ok(())
            }
            (None, None) => Ok(()),
        }
    }

    /// Record what a filed thread is for (`None`: a stream, no item). An id the registry does
    /// not know is refused rather than stored, so a page never lists mail under a ghost.
    pub(crate) fn link_item(&mut self, account: &str, thread_id: &str, item: Option<&str>) -> Result<()> {
        let title = match item {
            None => None,
            Some(id) => {
                let p = self
                    .shared
                    .as_ref()
                    .and_then(|s| s.registry.get(id))
                    .ok_or_else(|| EngineError::BadRequest(format!("no registry item {id:?}")))?;
                Some((p.id.clone(), p.name.clone()))
            }
        };
        let now = self.now();
        self.db.set_item(
            account,
            thread_id,
            title.as_ref().map(|(i, n)| (i.as_str(), n.as_str())),
            now,
        )?;
        Ok(())
    }

    /// `m`: mark what a thread is for without moving it (round 4). `id` is an option's id from
    /// [`keryx_store::item::options`] (`b14`, `errands`, `studies`); `None` clears the mark.
    /// keryx's own record: nothing on the mail server changes.
    pub fn mark_for(&mut self, r: &str, id: Option<&str>) -> Result<()> {
        let (account, thread, _) = self.thread(r)?;
        let f = match id {
            None => None,
            Some(id) => {
                let shared = self
                    .shared
                    .as_ref()
                    .ok_or_else(|| EngineError::BadRequest("no locus taxonomy".into()))?;
                let o = keryx_store::item::options(shared)
                    .into_iter()
                    .find(|o| o.id.eq_ignore_ascii_case(id))
                    .ok_or_else(|| EngineError::BadRequest(format!("nothing called {id:?} to be for")))?;
                Some(match o.item {
                    Some(item) => ("item", item, o.label),
                    None => ("folder", o.folder, o.label),
                })
            }
        };
        let now = self.now();
        self.db.set_for(
            &account,
            &thread,
            f.as_ref().map(|(k, t, l)| (*k, t.as_str(), l.as_str())),
            now,
        )?;
        Ok(())
    }

    /// Check preconditions, then apply every op. On a source failure the ops
    /// already done are reverted (best effort) and the changeset fails.
    pub fn apply(&mut self, cs: &mut Changeset) -> Result<()> {
        // Preconditions: every message still as the plan saw it, and the
        // live guard (checked again here, not only when planning).
        let mut diffs = Vec::new();
        for op in &cs.ops {
            if let Op::State { msg, before, .. } = op {
                match self.locate(msg)? {
                    Some(m) if !self.cfg.live && !bare_subject(&m.subject).starts_with(TEST_PREFIX) => {
                        cs.status = Status::Failed;
                        cs.error = Some(format!("live = false: {} isn't a {TEST_PREFIX} message", describe(&m)));
                        self.db.save_changeset(cs)?;
                        return Ok(());
                    }
                    Some(m) if state_of(&m) == *before => {}
                    Some(m) => diffs.push(format!(
                        "{} changed (now {})",
                        describe(&m),
                        state_summary(&state_of(&m))
                    )),
                    None => diffs.push(format!("message {} is gone", msg.message_id)),
                }
            }
        }
        if !diffs.is_empty() {
            cs.status = Status::Failed;
            cs.error = Some(format!("changed since: {}", diffs.join("; ")));
            self.db.save_changeset(cs)?;
            return Ok(());
        }
        let mut done: Vec<Op> = Vec::new();
        let mut resync: BTreeMap<String, ()> = BTreeMap::new();
        let mut failure = None;
        for op in cs.ops.clone() {
            match self.apply_op(&op, &mut resync) {
                Ok(()) => done.push(op),
                Err(e) => {
                    // A step can have half happened at the source: re-read that account, so
                    // the list shows where the mail really is.
                    if let Op::State { msg, .. } = &op {
                        resync.insert(msg.account.clone(), ());
                    }
                    failure = Some(e);
                    break;
                }
            }
        }
        if let Some(e) = failure {
            let mut stuck = 0;
            for op in done.iter().rev() {
                let undone = match op.inverse() {
                    Some(inv) => self.apply_op(&inv, &mut resync).is_ok(),
                    None => false,
                };
                if !undone {
                    stuck += 1;
                }
            }
            cs.status = Status::Failed;
            cs.error = Some(if stuck == 0 {
                format!("{e} (keryx undid what it had done; the account is re-read from Mail)")
            } else {
                format!("{e}; {stuck} change(s) already made couldn't be reverted — check the thread")
            });
        } else {
            cs.status = Status::Applied;
            let threads: BTreeMap<(String, String), ()> = cs
                .ops
                .iter()
                .filter_map(|op| match op {
                    Op::State { msg, .. } => Some((msg.account.clone(), msg.thread_id.clone())),
                    Op::Snooze { account, thread_id, .. } => Some((account.clone(), thread_id.clone())),
                    _ => None,
                })
                .map(|k| (k, ()))
                .collect();
            for (a, t) in threads.keys() {
                self.note_run("changeset", &format!("{}:{a}:{t}", cs.id), Some(a), Some(t))?;
            }
        }
        for account in resync.keys() {
            if let Ok(acc) = self.account(account).cloned()
                && let Err(e) = self.act.resync(&mut self.db, &acc)
            {
                tracing::warn!(account, error = %e, "resync after apply");
            }
        }
        self.db.save_changeset(cs)?;
        Ok(())
    }

    fn apply_op(&mut self, op: &Op, resync: &mut BTreeMap<String, ()>) -> std::result::Result<(), String> {
        match op {
            Op::State { msg, before, after } => {
                let account = self.account(&msg.account).map_err(|e| e.to_string())?.clone();
                // A Mail move may have renumbered the message: act on what's there now.
                let current = self.locate(msg).map_err(|e| e.to_string())?;
                let msg = match &current {
                    Some(m) => msg_ref(m),
                    None => msg.clone(),
                };
                match self.act.apply(&account, &msg, before, after)? {
                    Some(confirmed) => {
                        let in_inbox = location::in_inbox(&account, &confirmed.location);
                        self.db
                            .update_state(
                                &msg.account,
                                &msg.source_id,
                                &confirmed.location,
                                in_inbox,
                                confirmed.read,
                                confirmed.flagged,
                            )
                            .map_err(|e| e.to_string())?;
                    }
                    None => {
                        resync.insert(msg.account.clone(), ());
                    }
                }
                Ok(())
            }
            Op::Snooze {
                account,
                thread_id,
                until,
                ..
            } => self
                .db
                .set_snooze(
                    &Snooze {
                        account: account.clone(),
                        thread_id: thread_id.clone(),
                        until: *until,
                        created_at: self.now(),
                        created_by: "keryx".into(),
                    },
                    "",
                )
                .map_err(|e| e.to_string()),
            Op::Unsnooze { account, thread_id, .. } => self
                .db
                .clear_snooze(account, thread_id)
                .map(|_| ())
                .map_err(|e| e.to_string()),
            Op::Send(_) => Err("sends go through the hold queue, not apply".into()),
            Op::Unsubscribe { via, .. } => match via {
                keryx_core::changeset::UnsubscribeVia::OneClick { url } => self.act.unsubscribe(url),
            },
        }
    }

    /// Undo an applied changeset: the inverse ops, if every message is
    /// still where this changeset left it.
    pub fn undo(&mut self, id: &str, who: Who) -> Result<Changeset> {
        let mut orig = self
            .db
            .changeset(id)?
            .ok_or_else(|| EngineError::NotFound(format!("changeset {id}")))?;
        if orig.ops.iter().any(|o| o.inverse().is_none()) {
            return Err(EngineError::BadRequest(
                "sent mail can't be unsent (and unsubscribing can't be undone)".into(),
            ));
        }
        if orig.status != Status::Applied {
            return Err(EngineError::BadRequest(format!(
                "{id} is {}, not applied",
                orig.status.as_str()
            )));
        }
        let mut diffs = Vec::new();
        for op in &orig.ops {
            if let Op::State { msg, after, .. } = op {
                match self.locate(msg)? {
                    Some(m) if state_of(&m) == *after => {}
                    Some(m) => diffs.push(format!(
                        "{}: expected {}, now {}",
                        describe(&m),
                        state_summary(after),
                        state_summary(&state_of(&m))
                    )),
                    None => diffs.push(format!("message {} is gone", msg.message_id)),
                }
            }
        }
        if !diffs.is_empty() {
            return Err(EngineError::Conflict(diffs));
        }
        let mut cs = Changeset {
            id: new_id("cs"),
            author: who.author,
            interactive: who.interactive,
            created_at: self.now(),
            ops: orig.ops.iter().rev().filter_map(Op::inverse).collect(),
            risk: Risk::Low,
            reasons: vec![],
            status: Status::Pending,
            summary: format!("undo: {}", orig.summary),
            review_id: orig.review_id.clone(),
            undoes: Some(orig.id.clone()),
            error: None,
            hold_until: None,
            warnings: vec![],
        };
        self.apply(&mut cs)?;
        if cs.status == Status::Applied {
            orig.status = Status::Undone;
            self.db.save_changeset(&orig)?;
            // Decisions undone in a review are open again.
            if let Some(rid) = &orig.review_id {
                for p in self.db.proposals(rid)? {
                    if p.changeset_id.as_deref() == Some(&orig.id) {
                        self.db.reopen_proposal(&p.id)?;
                    }
                }
                self.db.set_review_status(rid, "open")?;
            }
        }
        Ok(cs)
    }

    /// `u`: undo the newest applied changeset that isn't an undo.
    pub fn undo_last(&mut self, who: Who) -> Result<Changeset> {
        let last = self
            .db
            .changesets(Some(Status::Applied), 50)?
            .into_iter()
            .find(|c| c.undoes.is_none() && c.author != Author::System)
            .ok_or_else(|| EngineError::NotFound("nothing to undo".into()))?;
        self.undo(&last.id, who)
    }

    /// Wake snoozed threads whose time has come, or that got a new message
    /// from someone else since they were snoozed.
    pub fn wake_snoozes(&mut self) -> Result<usize> {
        let now = self.now();
        let own = self.own();
        let mut woke = 0;
        for s in self.db.snoozes()? {
            let msgs = self.db.thread(&s.account, &s.thread_id)?;
            let replied = msgs
                .iter()
                .any(|m| m.date > s.created_at && !m.from.as_ref().is_some_and(|f| own.contains(&f.key())));
            if s.until > now && !replied {
                continue;
            }
            let req = TriageReq {
                intent: Intent::Unsnooze,
                refs: vec![keryx_core::model::thread_ref(&s.account, &s.thread_id)],
                summary: Some(
                    if replied {
                        "woke early: new reply"
                    } else {
                        "snooze ended"
                    }
                    .into(),
                ),
                review_id: None,
            };
            if msgs.is_empty() {
                // The thread is gone: nothing to wake.
                self.db.clear_snooze(&s.account, &s.thread_id)?;
                continue;
            }
            // The Unsnooze op clears the record once the thread is back; if
            // waking fails the snooze stays and is retried next tick.
            match self.triage(req, Who::SYSTEM) {
                Ok(o) if o.applied.as_ref().is_some_and(|c| c.status == Status::Applied) => woke += 1,
                Ok(o) => {
                    tracing::warn!(account = %s.account, thread = %s.thread_id, summary = %o.summary, "waking snooze")
                }
                Err(e) => tracing::warn!(account = %s.account, thread = %s.thread_id, error = %e, "waking snooze"),
            }
        }
        Ok(woke)
    }

    /// Clear needs-reply tags on threads Alec has replied to since tagging.
    pub fn housekeeping(&mut self) -> Result<usize> {
        let own = self.own();
        let mut cleared = 0;
        for t in self.db.tags(NEEDS_REPLY)? {
            let msgs = self.db.thread(&t.account, &t.thread_id)?;
            let replied = msgs
                .iter()
                .any(|m| m.date > t.created_at && m.from.as_ref().is_some_and(|f| own.contains(&f.key())));
            let gone = msgs.is_empty();
            if replied || gone {
                self.db.clear_tag(&t.account, &t.thread_id, NEEDS_REPLY)?;
                cleared += 1;
            }
        }
        Ok(cleared)
    }

    pub fn tag(&mut self, r: &str, reason: &str, who: Who) -> Result<()> {
        let (a, t, _) = self.thread(r)?;
        self.db.set_tag(&Tag {
            account: a,
            thread_id: t,
            kind: NEEDS_REPLY.into(),
            reason: keryx_core::text::sanitize(reason).chars().take(200).collect(),
            author: who.author.as_str().into(),
            created_at: self.now(),
        })?;
        let (a, t) = parse_thread_ref(r)
            .map(|(a, t)| (a.to_string(), t.to_string()))
            .unwrap_or_default();
        self.note_run("tag", r, Some(&a), Some(&t))?;
        Ok(())
    }

    pub fn untag(&mut self, r: &str) -> Result<bool> {
        let (a, t, _) = self.thread(r)?;
        Ok(self.db.clear_tag(&a, &t, NEEDS_REPLY)?)
    }

    // ---------------- cleanup reviews ----------------

    pub fn propose(&mut self, req: ProposeReq, who: Who) -> Result<ProposeOutcome> {
        self.run_room(&req.items.iter().map(|i| i.r#ref.clone()).collect::<Vec<_>>())?;
        let review_id = match &req.review_id {
            Some(id) => {
                let r = self
                    .db
                    .review(id)?
                    .ok_or_else(|| EngineError::NotFound(format!("review {id}")))?;
                if r.status != "open" {
                    self.db.set_review_status(id, "open")?;
                }
                r.id
            }
            None => {
                let id = new_id("rv");
                self.db.create_review(&Review {
                    id: id.clone(),
                    title: keryx_core::text::sanitize(&req.title).chars().take(120).collect(),
                    author: who.author.as_str().into(),
                    run_id: self.current_run.as_ref().map(|r| r.id.clone()),
                    created_at: self.now(),
                    status: "open".into(),
                })?;
                self.note_run("review", &id, None, None)?;
                id
            }
        };
        let existing = self.db.proposals(&review_id)?;
        let base = existing.len() as i64;
        let mut seen: HashSet<(String, String)> = existing.into_iter().map(|p| (p.account, p.thread_id)).collect();
        let mut out = ProposeOutcome {
            review_id: review_id.clone(),
            added: 0,
            rejected: Vec::new(),
        };
        for (i, item) in req.items.into_iter().enumerate() {
            match self.check_item(&item) {
                Ok((account, thread_id, msgs)) => {
                    let new = seen.insert((account.clone(), thread_id.clone()));
                    self.note_run(
                        "proposal",
                        &format!("{review_id}:{account}:{thread_id}"),
                        Some(&account),
                        Some(&thread_id),
                    )?;
                    self.db.add_proposal(&Proposal {
                        id: new_id("p"),
                        review_id: review_id.clone(),
                        position: base + i as i64,
                        account,
                        thread_id,
                        action: item.action,
                        reason: keryx_core::text::sanitize(&item.reason).chars().take(200).collect(),
                        fingerprint: state::fingerprint(&msgs),
                        status: ProposalStatus::Open,
                        decided_at: None,
                        changeset_id: None,
                    })?;
                    // A thread proposed again replaces its open proposal.
                    if new {
                        out.added += 1;
                    }
                }
                Err(e) => out.rejected.push((item.r#ref, e.to_string())),
            }
        }
        // Pushed once Claude stops adding (§8.4), unless a run pushes for it.
        if who.author == Author::Claude && self.current_run.is_none() {
            self.db.touch_review(&review_id, self.now())?;
        }
        Ok(out)
    }

    fn check_item(&self, item: &ProposeItem) -> Result<(String, String, Vec<Message>)> {
        let (a, t, msgs) = self.thread(&item.r#ref)?;
        match &item.action {
            Action::File { key, .. } => {
                let def = self
                    .cfg
                    .key(key)
                    .ok_or_else(|| EngineError::BadRequest(format!("no key {key:?}")))?;
                if !def.folders.contains_key(&a) {
                    return Err(EngineError::BadRequest(format!(
                        "key {} isn't available on {a}",
                        def.name
                    )));
                }
            }
            Action::Snooze { .. } if !self.cfg.snooze.enabled => {
                return Err(EngineError::BadRequest(SNOOZE_OFF.into()));
            }
            Action::Snooze { until } if *until <= self.now() => {
                return Err(EngineError::BadRequest("snooze time is in the past".into()));
            }
            _ => {}
        }
        Ok((a, t, msgs))
    }

    /// A review with each proposal's thread and whether it's gone stale.
    pub fn review(&self, id: &str) -> Result<ReviewView> {
        state::review_view(&self.db, id)?.ok_or_else(|| EngineError::NotFound(format!("review {id}")))
    }

    fn intent_of(action: &Action) -> Option<Intent> {
        match action {
            Action::Archive => Some(Intent::Archive),
            Action::File { key, item } => Some(Intent::File {
                key: key.clone(),
                item: item.clone(),
            }),
            Action::Snooze { until } => Some(Intent::Snooze { until: *until }),
            Action::Keep { .. } => None,
        }
    }

    /// Alec decides one proposal. Only Alec, interactively.
    pub fn decide(&mut self, proposal_id: &str, d: Decision, who: Who) -> Result<Option<Changeset>> {
        if !who.direct() {
            return Err(EngineError::Forbidden("only Alec decides proposals, in the TUI".into()));
        }
        let p = self
            .db
            .proposal(proposal_id)?
            .ok_or_else(|| EngineError::NotFound(format!("proposal {proposal_id}")))?;
        if p.status != ProposalStatus::Open {
            return Err(EngineError::BadRequest(format!(
                "{proposal_id} is already {}",
                p.status.as_str()
            )));
        }
        let (status, action) = match d {
            Decision::Decline => {
                self.db
                    .decide_proposal(&p.id, ProposalStatus::Declined, None, None, self.now())?;
                self.maybe_done(&p.review_id)?;
                return Ok(None);
            }
            Decision::Accept => (ProposalStatus::Accepted, p.action.clone()),
            Decision::Change(a) => (ProposalStatus::Changed, a),
        };
        let r = keryx_core::model::thread_ref(&p.account, &p.thread_id);
        let cs = self.carry_out(&[r], &action, &p.review_id, who)?;
        self.db.decide_proposal(
            &p.id,
            status,
            (status == ProposalStatus::Changed).then_some(&action),
            cs.as_ref().map(|c| c.id.as_str()),
            self.now(),
        )?;
        self.maybe_done(&p.review_id)?;
        Ok(cs)
    }

    /// Apply an accepted action as Alec. `Keep` changes nothing but may tag.
    fn carry_out(&mut self, refs: &[String], action: &Action, review_id: &str, who: Who) -> Result<Option<Changeset>> {
        if let Action::Keep { needs_reply: true, .. } = action {
            for r in refs {
                self.tag(r, "kept for a reply (cleanup)", who)?;
            }
        }
        let Some(intent) = Self::intent_of(action) else {
            return Ok(None);
        };
        let out = self.triage(
            TriageReq {
                intent,
                refs: refs.to_vec(),
                summary: Some(format!("cleanup: {}", action.group().to_lowercase())),
                review_id: Some(review_id.to_string()),
            },
            who,
        )?;
        match out.applied {
            Some(cs) if cs.status == Status::Applied => Ok(Some(cs)),
            Some(cs) => Err(EngineError::Source(cs.error.unwrap_or_else(|| "failed".into()))),
            None => Ok(None),
        }
    }

    /// `Y`: accept every open, non-stale proposal in a group as one change.
    pub fn accept_group(&mut self, review_id: &str, group: &str, who: Who) -> Result<AcceptGroup> {
        if !who.direct() {
            return Err(EngineError::Forbidden("only Alec decides proposals, in the TUI".into()));
        }
        let view = self.review(review_id)?;
        let mut by_action: BTreeMap<String, (Action, Vec<ReviewItem>)> = BTreeMap::new();
        let mut skipped_stale = 0;
        for it in view
            .items
            .into_iter()
            .filter(|i| i.proposal.status == ProposalStatus::Open && i.group == group)
        {
            if it.stale {
                skipped_stale += 1;
                continue;
            }
            let k = serde_json::to_string(&it.proposal.action).unwrap_or_default();
            by_action
                .entry(k)
                .or_insert_with(|| (it.proposal.action.clone(), Vec::new()))
                .1
                .push(it);
        }
        let mut out = AcceptGroup {
            accepted: 0,
            skipped_stale,
            changesets: vec![],
        };
        for (_, (action, items)) in by_action {
            let refs: Vec<String> = items.iter().map(|i| i.r#ref.clone()).collect();
            let cs = self.carry_out(&refs, &action, review_id, who)?;
            for it in &items {
                self.db.decide_proposal(
                    &it.proposal.id,
                    ProposalStatus::Accepted,
                    None,
                    cs.as_ref().map(|c| c.id.as_str()),
                    self.now(),
                )?;
            }
            out.accepted += items.len();
            if let Some(cs) = cs {
                out.changesets.push(cs.id);
            }
        }
        self.maybe_done(review_id)?;
        Ok(out)
    }

    fn maybe_done(&mut self, review_id: &str) -> Result<()> {
        let open = self
            .db
            .proposals(review_id)?
            .iter()
            .any(|p| p.status == ProposalStatus::Open);
        self.db
            .set_review_status(review_id, if open { "open" } else { "done" })?;
        Ok(())
    }

    /// Undo every applied decision of a review (newest first).
    pub fn undo_review(&mut self, review_id: &str, who: Who) -> Result<UndoReview> {
        let mut out = UndoReview::default();
        let mut ids: Vec<Changeset> = self
            .db
            .changesets(Some(Status::Applied), 10_000)?
            .into_iter()
            .filter(|c| c.review_id.as_deref() == Some(review_id) && c.undoes.is_none())
            .collect();
        ids.sort_by_key(|c| std::cmp::Reverse(c.created_at));
        for cs in ids {
            match self.undo(&cs.id, who) {
                Ok(_) => out.undone += 1,
                Err(e) => out.refused.push((cs.id.clone(), e.to_string())),
            }
        }
        // Declines and keeps are reopened too.
        for p in self.db.proposals(review_id)? {
            if p.status != ProposalStatus::Open && p.changeset_id.is_none() {
                self.db.reopen_proposal(&p.id)?;
            }
        }
        self.db.set_review_status(review_id, "open")?;
        Ok(out)
    }

    /// Alec's past overrides (declined or changed proposals), for Claude.
    pub fn cleanup_feedback(&self, account: Option<&str>, limit: usize) -> Result<Vec<Feedback>> {
        let mut out = Vec::new();
        for r in self.db.reviews(None)? {
            for p in self.db.proposals(&r.id)? {
                if !matches!(p.status, ProposalStatus::Declined | ProposalStatus::Changed) {
                    continue;
                }
                if account.is_some_and(|a| a != p.account) {
                    continue;
                }
                let msgs = self.db.thread(&p.account, &p.thread_id)?;
                let latest = msgs.last();
                out.push(Feedback {
                    account: p.account.clone(),
                    from: latest
                        .and_then(|m| m.from.as_ref())
                        .map(|a| a.email.clone())
                        .unwrap_or_default(),
                    subject: latest.map(|m| m.subject.clone()).unwrap_or_default(),
                    proposed: p.reason.clone(),
                    decision: p.status.as_str().into(),
                    instead: (p.status == ProposalStatus::Changed).then(|| p.action.group()),
                });
                if out.len() >= limit {
                    return Ok(out);
                }
            }
        }
        Ok(out)
    }
}

fn state_summary(s: &MsgState) -> String {
    let loc = match &s.location {
        Location::Labels { labels } => labels.join(","),
        Location::Mailboxes { physical, labels } => {
            if labels.is_empty() {
                physical.clone()
            } else {
                format!("{physical}+{}", labels.join(","))
            }
        }
    };
    format!(
        "{loc}{}{}",
        if s.read { "" } else { ", unread" },
        if s.flagged { ", flagged" } else { "" }
    )
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
