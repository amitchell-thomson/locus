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
from locus.retrieve.rerank import select  # pure selection logic; does not load the model
from locus.retrieve.search import Candidate, search

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


def test_assemble_dedupes_citations():
    # A proposition and a chunk from the same section share one provenance string:
    # it must appear once, not once per included unit.
    items = [_expanded("proposition", "a claim"), _expanded("chunk", "an excerpt")]
    out = assemble(items, budget=10_000)
    assert out.included == 2
    assert len(out.citations) == 1


# --- diversity-aware selection (rerank.select) -------------------------------------------


def _cand(kind, id_, doc_id, section_id):
    return Candidate(kind=kind, id=id_, doc_id=doc_id, section_id=section_id,
                     text="t", score=0.0)


def test_select_caps_units_per_section_and_kind():
    # Two chunks of the same section: only the better-ranked one survives while a
    # different section's chunk is available.
    ranked = [_cand("chunk", 1, 1, 1), _cand("chunk", 2, 1, 1), _cand("chunk", 3, 1, 2)]
    out = select(ranked, top_k=2, per_doc_cap=10)
    assert [c.id for c in out] == [1, 3]


def test_select_drops_section_summary_when_child_in_pool():
    # The section-summary candidate is redundant: expansion re-attaches the summary
    # to the surviving chunk anyway.
    ranked = [_cand("section", 1, 1, 1), _cand("chunk", 10, 1, 1), _cand("chunk", 11, 1, 2)]
    out = select(ranked, top_k=2, per_doc_cap=10)
    assert [(c.kind, c.id) for c in out] == [("chunk", 10), ("chunk", 11)]


def test_select_keeps_section_summary_without_child():
    ranked = [_cand("section", 1, 1, 1), _cand("chunk", 10, 1, 2)]
    out = select(ranked, top_k=2, per_doc_cap=10)
    assert [(c.kind, c.id) for c in out] == [("section", 1), ("chunk", 10)]


def test_select_per_doc_cap_promotes_second_document():
    # Doc 1 outranks everywhere, but the cap forces doc 2 into the top-k
    # (the cross-domain synthesis fix).
    ranked = [_cand("chunk", i, 1, i) for i in range(1, 6)] + [_cand("chunk", 99, 2, 99)]
    out = select(ranked, top_k=4, per_doc_cap=3)
    assert [c.id for c in out] == [1, 2, 3, 99]


def test_select_refills_rather_than_underfilling():
    # Caps would leave the top-k underfull -> the best skipped candidates come back,
    # and rank order is preserved in the output.
    ranked = [_cand("chunk", i, 1, i) for i in range(1, 5)]
    out = select(ranked, top_k=4, per_doc_cap=2)
    assert [c.id for c in out] == [1, 2, 3, 4]


def _scored(kind, id_, doc_id, section_id, score):
    c = _cand(kind, id_, doc_id, section_id)
    c.rerank_score = score
    return c


def test_select_refill_respects_min_score():
    # Doc cap skips ids 3 and 4; the refill may only restore the one above the floor.
    # An underfull top-k beats padding it with judged-irrelevant candidates.
    ranked = [
        _scored("chunk", 1, 1, 1, 5.0),
        _scored("chunk", 2, 1, 2, 4.0),
        _scored("chunk", 3, 1, 3, 1.0),   # skipped by cap, above floor -> refilled
        _scored("chunk", 4, 1, 4, -3.0),  # skipped by cap, below floor -> stays out
    ]
    out = select(ranked, top_k=4, per_doc_cap=2, min_score=0.0)
    assert [c.id for c in out] == [1, 2, 3]


def test_select_refill_unbounded_without_min_score():
    ranked = [
        _scored("chunk", 1, 1, 1, 5.0),
        _scored("chunk", 2, 1, 2, 4.0),
        _scored("chunk", 3, 1, 3, -3.0),
    ]
    out = select(ranked, top_k=3, per_doc_cap=2)  # min_score=None: old behaviour
    assert [c.id for c in out] == [1, 2, 3]


def test_assemble_citation_details_carry_best_rerank_score():
    a = _expanded("proposition", "a claim")
    b = _expanded("chunk", "an excerpt")
    a.candidate.rerank_score, b.candidate.rerank_score = 2.5, 4.0
    out = assemble([a, b], budget=10_000)
    assert len(out.citation_details) == 1  # shared provenance -> one detail
    d = out.citation_details[0]
    assert d.doc_id == 1 and d.rerank_score == 4.0
    assert d.text == out.citations[0]


def test_retrieve_low_confidence_flag(conn, monkeypatch):
    import types

    from locus.retrieve import pipeline as pl

    def fake_load(score):
        rcfg = types.SimpleNamespace(rerank_top_k=8, per_doc_cap=3, min_rerank_score=score)
        return lambda: types.SimpleNamespace(retrieve=rcfg)

    def fake_rerank(value):
        def _rr(query, candidates, top_k, per_doc_cap, min_score=None):
            for c in candidates:
                c.rerank_score = value
            return candidates[:top_k]
        return _rr

    # Best survivor below the floor -> flagged (signal, not filter: survivors still returned).
    monkeypatch.setattr(pl, "load", fake_load(0.0))
    monkeypatch.setattr(pl, "rerank", fake_rerank(-2.0))
    r = pl.retrieve("stability poles", conn=conn)
    assert r.low_confidence and r.survivors and r.citation_details

    # Above the floor -> not flagged.
    monkeypatch.setattr(pl, "rerank", fake_rerank(6.0))
    assert pl.retrieve("stability poles", conn=conn).low_confidence is False

    # No floor configured -> never flagged.
    monkeypatch.setattr(pl, "load", fake_load(None))
    monkeypatch.setattr(pl, "rerank", fake_rerank(-9.0))
    assert pl.retrieve("stability poles", conn=conn).low_confidence is False


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
