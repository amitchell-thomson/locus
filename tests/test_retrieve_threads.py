"""Retrieval returns a thread, not a flattened copy of one (`retrieve/threads.py`).

Model-free: the join is exact (`promote` records the note's path on the object), so nothing here
needs embeddings or a model. What is asserted is that the connections survive the trip into an
answer — the thing that made the thought worth storing as an object rather than as prose.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from locus.agent import state
from locus.agent.promote import PROMOTED_PATH_KEY
from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.retrieve import threads as T


@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "rt.db"
    migrate(db)
    c = get_connection(db)
    yield c
    c.close()


def _thread(conn, title, *, type_="idea", promoted="/vault/notes/threads/t-1.md",
            development=()):
    oid, _ = state.upsert_object(conn, type_=type_, title=title)
    state.apply_owner_edit(conn, oid, {type_: title}, source="test")
    if development:
        state.apply_owner_edit(
            conn, oid,
            {"development": [{"at": "2026-07-31", "text": t} for t in development]},
            source="test",
        )
    state.set_status(conn, oid, "active")
    if promoted:
        body = dict(state.get_object(conn, oid).body or {})
        body[PROMOTED_PATH_KEY] = promoted
        import json
        with conn:
            conn.execute("UPDATE objects SET body=? WHERE id=?", (json.dumps(body), oid))
    return oid


def test_a_retrieved_thread_note_carries_its_project_and_its_connections(conn):
    project, _ = state.upsert_object(conn, type_="project", title="regime-ml")
    a = _thread(conn, "markets are not stationary", promoted="/vault/notes/threads/a-1.md")
    b = _thread(conn, "macro regime predictor for tankers", promoted="/vault/notes/threads/b-2.md")
    state.add_links(conn, a, [
        state.ObjectLink("object", str(project), "relates"),
        state.ObjectLink("object", str(b), "relates"),
    ])

    ctx = T.context_for(conn, "/vault/notes/threads/a-1.md")
    assert ctx.project == "regime-ml"
    assert ctx.related == ("macro regime predictor for tankers",)
    rendered = ctx.render()
    assert "part of: regime-ml" in rendered
    assert "also touches" in rendered


def test_the_development_chain_travels_with_it(conn):
    """"how his view moved" is the thing a flattened note cannot say."""
    oid = _thread(conn, "do regimes persist?", type_="question",
                  promoted="/vault/notes/threads/q-3.md",
                  development=("in sample they do", "out of sample they do not"))
    ctx = T.context_for(conn, "/vault/notes/threads/q-3.md")
    assert ctx.development == ("in sample they do", "out of sample they do not")
    assert "you later wrote: out of sample they do not" in ctx.render()


def test_a_document_that_is_not_a_thread_gets_nothing(conn):
    _thread(conn, "a thread", promoted="/vault/notes/threads/a-1.md")
    assert T.context_for(conn, "papers/somebody-elses-paper.pdf") is None


def test_a_thread_he_dropped_stops_annotating_its_note(conn):
    """A cross means NO: what he threw away must not keep decorating the corpus."""
    oid = _thread(conn, "dropped idea", promoted="/vault/notes/threads/a-1.md")
    state.set_status(conn, oid, "archived")
    state.log_acceptance(conn, surface="object", candidate_key=str(oid), verdict="rejected")
    assert T.context_for(conn, "/vault/notes/threads/a-1.md") is None


def test_a_thread_he_ANSWERED_still_annotates_its_note(conn):
    """The other half of `archived`. A resolved question's note is already in the corpus; without
    this it came back stripped of the project and siblings that make it worth being an object."""
    project, _ = state.upsert_object(conn, type_="project", title="regime-ml")
    oid = _thread(conn, "answered idea", promoted="/vault/notes/threads/a-2.md")
    state.add_links(conn, oid, [state.ObjectLink("object", str(project), "relates")])
    state.set_status(conn, oid, "archived")
    state.log_acceptance(conn, surface="object", candidate_key=str(oid), verdict="kept")
    ctx = T.context_for(conn, "/vault/notes/threads/a-2.md")
    assert ctx is not None and ctx.project == "regime-ml"


def test_a_thread_with_no_connections_adds_nothing_to_the_answer(conn):
    """Grounded or silent: an unconnected thread's note already says everything it can."""
    _thread(conn, "a lone idea", promoted="/vault/notes/threads/a-1.md")
    assert T.contexts_for(conn, ["/vault/notes/threads/a-1.md"]) == {}


def test_the_basename_matches_when_the_uri_is_relative(conn):
    """`promoted_path` is absolute and a source_uri need not be."""
    _thread(conn, "an idea", promoted="/abs/vault/notes/threads/a-1.md",
            development=("a later pass",))
    assert T.context_for(conn, "notes/threads/a-1.md") is not None


def test_expansion_attaches_it_without_a_second_candidate(conn):
    """NOT a new retrieval arm: a parallel arm would put the same text in the pool twice, so it
    would compete with itself for the top-k and double-count against the per-doc diversity cap."""
    from locus.retrieve.expand import expand
    from locus.retrieve.search import Candidate

    project, _ = state.upsert_object(conn, type_="project", title="regime-ml")
    oid = _thread(conn, "an idea", promoted="/vault/notes/threads/a-1.md")
    state.add_links(conn, oid, [state.ObjectLink("object", str(project), "relates")])
    with conn:
        conn.execute(
            "INSERT INTO documents (id, content_hash, source_type, source_uri, raw_path, "
            "ingest_model, title, category) VALUES (1,'h','markdown',"
            "'/vault/notes/threads/a-1.md','raw/a','test','an idea','note')"
        )

    expanded = expand(conn, [Candidate(kind="chunk", id=1, doc_id=1, section_id=None,
                                       text="an idea", score=1.0)])
    assert len(expanded) == 1, "one candidate in, one out — no duplicate unit"
    assert expanded[0].thread.project == "regime-ml"
