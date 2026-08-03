"""Ideas that connect to each other (`link/threads.py`).

Model-free and joins-only by construction. What is asserted is that a connection is a FACT — two
threads naming the same canonical concept — rather than a distance, and that the noise filters
which make that useful actually hold.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from locus.agent import state
from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.link import threads as T


@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "threads.db"
    migrate(db)
    c = get_connection(db)
    yield c
    c.close()


def _doc(conn, doc_id, uri):
    with conn:
        conn.execute(
            "INSERT INTO documents (id, content_hash, source_type, source_uri, raw_path, "
            "ingest_model, category) VALUES (?,?,'markdown',?,?,'test','note')",
            (doc_id, f"h{doc_id}", uri, f"raw/{doc_id}"),
        )
        conn.execute("INSERT INTO sections (id, doc_id, position) VALUES (?,?,0)", (doc_id, doc_id))


def _entity(conn, doc_id, name, type_="concept"):
    with conn:
        conn.execute(
            "INSERT OR IGNORE INTO entities (doc_id, section_id, name, type) VALUES (?,?,?,?)",
            (doc_id, doc_id, name, type_),
        )


def _concept_in_two_docs(conn, name):
    """A canonical must span >= 2 documents to count as a thread running through his work."""
    for doc_id, uri in ((1, "notes/a.md"), (2, "notes/b.md")):
        if not conn.execute("SELECT 1 FROM documents WHERE id=?", (doc_id,)).fetchone():
            _doc(conn, doc_id, uri)
        _entity(conn, doc_id, name)


def _thread(conn, title, *, type_="idea", idea="", development=()):
    oid, _ = state.upsert_object(conn, type_=type_, title=title)
    body = {}
    if idea:
        body["idea"] = idea
    if development:
        body["development"] = [{"at": "2026-07-31", "text": t} for t in development]
    if body:
        state.apply_owner_edit(conn, oid, body, source="test")
    state.set_status(conn, oid, "active")
    return oid


# --- what counts as a connection ------------------------------------------------------------


def test_two_threads_naming_the_same_concept_are_linked(conn):
    _concept_in_two_docs(conn, "regime detection")
    a = _thread(conn, "should we add a macro regime detection signal?")
    b = _thread(conn, "regime detection for the tanker spread")

    links = T.link_threads(conn)
    assert len(links) == 1
    assert links[0].shared == ("regime detection",)
    assert {links[0].source_id, links[0].target_id} == {a, b}


def test_the_link_is_readable_from_either_end(conn):
    """`object_links` is directed; a thread should surface whichever was written first."""
    _concept_in_two_docs(conn, "regime detection")
    a = _thread(conn, "regime detection one")
    b = _thread(conn, "regime detection two")
    T.link_threads(conn)

    assert [o.id for o in T.related_threads(conn, a)] == [b]
    assert [o.id for o in T.related_threads(conn, b)] == [a]


def test_development_passes_count_as_the_threads_vocabulary(conn):
    """A thread is what he has written on it over time, not just its opening line."""
    _concept_in_two_docs(conn, "regime detection")
    a = _thread(conn, "an idea with a bare title",
                development=("this is really about regime detection",))
    b = _thread(conn, "regime detection elsewhere")
    assert len(T.link_threads(conn)) == 1
    assert [o.id for o in T.related_threads(conn, a)] == [b]


def test_unrelated_threads_are_not_linked(conn):
    _concept_in_two_docs(conn, "regime detection")
    _thread(conn, "regime detection thoughts")
    _thread(conn, "something else entirely about catering")
    assert T.link_threads(conn) == []


def test_re_running_adds_nothing(conn):
    _concept_in_two_docs(conn, "regime detection")
    a = _thread(conn, "regime detection one")
    _thread(conn, "regime detection two")
    T.link_threads(conn)
    before = len(state.links_for(conn, a))
    T.link_threads(conn)
    assert len(state.links_for(conn, a)) == before


# --- the filters that make it useful rather than noise ----------------------------------------


def test_a_concept_in_only_one_document_is_that_documents_vocabulary(conn):
    """Not a thread running through his work — so it must not connect anything."""
    _doc(conn, 1, "notes/a.md")
    _entity(conn, 1, "regime detection")
    _thread(conn, "regime detection one")
    _thread(conn, "regime detection two")
    assert T.link_threads(conn) == []


def test_very_short_names_never_connect(conn):
    """`ML`, `VaR`, `PDE` appear in half of everything; a link that fires on everything is noise
    wearing a citation."""
    _concept_in_two_docs(conn, "ML")
    _thread(conn, "some ML thing")
    _thread(conn, "another ML thing")
    assert T.link_threads(conn) == []


def test_a_head_word_is_kept_alongside_the_phrase_that_contains_it(conn):
    """Suppressing it silently cost the only real link in the corpus: one thread named `regime
    detection` and another named `regime`, and once the longer phrase won they shared nothing.
    Two people can discuss one thing at different levels of specificity."""
    vocabulary = {"regime detection": "regime detection", "regime": "regime"}
    found = T.concepts_in("we want regime detection for the tanker project", vocabulary)
    assert set(found) == {"regime detection", "regime"}


def test_matching_is_not_capped_even_when_reporting_is(conn):
    """Capping the MATCH scanned longest-phrase-first and stopped early, so a thread whose text
    had grown never reached the short concepts."""
    vocabulary = {f"concept number {i}": f"concept number {i}" for i in range(12)}
    vocabulary["regime"] = "regime"
    text = " ".join(vocabulary) + " regime"
    assert "regime" in T.concepts_in(text, vocabulary)


def test_the_passage_a_mark_was_written_beside_is_part_of_the_thread(conn):
    """The real reason linking was sparse. "interesting, can we plot this behavior?" names NO
    concept — the concept is in the paragraph he was reading when he wrote it."""
    _concept_in_two_docs(conn, "mean reversion")
    a = _thread(conn, "interesting, can we plot this behaviour?")
    with conn:
        conn.execute(
            "INSERT INTO pdf_annotations (source_uri, pdf_page, kind, bbox_key, covered_text, "
            "in_margin, captured_at, object_id) VALUES ('books/apm.pdf',70,'underline','k1',"
            "'performance beyond a year is reverting — mean reversion',0,'2026-07-30',?)",
            (a,),
        )
    b = _thread(conn, "mean reversion in the tanker spread")

    assert T.concepts_in(T._idea_text(state.get_object(conn, a), conn), {"mean reversion": "mean reversion"})
    assert len(T.link_threads(conn)) == 1, "the passage is what the idea is ABOUT"


def test_two_ideas_from_one_book_do_not_link_merely_for_sharing_it(conn):
    """Tried, and it produced the failure it was meant to prevent: all four ideas from *Advanced
    Portfolio Management* formed a complete graph via the string "Advanced Portfolio Management",
    asserting only that he read one book. Sharing a source is not sharing a thought."""
    _concept_in_two_docs(conn, "Advanced Portfolio Management")
    a = _thread(conn, "one idea")
    b = _thread(conn, "an unrelated idea")
    with conn:
        for i, oid in enumerate((a, b)):
            conn.execute(
                "INSERT INTO documents (id, content_hash, source_type, source_uri, raw_path, "
                "ingest_model, title, category) VALUES (?,?,'pdf','books/apm.pdf','raw/apm',"
                "'test','Advanced Portfolio Management','paper')"
                if i == 0 else "SELECT 1", (10, "hapm") if i == 0 else ()
            ) if i == 0 else None
            conn.execute(
                "INSERT INTO pdf_annotations (source_uri, pdf_page, kind, bbox_key, covered_text,"
                " in_margin, captured_at, object_id) VALUES ('books/apm.pdf',?,'underline',?,"
                "'a passage with no shared concept in it',0,'2026-07-30',?)",
                (i, f"k{i}", oid),
            )
    assert T.link_threads(conn) == [], "the document TITLE must not be part of the vocabulary"


def test_only_his_words_are_matched_not_the_agents_rationale(conn):
    """Linking two ideas because the PROPOSER used a word twice is a connection between two
    pieces of machine prose."""
    _concept_in_two_docs(conn, "regime detection")
    oid, _ = state.upsert_object(
        conn, type_="idea", title="a bare idea", body={"why": "agent says regime detection"}
    )
    state.set_status(conn, oid, "active")
    _thread(conn, "regime detection elsewhere")
    assert T.link_threads(conn) == []


def test_a_thread_he_dropped_is_not_connected(conn):
    """A cross means NO, so a rejected thread must not come back through the link graph."""
    _concept_in_two_docs(conn, "regime detection")
    a = _thread(conn, "regime detection one")
    _thread(conn, "regime detection two")
    state.set_status(conn, a, "archived")
    state.log_acceptance(conn, surface="object", candidate_key=str(a), verdict="rejected")
    assert T.link_threads(conn) == []


def test_a_thread_he_ANSWERED_is_still_connected(conn):
    """`archived` means two opposite things and only one of them is "forget this".

    Live: obj 78 ("what are the best methods for regime detection") was kept and answered, then
    archived by a tick — and was linked to neither of the other two regime threads, because the
    filter read `status` instead of his recorded judgement.
    """
    _concept_in_two_docs(conn, "regime detection")
    a = _thread(conn, "regime detection one")
    _thread(conn, "regime detection two")
    state.set_status(conn, a, "archived")
    state.log_acceptance(conn, surface="object", candidate_key=str(a), verdict="kept")
    assert [link.shared for link in T.link_threads(conn)] == [("regime detection",)]
