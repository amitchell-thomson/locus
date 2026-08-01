"""Capture what he WROTE on an accepted paper — the other half of the reading loop.

Accepting a paper ingested the text and then stopped watching it, so everything he underlined or
wrote afterwards stayed on the tablet forever (migration 0022). This closes that: every reading
the system has delivered and ingested is checked for new ink, and Loop B runs on the ones that
have any.

THREE THINGS MAKE IT CHEAP ENOUGH TO RUN HOURLY:

  1. only documents in `reading_targets` are swept — the ones we delivered, not the whole account;
  2. `rmapi find` tells us where each one sits, so a paper still in `Proposed` (unread) is skipped
     without downloading anything;
  3. the guard is `rmdoc.ink_hash`, not a file hash. Compositing is not byte-reproducible, so
     hashing bytes would report "changed" on every single run and re-pay a billed transcription
     pass each time — the lesson the daily page learned the expensive way.

Geometry does the work: `capture/annotate` decides which passage each mark covers by intersecting
rectangles, no model involved. Transcribing the HANDWRITING beside a mark is a separate, billed
vision pass and stays opt-in.
"""

from __future__ import annotations

import logging
import sqlite3
import tempfile
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path

from locus.reading import proposals as P
from locus.reading.deliver_remarkable import RmapiRunner, _subprocess_runner

log = logging.getLogger(__name__)


@dataclass
class SweepResult:
    source_uri: str
    title: str
    status: str          # 'marks' | 'unchanged' | 'unread' | 'gone' | 'failed'
    marks: int = 0
    folder: str | None = None
    detail: str = ""


def _utcnow() -> str:
    return datetime.now(timezone.utc).isoformat()


def sweep(
    conn: sqlite3.Connection,
    *,
    runner: RmapiRunner | None = None,
    rmapi_binary: str = "rmapi",
    root: str = "/Locus/Reading",
    limit: int = 40,
    fetch_timeout: int = 180,
) -> list[SweepResult]:
    """Check every delivered reading for new ink and record any marks it now carries."""
    from locus.capture.annotate import marks_for_document, store_marks
    from locus.capture.rmdoc import fetch_rmdoc, ink_hash, read_rmdoc
    from locus.reading.watch import list_reading_entries

    runner = runner or _subprocess_runner(rmapi_binary)

    targets = conn.execute(
        "SELECT t.id, t.source_uri, t.doc_uuid, t.device_path, t.stroke_fingerprint, "
        "       COALESCE(d.title, t.source_uri) AS title "
        "FROM reading_targets t LEFT JOIN documents d ON d.source_uri = t.source_uri "
        "ORDER BY COALESCE(t.last_swept, '') ASC LIMIT ?",
        (limit,),
    ).fetchall()
    if not targets:
        return []

    try:
        entries = list_reading_entries(runner, root=root)
    except RuntimeError as exc:
        log.warning("reading sweep: %s — nothing swept", exc)
        return []
    by_stem = {e.stem: e for e in entries}

    out: list[SweepResult] = []
    for t in targets:
        stem = Path(t["device_path"] or "").stem or (t["device_path"] or "")
        entry = by_stem.get(stem)
        if entry is None:
            out.append(SweepResult(t["source_uri"], t["title"], "gone"))
            continue
        if entry.folder == P.FOLDER_PROPOSED:
            # Still awaiting his verdict: nothing is written on it yet by definition, so this
            # costs no download. The folder is still recorded — where a reading sits is state
            # worth having even when there is no ink to read.
            _touch(conn, t["id"], folder=entry.folder)
            out.append(SweepResult(t["source_uri"], t["title"], "unread", folder=entry.folder))
            continue

        with tempfile.TemporaryDirectory() as tmp:
            try:
                # Three minutes, not the thirty-minute default: a marked-up paper is ~1 MB, this
                # runs hourly, and it holds the ingest lock while it waits. A slow fetch must
                # surface as one skipped document, never as a stalled pipeline.
                doc = read_rmdoc(fetch_rmdoc(
                    entry.path, tmp, rmapi_binary=rmapi_binary, timeout=fetch_timeout,
                ))
            except Exception as exc:                 # timeout, not synced, no ink, transport
                out.append(SweepResult(t["source_uri"], t["title"], "failed",
                                       folder=entry.folder, detail=str(exc)[:120]))
                _touch(conn, t["id"], folder=entry.folder)
                continue

            fingerprint = ink_hash(doc)
            if fingerprint == (t["stroke_fingerprint"] or ""):
                out.append(SweepResult(t["source_uri"], t["title"], "unchanged",
                                       folder=entry.folder))
                _touch(conn, t["id"], folder=entry.folder)
                continue

            marks = marks_for_document(doc)
            written = store_marks(
                conn, marks, source_uri=t["source_uri"], doc_uuid=doc.doc_uuid or "",
            ) if marks else 0

        _touch(conn, t["id"], folder=entry.folder, fingerprint=fingerprint, marks=written)
        out.append(SweepResult(t["source_uri"], t["title"], "marks", marks=written,
                               folder=entry.folder))
    return out


def _touch(
    conn: sqlite3.Connection, target_id: int, *, folder: str | None = None,
    fingerprint: str | None = None, marks: int | None = None,
) -> None:
    with conn:
        conn.execute(
            "UPDATE reading_targets SET last_swept = ?, device_folder = COALESCE(?, device_folder),"
            " stroke_fingerprint = COALESCE(?, stroke_fingerprint), marks = COALESCE(?, marks) "
            "WHERE id = ?",
            (_utcnow(), folder, fingerprint, marks, target_id),
        )
