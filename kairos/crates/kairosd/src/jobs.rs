//! Background jobs: feed fetches, periodic sync + replan on collisions, measured project time,
//! auto-done settlement. No pushes to the phone (plan B.4: "just noise"; removed 2026-10-25).

use crate::engine::Engine;
use chrono::{DateTime, Utc};
use kairos_core::constraints::Hm;
use std::sync::{Arc, Mutex};

pub struct Jobs {
    pub engine: Arc<Mutex<Engine>>,
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

impl Jobs {
    pub async fn run(self) {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
        let mut minutes: u64 = 0;
        loop {
            tick.tick().await;
            let eng = self.engine.clone();
            let do_sync = self.sync_every_min > 0 && minutes.is_multiple_of(self.sync_every_min);
            let do_feeds = self.feeds_every_min > 0 && minutes.is_multiple_of(self.feeds_every_min);
            let do_worked = minutes.is_multiple_of(5);
            minutes += 1;
            if do_worked {
                // The server's own Claude sessions (spec K6): read outside the engine lock, since
                // a first scan reads every recent transcript.
                let eng = self.engine.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    let Some(cache) = eng.lock().ok().map(|e| e.cfg.cache_dir()) else {
                        return;
                    };
                    let Ok(shared) = kairos_store::taxonomy::local() else {
                        return;
                    };
                    let now = chrono::Utc::now();
                    let spans = kairos_store::worked::scan(
                        &kairos_store::worked::claude_root(),
                        &cache.join(kairos_store::worked::STATE_FILE),
                        &shared.registry,
                        now,
                    );
                    let req = kairos_core::api::WorkedReq {
                        machine: "server".into(),
                        from: now - chrono::Duration::days(kairos_store::worked::KEEP_DAYS),
                        to: now + chrono::Duration::minutes(1),
                        spans,
                    };
                    if let Ok(mut e) = eng.lock() {
                        if let Err(err) = e.set_worked(&req) {
                            tracing::warn!("worked time: {err}");
                        }
                        // The registry may have changed with a deploy (spec K7).
                        e.registry = Some(shared.registry.clone());
                        match e.sync_registry() {
                            Ok(Some(cs)) => tracing::info!("{}", cs.summary),
                            Ok(None) => {}
                            Err(err) => tracing::warn!("projects from the registry: {err}"),
                        }
                        if let Err(err) = e.link_courses() {
                            tracing::warn!("linking course events: {err}");
                        }
                    }
                })
                .await;
            }
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
                        Ok(n) if n > 0 => tracing::info!("auto-done {n} week-old unanswered block(s)"),
                        Ok(_) => {}
                        Err(err) => tracing::warn!("settle: {err}"),
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
/// event now displaces (or that can come out of a squeeze).
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
