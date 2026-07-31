"""Loop B: reMarkable stroke geometry -> which passage was marked.

Model-free and device-free. A tiny PDF is built in-memory with pymupdf and strokes are
synthesised in reMarkable screen coordinates, so the coordinate transform and the text-linking
are both exercised end to end without the tablet or the cloud.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from locus.capture import annotate as ann
from locus.capture.rmdoc import SCREEN_H, SCREEN_W, AnnotatedPage, Stroke, to_page_coords
from locus.db.connection import get_connection
from locus.db.migrate import migrate

pymupdf = pytest.importorskip("pymupdf")

PAGE_W, PAGE_H = 432.0, 648.0   # the Advanced Portfolio Management geometry


@pytest.fixture()
def page():
    doc = pymupdf.open()
    p = doc.new_page(width=PAGE_W, height=PAGE_H)
    p.insert_text((72, 100), "Momentum is a robust anomaly across equity markets.", fontsize=11)
    p.insert_text((72, 130), "It has been observed in the US, Europe and Asia.", fontsize=11)
    p.insert_text((72, 160), "The effect is weaker after transaction costs.", fontsize=11)
    yield p
    doc.close()


@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "ann.db"
    migrate(db)
    c = get_connection(db)
    yield c
    c.close()


def _stroke(pts):
    return Stroke(to_page_coords(pts, page_width=PAGE_W, page_height=PAGE_H))


# ---------- coordinate transform ----------


def test_screen_centre_maps_to_page_centre():
    """rm x is measured from the PAGE centre, not the screen's left edge."""
    (x, _), = to_page_coords([(0.0, 0.0)], page_width=PAGE_W, page_height=PAGE_H)
    assert x == pytest.approx(PAGE_W / 2)


def test_page_is_fit_to_the_constraining_dimension():
    """A portrait page taller than the screen aspect fits by HEIGHT.

    The width-fit alternative was tried against the real book and put an underline a full line
    of text too high (2026-07-30), which is why this is pinned.
    """
    (_, y), = to_page_coords([(0.0, SCREEN_H)], page_width=PAGE_W, page_height=PAGE_H)
    assert y == pytest.approx(PAGE_H)


def test_strokes_outside_the_page_are_not_clipped():
    """The screen is wider than a portrait page: marginalia legitimately has no page x."""
    (x, _), = to_page_coords([(SCREEN_W / 2, 100.0)], page_width=PAGE_W, page_height=PAGE_H)
    assert x > PAGE_W


# ---------- the two .content pagemap schemas ----------


def test_new_format_pagemap_is_read_from_cpages():
    from locus.capture.rmdoc import _page_index

    assert _page_index(
        {"cPages": {"pages": [{"id": "a", "redir": {"value": 0}},
                              {"id": "b", "redir": {"value": 3}}]}}
    ) == {"a": 0, "b": 3}


def test_old_format_pagemap_is_read_from_the_parallel_lists():
    """formatVersion 1 — what `rmapi put` produces, so EVERY Locus-delivered page lands here.

    Missing this schema made the daily page report "not pushed back yet" while carrying 183
    strokes (2026-07-30); every stroke layer was dropped as unplaceable.
    """
    from locus.capture.rmdoc import _page_index

    assert _page_index(
        {"formatVersion": 1, "pages": ["a", "b", "c"], "redirectionPageMap": [0, 1, 2]}
    ) == {"a": 0, "b": 1, "c": 2}


def test_an_inserted_page_has_no_pdf_page_and_is_dropped():
    """-1 means a page the owner ADDED; it must not be guessed onto page 0."""
    from locus.capture.rmdoc import _page_index

    assert _page_index(
        {"pages": ["a", "ins", "b"], "redirectionPageMap": [0, -1, 1]}
    ) == {"a": 0, "b": 1}


# ---------- the spend guard's key ----------


def test_ink_hash_is_stable_across_renderings_and_moves_with_the_ink():
    """The guard keys on strokes because compositing is NOT byte-reproducible."""
    from locus.capture.rmdoc import RmDoc, ink_hash

    a = RmDoc("u", b"%PDF", [AnnotatedPage(0, "p", [Stroke([(1.0, 2.0), (3.0, 4.0)])])])
    same = RmDoc("u", b"%PDF-DIFFERENT-BYTES",
                 [AnnotatedPage(0, "p", [Stroke([(1.0, 2.0), (3.0, 4.0)])])])
    more = RmDoc("u", b"%PDF", [AnnotatedPage(0, "p", [Stroke([(1.0, 2.0), (3.0, 9.0)])])])

    assert ink_hash(a) == ink_hash(same)
    assert ink_hash(a) != ink_hash(more)


def test_composite_draws_the_ink_onto_the_page(page):
    """The daily page needs PIXELS: reading handwriting is a vision job, not a geometry one."""
    import pymupdf

    from locus.capture.rmdoc import RmDoc, composite_pdf

    doc = pymupdf.open()
    doc.new_page(width=PAGE_W, height=PAGE_H)
    blank = doc.tobytes()
    doc.close()

    strokes = [Stroke([(80.0, 300.0), (300.0, 300.0), (300.0, 360.0)])]
    out = composite_pdf(RmDoc("u", blank, [AnnotatedPage(0, "p", strokes)]), "/tmp/_ink.pdf")

    marked = pymupdf.open(str(out))
    try:
        drawn = marked[0].get_drawings()
        assert drawn, "the stroke must appear on the page or vision has nothing to read"
    finally:
        marked.close()
        out.unlink(missing_ok=True)


# ---------- classification ----------


def test_flat_wide_stroke_is_an_underline():
    kind, margin = ann.classify((100.0, 300.0, 300.0, 305.0), page_width=PAGE_W)
    assert (kind, margin) == ("underline", False)


def test_tall_narrow_stroke_is_a_bracket():
    kind, margin = ann.classify((100.0, 300.0, 108.0, 400.0), page_width=PAGE_W)
    assert (kind, margin) == ("bracket", False)


def test_a_stroke_past_the_right_edge_is_a_margin_note():
    kind, margin = ann.classify((PAGE_W - 5, 300.0, PAGE_W + 60, 340.0), page_width=PAGE_W)
    assert (kind, margin) == ("margin_note", True)


# ---------- text linking ----------


def test_an_underline_picks_up_the_line_it_sits_under(page):
    """Hand-drawn underlines sit BELOW the glyph box; a naive intersect finds nothing."""
    y = 103.0            # just under the first line's baseline
    strokes = [Stroke([(80.0, y), (300.0, y + 1.0)])]
    marks = ann.marks_for_page(page, AnnotatedPage(0, "p", strokes))
    assert len(marks) == 1
    assert marks[0].kind == "underline"
    assert "Momentum" in marks[0].covered_text
    assert "anomaly" in marks[0].covered_text


def test_an_underline_does_not_capture_the_line_below(page):
    y = 103.0
    marks = ann.marks_for_page(page, AnnotatedPage(0, "p", [Stroke([(80.0, y), (300.0, y)])]))
    assert "transaction" not in marks[0].covered_text


def test_a_bracket_selects_every_line_it_spans(page):
    strokes = [Stroke([(60.0, 92.0), (62.0, 135.0)])]
    marks = ann.marks_for_page(page, AnnotatedPage(0, "p", strokes))
    assert marks[0].kind == "bracket"
    assert "Momentum" in marks[0].covered_text
    assert "Europe" in marks[0].covered_text


def test_a_margin_note_reports_the_passage_at_its_height(page):
    """There is no text under marginalia; the useful answer is what it sits beside."""
    strokes = [Stroke([(PAGE_W - 2, 92.0), (PAGE_W + 40, 105.0)])]
    marks = ann.marks_for_page(page, AnnotatedPage(0, "p", strokes))
    assert marks[0].kind == "margin_note"
    assert marks[0].in_margin
    assert "Momentum" in marks[0].covered_text


def test_nearby_strokes_cluster_into_one_annotation(page):
    """Handwriting is dozens of strokes that mean one thing."""
    strokes = [Stroke([(300.0 + i * 3, 100.0), (302.0 + i * 3, 108.0)]) for i in range(20)]
    marks = ann.marks_for_page(page, AnnotatedPage(0, "p", strokes))
    assert len(marks) == 1
    assert marks[0].stroke_count == 20


def test_far_apart_strokes_stay_separate(page):
    strokes = [Stroke([(80.0, 103.0), (300.0, 103.0)]), Stroke([(80.0, 163.0), (300.0, 163.0)])]
    marks = ann.marks_for_page(page, AnnotatedPage(0, "p", strokes))
    assert len(marks) == 2


# ---------- persistence ----------


def test_store_is_idempotent_by_page_and_box(conn, page):
    strokes = [Stroke([(80.0, 103.0), (300.0, 103.0)])]
    marks = ann.marks_for_page(page, AnnotatedPage(0, "p", strokes))

    ann.store_marks(conn, marks, source_uri="books/apm.pdf")
    ann.store_marks(conn, marks, source_uri="books/apm.pdf")

    rows = conn.execute("SELECT * FROM pdf_annotations").fetchall()
    assert len(rows) == 1, "re-reading the same ink must revise, never duplicate"
    assert "Momentum" in rows[0]["covered_text"]


def test_re_sync_does_not_discard_a_derived_note(conn, page):
    """`note`/`object_id` are derived or owner-supplied; re-reading the ink must not wipe them."""
    marks = ann.marks_for_page(
        page, AnnotatedPage(0, "p", [Stroke([(80.0, 103.0), (300.0, 103.0)])])
    )
    ann.store_marks(conn, marks, source_uri="books/apm.pdf")
    conn.execute("UPDATE pdf_annotations SET note='idea for the regime project'")
    conn.commit()

    ann.store_marks(conn, marks, source_uri="books/apm.pdf")
    assert conn.execute("SELECT note FROM pdf_annotations").fetchone()["note"] == (
        "idea for the regime project"
    )


def test_marks_on_different_pages_are_distinct(conn, page):
    s = [Stroke([(80.0, 103.0), (300.0, 103.0)])]
    ann.store_marks(conn, ann.marks_for_page(page, AnnotatedPage(0, "p", s)),
                    source_uri="books/apm.pdf")
    ann.store_marks(conn, ann.marks_for_page(page, AnnotatedPage(7, "q", s)),
                    source_uri="books/apm.pdf")
    assert conn.execute("SELECT COUNT(*) c FROM pdf_annotations").fetchone()["c"] == 2


# ---------- the idea object type ----------


def test_idea_is_a_valid_object_type(conn):
    from locus.agent import state

    oid, created = state.upsert_object(
        conn, type_="idea", title="factor crowding applies to the regime project"
    )
    assert created and oid
    assert state.get_object(conn, oid).type == "idea"


def test_existing_object_types_survive_the_rebuild(conn):
    from locus.agent import state

    for t in ("project", "concept", "question", "reading"):
        assert state.upsert_object(conn, type_=t, title=f"x {t}")[1]


# ---------- reading the handwriting beside a mark ----------


class _FakeVision:
    """Returns a fixed reply and counts calls, so spend guards are testable."""

    def __init__(self, reply='{"text": "is a factor like a feature in ML?"}'):
        self.reply = reply
        self.calls = 0
        self.messages = self

    def create(self, **kw):
        self.calls += 1
        block = type("B", (), {"type": "text", "text": self.reply})()
        return type("R", (), {"content": [block]})()


def _ink(n_strokes: int, *, page=0):
    """A mark carrying `n_strokes` of scribble."""
    from locus.capture import annotate as a

    strokes = [Stroke([(100.0 + i, 300.0), (104.0 + i, 312.0), (108.0 + i, 300.0)])
               for i in range(n_strokes)]
    bbox = (100.0, 300.0, 100.0 + n_strokes + 8, 312.0)
    return a.Mark(kind="margin_note", pdf_page=page, bbox=bbox, covered_text="ctx",
                  stroke_count=n_strokes, strokes=strokes)


def test_a_gesture_is_not_worth_a_model_call():
    """Underlines and brackets carry no words. The real book's distribution has an empty band
    between 2 strokes and 13, so the threshold sits inside it."""
    from locus.capture import mark_text as mt

    assert not mt.has_ink(_ink(1))
    assert not mt.has_ink(_ink(2))
    assert mt.has_ink(_ink(13))


def test_ink_renders_to_a_png_without_the_page():
    """Rendered on its own: marginalia falls OUTSIDE the page rect and a composite drops it."""
    from locus.capture import mark_text as mt

    png = mt.render_ink(_ink(20))
    assert png.startswith(b"\x89PNG")


def test_a_mark_without_stroke_geometry_renders_nothing(conn):
    """A Mark rebuilt from the DB has no strokes; that must not raise."""
    from locus.capture import annotate as a
    from locus.capture import mark_text as mt

    assert mt.render_ink(a.Mark(kind="mark", pdf_page=0, bbox=(0, 0, 10, 10))) == b""


def test_transcription_is_stored_against_the_right_mark(conn):
    from locus.capture import annotate as a
    from locus.capture import mark_text as mt

    mark = _ink(20)
    a.store_marks(conn, [mark], source_uri="books/apm.pdf")
    client = _FakeVision()
    assert mt.transcribe_marks(conn, [mark], source_uri="books/apm.pdf", client=client) == 1

    row = conn.execute("SELECT note, covered_text FROM pdf_annotations").fetchone()
    assert row["note"] == "is a factor like a feature in ML?"
    assert row["covered_text"] == "ctx", "the passage and the comment travel together"


def test_already_transcribed_ink_is_not_re_read(conn):
    """Re-running must not re-pay for ink it has already read."""
    from locus.capture import annotate as a
    from locus.capture import mark_text as mt

    mark = _ink(20)
    a.store_marks(conn, [mark], source_uri="books/apm.pdf")
    client = _FakeVision()
    mt.transcribe_marks(conn, [mark], source_uri="books/apm.pdf", client=client)
    mt.transcribe_marks(conn, [mark], source_uri="books/apm.pdf", client=client)
    assert client.calls == 1


def test_gestures_cost_nothing(conn):
    from locus.capture import annotate as a
    from locus.capture import mark_text as mt

    marks = [_ink(1), _ink(2, page=1)]
    a.store_marks(conn, marks, source_uri="books/apm.pdf")
    client = _FakeVision()
    assert mt.transcribe_marks(conn, marks, source_uri="books/apm.pdf", client=client) == 0
    assert client.calls == 0


def test_the_spend_cap_is_honoured(conn):
    from locus.capture import annotate as a
    from locus.capture import mark_text as mt

    marks = [_ink(20, page=i) for i in range(5)]
    a.store_marks(conn, marks, source_uri="books/apm.pdf")
    client = _FakeVision()
    assert mt.transcribe_marks(
        conn, marks, source_uri="books/apm.pdf", client=client, limit=2
    ) == 2
    assert client.calls == 2


def test_an_unparseable_reply_writes_nothing(conn):
    """A bad reply loses one transcription; it must never write a guess."""
    from locus.capture import annotate as a
    from locus.capture import mark_text as mt

    mark = _ink(20)
    a.store_marks(conn, [mark], source_uri="books/apm.pdf")
    mt.transcribe_marks(
        conn, [mark], source_uri="books/apm.pdf", client=_FakeVision("sorry, I cannot read this")
    )
    assert conn.execute("SELECT note FROM pdf_annotations").fetchone()["note"] is None


def test_shapes_with_no_words_leave_the_note_empty(conn):
    """A dense squiggle is ink, but there is nothing to transcribe."""
    from locus.capture import annotate as a
    from locus.capture import mark_text as mt

    mark = _ink(20)
    a.store_marks(conn, [mark], source_uri="books/apm.pdf")
    assert mt.transcribe_marks(
        conn, [mark], source_uri="books/apm.pdf", client=_FakeVision('{"text": ""}')
    ) == 0
    assert conn.execute("SELECT note FROM pdf_annotations").fetchone()["note"] is None
