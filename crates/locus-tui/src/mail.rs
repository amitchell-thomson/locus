//! Mail, live (keryx-joins-locus X2): what keryx needs you to decide comes to locus's Approve,
//! as kairos's calendar changes do: cleanup proposals wait for your yes to file or archive.
//! Claude's drafts do not (10 Nov): they are in keryx's Drafts, where you read, edit and send. Everything goes through
//! keryxd's API, keryxd being the only writer to your mail; nothing here sends or files by
//! itself.

use crate::fixtures::Fixtures;
use keryx_core::changeset::Changeset;
use keryx_core::config::Config;
use keryx_store::api::{Decision, ReviewItem};
use keryx_store::client::{Client, Daemon};
use keryx_store::state::{Action, ProposalStatus};
use locus_contract::{
    AppEvent, AppEventKind, ApproveDetail, ApproveGroup, ApproveItem, ApproveKind, Item, ItemKind, MailDecision,
};
use std::collections::{BTreeMap, HashMap};

/// A connection to keryxd, from keryx's own config, as you (your keypresses in Approve).
pub struct Live {
    client: Client,
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

    /// Mail filed for each registry item (keryx-joins-locus X7), newest first, at most `per`
    /// threads an item: what its page shows.
    pub fn filed(&self, ids: &[String], per: usize) -> BTreeMap<String, Vec<Filed>> {
        let own = self.cfg.own_addresses();
        let mut out = BTreeMap::new();
        for id in ids {
            let mut v: Vec<Filed> = self
                .db
                .threads_for_item(id)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|(a, t, _)| {
                    let msgs = self.db.thread(&a, &t).ok()?;
                    let first = msgs.first()?;
                    let last = msgs.last()?;
                    let other = msgs
                        .iter()
                        .rev()
                        .find(|m| m.from.as_ref().is_some_and(|f| !own.contains(&f.key())))
                        .or(msgs.last())?;
                    Some(Filed {
                        subject: first.subject.clone(),
                        who: other.from.as_ref().map(|f| f.short().to_string()).unwrap_or_default(),
                        at: last.date,
                        unread: msgs.iter().any(|m| !m.read),
                        in_inbox: msgs.iter().any(|m| m.in_inbox),
                        needs_reply: self.db.tag(&a, &t, "needs_reply").ok().flatten().is_some(),
                    })
                })
                .collect();
            v.sort_by_key(|f| std::cmp::Reverse(f.at));
            v.truncate(per);
            if !v.is_empty() {
                out.insert(id.clone(), v);
            }
        }
        out
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

/// What a proposal would do, in a few words.
fn action_text(a: &Action) -> String {
    match a {
        Action::Archive => "archive".into(),
        Action::File { key, item: Some(i) } => format!("file → {key} ({i})"),
        Action::File { key, .. } => format!("file → {key}"),
        Action::Snooze { .. } => "snooze".into(),
        Action::Keep { .. } => "keep".into(),
    }
}

/// The action as a badge: the verb in capitals, a key as it is named (`FILE → Careers`).
fn badge_of(a: &Action) -> String {
    match a {
        Action::Archive => "ARCHIVE".into(),
        Action::File { key, .. } => format!("FILE → {key}"),
        Action::Snooze { .. } => "SNOOZE".into(),
        Action::Keep { .. } => "KEEP".into(),
    }
}

/// Home's mail line: how many threads wait on your reply.
pub fn apply_counts(fx: &mut Fixtures, need_reply: u32) {
    fx.home.mail.need_reply = need_reply;
}

/// One thread filed for an item.
#[derive(Debug, Clone, PartialEq)]
pub struct Filed {
    pub subject: String,
    pub who: String,
    pub at: i64,
    pub unread: bool,
    /// Still in your inbox: marked for this item (`m`) but not done.
    pub in_inbox: bool,
    pub needs_reply: bool,
}

/// An item's mail onto its page (X7, round 4): what is still in your inbox joins New, first and
/// said so ("needs reply" or "in your inbox"), as does anything unread; an application's
/// timeline gets every thread as a mail event, each once. A course's own page is a later
/// design session; until then its mail shows in New like any item's.
pub fn apply_filed(fx: &mut Fixtures, filed: &BTreeMap<String, Vec<Filed>>) {
    for (id, threads) in filed {
        let Some(p) = fx.projects.get_mut(id) else { continue };
        for f in threads {
            let Some(at) = chrono::DateTime::from_timestamp(f.at, 0).map(|t| t.fixed_offset()) else {
                continue;
            };
            let text = format!("{}: {}", f.who, f.subject);
            let shown = match (f.needs_reply, f.in_inbox) {
                (true, _) => format!("needs reply · {text}"),
                (false, true) => format!("in your inbox · {text}"),
                _ => text.clone(),
            };
            // A mark changes as you act (marked, replied, done): the page keeps one line.
            p.view
                .new
                .retain(|i| !(i.kind == ItemKind::Mail && i.text.ends_with(&text)));
            if f.unread || f.in_inbox {
                p.view.new.push(Item {
                    kind: ItemKind::Mail,
                    text: shown,
                    at,
                    unread: true,
                    detail: vec![],
                    routine: None,
                });
            }
            if let Some(app) = &mut p.view.application
                && !app
                    .events
                    .iter()
                    .any(|e| e.kind == AppEventKind::Mail && e.text == text)
            {
                app.events.push(AppEvent {
                    at,
                    kind: AppEventKind::Mail,
                    text,
                    detail: vec![],
                    calendar: None,
                });
            }
        }
        // Your inbox's threads first, then newest first.
        let waiting = |i: &Item| {
            i.kind == ItemKind::Mail && (i.text.starts_with("needs reply · ") || i.text.starts_with("in your inbox · "))
        };
        p.view.new.sort_by_key(|i| (!waiting(i), std::cmp::Reverse(i.at)));
        if let Some(app) = &mut p.view.application {
            app.events.sort_by_key(|e| e.at);
        }
    }
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
        let action = action_text(&i.proposal.action);
        let pv = previews.get(&i.r#ref).cloned().unwrap_or_default();
        let item = ApproveItem {
            text: format!("{}: {}", i.who, i.subject),
            to: Some(action.clone()),
            from: "keryx · claude".into(),
            new: true,
            opens: None,
            date: None,
            changeset: None,
            mail: Some(MailDecision::Proposal(i.proposal.id.clone())),
            detail: Some(ApproveDetail {
                action: badge_of(&i.proposal.action),
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

    #[test]
    fn filed_mail_reaches_its_items_page_once() {
        let mut fx = Fixtures::load(&resolve("default")).unwrap();
        let before = fx.projects["millennium"]
            .view
            .application
            .as_ref()
            .unwrap()
            .events
            .len();
        let new_before = fx.projects["millennium"].view.new.len();
        let filed: BTreeMap<String, Vec<Filed>> = [(
            "millennium".to_string(),
            vec![
                Filed {
                    subject: "Your interview slot".into(),
                    who: "Recruiting".into(),
                    at: 1_793_000_000,
                    unread: true,
                    in_inbox: false,
                    needs_reply: false,
                },
                Filed {
                    subject: "Application received".into(),
                    who: "Recruiting".into(),
                    at: 1_790_000_000,
                    unread: false,
                    in_inbox: true,
                    needs_reply: true,
                },
            ],
        )]
        .into();
        apply_filed(&mut fx, &filed);
        apply_filed(&mut fx, &filed);
        let p = &fx.projects["millennium"];
        let app = p.view.application.as_ref().unwrap();
        assert_eq!(app.events.len(), before + 2, "each thread once on the timeline");
        assert!(app.events.windows(2).all(|w| w[0].at <= w[1].at), "oldest first");
        assert_eq!(
            p.view.new.len(),
            new_before + 2,
            "the unread one and the one in your inbox"
        );
        assert_eq!(
            p.view.new[0].text, "needs reply · Recruiting: Application received",
            "what waits in your inbox comes first"
        );
        assert!(p.view.new.iter().any(|i| i.text == "Recruiting: Your interview slot"));
    }
}
