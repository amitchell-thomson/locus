"""Which project a piece of his own writing is about (`link/projects.py`).

Every case here is a real one from the first seven live threads (2026-08-03), because the defect
this module fixes was invisible to reasoning and only showed up in the data: three of four
mark-born ideas were linked to a project they were not about, and every question written on the
daily page was linked to no project at all.

Model-free: the cosine tier is exercised through an injected floor and a stub, so the assertions
are about the RULE, not about an embedding.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from locus.agent import state
from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.link import projects as P


@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "projects.db"
    migrate(db)
    c = get_connection(db)
    for title in (
        "regime-ml", "tanker-flow", "Alpha Fund", "OxAI", "imc-prosperity",
        "Swaps Momentum Strategy", "Citadel Analysis", "Optiver trading algorithms",
    ):
        object_id, _ = state.upsert_object(c, type_="project", title=title)
        state.set_status(c, object_id, "active")
    yield c
    c.close()


def _titles(rows):
    return sorted(t for _, t in rows)


def test_he_named_the_project_so_it_links(conn):
    """obj 80, live: '...a macro regime predictor in the tanker project?' had NO link to
    tanker-flow, because the only matcher was a cosine and it scored 0.657."""
    got = P.projects_for(conn, "does this suggest we should we macro regime predictor in the "
                               "tanker project?")
    assert _titles(got) == ["regime-ml", "tanker-flow"], "both are named and both are correct"


def test_a_multi_word_title_needs_all_of_its_distinctive_words(conn):
    """obj 95, live: 'a systematic strategy presents decisions/trades with rationale' matched
    `Swaps Momentum Strategy` on the bare word 'strategy'. One shared word is not a name."""
    got = P.projects_for(
        conn,
        "I like the idea of a systematic strategy presents decisions/trades with rationale",
        floor=1.1,   # cosine tier disabled: this asserts the naming rule alone
    )
    assert got == []


def test_generic_title_words_never_carry_a_match(conn):
    """`Alpha Fund` must not fire on a sentence about alpha, and `Citadel Analysis` not on
    'analysis' — those words are his whole vocabulary, so they say nothing about WHICH project."""
    assert P.projects_for(conn, "some analysis of alpha and the data pipeline", floor=1.1) == []


def test_an_unrelated_question_links_to_nothing(conn):
    """obj 79, live: a question about leetcode scored 0.830 against 'AIS capture' and 0.519
    against a project. Nothing here is about his projects, so nothing should be asserted."""
    assert P.projects_for(
        conn, "Is leetcode that important or should we focus on hackerrank/codeforces?",
        floor=0.70,
    ) == []


def test_the_cosine_tier_only_fires_above_the_floor(conn, monkeypatch):
    """The tier-2 fallback, with the embedding stubbed: below the floor it must return nothing
    rather than the nearest thing it found."""

    def fake_best_project(_conn, _text, **_kw):
        return "regime-ml", 0.68

    import locus.reading.relevance as relevance
    monkeypatch.setattr(relevance, "best_project", fake_best_project)

    assert P.projects_for(conn, "wholly unnamed prose about markets", floor=0.70) == []
    assert _titles(P.projects_for(conn, "wholly unnamed prose about markets", floor=0.60)) == [
        "regime-ml"
    ]


def test_naming_beats_the_cosine(conn, monkeypatch):
    """Tier order: a project he NAMED wins over whatever the embedding preferred, and the cosine
    is not even consulted — the deterministic fact is checkable by reading."""
    def explode(*_a, **_kw):  # pragma: no cover - must not be reached
        raise AssertionError("the cosine tier ran even though he named a project")

    import locus.reading.relevance as relevance
    monkeypatch.setattr(relevance, "best_project", explode)

    assert _titles(P.projects_for(conn, "more work on tanker-flow tomorrow")) == ["tanker-flow"]


def test_an_archived_project_is_not_offered(conn):
    with conn:
        conn.execute("UPDATE objects SET status='archived' WHERE title='tanker-flow'")
    assert P.projects_for(conn, "more work on tanker-flow tomorrow", floor=1.1) == []


def test_empty_text_is_not_a_match(conn):
    assert P.projects_for(conn, "   ") == []
