"""Reading his annotations back — `capture/review.py` and the `annotations` MCP tool.

Model-free and network-free. The device fetch and the rmapi listing are injected, so what is
under test is the logic that decides WHICH page to show and whether the numbers on it are right.

The two things worth breaking a build over:

  * the page-number convention. `pdf_annotations.pdf_page` is 0-based and every number a human
    reads is 1-based, so an off-by-one here shows the wrong page with total confidence.
  * the blank-mark path. 29 of 109 highlights on his live documents covered no text (they are
    over figures), and they are the whole reason the image register exists — a change that
    quietly filters them out would remove the feature's point while every other test passed.
"""

from __future__ import annotations

import sqlite3
from pathlib import Path

import pytest

from locus.capture import review
from locus.db.connection import get_connection
from locus.db.migrate import migrate

URI = "vault/incoming/papers/book.pdf"


@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "review.db"
    migrate(db)
    c = get_connection(db)
    yield c
    c.close()


def _mark(conn, *, page: int, kind="highlight", covered="", line="", note="",
          intent=None, margin=0, uri=URI, uuid="uuid-1", key=None):
    """`page` is 0-BASED here, as the table stores it."""
    conn.execute(
        "INSERT INTO pdf_annotations (source_uri, doc_uuid, pdf_page, kind, bbox_key, "
        "covered_text, line_text, note, intent, in_margin, captured_at) "
        "VALUES (?,?,?,?,?,?,?,?,?,?,'2026-09-05T00:00:00+00:00')",
        (uri, uuid, page, kind, key or f"k{page}-{kind}-{covered[:6]}-{note[:6]}",
         covered, line, note, intent, margin),
    )
    conn.commit()


def _document(conn, *, uri=URI, title="A Book"):
    conn.execute(
        "INSERT INTO documents (source_uri, content_hash, source_type, raw_path, title, category, "
        "ingest_model, ingested_at) "
        "VALUES (?,?,'pdf','raw/x.pdf',?,'paper','test','2026-09-01T00:00:00+00:00')",
        (uri, f"hash-{uri}", title),
    )
    conn.commit()


def _target(conn, *, uri=URI, device_path="/reading_list/A Book", uuid="uuid-1"):
    conn.execute(
        "INSERT INTO reading_targets (doc_uuid, device_path, source_uri, linked_by, created_at, "
        "title) VALUES (?,?,?,'manual','2026-09-01T00:00:00+00:00','A Book')",
        (uuid, device_path, uri),
    )
    conn.commit()


# ---------- the page-number convention ----------

def test_stored_pages_are_zero_based_and_shown_one_based(conn):
    """The single highest-consequence detail in the module."""
    _document(conn)
    _mark(conn, page=0, covered="first page")
    _mark(conn, page=53, covered="page fifty-four")

    doc = review.load(conn, URI)

    assert [m.page for m in doc.marks] == [1, 54]           # printed
    assert [m.page_index for m in doc.marks] == [0, 53]     # rendered
    assert doc.page_indexes == [0, 53]
    assert "p.1" in doc.render() and "p.54" in doc.render()


def test_page_filter_takes_the_number_he_would_read(conn):
    _document(conn)
    _mark(conn, page=53, covered="on fifty-four")
    _mark(conn, page=99, covered="on one hundred")

    doc = review.load(conn, URI, page=54)

    assert [m.covered_text for m in doc.marks] == ["on fifty-four"]


# ---------- blank marks: the reason images exist ----------

def test_a_mark_covering_nothing_is_reported_not_dropped(conn):
    """Ink over a figure covers no words. Those marks are the ones the image register is FOR,
    so they must survive to the output and say why they look empty."""
    _document(conn)
    _mark(conn, page=41, covered="", note="", kind="highlight")

    doc = review.load(conn, URI)
    out = doc.render()

    assert doc.blank_count == 1
    assert "covering no text" in out
    assert "page image" in out


def test_a_mark_with_only_handwriting_is_not_blank(conn):
    """A margin note covers nothing but says plenty — it is not the figure case."""
    _document(conn)
    _mark(conn, page=41, covered="", note="what does this mean?", kind="margin_note", margin=1)

    doc = review.load(conn, URI)

    assert doc.blank_count == 0
    assert "what does this mean?" in doc.render()


def test_image_selection_prefers_pages_whose_text_failed(conn):
    """Blank-mark pages first, then the densest — a page the text register handled fine is the
    one least worth spending context on."""
    from locus.mcp_server import _pages_to_image

    _document(conn)
    for i in range(4):                       # p.11: four marks, all with text
        _mark(conn, page=10, covered=f"words {i}", key=f"dense{i}")
    _mark(conn, page=20, covered="", key="blank")   # p.21: one blank mark

    doc = review.load(conn, URI)

    assert _pages_to_image(doc, 1) == [20]
    assert _pages_to_image(doc, 2) == [10, 20]


# ---------- resolution ----------

def test_resolve_prefers_an_exact_uri_over_a_title_substring(conn):
    _document(conn, title="Portfolio Management")
    _mark(conn, page=1, covered="x")
    other = "vault/incoming/papers/other.pdf"
    _document(conn, uri=other, title="Portfolio Management Notes")
    _mark(conn, page=1, covered="y", uri=other, uuid="uuid-2")

    assert [c[0] for c in review.resolve(conn, URI)] == [URI]
    assert len(review.resolve(conn, "portfolio")) == 2


def test_documents_with_marks_titles_an_unmapped_reading_target(conn):
    """A document still in Reading/Proposed keys its marks by device path and has no corpus row
    (§14). Those are exactly the ones he is reading now, so they must not show as a bare path."""
    _target(conn, uri="/Reading/Proposed/Thing", uuid="uuid-9")
    _mark(conn, page=0, covered="x", uri="/Reading/Proposed/Thing", uuid="uuid-9")

    assert review.documents_with_marks(conn)[0][1] == "A Book"


# ---------- the stale device pointer ----------

def test_stale_device_path_is_resolved_by_uuid_and_repaired(conn):
    """Measured live 2026-09-05: the book he reads daily was still recorded under
    `/reading_list/...`, a path the 2026-08 device reorganisation deleted. Marks kept arriving
    only because loop_b rebuilds the index every run instead of trusting this column."""
    _document(conn)
    _target(conn, device_path="/reading_list/A Book")
    _mark(conn, page=0, covered="x")
    doc = review.load(conn, URI)
    assert doc.device_path == "/reading_list/A Book"

    path, repaired = review.locate_device_copy(
        conn, doc,
        index_fn=lambda: {"uuid-1": ("/Reading/In-Progress/A Book", "A Book")},
        runner=lambda argv: (1, "", "file doesn't exist"),      # every stat fails
    )

    assert path == "/Reading/In-Progress/A Book"
    assert repaired is True
    # Written back, or every later call pays the slow listing again — and `reading/sweep.py`
    # reads the same column and stays broken.
    row = conn.execute("SELECT device_path FROM reading_targets WHERE source_uri=?", (URI,)).fetchone()
    assert row["device_path"] == "/Reading/In-Progress/A Book"


def test_a_document_no_longer_on_the_device_says_so(conn):
    """A deleted document is a real answer, not an error to paper over."""
    _document(conn)
    _target(conn, device_path="/gone/A Book")
    _mark(conn, page=0, covered="x")

    with pytest.raises(RuntimeError, match="not on the device"):
        review.locate_device_copy(
            conn, review.load(conn, URI),
            index_fn=lambda: {},
            runner=lambda argv: (1, "", "file doesn't exist"),
        )


# ---------- rendering ----------

def test_only_the_requested_pages_are_rasterised(tmp_path: Path, monkeypatch):
    """`transcribe.render_pdf_pages` does the whole document, which is right for a 4-page daily
    page and absurd for a 300-page book of which he marked nine pages."""
    rendered: list[list[int]] = []

    def fake_render(pdf_path, page_indexes, *, dpi):
        rendered.append(list(page_indexes))
        return {i: b"png" for i in page_indexes}

    monkeypatch.setattr(review, "_render_pages", fake_render)
    monkeypatch.setattr(
        "locus.capture.rmdoc.composite_pdf", lambda rmdoc, out: Path(out)
    )

    out = review.annotated_page_pngs(
        "/Reading/In-Progress/A Book", [3, 9],
        fetch=lambda path, dest: Path(dest) / "x.rmdoc",
        read=lambda p: object(),
    )

    assert rendered == [[3, 9]]
    assert sorted(out) == [3, 9]


def test_no_pages_requested_costs_no_fetch():
    called = []
    out = review.annotated_page_pngs(
        "/x", [], fetch=lambda *a: called.append(1), read=lambda p: None
    )
    assert out == {} and not called
