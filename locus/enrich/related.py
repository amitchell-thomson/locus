"""Write a grounded `> [!ai] Related` block into a capture note (agent-layer §8.1).

The Link use case (§1.2) made concrete for capture: a freshly-captured note is run through the
LOCAL retrieval engine, and the distinct corpus documents it surfaces are written back as an
owned block — so a handwritten Brevan-Howard swaps note points at the quant papers and project it
relates to, automatically.

Design guarantees:
  - **Grounded or silent (invariant 3).** Each line is a document that retrieval actually surfaced
    for this note AND that the cross-encoder scored at or above `[retrieve].min_rerank_score`; if
    nothing clears the floor, NO block is written (no invented links).
  - **The same library search heuris uses** (`retrieve.search` + `retrieve.rerank`), since Slice
    C (C2). It used the old full pipeline (expand, assemble, the facet-aware banner), which
    dragged the daily-page engine in behind every capture.
  - **Joins-only, no inference.** The block is a deterministic join over retrieved citations — no
    LLM in this pass, so it can't hallucinate a connection. (LLM phrasing is a later option, §10.)
  - **Owned-block write.** Goes through `vault.writer.upsert_block`, so it is atomic, provenance-
    marked, regenerated wholesale on re-run, and diverts to a sidecar on conflict.

Runs at capture time, BEFORE the note is ingested — so it grounds against the existing corpus, and
the note never matches itself. Retrieval + the writer are injectable for offline tests.
"""

from __future__ import annotations

import logging
import re
from dataclasses import dataclass

from locus.vault.writer import upsert_block

log = logging.getLogger(__name__)

_BLOCK_KIND = "related"
# Strip page markers / AI-fill brackets / frontmatter before using the note as a retrieval query.
_PAGE_MARKER_RE = re.compile(r"<!--.*?-->", re.DOTALL)
_FILL_RE = re.compile(r"[⟦⟧]")


@dataclass
class EnrichResult:
    related: int            # distinct related documents linked
    wrote_block: bool       # False if grounded-or-silent suppressed it
    to_sidecar: bool = False


def _query_from_note(note_text: str, *, limit: int = 2000) -> str:
    text = _FILL_RE.sub("", _PAGE_MARKER_RE.sub(" ", note_text))
    return text.strip()[:limit]


def _doc_titles(conn, doc_ids: list[int]) -> dict[int, str]:
    if not doc_ids:
        return {}
    placeholders = ",".join("?" * len(doc_ids))
    return {
        r["id"]: r["title"]
        for r in conn.execute(
            f"SELECT id, title FROM documents WHERE id IN ({placeholders})", doc_ids
        )
    }


def library_related(conn, query: str) -> list[tuple[int, float]]:
    """(doc_id, cross-encoder score) for each library unit the note's text finds that clears the
    floor. No floor configured keeps every scored unit, as the old pipeline's "flag, never
    filter" did when it had none."""
    from locus.config import load
    from locus.retrieve.rerank import score_pairs
    from locus.retrieve.search import search

    cands = [c for c in search(conn, query) if c.kind in ("chunk", "proposition", "section")]
    if not cands:
        return []
    try:
        scores = score_pairs(query, [c.text for c in cands])
    except RuntimeError as exc:  # no reranker installed: link nothing rather than fail the capture
        log.warning("related: no cross-encoder, so no Related block: %s", exc)
        return []
    floor = load().retrieve.min_rerank_score
    return [(c.doc_id, s) for c, s in zip(cands, scores) if floor is None or s >= floor]


def enrich_note(
    note_path,
    note_text: str,
    *,
    conn,
    run_id: str,
    top_k: int = 5,
    retrieve_fn=None,
    writer=upsert_block,
) -> EnrichResult:
    """Retrieve related documents for the note and write a grounded `> [!ai] Related` block.

    `retrieve_fn(query) -> [(doc_id, score)]` defaults to `library_related`; tests inject a fake.
    `wrote_block=False` when nothing cleared the floor (grounded-or-silent)."""
    if retrieve_fn is None:
        retrieve_fn = lambda q: library_related(conn, q)  # noqa: E731

    hits = retrieve_fn(_query_from_note(note_text))
    if not hits:
        return EnrichResult(related=0, wrote_block=False)

    # Distinct documents, best cross-encoder score first.
    best: dict[int, float] = {}
    for doc_id, score in hits:
        if doc_id not in best or score > best[doc_id]:
            best[doc_id] = score
    ranked = sorted(best, key=lambda d: best[d], reverse=True)[:top_k]
    titles = _doc_titles(conn, ranked)
    lines = [f"> - [[{titles.get(d) or f'doc {d}'}]]" for d in ranked]

    block = "> [!ai] Related\n" + "\n".join(lines)
    res = writer(note_path, _BLOCK_KIND, block, run_id=run_id)
    return EnrichResult(related=len(lines), wrote_block=True, to_sidecar=res.to_sidecar)
