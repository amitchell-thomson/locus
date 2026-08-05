"""Threads: the "Still open" section, and promotion of his own thinking into the corpus.

Model-free. The invariant under test throughout is AUTHORSHIP: only what the owner wrote may be
written out as a note, because a note in `vault/notes/` is ingested as HIS material.
"""

from __future__ import annotations

from datetime import date
from pathlib import Path

import pytest

from locus.agent import compose_daily as cd
from locus.agent import promote as pr
from locus.agent import pull_daily as pd
from locus.agent import state
from locus.db.connection import get_connection
from locus.db.migrate import migrate


@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "threads.db"
    migrate(db)
    c = get_connection(db)
    yield c
    c.close()


def _thread(conn, title="what drives carry in LNG?", type_="question", *, develop=()):
    """An owner-authored thread, optionally with development passes."""
    oid, _ = state.upsert_object(conn, type_=type_, title=title)
    state.apply_owner_edit(conn, oid, {type_: title}, source="daily:2026-07-30#Q1")
    state.set_status(conn, oid, "active")
    if develop:
        state.apply_owner_edit(
            conn, oid,
            {cd.DEVELOPMENT_KEY: [{"at": d, "text": t} for d, t in develop]},
            source="daily:2026-07-31#O1",
        )
    return oid


def _page(conn, today=date(2026, 8, 1)):
    page = cd.compose(conn, today=today)
    cd.persist(conn, page, md_path="/tmp/_home.md")
    return page


# ---------- the dead end: his own threads come back ----------


def test_an_active_question_is_offered_back(conn):
    """It became an `active` object and only `proposed` ones were ever shown again."""
    oid = _thread(conn)
    threads = cd.build_open(conn)
    assert [int(t.target_key) for t in threads] == [oid]
    assert threads[0].section == cd.SECTION_OPEN


def test_a_resolved_thread_stops_coming_back(conn):
    oid = _thread(conn)
    state.set_status(conn, oid, "archived")
    assert cd.build_open(conn) == []


def test_concepts_and_projects_are_not_threads(conn):
    """A concept is a thing that exists; a thread is something he has not finished with."""
    for t in ("concept", "project", "reading"):
        oid, _ = state.upsert_object(conn, type_=t, title=f"x {t}")
        state.set_status(conn, oid, "active")
    assert cd.build_open(conn) == []


def test_least_recently_touched_comes_first(conn):
    """The fair queue: surface what is going stale, not what he just wrote."""
    old = _thread(conn, "older question?")
    new = _thread(conn, "newer question?")
    conn.execute("UPDATE objects SET updated_at='2026-01-01T00:00:00Z' WHERE id=?", (old,))
    conn.commit()
    assert [int(t.target_key) for t in cd.build_open(conn, limit=2)] == [old, new]


def _open_on(page):
    return [t for t in page.threads if t.kind == "open"]


def test_threads_fit_the_page_and_are_anchored_and_rendered(conn):
    for i in range(5):
        _thread(conn, f"question number {i}?")
    page = _page(conn)
    threads = _open_on(page)
    assert 0 < len(threads) <= cd._FIT["ideas"] + cd._FIT["connect"]
    assert {a.anchor for a in page.anchors if a.kind == "open"} == {t.anchor for t in threads}
    body = cd.render(page)
    for t in threads:
        # Anchors have their own markup now (`_anchor`) rather than borrowing bold: `**` also
        # marks "On the shelf", and the item TITLE sat inside the anchor's own `**...**` span,
        # so styling `strong` turned all of them into accent-coloured sans.
        assert cd._anchor(t.anchor) in body


def test_prior_development_is_shown_so_he_can_continue(conn):
    _thread(conn, develop=[("2026-07-31", "term structure matters more than spot")])
    page = _page(conn)
    assert "term structure matters more than spot" in cd.render(page)


# ---------- develop / resolve / drop ----------


def _route(conn, page, text="", mark="none"):
    """Write on the first OPEN-thread region, wherever it landed in the shared `T*` series."""
    anchor = next(a.anchor for a in page.anchors if a.kind == "open")
    region = pd.ExtractedRegion(anchor, mark == "tick", text, mark=mark)
    return pd.route_regions(conn, page.page_date, [region])


def test_writing_develops_the_thread_and_keeps_it_open(conn):
    oid = _thread(conn)
    page = _page(conn)
    _route(conn, page, text="the answer is probably in the freight curve")

    obj = state.get_object(conn, oid)
    assert obj.status == "active", "developing is not finishing"
    assert cd.development_entries(obj.body) == ["the answer is probably in the freight curve"]


def test_development_appends_rather_than_replacing(conn):
    """A thread is successive passes; overwriting destroys how his view moved."""
    oid = _thread(conn, develop=[("2026-07-31", "first pass")])
    page = _page(conn)
    _route(conn, page, text="second pass")
    assert cd.development_entries(state.get_object(conn, oid).body) == [
        "first pass", "second pass"
    ]


def test_a_tick_resolves_it(conn):
    oid = _thread(conn)
    page = _page(conn)
    _route(conn, page, text="it is the freight curve", mark="tick")

    obj = state.get_object(conn, oid)
    assert obj.status == "archived"
    assert obj.body["resolution"] == "it is the freight curve"


def test_a_cross_drops_it_but_keeps_what_he_wrote(conn):
    oid = _thread(conn)
    page = _page(conn)
    _route(conn, page, text="wrong question entirely", mark="cross")

    obj = state.get_object(conn, oid)
    assert obj.status == "archived"
    assert "resolution" not in obj.body, "dropping is not resolving"
    assert cd.development_entries(obj.body) == ["wrong question entirely"]


def test_an_untouched_thread_is_re_offered(conn):
    oid = _thread(conn)
    page = _page(conn)
    _route(conn, page)
    assert state.get_object(conn, oid).status == "active"
    assert [int(t.target_key) for t in cd.build_open(conn)] == [oid]


# ---------- promotion into the corpus ----------


def test_a_thread_carrying_only_the_agents_rationale_is_not_promoted(conn):
    """The invariant: agent prose must never re-enter the corpus as his (invariant 5).

    The bar is HIS WORDS — not how many times he has written them. The first cut required
    development passes, which made every idea born from a margin note permanently unreachable by
    `locus query`: it carries the sentence he wrote and nothing else, because he wrote it once in
    a book and moved on.
    """
    oid, _ = state.upsert_object(
        conn, type_="idea", title="an agent guess",
        body={"why": "AGENT RATIONALE", "idea": "AGENT WROTE THIS"},
    )
    state.set_status(conn, oid, "active")
    assert pr.promote_thread(conn, oid, notes_dir=conn_dir(conn)) is None


def test_an_idea_he_wrote_once_still_reaches_the_corpus(conn):
    """All four of the first real mark-born ideas failed the old bar by construction."""
    oid, _ = state.upsert_object(conn, type_="idea", title="plot the reversal behaviour")
    state.apply_owner_edit(
        conn, oid, {"idea": "interesting, can we plot this behaviour?"}, source="mark:13"
    )
    state.set_status(conn, oid, "active")

    out = pr.promote_thread(conn, oid, notes_dir=conn_dir(conn))
    assert out is not None and out.status == "created"
    text = out.path.read_text(encoding="utf-8")
    assert "can we plot this behaviour?" in text
    assert "## Working notes" not in text, "no empty heading when he has not written on it yet"


def conn_dir(conn) -> Path:
    return Path(conn.execute("PRAGMA database_list").fetchone()[2]).parent / "notes"


def test_a_developed_thread_becomes_an_ingestable_note(conn):
    oid = _thread(conn, develop=[("2026-07-31", "the freight curve leads the spread by a week")])
    out = pr.promote_thread(conn, oid, notes_dir=conn_dir(conn))

    assert out is not None and out.status == "created"
    text = out.path.read_text(encoding="utf-8")
    assert text.startswith("---"), "notes_sync reads frontmatter"
    assert "category: note" in text
    assert "the freight curve leads the spread by a week" in text
    assert "what drives carry in LNG?" in text
    # It lands where notes_sync looks, and NOT under the corpus-excluded _generated/.
    assert out.path.parent.name == pr.THREADS_SUBDIR
    assert "_generated" not in str(out.path)


def test_only_owner_authored_text_reaches_the_corpus(conn):
    """The invariant. Agent rationale must never re-enter as if he wrote it."""
    oid = _thread(conn, develop=[("2026-07-31", "my own thinking")])
    # The structurer adds its own rationale, exactly as it does to any object.
    state.upsert_object(
        conn, type_="question", title="what drives carry in LNG?",
        body={"why": "AGENT GUESS about why this matters", "summary": "AGENT SUMMARY"},
    )
    text = pr.promote_thread(conn, oid, notes_dir=conn_dir(conn)).path.read_text()

    assert "my own thinking" in text
    assert "AGENT GUESS" not in text
    assert "AGENT SUMMARY" not in text


def test_re_promoting_unchanged_content_writes_nothing(conn):
    """The churn trap: promotion runs after every pull, and notes_sync re-embeds on any change.

    The first draft stamped a fresh `promoted:` timestamp into the frontmatter, which would
    have re-ingested and re-embedded every thread note on every hourly run.
    """
    oid = _thread(conn, develop=[("2026-07-31", "stable content")])
    first = pr.promote_thread(conn, oid, notes_dir=conn_dir(conn))
    before = first.path.read_text(encoding="utf-8")

    second = pr.promote_thread(conn, oid, notes_dir=conn_dir(conn))
    assert second.status == "unchanged"
    assert second.path.read_text(encoding="utf-8") == before, "the render must be deterministic"


def test_further_development_updates_the_same_note(conn):
    oid = _thread(conn, develop=[("2026-07-31", "first")])
    first = pr.promote_thread(conn, oid, notes_dir=conn_dir(conn))

    page = _page(conn)
    _route(conn, page, text="second")
    second = pr.promote_thread(conn, oid, notes_dir=conn_dir(conn))

    assert second.path == first.path, "one thread is one note, never a second file"
    assert second.status == "updated"
    body = second.path.read_text(encoding="utf-8")
    assert "first" in body and "second" in body


def test_promotion_bookkeeping_is_not_recorded_as_an_owner_edit(conn):
    """`owner_fields` decides what may enter the corpus; the agent must not enlarge it."""
    oid = _thread(conn, develop=[("2026-07-31", "content")])
    pr.promote_thread(conn, oid, notes_dir=conn_dir(conn))
    body = state.get_object(conn, oid).body
    assert pr.PROMOTED_PATH_KEY in body
    assert pr.PROMOTED_PATH_KEY not in pr.owner_fields(body)


def test_promotion_does_not_reorder_the_open_queue(conn):
    """Bookkeeping must not push a stale thread to the back as though he had touched it."""
    old = _thread(conn, "older?", develop=[("2026-07-31", "x")])
    _thread(conn, "newer?")
    conn.execute("UPDATE objects SET updated_at='2026-01-01T00:00:00Z' WHERE id=?", (old,))
    conn.commit()

    pr.promote_all(conn, notes_dir=conn_dir(conn))
    assert cd.build_open(conn, limit=2)[0].target_key == str(old)


def test_promote_all_is_idempotent(conn):
    _thread(conn, develop=[("2026-07-31", "a")])
    _thread(conn, "second?", type_="idea", develop=[("2026-07-31", "b")])
    first = [p for p in pr.promote_all(conn, notes_dir=conn_dir(conn)) if p.wrote]
    second = [p for p in pr.promote_all(conn, notes_dir=conn_dir(conn)) if p.wrote]

    assert len(first) == 2
    assert second == [], "a second run must not rewrite anything"


def test_a_thread_with_grounding_links_promotes(conn):
    """The gap that broke `locus daily-pull` live: every thread in these tests was link-free, so
    the grounding render was never exercised and deleting its helper looked safe."""
    oid = _thread(conn, develop=[("2026-07-31", "the freight curve leads the spread")])
    state.add_links(conn, oid, [
        state.ObjectLink("entity", state.entity_key("freight curve", "concept"), "about"),
        state.ObjectLink("doc", "papers/tanker.pdf", "raised_by"),
    ])
    out = pr.promote_thread(conn, oid, notes_dir=conn_dir(conn))

    assert out is not None
    text = out.path.read_text(encoding="utf-8")
    assert "freight curve (concept)" in text, "the U+001F separator must be decoded, not printed"
    assert "\x1f" not in text
    assert "tanker.pdf" in text
