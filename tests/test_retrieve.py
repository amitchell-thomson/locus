"""Library search (the dense, lexical and path arms) on a seeded DB.

A monkeypatched query embedding makes it deterministic with no Ollama. The pipeline's expand,
assemble and selection tests went with it in Slice C (C2).
"""

import struct
from pathlib import Path

import pytest

from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.retrieve import search as search_mod
from locus.retrieve.search import search

DIM = 768


def _vec(head):
    return struct.pack(f"{DIM}f", *(head + [0.0] * (DIM - len(head))))


def _seed(conn):
    conn.execute(
        "INSERT INTO documents (id, content_hash, source_type, source_uri, raw_path, title,"
        " ingest_model, source_date, category, thesis, method, result, limitations, section_map) VALUES "
        "(1,'h','pdf','u','p','Control Notes','m','2023-06-01','paper','THESIS','METHOD','RESULT','LIMITS',"
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


def test_facets_filter_by_date_and_category(conn):
    from locus.retrieve.search import Facets

    # Within the doc's date range + matching category: candidates survive.
    assert search(conn, "stability poles", Facets(since="2023-01-01", category="paper"))
    # Date window excludes the doc (dated 2023-06-01): no candidates.
    assert search(conn, "stability poles", Facets(until="2022-12-31")) == []
    # Wrong category: no candidates.
    assert search(conn, "stability poles", Facets(category="project")) == []
    # No facets: unrestricted, same as before.
    assert search(conn, "stability poles", None)


def test_lexical_arm_matches_text_terms(conn):
    # A term present in chunk text is found lexically even though we don't rely on embeddings.
    cands = search(conn, "criterion")
    assert any(c.kind == "chunk" and "lexical" in c.sources for c in cands)


# --- diversity-aware selection (rerank.select) -------------------------------------------


# --- guarded end-to-end ---


# --- facet-aware confidence + floor enforcement (round-3 evaluation) ----------------------


BRIDGE_Q = "How does spectral analysis from my notes relate to regime detection in finance?"


# --- implementation-intent source preference (round-3: prose-about-code outranks code) -----


# --- path arm + query-named exemption (round-3 residual: source under-retrieval) -----------


def test_path_arm_surfaces_query_named_file(conn):
    conn.execute(
        "INSERT INTO sections (id, doc_id, position, title, summary, file_path) VALUES "
        "(7,1,1,'retrieve/rerank.py','cross-encoder reranking module','locus/retrieve/rerank.py')"
    )
    conn.execute("INSERT INTO section_vectors(section_id, embedding) VALUES (7, ?)",
                 (_vec([0.0, 1.0, 0.0]),))  # far from the query embedding: dense won't find it
    conn.commit()
    cands = search(conn, "how does the rerank step order candidates?")
    hit = next((c for c in cands if c.file_path == "locus/retrieve/rerank.py"), None)
    assert hit is not None and "path" in hit.sources
    # Generic stems never fire: a file named test.py/config.py is a convention, not a target.
    assert all("path" not in c.sources for c in search(conn, "how do I config the test run?"))


