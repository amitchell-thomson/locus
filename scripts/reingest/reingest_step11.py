"""One-off driver for the step-11 figures re-ingest (2026-06-06).

Same semantics as `locus ingest --reingest` (flock-guarded, sequential, quarantine-not-
crash) but prints one line PER DOCUMENT as it completes, so progress is watchable from
the outside. Order: raw-store re-ingests first, then fresh incoming drops last — if an
incoming file's bytes match an existing doc, the incoming pass replaces it with proper
drop-folder metadata (category/source_uri derive from the incoming path, which wins).
"""

from __future__ import annotations

import logging
import sys
import time

from locus.config import load
from locus.db.connection import get_connection
from locus.ingest_lock import IngestLockHeld, ingest_lock
from locus.ingest_pipeline import ingest_file

logging.basicConfig(level=logging.WARNING)  # per-section INFO noise off; doc lines below

cfg = load()
raw = sorted(cfg.paths.raw_store.glob("*.pdf")) + sorted(cfg.paths.raw_store.glob("*.pptx"))
incoming = sorted(p for p in cfg.paths.incoming.rglob("*") if p.is_file() and p.name != ".gitkeep")
paths = raw + incoming

print(f"batch: {len(raw)} raw re-ingests + {len(incoming)} incoming = {len(paths)} files", flush=True)
t_start = time.time()

try:
    with ingest_lock():
        conn = get_connection(cfg.paths.db)
        try:
            for i, p in enumerate(paths, 1):
                t0 = time.time()
                r = ingest_file(p, conn, reingest=True)
                mins = (time.time() - t0) / 60
                if r.status == "ingested":
                    print(
                        f"[{i}/{len(paths)}] ingested  {p.name[:58]:<58} doc={r.doc_id} "
                        f"secs={r.sections} figs={r.figures} props={r.propositions} "
                        f"ents={r.entities} ({mins:.1f} min)",
                        flush=True,
                    )
                else:
                    print(
                        f"[{i}/{len(paths)}] {r.status.upper()}  {p.name[:58]} -- {r.error}",
                        flush=True,
                    )
        finally:
            conn.close()
except IngestLockHeld as exc:
    print(exc, flush=True)
    sys.exit(1)

print(f"DONE in {(time.time() - t_start) / 3600:.2f} h", flush=True)
