"""One-off driver for the round-5 remediation re-ingest (2026-06-07).

Activates: window-bounded summaries (PROMPT_VERSION 3 — v2 cache invalidated), the
base-concept entity prompt, and [repos].exclude_globs. Same semantics as
`locus ingest --reingest` (flock-guarded, sequential, quarantine-not-crash), one line
per document; tracked repos re-sync with --force semantics afterwards.
"""

from __future__ import annotations

import logging
import sys
import time

from locus.config import load
from locus.db.connection import get_connection
from locus.ingest_lock import IngestLockHeld, ingest_lock
from locus.ingest_pipeline import ingest_file
from locus.sync import sync_repos

logging.basicConfig(level=logging.WARNING)

cfg = load()
raw = sorted(cfg.paths.raw_store.glob("*.pdf")) + sorted(cfg.paths.raw_store.glob("*.pptx"))

print(f"batch: {len(raw)} raw re-ingests + {len(cfg.repos.paths)} repo syncs", flush=True)
t_start = time.time()

try:
    with ingest_lock():
        conn = get_connection(cfg.paths.db)
        try:
            for i, p in enumerate(raw, 1):
                t0 = time.time()
                r = ingest_file(p, conn, reingest=True)
                mins = (time.time() - t0) / 60
                if r.status == "ingested":
                    print(
                        f"[{i}/{len(raw)}] ingested  {p.name[:58]:<58} doc={r.doc_id} "
                        f"secs={r.sections} figs={r.figures} props={r.propositions} "
                        f"ents={r.entities} ({mins:.1f} min)",
                        flush=True,
                    )
                else:
                    print(
                        f"[{i}/{len(raw)}] {r.status.upper()}  {p.name[:58]} -- {r.error}",
                        flush=True,
                    )
            print("--- repo sync (force) ---", flush=True)
            for r in sync_repos(conn, force=True):
                print(f"repo {r.status}  {r.path}  doc={r.doc_id} -- {r.error or 'ok'}", flush=True)
        finally:
            conn.close()
except IngestLockHeld as exc:
    print(exc, flush=True)
    sys.exit(1)

print(f"DONE in {(time.time() - t_start) / 3600:.2f} h", flush=True)
