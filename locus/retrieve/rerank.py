"""Cross-encoder reranking (precision filter).

The bi-encoder/lexical candidates are coarse; a ms-marco-MiniLM cross-encoder scores each
(query, candidate text) pair jointly and keeps the top rerank_top_k. Runs on CPU on purpose
(§3/§4): the 8 GB GPU is reserved for Ollama, so we never let sentence-transformers grab CUDA.

Requires the `rerank` extra (sentence-transformers + torch): `uv sync --extra rerank`.
"""

from __future__ import annotations

from functools import lru_cache

from locus.retrieve.search import Candidate

_MODEL = "cross-encoder/ms-marco-MiniLM-L-6-v2"


@lru_cache(maxsize=1)
def _cross_encoder():
    try:
        from sentence_transformers import CrossEncoder
    except ImportError as exc:  # pragma: no cover
        raise RuntimeError(
            "Reranking needs the 'rerank' extra. Install it with: uv sync --extra rerank"
        ) from exc
    return CrossEncoder(_MODEL, device="cpu")  # force CPU; GPU is for Ollama


def rerank(query: str, candidates: list[Candidate], top_k: int) -> list[Candidate]:
    """Return the top_k candidates by cross-encoder relevance (rerank_score set on each)."""
    if not candidates:
        return []
    scores = _cross_encoder().predict([(query, c.text) for c in candidates])
    for c, s in zip(candidates, scores):
        c.rerank_score = float(s)
    candidates.sort(key=lambda c: c.rerank_score, reverse=True)
    return candidates[:top_k]
