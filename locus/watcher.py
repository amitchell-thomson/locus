"""Folder watcher (§10 slice 3): auto-ingest files dropped into data/incoming/.

Closes the loop with the laptop outbox — a file rsync'd into incoming is ingested without a
manual command. Uses simple polling (not filesystem events): ingest is unbounded-time (§2.4),
polling is dependency-free, survives restarts (it rescans the backlog), and a settle window
avoids grabbing a file mid-transfer.

Disposition per file (incoming is a disposable drop zone; the raw copy is kept in data/raw):
  ingested / skipped   -> removed from incoming
  unsupported / quarantined -> moved to incoming/.quarantine/<drop subpath> so it is not
                               retried forever (subpath preserved for category provenance)

Repo drops are RETIRED (2026-10-19, ADR-003). A directory under incoming/projects/ used to be
ingested whole as a code-repo snapshot (`locusdrop:<name>`); the plan removes repository ingest
(C.3), and `projects/<name>/` is now an AREA folder whose files are ordinary drops. Documents
already stored under `locusdrop:` keys are untouched.

Tracked-repo sync is a separate loop (`watch_repos`, `locus watch-repo`) sharing the ingest
lock. It is not scheduled: repository ingest is retired by the plan (C.3).
"""

from __future__ import annotations

import logging
import shutil
import time
from pathlib import Path

from locus.config import load
from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.ingest_lock import IngestLockHeld, ingest_lock
from locus.ingest_pipeline import IngestResult, ingest_file
from locus.sync import sync_repos

log = logging.getLogger(__name__)

QUARANTINE_DIRNAME = ".quarantine"
# UNSUPPORTED IS NOT QUARANTINED. Both are set aside so they are not retried every tick, but they
# mean opposite things: quarantine is "a bug to triage" (§14), while a PNG, a `uv.lock` and a
# `pyproject.toml` are simply not documents. Filing them together put 8 permanent files in the
# triage pile, so `locus status` warned every single day about something that was never going to
# be actionable — and a warning that is always on is one he learns to skip, which is the same
# failure as the daily page shouting about a failure that had already recovered.
UNSUPPORTED_DIRNAME = ".unsupported"
def _candidates(incoming: Path, settle: float) -> list[Path]:
    """Files ready to ingest: real files anywhere under incoming, stable for `settle` seconds.

    Recurses into subfolders — the drop-folder category convention (ingest_pipeline._category:
    `incoming/papers/x.pdf` -> category 'paper') means the laptop outbox rsyncs *subfolders*,
    so a flat scan would never see categorized drops. Dotfiles and anything inside a dotted
    directory (.quarantine) are skipped.
    """
    now = time.time()
    out: list[Path] = []
    for p in sorted(incoming.rglob("*")):
        if not p.is_file():
            continue
        rel = p.relative_to(incoming)
        if any(part.startswith(".") for part in rel.parts):
            continue  # dotfiles (.gitkeep) and dotted subtrees (.quarantine)
        try:
            if now - p.stat().st_mtime < settle:
                continue  # still being written / just landed — wait for it to settle
        except FileNotFoundError:
            continue
        out.append(p)
    return out


def _prune_empty_parents(start: Path, root: Path) -> None:
    """Remove now-empty ancestor dirs of an ingested drop, climbing up to (not incl.) root.

    A drained nested subfolder (incoming/career/career-documents/) is a remnant, not a drop
    target — the laptop outbox recreates category folders on the next rsync, so an emptied
    one is clutter. `rmdir` is self-guarding: a folder still holding a settle-pending sibling
    file simply isn't empty, so it survives to be pruned on a later tick once that file
    ingests. Never touches `root` itself (the incoming/.gitkeep anchor lives there) nor any
    dotted subtree (.quarantine).
    """
    d = start
    while d != root and root in d.parents:
        if any(part.startswith(".") for part in d.relative_to(root).parts):
            break  # don't climb into/prune .quarantine and friends
        try:
            d.rmdir()  # succeeds only when the directory is empty
        except OSError:
            break  # non-empty (pending siblings) or already gone — stop climbing
        d = d.parent


def process_once(conn, *, incoming: Path | None = None, settle: float = 3.0) -> list[IngestResult]:
    """Ingest every settled file in incoming once; dispose per result."""
    incoming = incoming or load().paths.incoming
    quarantine = incoming / QUARANTINE_DIRNAME
    unsupported = incoming / UNSUPPORTED_DIRNAME
    results: list[IngestResult] = []
    for path in _candidates(incoming, settle):
        result = ingest_file(path, conn)
        results.append(result)
        rel = path.relative_to(incoming)
        if result.status in ("ingested", "skipped"):
            path.unlink(missing_ok=True)  # canonical copy is already in data/raw
            _prune_empty_parents(path.parent, incoming)  # don't leave drained subfolders behind
        else:  # unsupported / quarantined — set aside so it is not retried every tick
            # Preserve the drop subpath: keeps category provenance visible and avoids
            # same-name collisions across category folders.
            home = unsupported if result.status == "unsupported" else quarantine
            dest = home / rel
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.move(str(path), str(dest))
            _prune_empty_parents(path.parent, incoming)  # the move drained the source dir too
        log.info("watch: %s -> %s", rel, result.status)
    return results


def watch(*, interval: float = 5.0, settle: float = 3.0, once: bool = False) -> None:
    """Poll data/incoming/ and ingest new files until interrupted (or one pass if `once`).

    Incoming-only: tracked-repo sync lives in `watch_repos` (`locus watch-repo`) so a
    commit-triggered repo re-ingest can never steal the slot from a bulk pour mid-tick.
    The two loops are mutually exclusive through the shared ingest lock — each holds it for
    its tick, and the other's tick simply skips (IngestLockHeld) and retries next interval.
    """
    cfg = load()
    migrate(cfg.paths.db)  # ensure the schema is current before ingesting
    conn = get_connection(cfg.paths.db)
    log.info("watching %s (interval %.0fs)", cfg.paths.incoming, interval)
    try:
        while True:
            try:
                with ingest_lock():
                    process_once(conn, incoming=cfg.paths.incoming, settle=settle)
            except IngestLockHeld as exc:
                log.info("tick skipped: %s", exc)
            if once:
                break
            time.sleep(interval)
    finally:
        conn.close()


def watch_repos(*, once: bool = False) -> None:
    """Poll tracked code repos (config [repos]) and incrementally re-ingest moved HEADs.

    Separate process from `locus watch` so a repo re-ingest is isolated from the incoming/
    pour — but mutually exclusive with it through the SAME ingest lock: a tick that finds the
    lock held (the pour mid-tick, or a manual `locus ingest`/`sync`) skips and retries, and
    while THIS loop holds it the pour's tick skips. Each moved repo re-ingests incrementally
    (repo_sync): only the files a commit changed are re-prepared, so even a held slot is
    brief. Polls every config [repos].check_interval seconds.
    """
    cfg = load()
    migrate(cfg.paths.db)
    conn = get_connection(cfg.paths.db)
    repos = [Path(p) for p in cfg.repos.paths]
    log.info(
        "watching %d tracked repo(s), HEAD check every %.0fs", len(repos), cfg.repos.check_interval
    )
    try:
        while True:
            try:
                with ingest_lock():
                    sync_repos(conn, repos)
            except IngestLockHeld as exc:
                log.info("repo tick skipped: %s", exc)
            if once:
                break
            time.sleep(cfg.repos.check_interval)
    finally:
        conn.close()
