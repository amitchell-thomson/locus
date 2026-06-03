"""Stage 5: ingest orchestration — write, idempotency, quarantine, routing.

The LLM passes and embeddings are monkeypatched with deterministic fakes, so these tests are
fast and need no Ollama. They exercise the orchestration and the transactional DB write.
"""

from pathlib import Path

import pymupdf
import pytest

from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.ingest import embed, entities, gaps, propositions, summarize, synthesis
from locus.ingest.entities import Entity
from locus.ingest.llm import IngestExtractionError
from locus.ingest.synthesis import DocSynthesis
from locus.ingest_pipeline import ingest_file


def _make_pdf(path: Path) -> Path:
    doc = pymupdf.open()
    page = doc.new_page()
    y = 72.0
    for text, size in [
        ("Section A", 20.0),
        ("Body sentence one. Body sentence two about systems.", 11.0),
        ("Section B", 20.0),
        ("More body content describing methods and results.", 11.0),
    ]:
        page.insert_text((72, y), text, fontsize=size)
        y += size + 10
    doc.save(str(path))
    doc.close()
    return path


def _count(conn, table: str) -> int:
    return conn.execute(f"SELECT COUNT(*) AS c FROM {table}").fetchone()["c"]


@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "ingest.db"
    migrate(db)
    c = get_connection(db)
    yield c
    c.close()


@pytest.fixture()
def fake_passes(monkeypatch):
    """Replace the LLM passes + embeddings with deterministic stand-ins."""
    monkeypatch.setattr(summarize, "summarize_section", lambda title, text, **k: f"summary::{title}")
    monkeypatch.setattr(propositions, "extract_propositions", lambda title, text, **k: ["claim A", "claim B"])
    monkeypatch.setattr(entities, "extract_entities", lambda title, text, **k: [Entity(name="Kalman filter", type="method")])
    monkeypatch.setattr(
        synthesis, "synthesize_document",
        lambda title, summaries, **k: DocSynthesis(thesis="T", method="M", result="R", limitations="L"),
    )
    monkeypatch.setattr(gaps, "flag_gaps", lambda title, context, **k: ["gap one"])
    monkeypatch.setattr(embed, "embed_text", lambda text: [0.1] * 768)
    monkeypatch.setattr(embed, "embed_texts", lambda texts: [[0.1] * 768 for _ in texts])


def test_ingest_populates_all_levels(tmp_path, conn, fake_passes):
    result = ingest_file(_make_pdf(tmp_path / "doc.pdf"), conn)

    assert result.status == "ingested"
    assert result.doc_id is not None
    assert result.sections >= 2

    assert _count(conn, "documents") == 1
    doc = conn.execute("SELECT thesis, gap_flags FROM documents").fetchone()
    assert doc["thesis"] == "T"
    import json
    assert json.loads(doc["gap_flags"]) == ["gap one"]

    # Every level + its vectors written, counts consistent.
    assert _count(conn, "sections") == result.sections
    assert _count(conn, "section_vectors") == result.sections
    assert _count(conn, "chunks") == result.chunks > 0
    assert _count(conn, "chunk_vectors") == result.chunks
    assert _count(conn, "propositions") == result.propositions == result.sections * 2
    assert _count(conn, "proposition_vectors") == result.propositions
    assert _count(conn, "entities") == result.entities >= 1


def test_reingest_is_a_noop(tmp_path, conn, fake_passes):
    pdf = _make_pdf(tmp_path / "doc.pdf")
    first = ingest_file(pdf, conn)
    docs_after_first = _count(conn, "documents")
    chunks_after_first = _count(conn, "chunks")

    second = ingest_file(pdf, conn)
    assert second.status == "skipped"
    assert second.doc_id == first.doc_id
    assert _count(conn, "documents") == docs_after_first == 1
    assert _count(conn, "chunks") == chunks_after_first


def test_failure_is_quarantined_with_no_partial_write(tmp_path, conn, monkeypatch, fake_passes):
    # A pass blows up mid-prepare; nothing must be written.
    def boom(*a, **k):
        raise IngestExtractionError("model produced garbage")

    monkeypatch.setattr(summarize, "summarize_section", boom)

    result = ingest_file(_make_pdf(tmp_path / "doc.pdf"), conn)
    assert result.status == "quarantined"
    assert "garbage" in (result.error or "")
    assert _count(conn, "documents") == 0
    assert _count(conn, "chunks") == 0


def test_unsupported_type_is_reported(tmp_path, conn):
    txt = tmp_path / "notes.txt"
    txt.write_text("not a pdf")
    result = ingest_file(txt, conn)
    assert result.status == "unsupported"


def test_missing_file_is_quarantined(tmp_path, conn):
    result = ingest_file(tmp_path / "nope.pdf", conn)
    assert result.status == "quarantined"
