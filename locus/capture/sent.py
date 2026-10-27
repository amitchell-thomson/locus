"""What Locus sent to the tablet, by the sha256 of the bytes (migration 0038, Slice 0.12).

Capture asks one question of every PDF on the device: did Locus send this? A yes means Claude
wrote it, so it is never ingested and his notes on it are promoted with the passages they were
about; a no, in an area folder, means it is a source. Recording happens BEFORE the upload in
`deliver_pdf`, so a push can never land on the device unrecorded: a failed record stops the push,
while a recorded push that then fails leaves a hash nothing on the device carries, which is
harmless.
"""

from __future__ import annotations

import hashlib
import sqlite3
from datetime import datetime, timezone
from pathlib import Path
from typing import Callable

# Takes the PDF about to be uploaded and the device folder it is going to.
SentRecorder = Callable[[Path, str], None]


def record_sent(
    conn: sqlite3.Connection, pdf: Path, folder: str, *, origin: str = "push",
    content_hash: str | None = None, now: str | None = None,
) -> str:
    """Record one sent PDF. Returns its hash. Idempotent: a resend of the same bytes is one row."""
    h = content_hash or hashlib.sha256(Path(pdf).read_bytes()).hexdigest()
    with conn:
        conn.execute(
            "INSERT INTO device_sent (content_hash, filename, folder, sent_at, origin) "
            "VALUES (?,?,?,?,?) ON CONFLICT(content_hash) DO NOTHING",
            (h, Path(pdf).name, folder.strip("/"),
             now or datetime.now(timezone.utc).isoformat(), origin),
        )
    return h


def sent_hashes(conn: sqlite3.Connection) -> set[str]:
    return {r[0] for r in conn.execute("SELECT content_hash FROM device_sent")}


def default_recorder() -> SentRecorder:
    """Record into the live database. Only built when the caller did not inject a transport,
    the same rule `deliver_pdf` applies to its inspector, so tests never touch the real DB."""

    def record(pdf: Path, folder: str) -> None:
        from locus.config import load
        from locus.db.connection import get_connection

        conn = get_connection(load().paths.db)
        try:
            record_sent(conn, pdf, folder)
        finally:
            conn.close()

    return record
