"""Recovery re-ingest for the 2026-06-06 audit's OOM finding.

Re-ingests ONLY the documents whose math-OCR pages fell back to raw text during the
step-11 batch (the GOT pass OOM'd against the still-resident figures VLM — fixed by
llm.unload_all()). Targets are selected live from the DB by their OOM gap signature, so
the list is self-verifying rather than hard-coded. Same per-doc progress lines as
scripts/reingest_step11.py; metadata continuity (category/source_uri) is inherited by
the raw-store re-ingest path.
"""

from __future__ import annotations

import json
import logging
import sys
import time

from locus.config import load
from locus.db.connection import get_connection
from locus.ingest_lock import IngestLockHeld, ingest_lock
from locus.ingest_pipeline import ingest_file

logging.basicConfig(level=logging.WARNING)

cfg = load()
conn = get_connection(cfg.paths.db)
targets = []
for r in conn.execute("SELECT id, title, raw_path, gap_flags FROM documents ORDER BY id"):
    gaps = json.loads(r["gap_flags"] or "[]")
    ooms = sum(1 for g in gaps if "engine-error: CUDA out of memory" in g)
    if ooms:
        targets.append((r["id"], r["title"], cfg.paths.raw_store / r["raw_path"], ooms))
conn.close()

print(f"recovery batch: {len(targets)} docs, {sum(t[3] for t in targets)} OOM'd pages", flush=True)
for doc_id, title, _, ooms in targets:
    print(f"  [{doc_id}] {title[:55]} ({ooms} pages)", flush=True)

t_start = time.time()
try:
    with ingest_lock():
        conn = get_connection(cfg.paths.db)
        try:
            for i, (doc_id, title, path, _) in enumerate(targets, 1):
                t0 = time.time()
                r = ingest_file(path, conn, reingest=True)
                mins = (time.time() - t0) / 60
                if r.status == "ingested":
                    print(
                        f"[{i}/{len(targets)}] ingested  {title[:52]:<52} doc={r.doc_id} "
                        f"secs={r.sections} figs={r.figures} ({mins:.1f} min)",
                        flush=True,
                    )
                else:
                    print(f"[{i}/{len(targets)}] {r.status.upper()}  {title[:52]} -- {r.error}", flush=True)
        finally:
            conn.close()
except IngestLockHeld as exc:
    print(exc, flush=True)
    sys.exit(1)

print(f"DONE in {(time.time() - t_start) / 3600:.2f} h", flush=True)
