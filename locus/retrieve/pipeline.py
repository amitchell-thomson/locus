"""Top-level retrieval: search -> rerank -> expand -> assemble (§7)."""

from __future__ import annotations

from dataclasses import dataclass, field

from locus.config import load
from locus.db.connection import get_connection
from locus.retrieve.assemble import Citation, assemble
from locus.retrieve.expand import expand
from locus.retrieve.rerank import rerank
from locus.retrieve.search import Candidate, Facets, search


@dataclass
class RetrievalResult:
    query: str
    context: str
    citations: list[str] = field(default_factory=list)
    citation_details: list[Citation] = field(default_factory=list)
    survivors: list[Candidate] = field(default_factory=list)
    # True when min_rerank_score is configured and even the best survivor falls below it —
    # the corpus likely does not cover this query. A signal for the consumer, never a filter.
    low_confidence: bool = False


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
        assembled = assemble(expand(conn, survivors))
        scores = [c.rerank_score for c in survivors if c.rerank_score is not None]
        low_confidence = (
            cfg.min_rerank_score is not None
            and bool(scores)
            and max(scores) < cfg.min_rerank_score
        )
        return RetrievalResult(
            query=query, context=assembled.text,
            citations=assembled.citations, citation_details=assembled.citation_details,
            survivors=survivors, low_confidence=low_confidence,
        )
    finally:
        if own:
            conn.close()
