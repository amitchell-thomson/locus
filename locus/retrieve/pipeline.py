"""Top-level retrieval: search -> rerank -> expand -> assemble (§7)."""

from __future__ import annotations

import re
from dataclasses import dataclass, field

from locus.config import load
from locus.db.connection import get_connection
from locus.retrieve.assemble import Citation, assemble
from locus.retrieve.expand import expand
from locus.retrieve.rerank import rerank, score_pairs
from locus.retrieve.search import Candidate, Facets, search


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


def retrieve(
    query: str, conn=None, facets: Facets | None = None, exclude=None
) -> RetrievalResult:
    """Run the full retrieval pipeline and return assembled context + provenance.

    `facets` optionally restricts retrieval to documents within a date range / category
    (CLAUDE.md §16); None retrieves over the whole corpus.

    `exclude` is an optional Candidate predicate dropped BEFORE rerank. Eval-only (the
    answer-key guard): since the locus repo self-ingested, chunks of the eval file contain
    the labelled queries verbatim and (correctly) win the live ranking — post-hoc scoring
    exclusion left them consuming top-k slots and crowding out the real targets, so the
    measurement needs them gone from the candidate pool. Never set in production paths.
    """
    own = conn is None
    if own:
        conn = get_connection(load().paths.db)
    try:
        cfg = load().retrieve
        candidates = search(conn, query, facets)
        if exclude is not None:
            candidates = [c for c in candidates if not exclude(c)]
        survivors = rerank(
            query, candidates, cfg.rerank_top_k, cfg.per_doc_cap,
            min_score=cfg.min_rerank_score,
            prefer_code=bool(_IMPL_INTENT.search(query)),
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
