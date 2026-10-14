//! Background jobs: Mail.app watch + poll, Gmail history poll.

use crate::engine::Engine;
use keryx_core::config::Config;
use keryx_store::db::Db;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

/// Mail.app is re-scanned when its Envelope Index changes (debounced) and at
/// least this often.
const MAIL_POLL: Duration = Duration::from_secs(60);
const DEBOUNCE: Duration = Duration::from_secs(2);

/// How often snoozes are checked and tags tidied.
const TICK: Duration = Duration::from_secs(30);

/// The background loop: Mail.app (watched + polled), Gmail (polled), snooze
/// wake-ups and tag housekeeping. Shares the engine (and its single writer
/// connection) with the HTTP handlers.
pub fn run(cfg: &Config, engine: Arc<Mutex<Engine>>, sync_now: mpsc::Receiver<()>) {
    let (tx, rx) = mpsc::channel::<()>();
    let _watcher = watch_mail(&cfg.paths.mail_dir.join("MailData"), tx);
    let gmail_every = Duration::from_secs(u64::from(cfg.gmail.poll_seconds));
    let mut last_mail = Instant::now() - MAIL_POLL;
    let mut last_gmail = Instant::now() - gmail_every;
    let mut last_tick = Instant::now() - TICK;
    loop {
        let forced = sync_now.try_recv().is_ok();
        if forced || last_mail.elapsed() >= MAIL_POLL {
            with(&engine, |e| run_mail(cfg, &mut e.db));
            last_mail = Instant::now();
        }
        if forced || last_gmail.elapsed() >= gmail_every {
            with(&engine, |e| sync_gmail_all(cfg, &mut e.db));
            last_gmail = Instant::now();
        }
        if last_tick.elapsed() >= TICK {
            with(&engine, |e| {
                match e.wake_snoozes() {
                    Ok(0) => {}
                    Ok(n) => tracing::info!(threads = n, "woke snoozed threads"),
                    Err(err) => tracing::warn!(error = %err, "snooze wake-ups"),
                }
                if let Err(err) = e.housekeeping() {
                    tracing::warn!(error = %err, "tag housekeeping");
                }
            });
            last_tick = Instant::now();
        }
        // Wait for a Mail change or the next tick.
        if rx.recv_timeout(Duration::from_secs(2)).is_ok() {
            std::thread::sleep(DEBOUNCE);
            while rx.try_recv().is_ok() {}
            with(&engine, |e| run_mail(cfg, &mut e.db));
            last_mail = Instant::now();
        }
    }
}

fn with(engine: &Arc<Mutex<Engine>>, f: impl FnOnce(&mut Engine)) {
    match engine.lock() {
        Ok(mut e) => f(&mut e),
        Err(_) => tracing::error!("engine lock poisoned"),
    }
}

fn run_mail(cfg: &Config, db: &mut Db) {
    if let Err(e) = crate::sync_mailapp(cfg, db) {
        tracing::error!(error = %e, "mail.app sync");
    }
}

/// Watch `MailData/` for writes to the Envelope Index (its WAL changes on
/// every Mail update).
fn watch_mail(dir: &std::path::Path, tx: mpsc::Sender<()>) -> Option<notify::RecommendedWatcher> {
    use notify::{RecursiveMode, Watcher};
    let mut w = notify::recommended_watcher(move |ev: notify::Result<notify::Event>| {
        if let Ok(ev) = ev
            && ev.paths.iter().any(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("Envelope Index"))
            })
        {
            let _ = tx.send(());
        }
    })
    .map_err(|e| tracing::warn!(error = %e, "can't watch Mail; polling every 60 s"))
    .ok()?;
    w.watch(dir, RecursiveMode::NonRecursive)
        .map_err(|e| tracing::warn!(error = %e, dir = %dir.display(), "can't watch Mail; polling every 60 s"))
        .ok()?;
    Some(w)
}

/// Gmail accounts sync once they're authorised (Phase 1, `keryx auth gmail`).
pub fn sync_gmail_all(cfg: &Config, db: &mut Db) {
    for acc in cfg
        .accounts
        .iter()
        .filter(|a| a.backend == keryx_core::config::Backend::Gmail)
    {
        if let Err(e) = crate::gmail_sync::sync_account(cfg, db, acc) {
            tracing::warn!(account = %acc.id, error = %e, "gmail sync");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watcher_fires_on_envelope_index_writes_only() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let _w = watch_mail(dir.path(), tx).expect("watcher");
        std::thread::sleep(Duration::from_millis(300));
        std::fs::write(dir.path().join("unrelated.plist"), b"x").unwrap();
        assert!(
            rx.recv_timeout(Duration::from_millis(1500)).is_err(),
            "other files are ignored"
        );
        std::fs::write(dir.path().join("Envelope Index-wal"), b"x").unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok(), "WAL write is noticed");
    }
}
