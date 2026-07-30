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
