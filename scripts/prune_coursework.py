"""Prune engineering coursework documents that link only to other coursework.

OWNER DECISION (2026-07-29): remove the coursework that carries NO bridging concept — documents
whose canonical entities never reach a paper/project/career/note. Rationale: not relevant to the
quant track, and it keeps the corpus balanced while the real material accumulates.

SELECTION (recomputed live, never hardcoded): a coursework document is kept if it carries at least
one canonical entity that also appears in a paper/project/career/note document. By construction
this cannot remove a bridging concept — every bridge is anchored in a document that carries it.
Recomputed at run time on purpose: freshly captured handwriting can turn a previously isolated
coursework document into a bridging one, and a stale id list would delete it anyway.

SAFETY:
  - `delete_document()` is the ingest pipeline's own routine — it clears the sqlite-vec tables
    (virtual, no foreign keys, so orphan vectors would collide on a reused row id) and the
    figure PNGs. Raw SQL DELETE would silently corrupt the vector store.
  - The source file is MOVED out of `vault/incoming/` into `vault/pruned/`, mirroring its subpath.
    Without this the next `locus watch` / `locus ingest` re-ingests everything just deleted —
    the deletion would look successful and silently undo itself.
  - Raw-store copies under `vault/raw/` are left untouched, so any document remains recoverable.

Dry run by default. Pass --apply to write.
"""

from __future__ import annotations

import argparse
import collections
import shutil
import sys
from pathlib import Path

sys.path.insert(0, "/home/alec/server-projects/locus")

from locus.config import PROJECT_ROOT, load  # noqa: E402
from locus.db.connection import get_connection  # noqa: E402
from locus.ingest_pipeline import delete_document  # noqa: E402

QUANT = {"paper", "project", "career", "note"}


def zero_bridge_coursework(conn) -> tuple[list[int], dict[int, str], dict[int, str]]:
    cats = {r["id"]: r["category"] for r in conn.execute("SELECT id, category FROM documents")}
    titles = {r["id"]: r["title"] for r in conn.execute("SELECT id, title FROM documents")}
    uris = {r["id"]: (r["source_uri"] or "") for r in conn.execute("SELECT id, source_uri FROM documents")}

    canon: dict[str, set[int]] = collections.defaultdict(set)
    for r in conn.execute(
        """
        SELECT COALESCE(a.canonical_name, e.name) AS n, e.doc_id AS d
        FROM entities e
        LEFT JOIN entity_aliases a ON a.variant_name=e.name AND a.variant_type=e.type
        """
    ):
        canon[r["n"]].add(r["d"])

    bridging = {
        n for n, ds in canon.items()
        if len(ds) >= 2
        and {cats.get(x) for x in ds} & QUANT
        and "coursework" in {cats.get(x) for x in ds}
    }
    carries: collections.Counter[int] = collections.Counter()
    for n in bridging:
        for d in canon[n]:
            if cats.get(d) == "coursework":
                carries[d] += 1

    doomed = [d for d, c in cats.items() if c == "coursework" and carries.get(d, 0) == 0]
    return sorted(doomed), titles, uris


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--apply", action="store_true", help="actually delete (default: dry run)")
    args = ap.parse_args()

    cfg = load()
    conn = get_connection(cfg.paths.db)
    try:
        doomed, titles, uris = zero_bridge_coursework(conn)
        total = conn.execute("SELECT COUNT(*) c FROM documents").fetchone()["c"]
        props = conn.execute(
            f"SELECT COUNT(*) c FROM propositions WHERE doc_id IN ({','.join('?' * len(doomed))})",
            doomed,
        ).fetchone()["c"] if doomed else 0

        print(f"corpus {total} docs | zero-bridge coursework: {len(doomed)} | propositions: {props}")
        for d in doomed[:10]:
            print(f"   [{d}] {titles[d][:70]}")
        if len(doomed) > 10:
            print(f"   … and {len(doomed) - 10} more")

        incoming = Path(cfg.paths.incoming).resolve()
        pruned_root = incoming.parent / "pruned"

        def source_path(uri: str) -> Path | None:
            """Resolve a source_uri to a real file.

            source_uri is stored RELATIVE for some documents ('vault/incoming/coursework/x.pdf')
            and absolute for others, so a bare Path(...).exists() check silently reports 'no files
            to move' — which would leave every pruned document sitting in the watch folder, to be
            re-ingested on the next run. Relative paths resolve against PROJECT_ROOT."""
            if not uri:
                return None
            p = Path(uri)
            if not p.is_absolute():
                p = PROJECT_ROOT / p
            p = p.resolve()
            return p if p.exists() and p.is_relative_to(incoming) else None

        movable = [(d, sp) for d in doomed if (sp := source_path(uris[d])) is not None]
        print(f"\nsource files under incoming/ to move aside: {len(movable)} -> {pruned_root}")
        if not args.apply:
            print("\nDRY RUN — nothing written. Re-run with --apply.")
            return

        moved = 0
        for _d, src in movable:
            dest = pruned_root / src.relative_to(incoming)
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.move(str(src), str(dest))
            moved += 1

        for d in doomed:
            delete_document(conn, d)

        left = conn.execute("SELECT COUNT(*) c FROM documents").fetchone()["c"]
        print(f"\ndeleted {len(doomed)} documents ({total} -> {left}); moved {moved} source files")
        print("NEXT: `locus link` (the alias substrate references deleted docs), then `locus audit`.")
    finally:
        conn.close()


if __name__ == "__main__":
    main()
