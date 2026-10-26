//! Mail, live (keryx-joins-locus X2): what keryx needs you to decide comes to locus's Approve,
//! the one approve place, as kairos's calendar changes do. Drafts Claude wrote wait for your yes
//! to send; cleanup proposals wait for your yes to file or archive. Everything goes through
//! keryxd's API, keryxd being the only writer to your mail; nothing here sends or files by
//! itself.

use crate::fixtures::Fixtures;
use keryx_core::changeset::Changeset;
use keryx_core::config::Config;
use keryx_store::api::{Decision, ReviewItem};
use keryx_store::client::{Client, Daemon};
use keryx_store::state::{Action, ProposalStatus};
use locus_contract::{ApproveGroup, ApproveItem, ApproveKind, MailDecision};

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
        Action::File { key } => format!("file → {key}"),
        Action::Snooze { .. } => "snooze".into(),
        Action::Keep { .. } => "keep".into(),
    }
}

/// Home's mail line: how many threads wait on your reply.
pub fn apply_counts(fx: &mut Fixtures, need_reply: u32) {
    fx.home.mail.need_reply = need_reply;
}

/// keryx's drafts and proposals onto Approve, replacing what an earlier pass put there.
pub fn apply(fx: &mut Fixtures, drafts: &[Changeset], proposals: &[ReviewItem]) {
    fx.approve
        .groups
        .retain(|g| !matches!(g.kind, ApproveKind::MailSend | ApproveKind::MailFiling));
    let sends: Vec<ApproveItem> = drafts
        .iter()
        .filter_map(|c| {
            let o = c.outgoing()?;
            let to = o.to.first().map(|a| a.display()).unwrap_or_default();
            Some(ApproveItem {
                text: format!("to {to}: {}", o.subject),
                to: None,
                from: format!("keryx · {}", c.author.as_str()),
                new: true,
                opens: None,
                date: None,
                changeset: None,
                mail: Some(MailDecision::Send(c.id.clone())),
            })
        })
        .collect();
    let filing: Vec<ApproveItem> = proposals
        .iter()
        .map(|i| ApproveItem {
            text: format!("{}: {}", i.who, i.subject),
            to: Some(action_text(&i.proposal.action)),
            from: "keryx · claude".into(),
            new: true,
            opens: None,
            date: None,
            changeset: None,
            mail: Some(MailDecision::Proposal(i.proposal.id.clone())),
        })
        .collect();
    if !filing.is_empty() {
        fx.approve.groups.insert(
            0,
            ApproveGroup {
                kind: ApproveKind::MailFiling,
                items: filing,
            },
        );
    }
    if !sends.is_empty() {
        fx.approve.groups.insert(
            0,
            ApproveGroup {
                kind: ApproveKind::MailSend,
                items: sends,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::resolve;
    use keryx_core::changeset::{Author, Op, Outgoing, Risk, Status};
    use keryx_core::model::Address;
    use keryx_store::state::Proposal;

    fn draft() -> Changeset {
        let a = |e: &str| Address {
            name: None,
            email: e.into(),
        };
        Changeset {
            id: "cs-1".into(),
            author: Author::Claude,
            interactive: false,
            created_at: 0,
            ops: vec![Op::Send(Outgoing {
                account: "personal".into(),
                thread_id: None,
                from: a("me@example.org"),
                to: vec![a("someone@example.org")],
                cc: vec![],
                bcc: vec![],
                subject: "Re: Thursday".into(),
                body: "Thursday works.".into(),
                in_reply_to: None,
                references: vec![],
                message_id: "<x@example.org>".into(),
                attachments: vec![],
            })],
            risk: Risk::High,
            reasons: vec![],
            status: Status::Pending,
            summary: "Reply".into(),
            review_id: None,
            undoes: None,
            error: None,
            hold_until: None,
            warnings: vec![],
        }
    }

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

    #[test]
    fn drafts_and_proposals_wait_in_approve_and_a_second_pass_replaces_them() {
        let mut fx = Fixtures::load(&resolve("default")).unwrap();
        apply(&mut fx, &[draft()], &[proposal()]);
        apply(&mut fx, &[draft()], &[proposal()]);
        let kinds: Vec<ApproveKind> = fx.approve.groups.iter().map(|g| g.kind).collect();
        assert_eq!(&kinds[..2], &[ApproveKind::MailSend, ApproveKind::MailFiling]);
        assert_eq!(kinds.iter().filter(|k| **k == ApproveKind::MailSend).count(), 1);
        let send = &fx.approve.groups[0].items[0];
        assert_eq!(send.text, "to someone@example.org: Re: Thursday");
        assert_eq!(send.mail, Some(MailDecision::Send("cs-1".into())));
        let file = &fx.approve.groups[1].items[0];
        assert_eq!(
            (file.to.as_deref(), file.mail.clone()),
            (Some("archive"), Some(MailDecision::Proposal("p-1".into())))
        );
        apply(&mut fx, &[], &[]);
        assert!(
            fx.approve
                .groups
                .iter()
                .all(|g| !matches!(g.kind, ApproveKind::MailSend | ApproveKind::MailFiling))
        );
    }
}
