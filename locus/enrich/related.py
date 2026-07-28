"""Write a grounded `> [!ai] Related` block into a capture note (agent-layer §8.1).

The Link use case (§1.2) made concrete for capture: a freshly-captured note is run through the
LOCAL retrieval engine, and the distinct corpus documents it surfaces are written back as an
owned block — so a handwritten Brevan-Howard swaps note points at the quant papers and project it
relates to, automatically.

Design guarantees:
  - **Grounded or silent (invariant 3).** Each line is a document that retrieval actually surfaced
    for this note; if retrieval is low-confidence or empty, NO block is written (no invented links).
  - **Joins-only, no inference.** The block is a deterministic join over retrieved citations — no
    LLM in this pass, so it can't hallucinate a connection. (LLM phrasing is a later option, §10.)
  - **Owned-block write.** Goes through `vault.writer.upsert_block`, so it is atomic, provenance-
    marked, regenerated wholesale on re-run, and diverts to a sidecar on conflict.

Runs at capture time, BEFORE the note is ingested — so it grounds against the existing corpus, and
the note never matches itself. Retrieval + the writer are injectable for offline tests.
"""

from __future__ import annotations

import re
from dataclasses import dataclass

from locus.vault.writer import upsert_block

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

    `retrieve_fn(query)` defaults to the live pipeline; tests inject a fake. Returns EnrichResult;
    `wrote_block=False` when retrieval was low-confidence/empty (grounded-or-silent)."""
    if retrieve_fn is None:
        from locus.retrieve import retrieve as _retrieve

        retrieve_fn = lambda q: _retrieve(q, conn=conn)  # noqa: E731

    result = retrieve_fn(_query_from_note(note_text))
    if getattr(result, "low_confidence", False) or not result.citation_details:
        return EnrichResult(related=0, wrote_block=False)

    # Distinct documents, best cross-encoder score first.
    best: dict[int, float] = {}
    for c in result.citation_details:
        score = c.rerank_score if c.rerank_score is not None else float("-inf")
        if c.doc_id not in best or score > best[c.doc_id]:
            best[c.doc_id] = score
    ranked = sorted(best, key=lambda d: best[d], reverse=True)[:top_k]
    titles = _doc_titles(conn, ranked)
    lines = [f"> - [[{titles.get(d) or f'doc {d}'}]]" for d in ranked]
    if not lines:
        return EnrichResult(related=0, wrote_block=False)

    block = "> [!ai] Related\n" + "\n".join(lines)
    res = writer(note_path, _BLOCK_KIND, block, run_id=run_id)
    return EnrichResult(related=len(lines), wrote_block=True, to_sidecar=res.to_sidecar)
