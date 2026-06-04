"""Retrieval pipeline (§7): search -> rerank -> expand -> assemble.

retrieve() runs the full pipeline and returns assembled, budget-capped context plus provenance,
ready to hand to a single Claude generation call (Stage 7).
"""

from locus.retrieve.pipeline import RetrievalResult, retrieve
from locus.retrieve.search import Facets

__all__ = ["retrieve", "RetrievalResult", "Facets"]
