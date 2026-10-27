//! Automated mail filed as it arrives (keryx-joins-locus X7, X-d): "Automated mail filed away
//! straight away", so the inbox holds only what is new to you.
//!
//! Each tick looks at inbox threads whose latest mail is newer than a mark (`meta`
//! `autofile.since`). The first run only sets the mark: the inbox of before is the clear-out's,
//! where every move waits in Approve, never this job's. A thread is filed only when it is
//! automated (computed from its headers, never claimed) and keryx can guess what it is for; it
//! is filed as the system, which the risk table makes low, applied at once, logged as a
//! changeset and undoable with `u`. The live guard still applies. Whatever has no guess stays
//! in the inbox, where you see it.

use crate::engine::{Engine, Result, Who};
use keryx_core::automated::thread_is_automated;
use keryx_store::api::{Intent, TriageReq};
use keryx_store::item;
use keryx_store::view;
use std::collections::BTreeMap;

const MARK: &str = "autofile.since";
/// Whether keryx was live when the mark was set ("1" / "0").
const LIVE_MARK: &str = "autofile.live";

impl Engine {
    /// File what arrived since the last tick. Returns how many threads were filed.
    pub fn file_arrivals(&mut self) -> Result<usize> {
        if !self.cfg.automated.file_on_arrival {
            return Ok(0);
        }
        let Some(shared) = self.shared.clone() else {
            return Ok(0);
        };
        let now = self.now();
        // Going live starts the count again: what arrived while keryx could touch only test
        // mail is not filed in one batch the minute it can touch everything (round 10).
        let live = if self.cfg.live { "1" } else { "0" };
        let was_live = self.db.meta(LIVE_MARK)?;
        let since = match self.db.meta(MARK)?.and_then(|v| v.parse::<i64>().ok()) {
            Some(t) if was_live.as_deref() == Some(live) || !self.cfg.live => t,
            _ => {
                self.db.set_meta(MARK, &now.to_string())?;
                self.db.set_meta(LIVE_MARK, live)?;
                return Ok(0);
            }
        };
        if was_live.as_deref() != Some(live) {
            self.db.set_meta(LIVE_MARK, live)?;
        }
        let rows = view::list(&self.db, &self.cfg, &view::View::Inbox, &view::Filter::default())?;
        let sent_to = view::sent_to(&self.db, &self.cfg)?;
        let own = self.cfg.own_addresses();
        let options = item::options(&shared);
        let mut newest = since;
        // (folder, item) → threads, so each destination is one changeset.
        let mut groups: BTreeMap<(String, Option<String>), Vec<String>> = BTreeMap::new();
        for r in rows.iter().filter(|r| r.date > since) {
            newest = newest.max(r.date);
            // Not live: only `[keryx-test]` mail could be changed, so only it is tried (the rest
            // would each leave a failed changeset in the log).
            if !self.cfg.live && !keryx_core::reply::bare_subject(&r.subject).starts_with(keryx_core::risk::TEST_PREFIX)
            {
                continue;
            }
            let msgs = self.db.thread(&r.account, &r.thread_id)?;
            if !thread_is_automated(&msgs, &sent_to, &own, &self.cfg.automated) {
                continue;
            }
            let Some(g) = item::guess(&self.db, &shared, &msgs, &own, true) else {
                continue;
            };
            let Some(o) = options.iter().find(|o| o.id == g.id) else {
                continue;
            };
            groups
                .entry((o.folder.clone(), o.item.clone()))
                .or_default()
                .push(r.r#ref.clone());
        }
        let mut filed = 0;
        for ((folder, item), refs) in groups {
            let what = item.clone().unwrap_or_else(|| folder.clone());
            let out = self.triage(
                TriageReq {
                    intent: Intent::File { key: folder, item },
                    refs,
                    summary: Some(format!("filed as it arrived, for {what}")),
                    review_id: None,
                },
                Who::SYSTEM,
            )?;
            if let Some(cs) = out.applied
                && cs.status == keryx_core::changeset::Status::Applied
            {
                filed += cs_threads(&cs);
            }
            for (r, why) in out.skipped {
                tracing::info!(thread = r, why, "not filed on arrival");
            }
        }
        self.db.set_meta(MARK, &newest.to_string())?;
        Ok(filed)
    }
}

/// How many threads a changeset touched.
fn cs_threads(cs: &keryx_core::changeset::Changeset) -> usize {
    cs.ops
        .iter()
        .filter_map(|op| match op {
            keryx_core::changeset::Op::State { msg, .. } => Some((msg.account.clone(), msg.thread_id.clone())),
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>()
        .len()
}
