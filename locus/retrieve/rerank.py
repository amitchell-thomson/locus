"""Cross-encoder reranking (precision filter) + diversity-aware selection.

The bi-encoder/lexical candidates are coarse; a ms-marco-MiniLM cross-encoder scores each
(query, candidate text) pair jointly. Runs on CPU on purpose (§3/§4): the 8 GB GPU is reserved
for Ollama, so we never let sentence-transformers grab CUDA.

The cut to rerank_top_k is NOT a plain top-k (2026-06-04 evaluation, build step 3): a plain
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


def score_pairs(query: str, texts: list[str]) -> list[float]:
    """Cross-encoder scores for (query, text) pairs.

    Used by the facet-aware confidence check (retrieve/pipeline.py): a multi-part query's
    survivors are rescored against each facet separately, so per-facet coverage is judged
    by the same model that set the floor the full-query scores fell below.
    """
    if not texts:
        return []
    return [float(s) for s in _cross_encoder().predict([(query, t) for t in texts])]


def rerank(
    query: str,
    candidates: list[Candidate],
    top_k: int,
    per_doc_cap: int,
    min_score: float | None = None,
    prefer_code: bool = False,
) -> list[Candidate]:
    """Score candidates with the cross-encoder, then take a diversity-aware top_k cut."""
    if not candidates:
        return []
    scores = _cross_encoder().predict([(query, c.text) for c in candidates])
    for c, s in zip(candidates, scores):
        c.rerank_score = float(s)
    candidates.sort(key=lambda c: c.rerank_score, reverse=True)
    return select(candidates, top_k, per_doc_cap, min_score=min_score, prefer_code=prefer_code)


def _is_source_unit(c: Candidate) -> bool:
    return bool(c.file_path) and c.file_path.endswith(".py")


def select(
    ranked: list[Candidate], top_k: int, per_doc_cap: int, min_score: float | None = None,
    prefer_code: bool = False,
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

    `prefer_code` (set by the pipeline for implementation-intent queries): natural-language-
    poor source chunks embed and rerank below the prose *about* them (round-3 evaluation:
    "how did I implement X" returned design docs, never the function), so when the cut holds
    no source-file unit but a decent one exists (>= min_score), it replaces the lowest-ranked
    selected unit — the §7 "code recall -> function body" contract needs the source present,
    not just described.

    Query-named exemption: a candidate from the 'path' arm exists because the query names
    its file ("how does Locus *rerank*..." -> retrieve/rerank.py). Two rules built for
    breadth misfire on it (round-3 residual): the per-doc cap treats a whole REPO as one
    document, so a repo's prose-about-code fills all its slots before the named file; and
    the section-redundancy demotion fires on the named file's own low-ranked chunks deep in
    the pool, which will never be selected and so re-attach nothing. Path-sourced candidates
    are exempt from both — they still earn their slot on cross-encoder score alone.
    """
    sections_with_units = {
        c.section_id for c in ranked if c.kind in ("proposition", "chunk", "figure")
    }
    selected: list[Candidate] = []
    skipped: list[Candidate] = []
    kind_taken: set[tuple[int | None, str]] = set()
    doc_counts: Counter[int] = Counter()

    for c in ranked:
        if len(selected) >= top_k:
            break
        query_named = "path" in c.sources
        redundant = (
            (c.section_id, c.kind) in kind_taken
            or (
                c.kind == "section"
                and c.section_id in sections_with_units
                and not query_named
            )
        )
        if redundant or (doc_counts[c.doc_id] >= per_doc_cap and not query_named):
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

    if prefer_code and selected and not any(_is_source_unit(c) for c in selected):
        chosen = {id(c) for c in selected}
        promo = next(
            (c for c in ranked if id(c) not in chosen and _is_source_unit(c)
             and not (min_score is not None
                      and (c.rerank_score is None or c.rerank_score < min_score))),
            None,
        )
        if promo is not None:
            if len(selected) >= top_k:  # swap out the lowest-ranked unit, keep the cut full
                selected.remove(max(selected, key=lambda c: rank[id(c)]))
            selected.append(promo)

    selected.sort(key=lambda c: rank[id(c)])  # restore rank order after refill
    return selected
