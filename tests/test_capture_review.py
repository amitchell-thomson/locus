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

    assert doc.blank_count == 1
    # Always reported — never filtered, whichever register the caller is in.
    assert "covering no text" in doc.render()
    # The "ask for the image" nudge belongs only where the images are NOT already coming.
    # `markups` returns them alongside, so telling the reader to ask for what it just handed
    # over is noise.
    assert "page image" in doc.render(image_hint=True)
    assert "page image" not in doc.render()


def test_a_mark_with_only_handwriting_is_not_blank(conn):
    """A margin note covers nothing but says plenty — it is not the figure case."""
    _document(conn)
    _mark(conn, page=41, covered="", note="what does this mean?", kind="margin_note", margin=1)

    doc = review.load(conn, URI)

    assert doc.blank_count == 0
    assert "what does this mean?" in doc.render()


def test_blank_marks_still_reach_the_reader(conn):
    """Page selection now ranks on real ink density (`pages_by_ink`), which needs the parsed
    bundle and is a better signal than counting stored marks. What must not regress is that a
    mark covering nothing still appears at all — it is the case the images exist for."""
    _document(conn)
    for i in range(4):
        _mark(conn, page=10, covered=f"words {i}", key=f"dense{i}")
    _mark(conn, page=20, covered="", key="blank")

    doc = review.load(conn, URI)

    assert doc.blank_count == 1
    assert "p.21" in doc.render()


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
    page and absurd for a 300-page book of which he marked nine pages.

    Asserted on the `margins=False` path, which is the one that goes through `_render_pages`."""
    rendered: list[list[int]] = []

    def fake_render(pdf_path, page_indexes, *, dpi):
        rendered.append(list(page_indexes))
        return {i: b"png" for i in page_indexes}

    monkeypatch.setattr(review, "_render_pages", fake_render)
    monkeypatch.setattr("locus.capture.rmdoc.composite_pdf", lambda rmdoc, out: Path(out))

    out = review.annotated_page_pngs(
        "/Reading/In-Progress/A Book", [3, 9], margins=False,
        fetch=lambda path, dest: Path(dest) / "x.rmdoc",
        read=lambda p: object(),
    )

    assert rendered == [[3, 9]]
    assert sorted(out) == [3, 9]


def test_the_default_render_keeps_margins(tmp_path: Path):
    """ONE renderer across the CLI and both MCP tools. When they diverged, the CLI looked right
    and was wrong — `annotations(images=True)` was still clipping while `markups` was not."""
    import pymupdf

    rmdoc = _rmdoc_with_margin_ink(tmp_path)
    out = review.annotated_page_pngs(
        "/Inbox/Draft", [0], dpi=72,
        fetch=lambda path, dest: Path(dest) / "x.rmdoc", read=lambda p: rmdoc,
    )

    img = pymupdf.open(stream=out[0], filetype="png")
    assert img[0].rect.width > 595
    img.close()


def test_no_pages_requested_costs_no_fetch():
    called = []
    out = review.annotated_page_pngs(
        "/x", [], fetch=lambda *a: called.append(1), read=lambda p: None
    )
    assert out == {} and not called


# ---------- margins: the clipping the whole feature exists to undo ----------

def _rmdoc_with_margin_ink(tmp_path: Path, *, x_from=-80.0, x_to=200.0):
    """A one-page 595x842 PDF with a stroke running from x=-80 (outside the paper) inward."""
    import pymupdf

    from locus.capture.rmdoc import AnnotatedPage, RmDoc, Stroke

    doc = pymupdf.open()
    doc.new_page(width=595, height=842)
    pdf_bytes = doc.tobytes()
    doc.close()

    stroke = Stroke(points=[(x, 400.0) for x in (x_from, x_from + 40, 0.0, 100.0, x_to)])
    return RmDoc(doc_uuid="uuid-margin", pdf_bytes=pdf_bytes,
                 pages=[AnnotatedPage(pdf_page=0, page_uuid="p0", strokes=[stroke])])


def test_margin_ink_survives_and_widens_the_canvas(tmp_path: Path):
    """The motivating bug. `composite_pdf` draws on the original page and pymupdf refuses
    anything outside the page rect, so margin writing was cut at the paper edge — silently,
    with a perfectly valid PNG coming back. Here the render must be WIDER than the page."""
    import pymupdf

    from locus.capture.rmdoc import composite_pages_with_margins

    rmdoc = _rmdoc_with_margin_ink(tmp_path)
    pngs = composite_pages_with_margins(rmdoc, [0], dpi=72)

    assert set(pngs) == {0}
    img = pymupdf.open(stream=pngs[0], filetype="png")
    width_pt = img[0].rect.width          # 72dpi => 1px == 1pt
    img.close()
    # 595pt of paper + 80pt of overhanging ink + padding, so comfortably wider than the page.
    assert width_pt > 595, f"canvas was {width_pt}pt — the margin was clipped again"


def test_ink_entirely_inside_the_page_still_shows_the_whole_page(tmp_path: Path):
    """min(0,...)/max(width,...) keep the paper fully visible; a tight crop to the ink would
    make a page of dense marginless notes unreadable."""
    import pymupdf

    from locus.capture.rmdoc import composite_pages_with_margins

    rmdoc = _rmdoc_with_margin_ink(tmp_path, x_from=100.0, x_to=200.0)
    pngs = composite_pages_with_margins(rmdoc, [0], dpi=72)
    img = pymupdf.open(stream=pngs[0], filetype="png")
    assert img[0].rect.width >= 595
    img.close()


def test_pages_without_ink_are_not_rendered(tmp_path: Path):
    from locus.capture.rmdoc import composite_pages_with_margins

    rmdoc = _rmdoc_with_margin_ink(tmp_path)
    assert composite_pages_with_margins(rmdoc, [5], dpi=72) == {}


# ---------- whole-device resolution ----------

class _FakeRmapi:
    """`find /` from a fixed tree; `stat` from a path -> uuid map."""

    def __init__(self, tree: dict[str, str]):
        self.tree = tree                      # path -> uuid
        self.calls: list[list[str]] = []

    def __call__(self, argv):
        self.calls.append(argv)
        if argv[0] == "find":
            return 0, "".join(f"[f] {p}\n" for p in self.tree), ""
        if argv[0] == "stat":
            uuid = self.tree.get(argv[1])
            if uuid is None:
                return 1, "", "file doesn't exist"
            import json
            return 0, json.dumps({"ID": uuid, "Name": Path(argv[1]).name}), ""
        return 1, "", "unexpected"


def test_a_document_in_inbox_resolves(conn):
    """The regression the brief names: `_live_index` walked only the reading folders, so the
    folder `to_remarkable` writes into was the one place the renderer could not see."""
    runner = _FakeRmapi({"/Inbox/2026-09-05 HH-TTF spread null": "u-hh",
                         "/Reading/Finished/Something Else": "u-other"})

    found = review.resolve_target(conn, "HH-TTF", runner=runner)

    assert [t.device_path for t in found] == ["/Inbox/2026-09-05 HH-TTF spread null"]
    assert found[0].doc_uuid == "u-hh"


def test_name_matching_reads_the_listing_and_stats_only_survivors(conn):
    """A stat is 0.14s and there are 79 files; the name is already in the `find` output, so
    stat-ing everything would spend a third of the time budget for nothing."""
    runner = _FakeRmapi({f"/Inbox/doc {i}": f"u{i}" for i in range(20)} | {"/Inbox/target": "u-t"})

    review.resolve_target(conn, "target", runner=runner)

    stats = [c for c in runner.calls if c[0] == "stat"]
    assert len(stats) == 1 and stats[0][1] == "/Inbox/target"


def test_an_ambiguous_fragment_returns_every_candidate(conn):
    """Two HH-TTF drafts are on his device right now; guessing would answer about the wrong one."""
    runner = _FakeRmapi({"/Inbox/HH-TTF first draft": "u1", "/Inbox/HH-TTF second draft": "u2"})

    assert len(review.resolve_target(conn, "HH-TTF", runner=runner)) == 2


def test_marks_are_found_by_uuid_after_a_folder_move(conn):
    """Marks for an un-ingested document are keyed by device path, which changes when he moves
    it. The uuid does not, so the stored marks must still be reachable."""
    _mark(conn, page=0, covered="x", uri="/Inbox/Draft", uuid="u-moved")
    runner = _FakeRmapi({"/Reading/Finished/Draft": "u-moved"})

    target = review.resolve_target(conn, "Draft", runner=runner)[0]

    assert target.doc_uuid == "u-moved"
    assert target.source_uri == "/Inbox/Draft"      # where the marks actually live


def test_locate_prefers_the_source_uri_when_it_is_a_device_path(conn):
    """An un-ingested document IS its device path, so this costs no listing at all."""
    runner = _FakeRmapi({"/Inbox/Draft": "u-1"})
    target = review.Target(device_path="", title="Draft", doc_uuid="u-1",
                           source_uri="/Inbox/Draft")

    located = review.locate(conn, target, runner=runner)

    assert located.device_path == "/Inbox/Draft"
    assert not [c for c in runner.calls if c[0] == "find"]


# ---------- sweeping from the tool ----------

def test_refresh_sweeps_and_stores_without_transcribing(conn, tmp_path, monkeypatch):
    """`refresh=True` on a document with no stored marks must store them, and must NEVER reach
    the billed transcription — this runs on a chat tool call."""
    import types

    rmdoc = _rmdoc_with_margin_ink(tmp_path)
    cfg = types.SimpleNamespace(
        paths=types.SimpleNamespace(db=tmp_path / "review.db"),
        capture=types.SimpleNamespace(rmapi_binary="rmapi"),
    )
    monkeypatch.setattr(review, "locate", lambda c, t, **k: t)
    called: list[str] = []
    monkeypatch.setattr(
        "locus.capture.transcribe.transcribe_pdf",
        lambda *a, **k: called.append("billed"),
    )

    target = review.Target(device_path="/Inbox/Draft", title="Draft", doc_uuid="uuid-margin")
    def fetch(path, dest):
        got = Path(dest) / "x.rmdoc"
        got.write_bytes(b"bundle")
        return got

    out = review.markups(
        conn, target, cfg=cfg, refresh=True, images=False,
        fetch=fetch, read=lambda p: rmdoc,
    )

    assert out.swept >= 1
    assert called == [], "a chat tool call must never trigger billed transcription"
    stored = conn.execute("SELECT COUNT(*) n FROM pdf_annotations").fetchone()["n"]
    assert stored == out.swept


def test_the_bundle_is_cached_so_a_second_look_does_not_refetch(conn, tmp_path):
    import types

    rmdoc = _rmdoc_with_margin_ink(tmp_path)
    cfg = types.SimpleNamespace(
        paths=types.SimpleNamespace(db=tmp_path / "review.db"),
        capture=types.SimpleNamespace(rmapi_binary="rmapi"),
    )
    fetches: list[str] = []

    def fetch(path, dest):
        fetches.append(path)
        p = Path(dest) / "x.rmdoc"
        p.write_bytes(b"bundle")
        return p

    target = review.Target(device_path="/Inbox/Draft", title="Draft", doc_uuid="uuid-margin")
    first, fetched_1 = review.load_rmdoc(target, cfg=cfg, fetch=fetch, read=lambda p: rmdoc)
    second, fetched_2 = review.load_rmdoc(target, cfg=cfg, fetch=fetch, read=lambda p: rmdoc)

    assert fetched_1 is True and fetched_2 is False
    assert len(fetches) == 1


# ---------- page ordering and the payload budget ----------

def test_pages_are_ranked_by_ink_then_returned_in_reading_order(tmp_path):
    from locus.capture.rmdoc import AnnotatedPage, RmDoc, Stroke

    def page(idx, n):
        return AnnotatedPage(pdf_page=idx, page_uuid=f"p{idx}",
                             strokes=[Stroke(points=[(0.0, 0.0)] * n)])

    rmdoc = RmDoc(doc_uuid="u", pdf_bytes=b"", pages=[page(0, 5), page(4, 90), page(2, 40)])

    assert review.pages_by_ink(rmdoc) == [0, 2, 4]          # reading order
    assert review.pages_by_ink(rmdoc, cap=2) == [2, 4]      # the two busiest, still in order


def test_the_byte_budget_never_drops_a_page_that_was_asked_for(tmp_path):
    """An explicit request for page 9 that quietly returns page 3 is worse than a large reply."""
    from locus.capture.rmdoc import AnnotatedPage, RmDoc, Stroke

    rmdoc = RmDoc(doc_uuid="u", pdf_bytes=b"", pages=[
        AnnotatedPage(pdf_page=i, page_uuid=f"p{i}", strokes=[Stroke(points=[(0.0, 0.0)] * (i + 1))])
        for i in range(3)
    ])
    rendered = {0: b"x" * 900, 1: b"x" * 900, 2: b"x" * 900}

    assert review._within_budget(rendered, rmdoc, max_bytes=1000, explicit=True) == rendered
    trimmed = review._within_budget(rendered, rmdoc, max_bytes=1000, explicit=False)
    assert list(trimmed) == [2]         # the densest page survives


def test_a_bundle_with_no_uuid_is_never_served_from_cache(conn, tmp_path):
    """Keying a uuid-less document on a placeholder would make every such document share one
    cache entry and hand back somebody else's bundle — silently, rendering perfectly."""
    import types

    cfg = types.SimpleNamespace(
        paths=types.SimpleNamespace(db=tmp_path / "review.db"),
        capture=types.SimpleNamespace(rmapi_binary="rmapi"),
    )
    (tmp_path / "cache" / "rmdoc").mkdir(parents=True)
    (tmp_path / "cache" / "rmdoc" / ".rmdoc").write_bytes(b"someone else")
    (tmp_path / "cache" / "rmdoc" / "unknown.rmdoc").write_bytes(b"someone else")

    fetches = []

    def fetch(path, dest):
        fetches.append(path)
        got = Path(dest) / "x.rmdoc"
        got.write_bytes(b"mine")
        return got

    from locus.capture.rmdoc import RmDoc

    parsed = RmDoc(doc_uuid="", pdf_bytes=b"", pages=[])
    target = review.Target(device_path="/Inbox/NoUuid", title="NoUuid", doc_uuid="")
    doc, fetched = review.load_rmdoc(target, cfg=cfg, fetch=fetch, read=lambda p: parsed)

    assert doc is parsed and fetched is True
    assert fetches == ["/Inbox/NoUuid"]
    # ...and nothing was written under a placeholder name either.
    assert not (tmp_path / "cache" / "rmdoc" / "unknown.rmdoc").read_bytes() == b"mine"
