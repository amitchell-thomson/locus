"""Cross-encoder reranking (precision filter) + diversity-aware selection.

The bi-encoder/lexical candidates are coarse; a ms-marco-MiniLM cross-encoder scores each
(query, candidate text) pair jointly. Runs on CPU on purpose (§3/§4): the 8 GB GPU is reserved
for Ollama, so we never let sentence-transformers grab CUDA.

The cut to rerank_top_k is NOT a plain top-k (2026-06-04 evaluation, PLAN.md step 3): a plain
cut lets one section occupy several slots (its proposition, chunk, and summary are separate
candidates) and lets the single best-matching document monopolise the whole top-k, which breaks
cross-domain synthesis queries. `select()` applies redundancy + diversity rules, relaxed only
when they would leave the top-k underfull.

Requires the `rerank` extra (sentence-transformers + torch): `uv sync --extra rerank`.
"""

from __future__ import annotations

import os
from collections import Counter
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


def rerank(
    query: str,
    candidates: list[Candidate],
    top_k: int,
    per_doc_cap: int,
    min_score: float | None = None,
) -> list[Candidate]:
    """Score candidates with the cross-encoder, then take a diversity-aware top_k cut."""
    if not candidates:
        return []
    scores = _cross_encoder().predict([(query, c.text) for c in candidates])
    for c, s in zip(candidates, scores):
        c.rerank_score = float(s)
    candidates.sort(key=lambda c: c.rerank_score, reverse=True)
    return select(candidates, top_k, per_doc_cap, min_score=min_score)


def select(
    ranked: list[Candidate], top_k: int, per_doc_cap: int, min_score: float | None = None
) -> list[Candidate]:
    """Diversity-aware cut of a rank-ordered candidate list (best first).

    Rules, each motivated by what the assembled context actually gains per slot:
      - one unit per (section_id, kind): a section's 2nd-best chunk adds little beyond its
        best one, but costs a slot another section or document could use;
      - a 'section' (summary) candidate is redundant when any proposition/chunk of the same
        section is in the pool: expansion re-attaches the summary to the child for free;
      - at most per_doc_cap units per document, so one best-matching document cannot
        monopolise the top-k (cross-domain queries need the *set*, not the single best doc).

    The rules are soft: if they leave fewer than top_k selected, remaining slots are refilled
    with the best skipped candidates, so this never returns fewer results than a plain top-k
    would. Output is in rank (score) order.

    `min_score` bounds the *refill only*: relaxing a diversity cap to keep the top-k full is
    justified for a decent candidate, not for one the cross-encoder already judged irrelevant
    (the 2026-06-05 evaluation: negative-control queries padded the top-k with noise). The
    primary pass is NOT score-filtered — weak-but-best results still surface, and the pipeline
    flags low confidence instead of hiding them (flag, never filter).
    """
    sections_with_units = {
        c.section_id for c in ranked if c.kind in ("proposition", "chunk")
    }
    selected: list[Candidate] = []
    skipped: list[Candidate] = []
    kind_taken: set[tuple[int | None, str]] = set()
    doc_counts: Counter[int] = Counter()

    for c in ranked:
        if len(selected) >= top_k:
            break
        redundant = (
            (c.section_id, c.kind) in kind_taken
            or (c.kind == "section" and c.section_id in sections_with_units)
        )
        if redundant or doc_counts[c.doc_id] >= per_doc_cap:
            skipped.append(c)
            continue
        selected.append(c)
        kind_taken.add((c.section_id, c.kind))
        doc_counts[c.doc_id] += 1

    for c in skipped:  # relax the caps rather than return an underfull top-k
        if len(selected) >= top_k:
            break
        if min_score is not None and (c.rerank_score is None or c.rerank_score < min_score):
            continue  # an underfull top-k beats padding it with judged-irrelevant candidates
        selected.append(c)

    rank = {id(c): i for i, c in enumerate(ranked)}
    selected.sort(key=lambda c: rank[id(c)])  # restore rank order after refill
    return selected
