"""The daily reMarkable page (agent-layer plan §9) — composition, caps, anchors, rendering.

Model-free: the composer is aggregate-only by design, so a seeded tmp DB exercises all of it.
What is asserted here is mostly the §9 longevity guardrails, because those outrank any feature
on this surface: hard caps, no guilt metrics, empty-is-valid, and — the one the pull-back
depends on — that every writable region carries a stable anchor recorded in `daily_anchors`.
"""

from __future__ import annotations

from datetime import date
from pathlib import Path

import pytest

from locus.agent import compose_daily as cd
from locus.agent import state
from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.learn import review as learn_review


@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "daily.db"
    migrate(db)
    c = get_connection(db)
    yield c
    c.close()


def _proposed(conn, title: str, *, created_at: str, why: str = "because") -> int:
    oid, _ = state.upsert_object(
        conn, type_="concept", title=title, body={"why": why}, now=lambda: created_at
    )
    state.add_links(
        conn, oid, [state.ObjectLink("entity", state.entity_key(title, "concept"), "about")]
    )
    return oid


# ---------- migration ----------


def test_0013_tables_exist(conn):
    names = {r["name"] for r in conn.execute("SELECT name FROM sqlite_master WHERE type='table'")}
    for expected in ("daily_pages", "daily_anchors", "annotations"):
        assert expected in names, f"missing table: {expected}"


def test_annotations_unique_by_date_and_anchor(conn):
    """The idempotency contract: re-pulling a page updates a region, never duplicates it."""
    import sqlite3

    conn.execute(
        "INSERT INTO annotations (page_date, anchor, captured_at) VALUES ('2026-07-30','B1','t')"
    )
    with pytest.raises(sqlite3.IntegrityError):
        conn.execute(
            "INSERT INTO annotations (page_date, anchor, captured_at) VALUES ('2026-07-30','B1','t')"
        )


# ---------- §9 guardrails ----------


def test_empty_is_a_valid_calm_state(conn):
    page = cd.compose(conn)
    assert page.is_empty
    body = cd.render(page)
    assert "Nothing to surface today." in body
    # No empty section headings, and above all no counts to feel behind on.
    for banned in ("Connections", "Recall", "Read next", "Awaiting your call"):
        assert banned not in body


def test_blessing_section_is_capped_and_never_announces_the_backlog(conn):
    # 12 pending — four times the cap. 43 must not become a wall (§9).
    for i in range(12):
        _proposed(conn, f"concept {i}", created_at=f"2026-07-{i + 1:02d}T00:00:00+00:00")

    page = cd.compose(conn)
    assert len(page.blessings) == cd.MAX_BLESSINGS

    body = cd.render(page)
    # No guilt metrics: the number pending appears nowhere on the page.
    assert "12" not in body
    for phrase in ("pending", "remaining", "unread", "streak", "of 12"):
        assert phrase not in body.lower()


def test_blessings_are_offered_oldest_first(conn):
    """Oldest-first is the fair queue, and it is what makes the backlog actually drain."""
    _proposed(conn, "newest", created_at="2026-07-30T00:00:00+00:00")
    _proposed(conn, "oldest", created_at="2026-01-01T00:00:00+00:00")
    _proposed(conn, "middle", created_at="2026-04-01T00:00:00+00:00")

    titles = [b.title for b in cd.compose(conn).blessings]
    assert titles == ["oldest", "middle", "newest"]


def test_blessed_objects_are_not_re_offered(conn):
    oid = _proposed(conn, "already blessed", created_at="2026-01-01T00:00:00+00:00")
    state.set_status(conn, oid, "active")
    assert cd.compose(conn).blessings == []


def test_recalls_are_capped(conn):
    for i in range(9):
        learn_review.schedule_prompt(
            conn, prompt_kind="object", prompt_ref=str(i), today=date(2026, 1, 1)
        )
    page = cd.compose(conn, today=date(2026, 6, 1))
    assert len(page.recalls) == cd.MAX_RECALLS


# ---------- anchors ----------


def test_every_writable_region_has_an_anchor_and_it_is_rendered(conn):
    _proposed(conn, "alpha", created_at="2026-01-01T00:00:00+00:00")
    _proposed(conn, "beta", created_at="2026-01-02T00:00:00+00:00")
    learn_review.schedule_prompt(
        conn, prompt_kind="object", prompt_ref="1", today=date(2026, 1, 1)
    )

    page = cd.compose(conn, today=date(2026, 6, 1))
    body = cd.render(page)

    assert page.anchors, "a page with content must carry anchors"
    for a in page.anchors:
        assert a.anchor in body, f"anchor {a.anchor} not printed on the page"
    # Anchors are unique within a page — the pull-back keys on them.
    labels = [a.anchor for a in page.anchors]
    assert len(labels) == len(set(labels))


def test_persist_records_anchors_and_a_rebuild_replaces_them(conn):
    _proposed(conn, "alpha", created_at="2026-01-01T00:00:00+00:00")
    page = cd.compose(conn, today=date(2026, 6, 1))
    cd.persist(conn, page, md_path="/tmp/_home.md")

    stored = cd.anchors_for(conn, page.page_date)
    assert {a.anchor for a in page.anchors} == set(stored)
    assert stored["B1"].target_kind == "object"

    # A second object is proposed and the page is rebuilt the same day: the anchors for that
    # date are REPLACED, so B1 cannot keep pointing at what used to sit there.
    _proposed(conn, "zeta", created_at="2025-01-01T00:00:00+00:00")  # older => takes B1
    page2 = cd.compose(conn, today=date(2026, 6, 1))
    cd.persist(conn, page2, md_path="/tmp/_home.md")

    stored2 = cd.anchors_for(conn, page.page_date)
    assert stored2["B1"].label == "zeta"
    assert len(conn.execute("SELECT * FROM daily_pages").fetchall()) == 1


def test_persist_leaves_annotations_alone(conn):
    """Evidence the owner produced outlives a rebuild of the page that prompted it."""
    _proposed(conn, "alpha", created_at="2026-01-01T00:00:00+00:00")
    page = cd.compose(conn, today=date(2026, 6, 1))
    cd.persist(conn, page, md_path="/tmp/_home.md")
    conn.execute(
        "INSERT INTO annotations (page_date, anchor, text, captured_at) VALUES (?,?,?,?)",
        (page.page_date, "B1", "owner wrote this", "t"),
    )
    conn.commit()

    cd.persist(conn, cd.compose(conn, today=date(2026, 6, 1)), md_path="/tmp/_home.md")
    row = conn.execute("SELECT text FROM annotations WHERE page_date=? AND anchor='B1'",
                       (page.page_date,)).fetchone()
    assert row["text"] == "owner wrote this"


# ---------- rendering ----------


def test_blessing_rows_render_a_tickable_box_and_writing_lines(conn):
    _proposed(conn, "alpha", created_at="2026-01-01T00:00:00+00:00", why="it recurs in two docs")
    body = cd.render(cd.compose(conn, today=date(2026, 6, 1)))

    assert "[   ]" in body, "the tick box must be ASCII — the ballot-box glyph has no font cover"
    assert "it recurs in two docs" in body, "the one-line why must be on the page"
    assert "***" in body, "ruled writing lines must use *** (--- is eaten by pandoc)"
    assert " " not in body, "a non-breaking space stops the next line parsing as a rule"


def test_rendered_page_has_no_nbsp_anywhere(conn):
    """U+00A0 is not markdown whitespace: it silently changes how the next line parses."""
    _proposed(conn, "alpha", created_at="2026-01-01T00:00:00+00:00")
    learn_review.schedule_prompt(
        conn, prompt_kind="object", prompt_ref="1", today=date(2026, 1, 1)
    )
    assert " " not in cd.render(cd.compose(conn, today=date(2026, 6, 1)))


def test_entity_grounding_is_printed_readably(conn):
    """The entity key separator is U+001F — invisible, so the raw key reads as one run-on word."""
    _proposed(conn, "matching engine", created_at="2026-01-01T00:00:00+00:00")
    body = cd.render(cd.compose(conn, today=date(2026, 6, 1)))
    assert "matching engine (concept)" in body
    assert "\x1f" not in body


# ---------- the reading marks section (Loop B's consumer) ----------


def _mark(conn, *, uri="books/apm.pdf", page=70, text, kind="underline", at="2026-07-30T10:00:00"):
    with conn:
        conn.execute(
            "INSERT INTO pdf_annotations (source_uri, pdf_page, kind, bbox_key, covered_text, "
            "in_margin, captured_at) VALUES (?,?,?,?,?,0,?)",
            (uri, page, kind, f"k{page}-{len(text)}", text, at),
        )


_PASSAGE = "the extreme case in which every hedge fund holds a copy of the same portfolio"


def test_marked_passages_are_surfaced(conn):
    """Loop B stored 26 marks with NOTHING reading them — capture was write-only."""
    _mark(conn, text=_PASSAGE)
    marks = cd.build_marks(conn)
    assert len(marks) == 1
    assert marks[0].passage == _PASSAGE
    assert marks[0].page == 71, "pages are printed 1-based for a human"


def test_a_mark_already_turned_into_something_is_not_re_offered(conn):
    """Once a passage has become an idea it has moved on; re-offering it is an unread count."""
    _mark(conn, text=_PASSAGE)
    conn.execute("UPDATE pdf_annotations SET object_id=1")
    conn.commit()
    assert cd.build_marks(conn) == []


def test_transcribing_the_ink_does_not_hide_the_mark(conn):
    """`note` is the transcription, NOT a has-been-dealt-with flag.

    The first cut filtered on `note IS NULL`, so reading his handwriting HID the mark —
    exactly backwards, since a passage he wrote a paragraph about is the most worth returning.
    """
    _mark(conn, text=_PASSAGE)
    conn.execute("UPDATE pdf_annotations SET note='is this any good?'")
    conn.commit()
    marks = cd.build_marks(conn)
    assert len(marks) == 1
    assert marks[0].note == "is this any good?"


def test_his_own_words_rank_above_a_bare_underline(conn):
    _mark(conn, uri="books/a.pdf", page=1, text=_PASSAGE + " bare", at="2026-07-31T10:00:00")
    _mark(conn, uri="books/b.pdf", page=2, text=_PASSAGE + " annotated", at="2026-07-30T10:00:00")
    conn.execute("UPDATE pdf_annotations SET note='my objection' WHERE source_uri='books/b.pdf'")
    conn.commit()
    assert cd.build_marks(conn, limit=1)[0].source_uri == "books/b.pdf"


def test_his_comment_is_printed_with_the_passage(conn):
    _mark(conn, text=_PASSAGE)
    conn.execute("UPDATE pdf_annotations SET note='no momentum in Japan??'")
    conn.commit()
    body = cd.render(cd.compose(conn, today=date(2026, 7, 31)))
    assert "no momentum in Japan??" in body
    assert _PASSAGE in body, "the objection and the claim must travel together"


def test_a_stray_stroke_is_not_a_passage(conn):
    """Below a few words a mark means nothing when it comes back a week later."""
    _mark(conn, text="the")
    assert cd.build_marks(conn) == []


def test_one_passage_per_document(conn):
    """Two quotes from the same chapter are one thought; a book must not own the section."""
    _mark(conn, page=70, text=_PASSAGE)
    _mark(conn, page=71, text=_PASSAGE + " and again on the next page entirely")
    _mark(conn, uri="books/other.pdf", page=5, text=_PASSAGE + " but in a different book")
    assert {m.source_uri for m in cd.build_marks(conn)} == {"books/apm.pdf", "books/other.pdf"}


def test_marks_are_capped_and_anchored_and_rendered(conn):
    for i in range(5):
        _mark(conn, uri=f"books/b{i}.pdf", page=i, text=f"{_PASSAGE} number {i}")
    page = cd.compose(conn, today=date(2026, 7, 30))
    assert len(page.marks) <= cd.MAX_MARKS

    anchors = {a.anchor for a in page.anchors if a.kind == "mark"}
    assert anchors == {m.anchor for m in page.marks}
    body = cd.render(page)
    for m in page.marks:
        assert f"**{m.anchor}.**" in body, "an unanchored region cannot be routed back"


def test_marks_degrade_silently_when_loop_b_has_never_run(conn):
    conn.execute("DROP TABLE pdf_annotations")
    conn.commit()
    assert cd.build_marks(conn) == []
