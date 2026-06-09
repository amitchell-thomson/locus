"""Backfill documents.gap_flags with the precision-filtered gap pass (no re-ingest needed).

The 2026-06-05 second evaluation found the repaired gap pass emitting false absences
("does not detail θC" while the text states θC=0.8) — a summarisation artifact, since the
model reads summaries. gaps.flag_gaps now carries a hardened prompt + filter_gaps (keep only
deferral-hint-backed or genuinely-unattested claims). Gap flags are not embedded, so stored
documents can be corrected in place: re-run the pass from stored summaries + chunk text,
PRESERVING the pipeline audit-trail lines (math-OCR fallbacks, degraded passes).

Usage:
  uv run python scripts/backfill_gaps.py            # dry run
  uv run python scripts/backfill_gaps.py --apply    # write

Needs Ollama (one structured call per document); don't run during an ingest.
"""

from __future__ import annotations

import argparse
import json

from locus.config import load
from locus.db.connection import get_connection
from locus.eval.metrics import semantic_gaps
from locus.ingest.gaps import flag_gaps


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--apply", action="store_true", help="write changes (default: dry run)")
    args = parser.parse_args()

    conn = get_connection(load().paths.db)
    try:
        docs = conn.execute("SELECT id, title, gap_flags FROM documents ORDER BY id").fetchall()
        for d in docs:
            old = json.loads(d["gap_flags"] or "[]")
            audit_lines = [g for g in old if g not in semantic_gaps(old)]  # always preserved
            sections = []
            for s in conn.execute(
                "SELECT id, title, summary FROM sections WHERE doc_id=? ORDER BY position",
                (d["id"],),
            ):
                raw = "\n".join(
                    r["raw_text"] for r in conn.execute(
                        "SELECT raw_text FROM chunks WHERE section_id=? ORDER BY position",
                        (s["id"],),
                    )
                )
                sections.append((s["title"], s["summary"] or "", raw))
            doc_row = conn.execute(
                "SELECT thesis, method, result, limitations FROM documents WHERE id=?",
                (d["id"],),
            ).fetchone()
            context = (
                f"Thesis: {doc_row['thesis']}\nMethod: {doc_row['method']}\n"
                f"Result: {doc_row['result']}\nLimitations: {doc_row['limitations']}"
            )
            new_semantic = flag_gaps(d["title"], context, sections=sections)
            new = new_semantic + audit_lines
            n_old = len(semantic_gaps(old))
            print(f"[{d['id']:>2}] semantic gaps {n_old} -> {len(new_semantic)} "
                  f"(+{len(audit_lines)} audit)  {d['title'][:48]}")
            for g in new_semantic:
                print(f"      - {g[:100]}")
            if args.apply:
                with conn:
                    conn.execute(
                        "UPDATE documents SET gap_flags=? WHERE id=?",
                        (json.dumps(new), d["id"]),
                    )
        print("\napplied" if args.apply else "\ndry run (pass --apply to write)")
    finally:
        conn.close()


if __name__ == "__main__":
    main()
