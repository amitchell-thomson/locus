"""Turn an accepted proposal into a corpus document — the one step that writes to the corpus.

Deliberately separate from `watch.scan`, which only READS the device. Keeping the transport read
apart from the corpus write means a flaky `rmapi find` can never itself cause an ingest, and the
accept decision stays reviewable in the DB between the two.

There is no special-casing here: an accepted paper is copied into `vault/incoming/paper/` and goes
through the ordinary ingest spine like anything else the owner drops. That is the point of making
the folder move the gate — once he has made the gesture, this is just a file arriving.

TWO THINGS NEVER INGEST:

  - a **stub**. A proposal with no `local_path` is our description of a work, not the work. It
    stays `accepted` and waits for him to supply the real file (invariant 5 — agent output must
    never re-enter the corpus as though it were his material, and a stub is the purest case);
  - a proposal whose file has gone missing. Reported, not guessed at.

On success the `reading_targets` mapping is written, which is what finally lets the marks he makes
on the paper join back to the document (see migration 0017).
"""

from __future__ import annotations

import logging
import shutil
import sqlite3
from dataclasses import dataclass
from pathlib import Path

from locus import config as _config
from locus.ingest_lock import IngestLockHeld, ingest_lock
from locus.reading import proposals as P

log = logging.getLogger(__name__)

# Accepted readings are papers; the drop folder IS the category (`ingest_pipeline._category`
# singularises, so `paper/` and `papers/` both resolve to 'paper').
DEFAULT_DROP = "paper"


@dataclass
class AcceptResult:
    proposal_id: int
    title: str
    status: str          # 'ingested' | 'awaiting-file' | 'missing-file' | 'failed'
    doc_id: int | None = None
    detail: str = ""


def _source_uri(conn: sqlite3.Connection, doc_id: int) -> str | None:
    row = conn.execute("SELECT source_uri FROM documents WHERE id=?", (doc_id,)).fetchone()
    return row["source_uri"] if row else None


def ingest_accepted(
    conn: sqlite3.Connection,
    *,
    incoming: Path | None = None,
    drop_folder: str = DEFAULT_DROP,
    dry_run: bool = False,
) -> list[AcceptResult]:
    """Ingest every proposal sitting at `status='accepted'` that has a real file behind it."""
    from locus.ingest_pipeline import ingest_file

    accepted = P.list_proposals(conn, status="accepted", limit=200)
    if not accepted:
        return []

    incoming = Path(incoming or _config.load().paths.incoming) / drop_folder
    results: list[AcceptResult] = []

    for prop in accepted:
        if prop.is_stub:
            results.append(AcceptResult(
                prop.id, prop.title, "awaiting-file",
                detail="stub — supply the real file to ingest it",
            ))
            continue

        src = Path(prop.local_path or "")
        if not src.is_file():
            results.append(AcceptResult(
                prop.id, prop.title, "missing-file", detail=f"{src} is gone",
            ))
            continue

        if dry_run:
            results.append(AcceptResult(prop.id, prop.title, "ingested", detail="dry-run"))
            continue

        incoming.mkdir(parents=True, exist_ok=True)
        dest = incoming / (prop.filename or src.name)
        shutil.copy2(src, dest)

        try:
            with ingest_lock():
                result = ingest_file(dest, conn, category=drop_folder)
        except IngestLockHeld:
            results.append(AcceptResult(
                prop.id, prop.title, "failed",
                detail="another ingest holds the lock — will retry next run",
            ))
            continue

        if result.status not in ("ingested", "skipped") or result.doc_id is None:
            results.append(AcceptResult(
                prop.id, prop.title, "failed",
                detail=result.error or f"ingest returned {result.status}",
            ))
            continue

        uri = _source_uri(conn, result.doc_id)
        if uri:
            # THE join key. Without this the marks he makes on this paper never reach the document
            # (migration 0017), and half the reason for delivering it in the first place is lost.
            P.link_target(
                conn, source_uri=uri, doc_uuid=prop.device_uuid,
                device_path=prop.filename, proposal_id=prop.id, linked_by="delivery",
            )
        P.set_status(conn, prop.id, "ingested")
        results.append(AcceptResult(prop.id, prop.title, "ingested", doc_id=result.doc_id))

    return results
