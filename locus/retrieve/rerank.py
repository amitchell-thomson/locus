"""Cross-encoder reranking (precision filter).

The bi-encoder/lexical candidates are coarse; a ms-marco-MiniLM cross-encoder scores each
(query, candidate text) pair jointly and keeps the top rerank_top_k. Runs on CPU on purpose
(§3/§4): the 8 GB GPU is reserved for Ollama, so we never let sentence-transformers grab CUDA.

Requires the `rerank` extra (sentence-transformers + torch): `uv sync --extra rerank`.
"""

from __future__ import annotations

import os
from functools import lru_cache
from pathlib import Path

from locus.retrieve.search import Candidate

_MODEL = "cross-encoder/ms-marco-MiniLM-L-6-v2"

# Quiet the HuggingFace/transformers startup chatter (progress bars + warnings) so the CLI
# output stays clean. Must be set before sentence-transformers / huggingface_hub are imported.
os.environ.setdefault("HF_HUB_DISABLE_PROGRESS_BARS", "1")
os.environ.setdefault("TRANSFORMERS_VERBOSITY", "error")
os.environ.setdefault("TOKENIZERS_PARALLELISM", "false")


def _model_cached() -> bool:
    base = Path(os.environ.get("HF_HOME") or (Path.home() / ".cache" / "huggingface"))
    return (base / "hub" / ("models--" + _MODEL.replace("/", "--"))).exists()


# Once the reranker is cached (the normal case) stay offline: this skips the Hub ping and its
# native "unauthenticated requests" warning. The first run (no cache) downloads online, then
# every run after is offline + silent.
if _model_cached():
    os.environ.setdefault("HF_HUB_OFFLINE", "1")


@lru_cache(maxsize=1)
def _cross_encoder():
    import logging

    for name in ("transformers", "huggingface_hub", "sentence_transformers"):
        logging.getLogger(name).setLevel(logging.ERROR)
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
