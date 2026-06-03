"""Folder watcher (§10 slice 3): auto-ingest files dropped into vault/incoming/.

Closes the loop with the laptop outbox — a file rsync'd into incoming is ingested without a
manual command. Uses simple polling (not filesystem events): ingest is unbounded-time (§2.4),
polling is dependency-free, survives restarts (it rescans the backlog), and a settle window
avoids grabbing a file mid-transfer.

Disposition per file (incoming is a disposable drop zone; the raw copy is kept in vault/raw):
  ingested / skipped   -> removed from incoming
  unsupported / quarantined -> moved to incoming/.quarantine/ so it is not retried forever
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
    """Files ready to ingest: real files, not dotfiles/.gitkeep, and stable for `settle` seconds."""
    now = time.time()
    out: list[Path] = []
    for p in sorted(incoming.iterdir()):
        if p.name == ".gitkeep" or p.name.startswith("."):
            continue
        if not p.is_file():
            continue
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
        if result.status in ("ingested", "skipped"):
            path.unlink(missing_ok=True)  # canonical copy is already in vault/raw
        else:  # unsupported / quarantined — set aside so it is not retried every tick
            quarantine.mkdir(exist_ok=True)
            shutil.move(str(path), str(quarantine / path.name))
        log.info("watch: %s -> %s", path.name, result.status)
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
