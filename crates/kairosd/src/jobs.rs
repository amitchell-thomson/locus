//! Background jobs: feed fetches, periodic sync + replan on collisions,
//! morning summary, evening check-in, auto-done settlement.

use crate::engine::Engine;
use crate::notify::Ntfy;
use chrono::{DateTime, Duration, NaiveTime, Utc};
use kairos_core::constraints::Hm;
use kairos_core::model::{Author, EventKind, Span};
use std::sync::{Arc, Mutex};

pub struct Jobs {
    pub engine: Arc<Mutex<Engine>>,
    pub ntfy: Option<Ntfy>,
    /// Run `sync.full_cmd` every this many minutes (0 = never).
    pub sync_every_min: u64,
    pub feeds_every_min: u64,
}

fn at_local(e: &Engine, hm: &str) -> Option<DateTime<Utc>> {
    let t = Hm::parse(hm)?.to_time();
    let tz = e.tz();
    let today = e.now().with_timezone(&tz).date_naive();
    Some(kairos_core::time::resolve_local(today.and_time(t), tz))
}

/// Run `f` at most once per local day after `hm`.
fn daily(e: &Engine, key: &str, hm: &str) -> bool {
    let Some(when) = at_local(e, hm) else { return false };
    let now = e.now();
    let today = now.with_timezone(&e.tz()).date_naive().to_string();
    let k = format!("job:{key}");
    if now < when || e.db.kv_get(&k).ok().flatten().as_deref() == Some(today.as_str()) {
        return false;
    }
    let _ = e.db.kv_set(&k, &today);
    true
}

pub fn morning_text(e: &Engine) -> String {
    let tz = e.tz();
    let now = e.now();
    let today = now.with_timezone(&tz).date_naive();
    let from = kairos_core::time::resolve_local(today.and_time(NaiveTime::MIN), tz);
    let to = from + Duration::days(1);
    let mut lines = Vec::new();
    if let Ok(evs) = e.events(from, to) {
        for i in evs.iter().filter(|i| {
            matches!(
                i.overlay,
                kairos_core::config::Overlay::Personal | kairos_core::config::Overlay::Family
            )
        }) {
            let when = match i.span {
                Span::AllDay { .. } => "all day".to_string(),
                Span::Timed { start, .. } => start.with_timezone(&tz).format("%H:%M").to_string(),
            };
            let mark = match i.meta.kind {
                EventKind::Deadline => "⚑ ",
                EventKind::TaskBlock => "~ ",
                EventKind::Event | EventKind::Proposal => "",
            };
            lines.push(format!("{when} {mark}{}", i.title.trim_start_matches("⚑ ")));
        }
    }
    if let Ok(pending) = e.db.cs_list(Some(kairos_core::changeset::Status::Pending), 50)
        && !pending.is_empty()
    {
        lines.push(format!("{} change(s) waiting for approval", pending.len()));
    }
    if let Ok(applied) = e.db.cs_since(now - Duration::days(1)) {
        let auto: Vec<_> = applied
            .iter()
            .filter(|c| c.status == kairos_core::changeset::Status::Applied && c.author != Author::Alec)
            .collect();
        if !auto.is_empty() {
            lines.push(format!(
                "{} automatic change(s) since yesterday (kairos changes)",
                auto.len()
            ));
        }
    }
    if lines.is_empty() {
        "Nothing scheduled today.".into()
    } else {
        lines.join("\n")
    }
}

impl Jobs {
    pub async fn run(self) {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
        let mut minutes: u64 = 0;
        loop {
            tick.tick().await;
            let eng = self.engine.clone();
            let ntfy = self.ntfy.clone();
            let do_sync = self.sync_every_min > 0 && minutes.is_multiple_of(self.sync_every_min);
            let do_feeds = self.feeds_every_min > 0 && minutes.is_multiple_of(self.feeds_every_min);
            minutes += 1;
            let r = tokio::task::spawn_blocking(move || -> Result<(), String> {
                let mut e = eng.lock().map_err(|_| "engine lock poisoned".to_string())?;
                if do_feeds {
                    for (id, r) in kairos_store::feeds::fetch_all(&e.cfg) {
                        if let Err(err) = r {
                            tracing::warn!(feed = %id, "fetch failed: {err}");
                        }
                    }
                }
                if do_sync {
                    tick_sync(&mut e)?;
                }
                match e.replan_if_needed() {
                    Ok(Some(p)) if p.changeset.is_some() => tracing::info!("replanned: {}", p.summary),
                    Ok(_) => {}
                    Err(err) => tracing::warn!("replan: {err}"),
                }
                if daily(&e, "settle", &e.cfg.notify.autodone_at.clone()) {
                    match e.settle() {
                        Ok(n) if n > 0 => tracing::info!("auto-done {n} unanswered block(s)"),
                        Ok(_) => {}
                        Err(err) => tracing::warn!("settle: {err}"),
                    }
                }
                if let Some(n) = &ntfy {
                    if daily(&e, "morning", &e.cfg.notify.morning_summary.clone()) {
                        let body = n.simple("kairos · today", &morning_text(&e), None);
                        if let Err(err) = n.send(&body) {
                            tracing::warn!("morning push: {err}");
                        }
                    }
                    if daily(&e, "checkin", &e.cfg.notify.evening_checkin.clone()) {
                        let items = e.checkin_items().unwrap_or_default();
                        if !items.is_empty() {
                            let day = e.now().with_timezone(&e.tz()).date_naive().to_string();
                            let list: Vec<String> = items
                                .iter()
                                .map(|i| format!("• {} ({} min)", i.title, i.planned_min))
                                .collect();
                            let msg = format!(
                                "{}\nUnanswered blocks count as done at {} tomorrow.",
                                list.join("\n"),
                                e.cfg.notify.autodone_at
                            );
                            let body = n.simple("kairos · check-in", &msg, Some(("Check in", n.checkin_url(&day))));
                            if let Err(err) = n.send(&body) {
                                tracing::warn!("check-in push: {err}");
                            }
                        }
                    }
                }
                Ok(())
            })
            .await;
            match r {
                Ok(Err(e)) => tracing::warn!("job tick: {e}"),
                Err(e) => tracing::warn!("job tick panicked: {e}"),
                Ok(Ok(())) => {}
            }
        }
    }
}

/// Full sync, then replan: events may have changed on iCloud (phone,
/// Calendar.app, Family). The plan is stable, so this only moves work an
/// event now displaces (or that can come out of a squeeze), and refreshes
/// proposed project time.
fn tick_sync(e: &mut Engine) -> Result<(), String> {
    let r = kairos_store::sync::run_full(&e.cfg);
    for pc in &r.followed_empty {
        tracing::info!(collection = %pc, "emptied on iCloud; followed with --force-delete");
    }
    if let Some(err) = r.error {
        tracing::warn!("sync failed: {err}");
    }
    e.replan_soon = true;
    Ok(())
}
