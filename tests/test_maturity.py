"""Retrieval down-weighting of maturity=rough capture notes (agent-layer §6.1, Phase 1 ①).

The mechanism is a subtractive penalty on the cross-encoder rerank_score of rough-doc
candidates (retrieve/pipeline._apply_maturity_penalty), applied before the final sort — a
monotonic demotion, never a filter. These tests are pure/DB, no model.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.retrieve.pipeline import _apply_maturity_penalty
from locus.retrieve.search import Candidate, rough_doc_ids


def _doc(conn, h: str, maturity: str) -> int:
    cur = conn.execute(
        "INSERT INTO documents (content_hash, source_type, source_uri, raw_path, title, "
        "ingest_model, maturity) VALUES (?,?,?,?,?,?,?)",
        (h, "markdown", f"{h}.md", f"{h}.md", h, "test", maturity),
    )
    return cur.lastrowid


@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "m.db"
    migrate(db)
    c = get_connection(db)
    with c:
        _doc(c, "rough1", "rough")
        _doc(c, "tidy1", "tidy")
    yield c
    c.close()


def test_rough_doc_ids_selects_only_rough(conn):
    rough = rough_doc_ids(conn)
    titles = {
        r["id"]: r["maturity"]
        for r in conn.execute("SELECT id, maturity FROM documents")
    }
    assert all(titles[i] == "rough" for i in rough)
    assert len(rough) == 1


def _cand(doc_id: int, score: float) -> Candidate:
    c = Candidate("chunk", doc_id, doc_id, doc_id, "text", 0.0)
    c.rerank_score = score
    return c


def test_penalty_demotes_rough_below_equal_tidy():
    rough = _cand(1, 5.0)
    tidy = _cand(2, 5.0)
    _apply_maturity_penalty([rough, tidy], rough_ids={1}, penalty=1.5)
    assert rough.rerank_score == pytest.approx(3.5)
    assert tidy.rerank_score == pytest.approx(5.0)  # untouched


def test_penalty_is_demotion_not_filter():
    # A strongly-matching rough note still out-ranks a weak tidy one after the penalty
    # (flag/down-weight, never filter — principle 8).
    rough = _cand(1, 8.0)
    tidy = _cand(2, 4.0)
    _apply_maturity_penalty([rough, tidy], rough_ids={1}, penalty=1.5)
    assert rough.rerank_score > tidy.rerank_score


def test_penalty_noop_when_no_rough_or_zero_penalty():
    c = _cand(1, 5.0)
    _apply_maturity_penalty([c], rough_ids=set(), penalty=1.5)
    assert c.rerank_score == 5.0
    _apply_maturity_penalty([c], rough_ids={1}, penalty=0.0)
    assert c.rerank_score == 5.0


def test_penalty_skips_unscored_candidates():
    c = _cand(1, 0.0)
    c.rerank_score = None  # not yet cross-encoder-scored
    _apply_maturity_penalty([c], rough_ids={1}, penalty=1.5)
    assert c.rerank_score is None
