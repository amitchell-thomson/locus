"""Backfill the code concept pass over already-ingested repositories (Phase 1, CLAUDE.md §1.2).

Runs `ingest/concepts.extract_code_concepts` over each `source_type='code'` doc's STORED
narrative (synthesis + section summaries + README text reconstructed from chunks) and inserts
the resulting concept entities. No re-extract, no re-embed, no re-ingest — entities aren't
vectors, and the narrative is already in the DB, so this is cheap (one local-qwen call per
repo). Idempotent: each concept anchors to a real section and is written with
INSERT OR IGNORE on UNIQUE(doc_id, section_id, name, type), so re-running adds only genuinely
new concepts.

After this:
    locus link              # fold the new concept surfaces into entity_aliases
    locus export-obsidian   # the orphaned projects should now link

See docs/code-concept-extraction-plan.md §8. Run:
    uv run python -m scripts.backfill_code_concepts [--dry-run]
"""

from __future__ import annotations

import argparse
import sys

from locus.config import load
from locus.db.connection import get_connection
from locus.ingest import concepts
from locus.ingest.entities import is_grounded


def _readme_text(conn, doc_id: int, limit: int = 12000) -> str:
    """Reconstruct a repo's README/markdown text from its chunks (sections store no raw text).
    The PRIMARY concept surface — supply generously; `extract_code_concepts` caps it to its
    markdown budget and weights it above the code summaries."""
    rows = conn.execute(
        """
        SELECT c.raw_text FROM chunks c JOIN sections s ON s.id = c.section_id
        WHERE c.doc_id = ?
          AND (lower(s.title) LIKE '%.md' OR lower(s.title) LIKE '%.markdown'
               OR lower(s.title) LIKE '%readme%')
        ORDER BY s.position, c.position
        """,
        (doc_id,),
    ).fetchall()
    return "\n".join(r["raw_text"] for r in rows)[:limit]


def _anchor_section(name: str, sections: list) -> int:
    """sections.id of the section whose summary attests `name` (fallback: the first section).
    Mirrors ingest_pipeline._attach_concepts so backfill and at-ingest anchor identically."""
    for s in sections:
        if is_grounded(name, s["summary"] or ""):
            return s["id"]
    return sections[0]["id"]


def backfill(conn, *, dry_run: bool = False, log=print) -> int:
    cfg = load().concepts
    stop = set(cfg.stoplist) or None
    docs = conn.execute(
        "SELECT id, title, thesis, method, result, limitations "
        "FROM documents WHERE source_type='code' ORDER BY id"
    ).fetchall()
    total = 0
    for d in docs:
        sections = conn.execute(
            "SELECT id, position, title, summary FROM sections WHERE doc_id=? ORDER BY position",
            (d["id"],),
        ).fetchall()
        if not sections:
            log(f"  [{d['id']}] {d['title']!r}: no sections, skipped")
            continue
        synthesis_text = (
            f"Thesis: {d['thesis']}\nMethod: {d['method']}\n"
            f"Result: {d['result']}\nLimitations: {d['limitations']}"
        )
        summaries = [s["summary"] or "" for s in sections]
        ents = concepts.extract_code_concepts(
            d["title"], synthesis_text, summaries, readme_text=_readme_text(conn, d["id"]),
            stoplist=stop, max_concepts=cfg.max_per_doc,
        )
        log(f"  [{d['id']}] {d['title'][:48]!r}: {len(ents)} concept(s)")
        if dry_run:
            log("      " + ", ".join(f"{e.name} ({e.type})" for e in ents))
            continue
        written = 0
        with conn:
            for e in ents:
                cur = conn.execute(
                    "INSERT OR IGNORE INTO entities (doc_id, section_id, name, type) VALUES (?,?,?,?)",
                    (d["id"], _anchor_section(e.name, sections), e.name, str(e.type)),
                )
                written += max(0, cur.rowcount)
        total += written
    log(f"\n{'(dry run) ' if dry_run else ''}backfill complete: {total} new concept entities across {len(docs)} repos")
    if not dry_run and total:
        log("Next: `locus link` (fold concepts into entity_aliases), then `locus export-obsidian`.")
    return total


def main(argv=None) -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--dry-run", action="store_true", help="print concepts without writing")
    args = ap.parse_args(argv)

    from locus.ingest_lock import IngestLockHeld, ingest_lock

    conn = get_connection(load().paths.db)
    try:
        if args.dry_run:
            backfill(conn, dry_run=True)
            return
        # Mutually exclusive with watch/ingest — concurrent writers contend on the DB (§7).
        with ingest_lock():
            backfill(conn, dry_run=False)
    except IngestLockHeld as exc:
        print(exc)
        sys.exit(1)
    finally:
        conn.close()


if __name__ == "__main__":
    main()
