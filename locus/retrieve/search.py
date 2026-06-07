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
from locus.link.related import aliases_built

# Entity-arm bounds: ignore very short names, and cap how many entity candidates we add.
_MIN_ENTITY_LEN = 4
_MAX_ENTITY_CANDIDATES = 10

# Path-arm bounds (round-3 evaluation: source files under-retrieve against the prose about
# them — "the evaluation module" must surface regimes/evaluation.py). File stems too generic
# to identify a file are skipped: they name a *convention*, not the file the query means.
_MAX_PATH_CANDIDATES = 8
_PATH_STEM_STOP = frozenset(
    "main app cli run test tests base init core docs util utils common config "
    "setup types data index readme claude plan license makefile".split()
)


@dataclass
class Candidate:
    kind: str  # 'proposition' | 'chunk' | 'section' | 'figure'
    id: int
    doc_id: int
    section_id: int | None
    text: str
    score: float  # arm-specific (vec distance or bm25); not cross-arm comparable
    sources: set[str] = field(default_factory=set)  # 'dense' | 'lexical' | 'entity' | 'path'
    rerank_score: float | None = None
    file_path: str | None = None  # code provenance; lets selection prefer source units


@dataclass
class Facets:
    """Document-level retrieval filters (CLAUDE.md §16). All optional; unset = no restriction.

    `since`/`until` are inclusive ISO 'YYYY-MM-DD' bounds on documents.source_date (string
    comparison is correct for zero-padded ISO dates). `category` is an exact match. A document
    with a NULL source_date is excluded by any date bound — the correct semantics for an
    unknown date.
    """

    since: str | None = None
    until: str | None = None
    category: str | None = None

    def active(self) -> bool:
        return bool(self.since or self.until or self.category)


def _vec_blob(vec: list[float]) -> bytes:
    return struct.pack(f"{len(vec)}f", *vec)


def _eligible_docs(conn, facets: Facets | None) -> set[int] | None:
    """Doc ids matching the facets, or None when no facet is set (meaning: no restriction)."""
    if facets is None or not facets.active():
        return None
    clauses: list[str] = []
    params: list[str] = []
    if facets.since:
        clauses.append("source_date >= ?")
        params.append(facets.since)
    if facets.until:
        clauses.append("source_date <= ?")
        params.append(facets.until)
    if facets.category:
        clauses.append("category = ?")
        params.append(facets.category)
    sql = "SELECT id FROM documents WHERE " + " AND ".join(clauses)
    return {r["id"] for r in conn.execute(sql, params)}


def _fts_query(query: str) -> str | None:
    """Build a safe FTS5 MATCH string: OR of the query's word tokens (recall + bm25 ranks)."""
    tokens = re.findall(r"\w+", query.lower())
    return " OR ".join(tokens) if tokens else None


def search(conn, query: str, facets: Facets | None = None) -> list[Candidate]:
    cfg = load().retrieve
    qvec = _vec_blob(embed_text(query))
    eligible = _eligible_docs(conn, facets)  # None => unrestricted
    out: dict[tuple[str, int], Candidate] = {}

    def add(kind, id_, doc_id, section_id, text, score, source, file_path=None):
        if eligible is not None and doc_id not in eligible:
            return  # outside the active date/category facets
        key = (kind, id_)
        if key in out:
            out[key].sources.add(source)
            out[key].score = min(out[key].score, score)  # keep best (smaller is better)
        else:
            out[key] = Candidate(
                kind, id_, doc_id, section_id, text or "", score, {source},
                file_path=file_path,
            )

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
        "SELECT c.id, c.doc_id, c.section_id, c.raw_text, c.file_path, k.distance FROM "
        "(SELECT chunk_id, distance FROM chunk_vectors "
        " WHERE embedding MATCH ? ORDER BY distance LIMIT ?) k "
        "JOIN chunks c ON c.id = k.chunk_id",
        (qvec, cfg.fine_top_k),
    ):
        add("chunk", r["id"], r["doc_id"], r["section_id"], r["raw_text"], r["distance"],
            "dense", file_path=r["file_path"])

    for r in conn.execute(
        "SELECT s.id, s.doc_id, s.summary, s.file_path, k.distance FROM "
        "(SELECT section_id, distance FROM section_vectors "
        " WHERE embedding MATCH ? ORDER BY distance LIMIT ?) k "
        "JOIN sections s ON s.id = k.section_id",
        (qvec, cfg.section_top_k),
    ):
        add("section", r["id"], r["doc_id"], r["id"], r["summary"], r["distance"], "dense",
            file_path=r["file_path"])

    # Figures (step 11): the indexed text is caption+description (ingest/figures.index_text),
    # which is also what the cross-encoder scores. The image itself attaches at generation.
    for r in conn.execute(
        "SELECT f.id, f.doc_id, f.section_id, f.caption, f.description, k.distance FROM "
        "(SELECT figure_id, distance FROM figure_vectors "
        " WHERE embedding MATCH ? ORDER BY distance LIMIT ?) k "
        "JOIN figures f ON f.id = k.figure_id",
        (qvec, cfg.figure_top_k),
    ):
        text = "\n".join(p for p in (r["caption"], r["description"]) if p)
        add("figure", r["id"], r["doc_id"], r["section_id"], text, r["distance"], "dense")

    # --- lexical (FTS5 / BM25) over chunk text ---
    fq = _fts_query(query)
    if fq:
        for r in conn.execute(
            "SELECT c.id, c.doc_id, c.section_id, c.raw_text, c.file_path, bm25(chunks_fts) AS s "
            "FROM chunks_fts JOIN chunks c ON c.id = chunks_fts.rowid "
            "WHERE chunks_fts MATCH ? ORDER BY s LIMIT ?",
            (fq, cfg.fine_top_k),
        ):
            add("chunk", r["id"], r["doc_id"], r["section_id"], r["raw_text"], r["s"],
                "lexical", file_path=r["file_path"])

    # --- path-anchored: code sections whose file stem is named in the query ---
    # NL-poor source chunks rank below the prose ABOUT them in both dense and lexical arms
    # (2026-06-05 round-3 evaluation), so a query that names a module ("the evaluation
    # module", "how does locus rerank...") may never get the file into the pool at all —
    # and selection cannot prefer what search never surfaced. The candidate text is the
    # section summary (grounded signature summaries since round 3), which the
    # cross-encoder can score honestly against the query.
    q_lower = query.lower()
    q_words = set(re.findall(r"[a-z0-9_]+", q_lower))
    path_added = 0
    for r in conn.execute(
        "SELECT id, doc_id, summary, file_path FROM sections "
        "WHERE file_path IS NOT NULL ORDER BY length(file_path) DESC"
    ):
        if path_added >= _MAX_PATH_CANDIDATES:
            break
        name = r["file_path"].rsplit("/", 1)[-1]
        stem = name.rsplit(".", 1)[0].lower()
        if len(stem) < 3 or stem in _PATH_STEM_STOP:
            continue
        if stem in q_words or (("_" in stem) and stem.replace("_", " ") in q_lower):
            add("section", r["id"], r["doc_id"], r["id"], r["summary"], 0.0, "path",
                file_path=r["file_path"])
            path_added += 1

    # --- entity-anchored: sections naming an entity that appears in the query ---
    # With the alias substrate built (`locus link`, step 12), matching is canonical-group
    # aware: a query naming ANY variant ("KL divergence") surfaces sections naming any
    # sibling variant ("Kullback-Leibler (KL) divergence"). Before the first link run the
    # table is empty and matching falls back to raw entity names (pre-step-12 behaviour).
    added = 0
    if aliases_built(conn):
        matched: set[tuple[str, str]] = set()
        for r in conn.execute(
            "SELECT variant_name, canonical_name, canonical_type FROM entity_aliases"
        ):
            name = r["variant_name"] or ""
            if len(name) >= _MIN_ENTITY_LEN and name.lower() in q_lower:
                matched.add((r["canonical_name"], r["canonical_type"]))
        for canon_name, canon_type in sorted(matched):  # deterministic under the cap
            if added >= _MAX_ENTITY_CANDIDATES:
                break
            for r in conn.execute(
                "SELECT DISTINCT e.section_id, e.doc_id, s.summary, s.file_path "
                "FROM entity_aliases a "
                "JOIN entities e ON e.name = a.variant_name AND e.type = a.variant_type "
                "JOIN sections s ON s.id = e.section_id "
                "WHERE a.canonical_name = ? AND a.canonical_type = ?",
                (canon_name, canon_type),
            ):
                if added >= _MAX_ENTITY_CANDIDATES:
                    break
                add("section", r["section_id"], r["doc_id"], r["section_id"], r["summary"],
                    0.0, "entity", file_path=r["file_path"])
                added += 1
    else:
        for r in conn.execute(
            "SELECT DISTINCT e.name, e.section_id, e.doc_id, s.summary, s.file_path "
            "FROM entities e JOIN sections s ON s.id = e.section_id"
        ):
            if added >= _MAX_ENTITY_CANDIDATES:
                break
            name = r["name"] or ""
            if len(name) >= _MIN_ENTITY_LEN and name.lower() in q_lower:
                add("section", r["section_id"], r["doc_id"], r["section_id"], r["summary"], 0.0,
                    "entity", file_path=r["file_path"])
                added += 1

    return list(out.values())
