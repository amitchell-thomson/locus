"""Folder watcher (§10 slice 3): auto-ingest files dropped into vault/incoming/.

Closes the loop with the laptop outbox — a file rsync'd into incoming is ingested without a
manual command. Uses simple polling (not filesystem events): ingest is unbounded-time (§2.4),
polling is dependency-free, survives restarts (it rescans the backlog), and a settle window
avoids grabbing a file mid-transfer.

Disposition per file (incoming is a disposable drop zone; the raw copy is kept in vault/raw):
  ingested / skipped   -> removed from incoming
  unsupported / quarantined -> moved to incoming/.quarantine/<drop subpath> so it is not
                               retried forever (subpath preserved for category provenance)
"""

from __future__ import annotations

import logging
import shutil
import time
from pathlib import Path

from locus.config import load
from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.ingest_pipeline import IngestResult, ingest_file

log = logging.getLogger(__name__)

QUARANTINE_DIRNAME = ".quarantine"


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
        if any(part.startswith(".") for part in p.relative_to(incoming).parts):
            continue  # dotfiles (.gitkeep) and dotted subtrees (.quarantine)
        try:
            if now - p.stat().st_mtime < settle:
                continue  # still being written / just landed — wait for it to settle
        except FileNotFoundError:
            continue
        out.append(p)
    return out


def process_once(conn, *, incoming: Path | None = None, settle: float = 3.0) -> list[IngestResult]:
    """Ingest every settled file in incoming once; dispose of each per its result."""
    incoming = incoming or load().paths.incoming
    quarantine = incoming / QUARANTINE_DIRNAME
    results: list[IngestResult] = []
    for path in _candidates(incoming, settle):
        result = ingest_file(path, conn)
        results.append(result)
        rel = path.relative_to(incoming)
        if result.status in ("ingested", "skipped"):
            path.unlink(missing_ok=True)  # canonical copy is already in vault/raw
        else:  # unsupported / quarantined — set aside so it is not retried every tick
            # Preserve the drop subpath: keeps category provenance visible and avoids
            # same-name collisions across category folders.
            dest = quarantine / rel
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.move(str(path), str(dest))
        log.info("watch: %s -> %s", rel, result.status)
    return results


def watch(*, interval: float = 5.0, settle: float = 3.0, once: bool = False) -> None:
    """Poll vault/incoming/ and ingest new files until interrupted (or one pass if `once`)."""
    cfg = load()
    migrate(cfg.paths.db)  # ensure the schema is current before ingesting
    conn = get_connection(cfg.paths.db)
    log.info("watching %s (interval %.0fs)", cfg.paths.incoming, interval)
    try:
        while True:
            process_once(conn, incoming=cfg.paths.incoming, settle=settle)
            if once:
                break
            time.sleep(interval)
    finally:
        conn.close()
