"""Grounded Related enrichment (agent-layer §8.1, Phase 1 ④). Retrieval is faked; the vault writer
is faked. Asserts grounded-or-silent and that the block cites the retrieved documents by title."""

from __future__ import annotations

import types
from pathlib import Path

import pytest

from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.enrich.related import enrich_note


@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "e.db"
    migrate(db)
    c = get_connection(db)
    with c:
        for i, title in ((1, "Regime ML paper"), (2, "Tanker Flow project"), (3, "Swaps notes")):
            c.execute(
                "INSERT INTO documents (id, content_hash, source_type, source_uri, raw_path, "
                "title, ingest_model) VALUES (?,?,?,?,?,?,?)",
                (i, f"h{i}", "markdown", f"u{i}", f"r{i}", title, "t"),
            )
    yield c
    c.close()


def _result(hits):
    """What `library_related` returns: (doc_id, score) for units that cleared the floor."""
    return list(hits)


def _writer():
    calls = []

    def w(path, kind, body, *, run_id, expected_hash=None):
        calls.append(types.SimpleNamespace(path=path, kind=kind, body=body, run_id=run_id))
        return types.SimpleNamespace(to_sidecar=False)

    w.calls = calls
    return w


def test_writes_grounded_related_block_by_title(conn):
    # Retrieval surfaces docs 2 (best), 1, and 2 again -> distinct, best-first.
    cites = [(1, 3.0), (2, 7.0), (2, 1.0)]
    w = _writer()
    r = enrich_note("note.md", "swaps momentum notes", conn=conn, run_id="9",
                    retrieve_fn=lambda q: _result(cites), writer=w)

    assert r.wrote_block is True and r.related == 2
    body = w.calls[0].body
    assert body.startswith("> [!ai] Related")
    # best score first (doc 2), then doc 1 — cited by real title
    assert body.index("[[Tanker Flow project]]") < body.index("[[Regime ML paper]]")
    assert "[[Swaps notes]]" not in body  # doc 3 wasn't retrieved -> not linked (grounded only)


def test_nothing_above_the_floor_writes_no_block(conn, monkeypatch):
    from locus.enrich import related
    from locus.retrieve import rerank, search

    monkeypatch.setattr(search, "search", lambda conn, q, facets=None: [
        types.SimpleNamespace(kind="chunk", doc_id=1, text="a")])
    monkeypatch.setattr(rerank, "score_pairs", lambda q, texts: [-9.0 for _ in texts])
    monkeypatch.setattr("locus.config.load", lambda: types.SimpleNamespace(
        retrieve=types.SimpleNamespace(min_rerank_score=0.0)))
    assert related.library_related(conn, "x") == []
    w = _writer()
    r = enrich_note("note.md", "x", conn=conn, run_id="1",
                    retrieve_fn=lambda q: related.library_related(conn, q), writer=w)
    assert r.wrote_block is False and r.related == 0
    assert w.calls == []  # grounded-or-silent: nothing written


def test_empty_retrieval_writes_no_block(conn):
    w = _writer()
    r = enrich_note("note.md", "x", conn=conn, run_id="1",
                    retrieve_fn=lambda q: _result([]), writer=w)
    assert r.wrote_block is False
    assert w.calls == []


def test_query_strips_page_markers_and_fill_brackets(conn):
    seen = {}

    def fake_retrieve(q):
        seen["q"] = q
        return _result([(1, 5.0)])

    enrich_note("note.md", "<!-- page 1 -->\nThe ⟦leg⟧ resets on SOFR.", conn=conn, run_id="1",
                retrieve_fn=fake_retrieve, writer=_writer())
    assert "<!-- page 1 -->" not in seen["q"]
    assert "⟦" not in seen["q"] and "⟧" not in seen["q"]
    assert "resets on SOFR" in seen["q"]
