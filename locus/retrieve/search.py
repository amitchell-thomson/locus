"""Candidate generation: dense KNN (propositions/chunks/sections) + lexical FTS5 + entities.

Hybrid retrieval (§7 + §15.2): the dense arms match on meaning; the lexical (BM25) arm catches
exact symbols/terms/names that a general embedder blurs; the entity arm boosts sections naming
a query entity. Candidates are merged and de-duplicated by (kind, id); scores are NOT comparable
across arms — the cross-encoder rerank (rerank.py) is what actually orders them.
"""

from __future__ import annotations

import re
import struct
from dataclasses import dataclass, field

from locus.config import load
from locus.ingest.embed import embed_text

# Entity-arm bounds: ignore very short names, and cap how many entity candidates we add.
_MIN_ENTITY_LEN = 4
_MAX_ENTITY_CANDIDATES = 10


@dataclass
class Candidate:
    kind: str  # 'proposition' | 'chunk' | 'section'
    id: int
    doc_id: int
    section_id: int | None
    text: str
    score: float  # arm-specific (vec distance or bm25); not cross-arm comparable
    sources: set[str] = field(default_factory=set)  # 'dense' | 'lexical' | 'entity'
    rerank_score: float | None = None


def _vec_blob(vec: list[float]) -> bytes:
    return struct.pack(f"{len(vec)}f", *vec)


def _fts_query(query: str) -> str | None:
    """Build a safe FTS5 MATCH string: OR of the query's word tokens (recall + bm25 ranks)."""
    tokens = re.findall(r"\w+", query.lower())
    return " OR ".join(tokens) if tokens else None


def search(conn, query: str) -> list[Candidate]:
    cfg = load().retrieve
    qvec = _vec_blob(embed_text(query))
    out: dict[tuple[str, int], Candidate] = {}

    def add(kind, id_, doc_id, section_id, text, score, source):
        key = (kind, id_)
        if key in out:
            out[key].sources.add(source)
            out[key].score = min(out[key].score, score)  # keep best (smaller is better)
        else:
            out[key] = Candidate(kind, id_, doc_id, section_id, text or "", score, {source})

    # --- dense KNN (vec0 KNN in a subquery, then join — the sqlite-vec join pattern) ---
    for r in conn.execute(
        "SELECT p.id, p.doc_id, p.section_id, p.text, k.distance FROM "
        "(SELECT proposition_id, distance FROM proposition_vectors "
        " WHERE embedding MATCH ? ORDER BY distance LIMIT ?) k "
        "JOIN propositions p ON p.id = k.proposition_id",
        (qvec, cfg.proposition_top_k),
    ):
        add("proposition", r["id"], r["doc_id"], r["section_id"], r["text"], r["distance"], "dense")

    for r in conn.execute(
        "SELECT c.id, c.doc_id, c.section_id, c.raw_text, k.distance FROM "
        "(SELECT chunk_id, distance FROM chunk_vectors "
        " WHERE embedding MATCH ? ORDER BY distance LIMIT ?) k "
        "JOIN chunks c ON c.id = k.chunk_id",
        (qvec, cfg.fine_top_k),
    ):
        add("chunk", r["id"], r["doc_id"], r["section_id"], r["raw_text"], r["distance"], "dense")

    for r in conn.execute(
        "SELECT s.id, s.doc_id, s.summary, k.distance FROM "
        "(SELECT section_id, distance FROM section_vectors "
        " WHERE embedding MATCH ? ORDER BY distance LIMIT ?) k "
        "JOIN sections s ON s.id = k.section_id",
        (qvec, cfg.section_top_k),
    ):
        add("section", r["id"], r["doc_id"], r["id"], r["summary"], r["distance"], "dense")

    # --- lexical (FTS5 / BM25) over chunk text ---
    fq = _fts_query(query)
    if fq:
        for r in conn.execute(
            "SELECT c.id, c.doc_id, c.section_id, c.raw_text, bm25(chunks_fts) AS s "
            "FROM chunks_fts JOIN chunks c ON c.id = chunks_fts.rowid "
            "WHERE chunks_fts MATCH ? ORDER BY s LIMIT ?",
            (fq, cfg.fine_top_k),
        ):
            add("chunk", r["id"], r["doc_id"], r["section_id"], r["raw_text"], r["s"], "lexical")

    # --- entity-anchored: sections naming an entity that appears in the query ---
    q_lower = query.lower()
    added = 0
    for r in conn.execute(
        "SELECT DISTINCT e.name, e.section_id, e.doc_id, s.summary "
        "FROM entities e JOIN sections s ON s.id = e.section_id"
    ):
        if added >= _MAX_ENTITY_CANDIDATES:
            break
        name = r["name"] or ""
        if len(name) >= _MIN_ENTITY_LEN and name.lower() in q_lower:
            add("section", r["section_id"], r["doc_id"], r["section_id"], r["summary"], 0.0, "entity")
            added += 1

    return list(out.values())
