"""Top-level retrieval: search -> rerank -> expand -> assemble (§7)."""

from __future__ import annotations

from dataclasses import dataclass, field

from locus.config import load
from locus.db.connection import get_connection
from locus.retrieve.assemble import Citation, assemble
from locus.retrieve.expand import expand
from locus.retrieve.rerank import rerank
from locus.retrieve.search import Candidate, Facets, search


# How far below the floor a score must sit to read as "definitely irrelevant" rather than
# "weak / partial match". Cross-domain bridge queries score each unit against the FULL
# conjunctive query, so complementary-facet material lands moderately below the floor
# (observed -2..-4) while truly absent topics land far below (observed -6..-9). The margin
# separates the two so the low-confidence banner doesn't overclaim on the headline
# co-retrieve-and-bridge use case (2026-06-05 second evaluation, finding #1).
DEEP_FLOOR_MARGIN = 4.0


@dataclass
class RetrievalResult:
    query: str
    context: str
    citations: list[str] = field(default_factory=list)
    citation_details: list[Citation] = field(default_factory=list)
    survivors: list[Candidate] = field(default_factory=list)
    # True when min_rerank_score is configured and even the best survivor falls below it —
    # a signal for the consumer, never a filter of the best material.
    low_confidence: bool = False
    # Which low-confidence story the scores tell: None (confident), "ambiguous" (best score
    # within DEEP_FLOOR_MARGIN of the floor — weak coverage OR a multi-part query whose
    # facets are covered separately), "absent" (best score below even the deep floor — the
    # corpus very likely does not cover this).
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


def retrieve(query: str, conn=None, facets: Facets | None = None) -> RetrievalResult:
    """Run the full retrieval pipeline and return assembled context + provenance.

    `facets` optionally restricts retrieval to documents within a date range / category
    (CLAUDE.md §16); None retrieves over the whole corpus.
    """
    own = conn is None
    if own:
        conn = get_connection(load().paths.db)
    try:
        cfg = load().retrieve
        candidates = search(conn, query, facets)
        survivors = rerank(
            query, candidates, cfg.rerank_top_k, cfg.per_doc_cap,
            min_score=cfg.min_rerank_score,
        )
        band: str | None = None
        floor = cfg.min_rerank_score
        scores = [c.rerank_score for c in survivors if c.rerank_score is not None]
        if floor is not None and scores:
            deep = floor - DEEP_FLOOR_MARGIN
            best = max(scores)
            if best < deep:
                band = "absent"
            elif best < floor:
                band = "ambiguous"
            else:
                # Signal exists: drop only the definitely-irrelevant tail (below the deep
                # floor). Moderate-negative units survive — on multi-part queries they are
                # the complementary facets the synthesis use case needs.
                survivors = [
                    c for c in survivors
                    if c.rerank_score is None or c.rerank_score >= deep
                ]
        assembled = assemble(expand(conn, survivors))
        return RetrievalResult(
            query=query, context=assembled.text,
            citations=assembled.citations, citation_details=assembled.citation_details,
            survivors=survivors, low_confidence=band is not None, confidence_band=band,
        )
    finally:
        if own:
            conn.close()
