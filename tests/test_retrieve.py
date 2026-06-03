"""Stage 6: retrieval — search (hybrid), expand, assemble.

search/expand/assemble run on a seeded DB with a monkeypatched query embedding, so they're
deterministic and need no Ollama. The cross-encoder rerank + full pipeline are guarded
integration tests (need the rerank extra + Ollama).
"""

import struct
from pathlib import Path

import pytest

from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.retrieve import search as search_mod
from locus.retrieve.assemble import assemble
from locus.retrieve.expand import Expanded, expand
from locus.retrieve.search import Candidate, search

DIM = 768


def _vec(head):
    return struct.pack(f"{DIM}f", *(head + [0.0] * (DIM - len(head))))


def _seed(conn):
    conn.execute(
        "INSERT INTO documents (id, content_hash, source_type, source_uri, raw_path, title,"
        " ingest_model, thesis, method, result, limitations, section_map) VALUES "
        "(1,'h','pdf','u','p','Control Notes','m','THESIS','METHOD','RESULT','LIMITS',"
        " '[{\"position\":0,\"title\":\"Stability\",\"page_start\":5,\"page_end\":7}]')"
    )
    conn.execute("INSERT INTO sections (id, doc_id, position, title, summary) VALUES (1,1,0,'Stability','poles determine stability')")
    conn.execute("INSERT INTO chunks (id, section_id, doc_id, position, raw_text, embed_model) VALUES (1,1,1,0,'stability poles feedback criterion','m')")
    conn.execute("INSERT INTO propositions (id, section_id, doc_id, position, text, embed_model) VALUES (1,1,1,0,'Stability is determined by the poles.','m')")
    conn.execute("INSERT INTO entities (doc_id, section_id, name, type) VALUES (1,1,'Nyquist criterion','theorem')")
    conn.execute("INSERT INTO chunk_vectors(chunk_id, embedding) VALUES (1, ?)", (_vec([1.0, 0.0, 0.0]),))
    conn.execute("INSERT INTO proposition_vectors(proposition_id, embedding) VALUES (1, ?)", (_vec([0.9, 0.1, 0.0]),))
    conn.execute("INSERT INTO section_vectors(section_id, embedding) VALUES (1, ?)", (_vec([0.8, 0.2, 0.0]),))
    conn.commit()


@pytest.fixture()
def conn(tmp_path: Path, monkeypatch):
    db = tmp_path / "r.db"
    migrate(db)
    c = get_connection(db)
    _seed(c)
    # Query embeds near the seeded vectors; no Ollama needed.
    monkeypatch.setattr(search_mod, "embed_text", lambda q: [1.0, 0.0, 0.0] + [0.0] * (DIM - 3))
    yield c
    c.close()


def test_search_returns_all_arms(conn):
    cands = search(conn, "stability poles")
    kinds = {c.kind for c in cands}
    assert {"proposition", "chunk", "section"} <= kinds
    # The chunk is found by BOTH dense and lexical (hybrid merge).
    chunk = next(c for c in cands if c.kind == "chunk" and c.id == 1)
    assert {"dense", "lexical"} <= chunk.sources


def test_lexical_arm_matches_text_terms(conn):
    # A term present in chunk text is found lexically even though we don't rely on embeddings.
    cands = search(conn, "criterion")
    assert any(c.kind == "chunk" and "lexical" in c.sources for c in cands)


def test_entity_arm_fires_on_named_entity(conn):
    cands = search(conn, "explain the Nyquist criterion please")
    assert any("entity" in c.sources for c in cands)


def test_expand_attaches_parent_context(conn):
    cands = search(conn, "stability poles")
    chunk = next(c for c in cands if c.kind == "chunk")
    (ex,) = [e for e in expand(conn, [chunk])]
    assert ex.doc_title is not None and ex.thesis == "THESIS"
    assert ex.section_title == "Stability"
    assert ex.section_summary == "poles determine stability"
    assert ex.page_start == 5 and ex.page_end == 7


def _expanded(kind, text):
    c = Candidate(kind=kind, id=1, doc_id=1, section_id=1, text=text, score=0.0)
    return Expanded(
        candidate=c, doc_id=1, doc_title="Doc", thesis="t", method="m", result="r",
        limitations="l", section_id=1, section_title="Sec", section_summary="summary",
        page_start=1, page_end=2,
    )


def test_assemble_drops_finest_first_under_budget():
    items = [_expanded("proposition", "a claim"), _expanded("chunk", "x " * 400)]
    # Budget large enough for synthesis+summary+claim, but not the big chunk.
    out = assemble(items, budget=120)
    assert "a claim" in out.text
    assert out.dropped >= 1  # the chunk was dropped
    assert "thesis: t" in out.text  # coarse content kept


def test_assemble_includes_provenance():
    out = assemble([_expanded("chunk", "excerpt text")], budget=10_000)
    assert out.citations
    assert any("Doc" in c for c in out.citations)


# --- guarded end-to-end ---

def _stack_ready():
    try:
        import sentence_transformers  # noqa: F401
        from locus.ingest.embed import embed_texts
        return bool(embed_texts(["ping"]))
    except Exception:
        return False


@pytest.mark.skipif(not _stack_ready(), reason="rerank extra / Ollama unavailable")
def test_retrieve_end_to_end(conn, monkeypatch):
    from locus.retrieve import retrieve
    # real embedding for the query (Ollama up); rerank uses the cross-encoder.
    monkeypatch.undo()
    r = retrieve("stability poles", conn=conn)
    assert r.survivors
    assert isinstance(r.context, str)
