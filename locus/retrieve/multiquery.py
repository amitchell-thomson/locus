"""Multi-query cross-domain expansion (H2 fix, CLAUDE.md §13).

A query phrased in one discipline's vocabulary doesn't surface documents written in
another's. The 2026-06-09 audit pinned the mechanism: a finance-phrased "regime-switching
state-space / Kalman" query DID surface the engineering control doc via the shared "state
space" canonical (the entity bridge fires), but the cross-encoder reranker scored it -3.25 —
below the floor — because finance-phrased query text is semantically distant from EE control
prose. Entity-linking can't fix that; only reranking against matching-vocabulary query text
can.

So: rephrase the query into several disciplinary vocabularies (control engineering, ML,
finance, statistics, ...), retrieve each, and rerank every candidate against the VARIANT
THAT RETRIEVED IT (max over variants) — the engineering doc is then scored against
engineering phrasing (high) instead of the finance phrasing (demoted). The rephrasing is a
local qwen call, so retrieval stays API-free. Best-effort: any failure falls back to the
single-query pass (reformulate returns []).
"""

from __future__ import annotations

from pydantic import BaseModel

from locus.ingest.llm import generate_structured

_SYSTEM = (
    "You rewrite a search query so a retrieval engine can find relevant documents no matter "
    "which field's vocabulary they are written in. Given one query, produce alternative "
    "phrasings of the SAME information need, each expressed in the terminology of a DIFFERENT "
    "academic or professional discipline (e.g. control engineering, machine learning, "
    "statistics, finance, physics, signal processing). Translate the concepts into each "
    "field's standard terms — e.g. a finance 'regime-switching state-space model' becomes, in "
    "control engineering, 'state-space representation with a Kalman filter / observer'. "
    "Rules: do NOT answer the question; do NOT repeat the original wording; keep each phrasing "
    "concise (a search query, not a sentence); only rephrase into fields where the concept "
    "genuinely has a counterpart. Use the reformulate tool."
)


class _Reformulations(BaseModel):
    variants: list[str]


def _build_user(query: str, k: int) -> str:
    return f"Query: {query!r}\n\nProduce {k} cross-disciplinary reformulations."


def reformulate(query: str, k: int, *, client=None, model: str | None = None) -> list[str]:
    """Up to `k` alternate phrasings of `query`, each in a different discipline's vocabulary.

    Local qwen (keeps retrieval API-free). De-duplicates against the original and each other.
    Best-effort: returns [] on any failure so the caller falls back to a single-query pass.
    """
    if k <= 0:
        return []
    try:
        out = generate_structured(
            _Reformulations, _build_user(query, k), system=_SYSTEM, client=client, model=model
        )
    except Exception:  # reformulation must never break retrieval — degrade to single-query
        return []
    seen = {query.strip().lower()}
    variants: list[str] = []
    for v in out.variants:
        v = (v or "").strip()
        if v and v.lower() not in seen:
            seen.add(v.lower())
            variants.append(v)
    return variants[:k]
