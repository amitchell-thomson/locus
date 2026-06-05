"""Ingest orchestration: one PDF -> a fully populated 3-level DB (CLAUDE.md §6).

Per file:
  hash -> skip if the content_hash already exists (idempotent)
       -> copy the raw file into the flat raw store
       -> extract (pymupdf) -> ordered sections
       -> per section: summary + propositions + entities + chunks  (local LLM + embeddings)
       -> doc: synthesis (thesis/method/result/limitations) + gap flags
       -> write L1 + L2 + L3 + entities in ONE transaction (atomic; rolls back on error)

Any failure quarantines that single file (logged, returned in the result) without aborting a
batch and without leaving a partial document (the write is transactional, and all expensive
work happens before the write opens).

Deferred (noted in §6): cross_section_deps and tags are not yet produced, so those columns
are written empty / left unpopulated.
"""

from __future__ import annotations

import hashlib
import json
import logging
import shutil
import struct
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path

from locus.config import load
from locus.db.connection import get_connection
from locus.extract import pdf as pdf_extract
from locus.ingest import chunk, embed, entities, gaps, llm, propositions, summarize, synthesis

log = logging.getLogger(__name__)


@dataclass
class IngestResult:
    path: str
    status: str  # "ingested" | "skipped" | "quarantined" | "unsupported"
    doc_id: int | None = None
    sections: int = 0
    chunks: int = 0
    propositions: int = 0
    entities: int = 0
    error: str | None = None


# --- small helpers -----------------------------------------------------------------------


def _hash_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def _source_type(path: Path) -> str | None:
    return "pdf" if path.suffix.lower() == ".pdf" else None  # code/video added later


# Canonical kind names (CLAUDE.md §15.4/§16): folder names matching one of these (singular
# or plural) are normalized to it, so the core vocabulary stays stable across drops.
CATEGORIES = {"paper", "project", "achievement", "note", "cv", "code", "video"}


def _category(path: Path) -> str:
    """Derive a category from the drop location. The owner's folder taxonomy is authoritative.

    Primary rule: the first folder under vault/incoming/ IS the category, free-form —
    `incoming/engineering/x.pdf` → 'engineering', `incoming/papers/y.pdf` → 'paper' (known
    kinds are singularized so the §15.4 vocabulary stays canonical). The laptop outbox rsyncs
    subfolders verbatim, so sorting once in the local drop folder carries all the way through.
    Files ingested from outside incoming/ fall back to scanning path parts for a known kind;
    anything else is 'uncategorized'.
    """
    try:
        rel = path.resolve().relative_to(load().paths.incoming)
        if len(rel.parts) > 1:  # at least one folder between incoming/ and the file
            top = rel.parts[0].lower()
            norm = top.rstrip("s")  # papers -> paper, notes -> note
            return norm if norm in CATEGORIES else top
    except ValueError:
        pass  # not under incoming/ — use the known-kind scan below
    for part in path.parts:
        norm = part.lower().rstrip("s")
        if norm in CATEGORIES:
            return norm
    return "uncategorized"


def _mtime_date(path: Path) -> str:
    """File modification time as an ISO date — the fallback when no embedded date exists."""
    return datetime.fromtimestamp(path.stat().st_mtime, tz=timezone.utc).date().isoformat()


def _vec_blob(vec: list[float]) -> bytes:
    """Pack a float vector as the little-endian float32 blob sqlite-vec stores."""
    return struct.pack(f"{len(vec)}f", *vec)


def _copy_to_raw(path: Path, content_hash: str) -> str:
    """Copy the source into the flat raw store under a hash-named file; return that name."""
    raw_store = load().paths.raw_store
    raw_store.mkdir(parents=True, exist_ok=True)
    name = f"{content_hash}{path.suffix.lower()}"
    dest = raw_store / name
    if not dest.exists():
        shutil.copy2(path, dest)
    return name


# --- the heavy, no-DB work: extract -> LLM passes -> embeddings ---------------------------


@dataclass
class _PreparedSection:
    position: int
    title: str | None
    page_start: int
    page_end: int
    summary: str
    summary_vec: list[float]
    chunks: list[str]
    chunk_vecs: list[list[float]]
    propositions: list[str]
    proposition_vecs: list[list[float]]
    entities: list  # list[entities.Entity]


@dataclass
class _Prepared:
    title: str | None
    source_date: str | None  # ISO 'YYYY-MM-DD' from the source's metadata; None if absent
    synthesis: synthesis.DocSynthesis
    gaps: list[str]
    sections: list[_PreparedSection]
    embed_model: str


def _prepare(path: Path) -> _Prepared:
    # mathocr=True: the pipeline (unlike ad-hoc extraction) runs the math-OCR pass on pages
    # the damage/math detector flags, per config [mathocr] (engine='off' disables).
    doc = pdf_extract.extract_pdf(path, mathocr=True)
    # If the OCR engine's VRAM forced Ollama to load the ingest model split across CPU/GPU,
    # evict it now (no request in flight) so the passes below run it fully on the GPU.
    llm.unload_if_split()
    prepared: list[_PreparedSection] = []
    summaries: list[str] = []

    for sec in doc.sections:
        summary = summarize.summarize_section(sec.title, sec.text)
        props = propositions.extract_propositions(sec.title, sec.text)
        ents = entities.extract_entities(sec.title, sec.text)
        chunks = chunk.chunk_text(sec.text)
        prepared.append(
            _PreparedSection(
                position=sec.position,
                title=sec.title,
                page_start=sec.page_start,
                page_end=sec.page_end,
                summary=summary,
                summary_vec=embed.embed_text(summary),
                chunks=chunks,
                chunk_vecs=embed.embed_texts(chunks),
                propositions=props,
                proposition_vecs=embed.embed_texts(props),
                entities=ents,
            )
        )
        summaries.append(summary)
        log.info("section %s/%s done (%s)", sec.position + 1, len(doc.sections), sec.title)

    # Doc-level entity hygiene: collapse plural/singular surface variants (evidence-based).
    entities.merge_plural_variants([p.entities for p in prepared])

    syn = synthesis.synthesize_document(doc.title, summaries)
    context = (
        f"Thesis: {syn.thesis}\nMethod: {syn.method}\n"
        f"Result: {syn.result}\nLimitations: {syn.limitations}"
    )
    gap_list = gaps.flag_gaps(doc.title, context)
    # Persist the math-OCR audit trail (extract/mathocr.py): pages where QC kept the original
    # text are extraction gaps in the §15.0 sense — content quality was knowingly degraded
    # there, and that must be queryable, not just logged.
    gap_list += [f"math-OCR kept original text on {f}" for f in doc.ocr_fallbacks]
    if doc.ocr_replaced:
        log.info("math-OCR replaced %d page(s); %d fallback(s)",
                 len(doc.ocr_replaced), len(doc.ocr_fallbacks))
    return _Prepared(doc.title, doc.source_date, syn, gap_list, prepared, embed.embedding_model())


# --- transactional write -----------------------------------------------------------------


def _write(conn, path: Path, content_hash: str, source_type: str, raw_name: str, prep: _Prepared) -> IngestResult:
    section_map = [
        {"position": s.position, "title": s.title, "page_start": s.page_start, "page_end": s.page_end}
        for s in prep.sections
    ]
    # Embedded source date if the extractor found one, else the file's mtime (always non-null).
    source_date = prep.source_date or _mtime_date(path)
    category = _category(path)
    n_chunks = n_props = n_ents = 0

    with conn:  # one transaction: commit on success, rollback on any error
        cur = conn.execute(
            "INSERT INTO documents (content_hash, source_type, source_uri, raw_path, title, "
            "ingest_model, source_date, category, thesis, method, result, limitations, "
            "section_map, gap_flags) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            (
                content_hash, source_type, str(path), raw_name, prep.title, llm.ingest_model(),
                source_date, category,
                prep.synthesis.thesis, prep.synthesis.method, prep.synthesis.result,
                prep.synthesis.limitations, json.dumps(section_map), json.dumps(prep.gaps),
            ),
        )
        doc_id = cur.lastrowid

        for s in prep.sections:
            cur = conn.execute(
                "INSERT INTO sections (doc_id, position, title, summary, cross_section_deps, "
                "file_path, call_graph) VALUES (?,?,?,?,?,?,?)",
                (doc_id, s.position, s.title, s.summary, None, None, None),
            )
            section_id = cur.lastrowid
            conn.execute(
                "INSERT INTO section_vectors (section_id, embedding) VALUES (?,?)",
                (section_id, _vec_blob(s.summary_vec)),
            )
            for i, (text, vec) in enumerate(zip(s.chunks, s.chunk_vecs)):
                cur = conn.execute(
                    "INSERT INTO chunks (section_id, doc_id, position, raw_text, embed_model) "
                    "VALUES (?,?,?,?,?)",
                    (section_id, doc_id, i, text, prep.embed_model),
                )
                conn.execute(
                    "INSERT INTO chunk_vectors (chunk_id, embedding) VALUES (?,?)",
                    (cur.lastrowid, _vec_blob(vec)),
                )
                n_chunks += 1
            for i, (text, vec) in enumerate(zip(s.propositions, s.proposition_vecs)):
                cur = conn.execute(
                    "INSERT INTO propositions (section_id, doc_id, position, text, embed_model) "
                    "VALUES (?,?,?,?,?)",
                    (section_id, doc_id, i, text, prep.embed_model),
                )
                conn.execute(
                    "INSERT INTO proposition_vectors (proposition_id, embedding) VALUES (?,?)",
                    (cur.lastrowid, _vec_blob(vec)),
                )
                n_props += 1
            for e in s.entities:
                cur = conn.execute(
                    "INSERT OR IGNORE INTO entities (doc_id, section_id, name, type) "
                    "VALUES (?,?,?,?)",
                    (doc_id, section_id, e.name, str(e.type)),
                )
                n_ents += max(0, cur.rowcount)

    return IngestResult(
        str(path), "ingested", doc_id=doc_id, sections=len(prep.sections),
        chunks=n_chunks, propositions=n_props, entities=n_ents,
    )


# --- public entry points -----------------------------------------------------------------


def delete_document(conn, doc_id: int) -> None:
    """Delete a document and all of its rows.

    The L1/L2/L3 + entity tables cascade via ON DELETE CASCADE, but the sqlite-vec `vec0`
    vector tables are virtual and carry NO foreign key, so their rows must be removed
    explicitly — otherwise vectors orphan and a reused row id later collides on insert.
    """
    with conn:
        sids = [r["id"] for r in conn.execute("SELECT id FROM sections WHERE doc_id=?", (doc_id,))]
        cids = [r["id"] for r in conn.execute("SELECT id FROM chunks WHERE doc_id=?", (doc_id,))]
        pids = [r["id"] for r in conn.execute("SELECT id FROM propositions WHERE doc_id=?", (doc_id,))]
        conn.executemany("DELETE FROM section_vectors WHERE section_id=?", [(i,) for i in sids])
        conn.executemany("DELETE FROM chunk_vectors WHERE chunk_id=?", [(i,) for i in cids])
        conn.executemany("DELETE FROM proposition_vectors WHERE proposition_id=?", [(i,) for i in pids])
        conn.execute("DELETE FROM documents WHERE id=?", (doc_id,))


def ingest_file(path: str | Path, conn, *, reingest: bool = False) -> IngestResult:
    """Ingest one file into the given DB connection. Never raises: failures are quarantined.

    If `reingest` is True, an already-present document (same content hash) is deleted and
    rebuilt instead of skipped — used to re-run an updated pipeline over existing files.
    """
    path = Path(path)
    if not path.exists():
        return IngestResult(str(path), "quarantined", error="file not found")
    source_type = _source_type(path)
    if source_type is None:
        return IngestResult(str(path), "unsupported", error=f"unsupported type: {path.suffix}")
    try:
        content_hash = _hash_file(path)
        existing = conn.execute(
            "SELECT id FROM documents WHERE content_hash=?", (content_hash,)
        ).fetchone()
        if existing:
            if not reingest:
                return IngestResult(str(path), "skipped", doc_id=existing["id"])
            delete_document(conn, existing["id"])

        raw_name = _copy_to_raw(path, content_hash)
        prep = _prepare(path)
        return _write(conn, path, content_hash, source_type, raw_name, prep)
    except Exception as exc:  # quarantine this doc, keep the batch alive (§6)
        log.warning("Quarantined %s: %s", path, exc)
        return IngestResult(str(path), "quarantined", error=str(exc))


def ingest_paths(paths: list[str | Path], conn=None, *, reingest: bool = False) -> list[IngestResult]:
    """Ingest several files into the configured DB (or a provided connection)."""
    own = conn is None
    if own:
        conn = get_connection(load().paths.db)
    try:
        return [ingest_file(p, conn, reingest=reingest) for p in paths]
    finally:
        if own:
            conn.close()
