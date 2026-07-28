"""Top-level retrieval: search -> rerank -> expand -> assemble (§7)."""

from __future__ import annotations

import re
from dataclasses import dataclass, field

from locus.config import load
from locus.db.connection import get_connection
from locus.retrieve.assemble import Citation, assemble
from locus.retrieve.expand import expand
from locus.retrieve.rerank import score_pairs, select
from locus.retrieve.search import Candidate, Facets, rough_doc_ids, search


# How far below the floor a score must sit to read as "definitely irrelevant" rather than
# "weak / partial match". Cross-domain bridge queries score each unit against the FULL
# conjunctive query, so complementary-facet material lands moderately below the floor
# (observed -2..-4) while truly absent topics land far below (observed -6..-9). The margin
# separates the two so the low-confidence banner doesn't overclaim on the headline
# co-retrieve-and-bridge use case (2026-06-05 second evaluation, finding #1).
DEEP_FLOOR_MARGIN = 4.0

# The facet check below rescores survivors against query *fragments* ("regime detection
# in my quant work"), and the cross-encoder scores fragments systematically lower than
# the full natural-language questions `min_rerank_score` was calibrated on. Allowance
# measured on the live corpus (2026-06-05 round-3 fix): units genuinely covering their
# facet score -1.0..+0.9 against it, units not covering it -3.9..-11.4 — a wide gap, so
# a 2-point allowance separates them with plenty of slack on both sides.
FACET_FLOOR_MARGIN = 2.0

# Bridge phrasings that mark a multi-part (cross-domain) query whose halves should be
# judged separately. Deliberately explicit connectives only — a bare "and" splits ordinary
# compound noun phrases, which would mis-fire the facet check on single-topic queries.
_BRIDGE = re.compile(
    r"\b(?:relate[sd]?\s+to|relate\b|connect(?:s|ed)?\s+(?:to|with)|"
    r"compare[sd]?\s+(?:to|with|against)|versus|vs\.?|intersect(?:s|ed)?\s+with|"
    r"link(?:s|ed)?\s+to|combined?\s+with|map(?:s|ped)?\s+onto|inform(?:s|ed)?\b)\b",
    re.IGNORECASE,
)
_BETWEEN = re.compile(r"\bbetween\b(.+?)\band\b(.+)", re.IGNORECASE | re.DOTALL)

# Implementation-intent markers: the consumer wants the function body, not prose about it
# (§7: "code recall -> function body + file:line"). Routed to rerank's prefer_code rule.
_IMPL_INTENT = re.compile(
    r"\b(?:implement(?:ed|ation|s)?|function|method|class|source\s+code|the\s+code\b|"
    r"defined?\s+in|wrote|written|codebase)\b",
    re.IGNORECASE,
)


def split_facets(query: str) -> list[str]:
    """Deterministically split a bridge query into its facets ([] when not bridge-shaped).

    'How does X relate to Y?' -> ['How does X', 'Y?'] -> the substance of each side. Only
    facets with >= 2 words count: the cross-encoder needs enough content per side to score
    meaningfully, and a degenerate split must not unlock the facet path on a simple query.
    """
    m = _BETWEEN.search(query)
    parts = [m.group(1), m.group(2)] if m else _BRIDGE.split(query)
    facets = [p.strip(" \t?.,;:!") for p in parts if p and p.strip()]
    facets = [f for f in facets if len(f.split()) >= 2]
    return facets if len(facets) >= 2 else []


# Correlative / parallel conjunctions marking a query with two distinct facets that a SINGLE
# retrieval pass under-serves: both facets exist in the corpus, but the dominant one
# monopolises the top-k and crowds the other out (unlike _BRIDGE, which is cross-DOMAIN
# vocabulary mismatch). 2026-06-09: "...Fourier analysis used both for signals and for solving
# PDEs" returned only PDE docs — yet the distributed "...used for signals" surfaces the
# Time-Frequency docs at +4.7. Two patterns, "both X and Y" tried first (it strips the "both"
# cleanly); else a repeated preposition ("for X and for Y", "in X and in Y").
_BOTH_AND = re.compile(r"^(.*?)\bboth\b\s+(.+?)\s+\band\b\s+(.+?)([?.!]*)$", re.IGNORECASE | re.DOTALL)
_PREP = r"(?:for|in|of|on|to|with|about|across|over|into|from)"
_PREP_AND = re.compile(
    rf"^(.*?)\b({_PREP})\s+(.+?)\s+and\s+(?:\2)\s+(.+?)([?.!]*)$", re.IGNORECASE | re.DOTALL
)


def decompose_conjunction(query: str) -> list[str]:
    """Two focused sub-queries for a conjunctive ('both X and Y') query, [] otherwise.

    Distributes the shared context over each conjunct so the under-represented facet gets its
    own retrieval pass: 'How is Fourier used both for signals and for solving PDEs?' ->
    ['How is Fourier used for signals?', 'How is Fourier used for solving PDEs?']. Each
    conjunct must be >= 2 words (a degenerate 'both cats and dogs' split must not fire), and
    the two sub-queries must differ from each other and from the original. Deterministic — no
    LLM, no cost. Used only to widen the retrieval pool (multi-query expansion), never to
    relabel confidence.
    """
    m = _BOTH_AND.search(query)
    if m:
        prefix, x, y, tail = m.group(1).strip(), m.group(2).strip(), m.group(3).strip(), m.group(4)
    else:
        m = _PREP_AND.search(query)
        if not m:
            return []
        prefix, prep = m.group(1).strip(), m.group(2)
        x, y, tail = f"{prep} {m.group(3).strip()}", f"{prep} {m.group(4).strip()}", m.group(5)
    if len(x.split()) < 2 or len(y.split()) < 2:
        return []
    head = f"{prefix} " if prefix else ""
    sub1, sub2 = f"{head}{x}{tail}".strip(), f"{head}{y}{tail}".strip()
    lo = query.lower()
    if sub1.lower() == sub2.lower() or sub1.lower() == lo or sub2.lower() == lo:
        return []
    return [sub1, sub2]


@dataclass
class RetrievedFigure:
    """A figure survivor's image hand-off: what generation surfaces (query.py, MCP) need
    to attach the actual image (tier 3, §15.1). At most [figures].image_cap per retrieval,
    best rerank scores first."""

    raw_path: str  # PNG name in the flat raw store
    page: int | None  # 1-based page / slide number
    kind: str | None  # 'raster' | 'vector' | 'slide'
    caption: str | None
    doc_title: str | None
    rerank_score: float | None


@dataclass
class RetrievalResult:
    query: str
    context: str
    citations: list[str] = field(default_factory=list)
    citation_details: list[Citation] = field(default_factory=list)
    survivors: list[Candidate] = field(default_factory=list)
    figures: list[RetrievedFigure] = field(default_factory=list)
    # How many figure units are cited in the context/sources. When > len(figures), the
    # image cap (or the image-attachment floor) truncated — consumers say so explicitly
    # rather than leaving "[figure on p.N]" citations dangling without an image
    # (2026-06-06 audit finding).
    figures_cited: int = 0
    # True when min_rerank_score is configured and even the best survivor falls below it —
    # a signal for the consumer, never a filter of the best material.
    low_confidence: bool = False
    # Which low-confidence story the scores tell: None (confident — including a multi-part
    # query each of whose facets clears the floor separately; see the facet check below),
    # "ambiguous" (best score within DEEP_FLOOR_MARGIN of the floor — weak coverage),
    # "absent" (best score below even the deep floor — the corpus very likely does not
    # cover this).
    confidence_band: str | None = None


def confidence_banner(band: str | None) -> str:
    """The consumer-facing wording for each confidence band ('' when confident).

    Shared by the MCP server, CLI, and query prompt so every surface tells the same —
    honest — story: 'ambiguous' must not overclaim absence (the 2026-06-05 second
    evaluation caught the old single-message banner stamping 'corpus may not cover this'
    on cross-domain synthesis queries whose facets the corpus covers well separately).
    """
    if band == "absent":
        return (
            "LOW CONFIDENCE — every retrieved unit scores far below the relevance floor; "
            "the corpus very likely does not cover this query."
        )
    if band == "ambiguous":
        return (
            "LOW CONFIDENCE — no single retrieved unit clears the relevance floor. For a "
            "multi-part query this often means the corpus covers the parts separately "
            "(check the per-source scores and documents); for a single topic it may "
            "simply be weakly covered."
        )
    return ""


def _excluded_doc_ids(conn, source_uris: list[str]) -> set[int]:
    """Doc ids whose source_uri is in the configured exclusion list (exact match)."""
    if not source_uris:
        return set()
    placeholders = ",".join("?" * len(source_uris))
    rows = conn.execute(
        f"SELECT id FROM documents WHERE source_uri IN ({placeholders})", source_uris
    ).fetchall()
    return {r["id"] for r in rows}


def _score(query: str, candidates: list[Candidate]) -> None:
    """Cross-encoder-score candidates against `query`, in place."""
    if not candidates:
        return
    for c, s in zip(candidates, score_pairs(query, [c.text for c in candidates])):
        c.rerank_score = float(s)


def _apply_maturity_penalty(candidates, rough_ids: set[int], penalty: float) -> None:
    """Down-weight `maturity=rough` candidates by subtracting `penalty` from their cross-encoder
    score, in place (agent-layer §6.1). Applied after all scoring (incl. expansion variants) and
    before the final sort, so a rough note must out-score competitors by `penalty` to still rank —
    a monotonic demotion, not a filter (a dominant rough match still surfaces). No-op when the
    corpus has no rough docs or the penalty is disabled."""
    if not rough_ids or not penalty:
        return
    for c in candidates:
        if c.doc_id in rough_ids and c.rerank_score is not None:
            c.rerank_score -= penalty


def _rerank_with_expansion(query, candidates, gather, cfg, *, prefer_code, rough_ids=None) -> list[Candidate]:
    """Score + diversity-select, expanding the retrieval pool when warranted (multiquery.py).

    Two complementary expansion variant sources, both retrieved separately and merged keeping
    the MAX score per unit (so a unit is ranked by its best-matching phrasing, never demoted by
    the original's):
      - cross-DOMAIN reformulations (LLM, `reformulate`): a bridge query whose target is
        written in another field's vocabulary — gated on `multi_query_k > 0`.
      - facet DECOMPOSITION (deterministic, free, `decompose_conjunction`): a 'both X and Y'
        query whose under-represented facet is crowded out of a single pass — the shared
        context is distributed over each conjunct so each facet gets its own retrieval.
    Expansion triggers on a bridge shape, a conjunction, OR a low-confidence single pass.
    Off / untriggered, this is exactly the old single-pass rerank: score, sort, select.
    """
    floor = cfg.min_rerank_score
    _score(query, candidates)
    base_best = max((c.rerank_score for c in candidates if c.rerank_score is not None), default=None)
    low_conf = floor is not None and base_best is not None and base_best < floor
    facet_subqueries = decompose_conjunction(query)
    expand_on = getattr(cfg, "multi_query_expansion", False) and (
        bool(split_facets(query)) or bool(facet_subqueries) or low_conf
    )
    if expand_on:
        from locus.retrieve.multiquery import reformulate

        variants = list(facet_subqueries)
        if getattr(cfg, "multi_query_k", 0) > 0:
            variants += reformulate(query, cfg.multi_query_k)
        pool: dict[tuple[str, int], Candidate] = {(c.kind, c.id): c for c in candidates}
        for variant in variants:
            vc = gather(variant)
            _score(variant, vc)
            for c in vc:
                key = (c.kind, c.id)
                existing = pool.get(key)
                if existing is None:
                    pool[key] = c
                else:  # same unit found by another variant: keep its best score, union arms
                    existing.sources |= c.sources
                    if (c.rerank_score or float("-inf")) > (existing.rerank_score or float("-inf")):
                        existing.rerank_score = c.rerank_score
        candidates = list(pool.values())
    _apply_maturity_penalty(candidates, rough_ids or set(), getattr(cfg, "rough_penalty", 0.0))
    candidates.sort(
        key=lambda c: c.rerank_score if c.rerank_score is not None else float("-inf"), reverse=True
    )
    return select(candidates, cfg.rerank_top_k, cfg.per_doc_cap, min_score=floor, prefer_code=prefer_code)


def retrieve(
    query: str, conn=None, facets: Facets | None = None, exclude=None,
    *, include_excluded: bool = False,
) -> RetrievalResult:
    """Run the full retrieval pipeline and return assembled context + provenance.

    `facets` optionally restricts retrieval to documents within a date range / category
    (CLAUDE.md §16); None retrieves over the whole corpus.

    Two candidate-pool filters run BEFORE rerank:
      - config `[retrieve].exclude_source_uris` — production exclusion of self-ingestion
        noise (the locus repo's own code/tests/README competing with real queries); skipped
        when `include_excluded=True` (the `--include-excluded` escape hatch).
      - `exclude` — an optional Candidate predicate. Eval-only (the answer-key guard): the
        eval file's chunks contain the labelled queries verbatim and would win the live
        ranking, so the measurement needs them gone from the pool. Never set in production.
    """
    own = conn is None
    if own:
        conn = get_connection(load().paths.db)
    try:
        cfg = load().retrieve
        ex_ids: set[int] = set()
        excluded_uris = getattr(cfg, "exclude_source_uris", None)
        if not include_excluded and excluded_uris:
            ex_ids = _excluded_doc_ids(conn, excluded_uris)

        def _gather(q: str) -> list[Candidate]:
            cands = search(conn, q, facets)
            if ex_ids:
                cands = [c for c in cands if c.doc_id not in ex_ids]
            if exclude is not None:
                cands = [c for c in cands if not exclude(c)]
            return cands

        candidates = _gather(query)
        survivors = _rerank_with_expansion(
            query, candidates, _gather, cfg,
            prefer_code=bool(_IMPL_INTENT.search(query)), rough_ids=rough_doc_ids(conn),
        )
        band: str | None = None
        floor = cfg.min_rerank_score
        scores = [c.rerank_score for c in survivors if c.rerank_score is not None]
        facet_best: dict[int, float] = {}  # id(candidate) -> best per-facet score
        if floor is not None and scores:
            deep = floor - DEEP_FLOOR_MARGIN
            best = max(scores)
            if best < floor:
                # Facet-aware check (round-3 evaluation, two-rounds-standing finding): a
                # bridge query's units each cover ONE side, so every full-query score sits
                # below the floor even when the corpus covers both sides well. Rescore the
                # survivors against each facet separately; if every facet is cleared by
                # some unit, the retrieval is confident — the banner must not stamp "weak
                # coverage" on exactly the co-retrieved set the synthesis use case needs.
                qfacets = split_facets(query)
                facet_floor = floor - FACET_FLOOR_MARGIN
                per_facet_best: list[float] = []
                for f in qfacets:
                    fs = score_pairs(f, [c.text for c in survivors])
                    per_facet_best.append(max(fs, default=float("-inf")))
                    for c, s in zip(survivors, fs):
                        facet_best[id(c)] = max(facet_best.get(id(c), float("-inf")), s)
                if qfacets and all(b >= facet_floor for b in per_facet_best):
                    band = None  # every part separately covered: confident
                elif best < deep:
                    band = "absent"
                else:
                    band = "ambiguous"
            if band is None:
                # Signal exists (full-query or per-facet): enforce the floor we computed —
                # drop the definitely-irrelevant tail instead of printing it as co-equal
                # results (round-3 evaluation, recommendation #3). A unit survives on its
                # full-query score (>= deep floor) or by covering a facet (>= facet floor).
                facet_floor = floor - FACET_FLOOR_MARGIN
                survivors = [
                    c for c in survivors
                    if c.rerank_score is None or c.rerank_score >= deep
                    or facet_best.get(id(c), float("-inf")) >= facet_floor
                ]
        expanded = expand(conn, survivors)
        assembled = assemble(expanded)
        # Figure survivors' image hand-off (tier 3): best-scored first, capped — the
        # consumer attaches the actual PNGs to its (multimodal) generation call.
        # The min_rerank_score floor applies to IMAGE attachment only (2026-06-06 audit:
        # a -0.36 marketing screenshot rode along as a full image block): flag-never-filter
        # still governs the figure's TEXT and citation, but image tokens are not spent on
        # candidates the cross-encoder already judged irrelevant.
        fig_expanded = [e for e in expanded if e.figure_path]
        figures_cited = len(fig_expanded)
        fig_expanded.sort(
            key=lambda e: e.candidate.rerank_score
            if e.candidate.rerank_score is not None
            else float("-inf"),
            reverse=True,
        )
        if floor is not None:
            fig_expanded = [
                e for e in fig_expanded
                if e.candidate.rerank_score is None or e.candidate.rerank_score >= floor
            ]
        figures = [
            RetrievedFigure(
                raw_path=e.figure_path, page=e.figure_page, kind=e.figure_kind,
                caption=e.figure_caption, doc_title=e.doc_title,
                rerank_score=e.candidate.rerank_score,
            )
            for e in fig_expanded[: load().figures.image_cap]
        ]
        return RetrievalResult(
            query=query, context=assembled.text,
            citations=assembled.citations, citation_details=assembled.citation_details,
            survivors=survivors, figures=figures, figures_cited=figures_cited,
            low_confidence=band is not None, confidence_band=band,
        )
    finally:
        if own:
            conn.close()
