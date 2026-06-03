"""Top-level retrieval: search -> rerank -> expand -> assemble (§7)."""

from __future__ import annotations

from dataclasses import dataclass, field

from locus.config import load
from locus.db.connection import get_connection
from locus.retrieve.assemble import assemble
from locus.retrieve.expand import expand
from locus.retrieve.rerank import rerank
from locus.retrieve.search import Candidate, search


@dataclass
class RetrievalResult:
    query: str
    context: str
    citations: list[str] = field(default_factory=list)
    survivors: list[Candidate] = field(default_factory=list)


def retrieve(query: str, conn=None) -> RetrievalResult:
    """Run the full retrieval pipeline and return assembled context + provenance."""
    own = conn is None
    if own:
        conn = get_connection(load().paths.db)
    try:
        cfg = load().retrieve
        candidates = search(conn, query)
        survivors = rerank(query, candidates, cfg.rerank_top_k)
        assembled = assemble(expand(conn, survivors))
        return RetrievalResult(
            query=query, context=assembled.text,
            citations=assembled.citations, survivors=survivors,
        )
    finally:
        if own:
            conn.close()
