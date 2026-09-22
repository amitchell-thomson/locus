"""A native NOTEBOOK — handwriting on the tablet, no PDF underneath — reads like any document.

THE BUG (2026-09-22). He asked a session to read his call notes, a notebook called "Jump call" in
`Rough notes/`. `markups` answered "not a PDF-backed rmdoc", because `read_rmdoc` refused any
bundle without a `.pdf` in it. The one kind of document that is ALL his handwriting was the one
kind the reader could not open.

The bundle here is a real zip in the tablet's shape (`.content` + `.rm` layers, no `.pdf`); only
the `.rm` stroke decoder is stubbed, since producing a v6 scene file is rmscene's business.
"""

from __future__ import annotations

import json
import types
import zipfile
from pathlib import Path

import pytest

from locus.capture import review, rmdoc
from locus.db.connection import get_connection
from locus.db.migrate import migrate

UUID = "5f0c2a10-1111-4222-8333-944455556666"


@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "nb.db"
    migrate(db)
    c = get_connection(db)
    yield c
    c.close()


def _notebook(tmp_path: Path, monkeypatch, *, pages=("p-a", "p-b", "p-c"), inked=("p-a", "p-c")):
    """A notebook bundle: v2 manifest with no `redir` anywhere, `.rm` layers on `inked` pages."""
    content = {
        "fileType": "notebook",
        "formatVersion": 2,
        "cPages": {"pages": [{"id": pid} for pid in pages]},
    }
    path = tmp_path / "Jump call.rmdoc"
    with zipfile.ZipFile(path, "w") as z:
        z.writestr(f"{UUID}.content", json.dumps(content))
        z.writestr(f"{UUID}.metadata", json.dumps({"visibleName": "Jump call"}))
        for pid in inked:
            z.writestr(f"{UUID}/{pid}.rm", pid.encode())

    # One diagonal stroke per layer, in screen coordinates (x is 0 at the page's centre).
    monkeypatch.setattr(
        rmdoc, "_parse_rm", lambda data: [([(-300.0, 200.0), (300.0, 900.0)], 2, 0)]
    )
    return path


def test_a_notebook_bundle_parses_instead_of_raising(tmp_path, monkeypatch):
    doc = rmdoc.read_rmdoc(_notebook(tmp_path, monkeypatch))

    assert doc.is_notebook
    assert doc.pdf_bytes == b""
    assert doc.doc_uuid == UUID, "the uuid comes from the manifest when there is no PDF to name it"
    assert doc.page_map == [None, None, None], "every page of a notebook is a blank tablet page"
    assert [p.pdf_page for p in doc.pages] == [0, 2]
    assert all(p.inserted for p in doc.pages)


def test_a_bundle_with_no_manifest_is_still_refused(tmp_path):
    path = tmp_path / "junk.rmdoc"
    with zipfile.ZipFile(path, "w") as z:
        z.writestr("readme.txt", "nothing here")
    with pytest.raises(ValueError, match="no .content"):
        rmdoc.read_rmdoc(path)


def test_every_page_of_a_short_notebook_renders_including_the_blank_one(tmp_path, monkeypatch):
    """The blank middle page comes back too: a notebook is short, and a page he left empty
    between two written ones is part of how the notes read."""
    import pymupdf

    doc = rmdoc.read_rmdoc(_notebook(tmp_path, monkeypatch))
    wanted = review.pages_to_render(doc, 12)
    pngs = rmdoc.composite_pages_with_margins(doc, wanted, dpi=72)

    assert sorted(pngs) == [0, 1, 2]
    img = pymupdf.open(stream=pngs[0], filetype="png")
    assert img[0].rect.width >= rmdoc.INSERTED_PAGE_WIDTH * 72 / 72
    img.close()


def test_a_notebook_composites_to_a_pdf_with_one_sheet_per_inked_page(tmp_path, monkeypatch):
    import pymupdf

    doc = rmdoc.read_rmdoc(_notebook(tmp_path, monkeypatch))
    out = rmdoc.composite_pdf(doc, tmp_path / "flat.pdf")

    flat = pymupdf.open(str(out))
    try:
        assert flat.page_count == 2
    finally:
        flat.close()


def test_a_notebook_never_matches_a_corpus_document_by_hash(conn):
    """sha256 of nothing is not a document identity."""
    from locus.capture.loop_b import _corpus_uri_for

    assert _corpus_uri_for(conn, b"") is None


def test_markups_reads_a_notebook_end_to_end(conn, tmp_path, monkeypatch):
    """The call that failed on his call notes: resolve, fetch, sweep, render."""
    path = _notebook(tmp_path, monkeypatch)
    cfg = types.SimpleNamespace(
        paths=types.SimpleNamespace(db=tmp_path / "nb.db"),
        capture=types.SimpleNamespace(rmapi_binary="rmapi"),
    )
    monkeypatch.setattr(review, "locate", lambda c, t, **k: t)
    target = review.Target(device_path="/Rough notes/Jump call", title="Jump call", doc_uuid=UUID)

    m = review.markups(
        conn, target, cfg=cfg, fetch=lambda p, dest: path, read=rmdoc.read_rmdoc,
    )

    assert m.notebook
    assert sorted(m.pages) == [0, 1, 2]
    assert m.inked_pages == [0, 2]
    assert m.swept >= 2
    assert m.target.source_uri == "/Rough notes/Jump call", "no PDF, so it keys by device path"

    text = m.marks.render(notebook=m.notebook)
    assert "NOTEBOOK" in text
    assert "were ADDED on the tablet" not in text, "a notebook is not additions to a PDF"
