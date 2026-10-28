"""A notebook is transcribed one changed page at a time (capture/notebook.py, 28 Oct).

Real render (rmdoc.composite_pdf over a native notebook) and a fake page transcriber that reads
back which pages it was sent, so what is asserted is the thing that costs money: which pages go
to the vision model.
"""

from __future__ import annotations

from pathlib import Path

import pymupdf

from locus.capture.notebook import transcribe_notebook
from locus.capture.rmdoc import AnnotatedPage, RmDoc, Stroke, composite_pdf
from locus.capture.transcribe import PageTranscript, Transcript


def _page(pos: int, uuid: str, y: float) -> AnnotatedPage:
    return AnnotatedPage(pos, uuid, [Stroke([(50.0, y), (200.0, y + 5)])], source_page=None)


def _notebook(*pages: AnnotatedPage) -> RmDoc:
    return RmDoc(doc_uuid="nb", pdf_bytes=b"", pages=list(pages),
                 page_map=[None] * (max(p.pdf_page for p in pages) + 1))


class _Reader:
    """Transcribes each page it is sent as 'page <n of the part>', counting calls."""

    def __init__(self):
        self.sent: list[int] = []

    def __call__(self, pdf: Path) -> Transcript:
        doc = pymupdf.open(str(pdf))
        n = doc.page_count
        doc.close()
        self.sent.append(n)
        t = Transcript(input_tokens=100 * n, output_tokens=10 * n)
        for i in range(n):
            t.pages.append(PageTranscript(page=i + 1, markdown=f"text {len(self.sent)}.{i}",
                                          illegible=0))
        return t


def _run(tmp_path, rmdoc, reader):
    return transcribe_notebook(rmdoc, out_pdf=tmp_path / "nb.pdf",
                               cache_file=tmp_path / "cache" / "nb.json",
                               transcribe_pdf=reader, composite=composite_pdf)


def test_only_the_changed_page_is_read_again(tmp_path):
    """Jot only grows: a new line on its last page must not re-read the pages before it."""
    reader = _Reader()
    first = _notebook(_page(0, "a", 100), _page(1, "b", 100), _page(2, "c", 100))
    t1, n1 = _run(tmp_path, first, reader)
    assert n1 == 3 and reader.sent == [3]

    grown = _notebook(_page(0, "a", 100), _page(1, "b", 100), _page(2, "c", 300))
    t2, n2 = _run(tmp_path, grown, reader)
    assert n2 == 1 and reader.sent == [3, 1]
    # the note is still the whole notebook, in order, with page c's new text
    assert [p.markdown for p in t2.pages] == ["text 1.0", "text 1.1", "text 2.0"]
    assert [p.page for p in t2.pages] == [1, 2, 3]


def test_nothing_changed_reads_nothing(tmp_path):
    reader = _Reader()
    nb = _notebook(_page(0, "a", 100), _page(1, "b", 100))
    _run(tmp_path, nb, reader)
    t, n = _run(tmp_path, nb, reader)
    assert n == 0 and reader.sent == [2] and len(t.pages) == 2


def test_a_new_page_is_read_and_a_deleted_page_leaves_the_note(tmp_path):
    reader = _Reader()
    _run(tmp_path, _notebook(_page(0, "a", 100), _page(1, "b", 100)), reader)
    t, n = _run(tmp_path, _notebook(_page(0, "a", 100), _page(1, "x", 100)), reader)
    assert n == 1
    assert [p.markdown for p in t.pages] == ["text 1.0", "text 2.0"]
    assert "b" not in (tmp_path / "cache" / "nb.json").read_text().split('"key"')[0]


def test_a_notebook_with_no_ink_is_none(tmp_path):
    empty = RmDoc(doc_uuid="nb", pdf_bytes=b"", pages=[], page_map=[None])
    assert _run(tmp_path, empty, _Reader()) == (None, 0)


def test_the_full_render_is_kept_for_the_note(tmp_path):
    reader = _Reader()
    _run(tmp_path, _notebook(_page(0, "a", 100), _page(1, "b", 100)), reader)
    doc = pymupdf.open(str(tmp_path / "nb.pdf"))
    assert doc.page_count == 2
    doc.close()
    assert not (tmp_path / "nb.changed.pdf").exists()
