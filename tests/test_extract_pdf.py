"""Stage 2: PDF extraction — section strategies, title resolution, math flag, edge cases.

Uses small synthetic PDFs built with PyMuPDF so the tests are deterministic and committable
(the real corpus lives only in the local vault). The bold-at-body-size heading path is
validated separately against a real document during development; here we drive the heading
heuristic with font size, which we can control precisely.
"""

from pathlib import Path

import pymupdf
import pytest

from locus.extract.pdf import extract_pdf

BODY = 11.0
HEAD = 20.0


def _make_pdf(path: Path, pages: list[list[tuple[str, float]]], *, toc=None, title=None) -> Path:
    """Build a PDF. `pages` is a list of pages; each page is a list of (text, fontsize)."""
    doc = pymupdf.open()
    for page_lines in pages:
        page = doc.new_page()
        y = 72.0
        for text, size in page_lines:
            page.insert_text((72, y), text, fontsize=size, fontname="helv")
            y += size + 10
    if title is not None:
        doc.set_metadata({"title": title})
    if toc is not None:
        doc.set_toc(toc)
    doc.save(str(path))
    doc.close()
    return path


def test_outline_strategy(tmp_path: Path):
    pdf = _make_pdf(
        tmp_path / "outlined.pdf",
        pages=[
            [("Introduction", BODY), ("This is the intro body text.", BODY)],
            [("Methods", BODY), ("This describes the methods used.", BODY)],
        ],
        toc=[[1, "Introduction", 1], [1, "Methods", 2]],
    )
    doc = extract_pdf(pdf)
    assert doc.section_strategy == "outline"
    titles = [s.title for s in doc.sections]
    assert "Introduction" in titles
    assert "Methods" in titles
    methods = next(s for s in doc.sections if s.title == "Methods")
    assert "methods used" in methods.text


def test_headings_strategy_by_font_size(tmp_path: Path):
    pdf = _make_pdf(
        tmp_path / "headings.pdf",
        pages=[
            [
                ("Section One", HEAD),
                ("Body text under section one.", BODY),
                ("Section Two", HEAD),
                ("Body text under section two.", BODY),
            ]
        ],
    )
    doc = extract_pdf(pdf)
    assert doc.section_strategy == "headings"
    titles = [s.title for s in doc.sections]
    assert "Section One" in titles
    assert "Section Two" in titles


def test_numbered_list_items_do_not_explode_into_sections(tmp_path: Path):
    """Regression: body-size numbered lines are list items, not headings."""
    pdf = _make_pdf(
        tmp_path / "list.pdf",
        pages=[
            [
                ("Real Heading", HEAD),
                ("1. first list item", BODY),
                ("2. second list item", BODY),
                ("3. third list item", BODY),
                ("4. fourth list item", BODY),
            ]
        ],
    )
    doc = extract_pdf(pdf)
    # One real heading (+ optional front matter), not one section per numbered item.
    assert len(doc.sections) <= 2
    assert any(s.title == "Real Heading" for s in doc.sections)


def test_single_section_fallback(tmp_path: Path):
    pdf = _make_pdf(
        tmp_path / "flat.pdf",
        pages=[[("Just uniform body text with no headings at all.", BODY)]],
    )
    doc = extract_pdf(pdf)
    assert doc.section_strategy == "single"
    assert len(doc.sections) == 1
    assert "uniform body text" in doc.sections[0].text


def test_title_from_metadata(tmp_path: Path):
    pdf = _make_pdf(
        tmp_path / "meta.pdf",
        pages=[[("Some body.", BODY)]],
        title="A Proper Document Title",
    )
    assert extract_pdf(pdf).title == "A Proper Document Title"


def test_title_falls_back_to_filename_with_spaces(tmp_path: Path):
    # No metadata title, no extractable text -> filename stem; path has spaces.
    pdf = tmp_path / "my spaced report.pdf"
    doc = pymupdf.open()
    doc.new_page()  # blank page, no text
    doc.save(str(pdf))
    doc.close()
    result = extract_pdf(pdf)
    assert result.title == "my spaced report"
    assert result.source_path.endswith("my spaced report.pdf")


def test_math_flag(tmp_path: Path):
    mathy = _make_pdf(
        tmp_path / "mathy.pdf",
        pages=[[("Equation One", HEAD), (r"We use \frac{a}{b} and \sum and \int over x.", BODY)]],
    )
    plain = _make_pdf(
        tmp_path / "plain.pdf",
        pages=[[("Plain Heading", HEAD), ("Ordinary prose with no mathematics here.", BODY)]],
    )
    mathy_sec = next(s for s in extract_pdf(mathy).sections if s.title == "Equation One")
    plain_sec = next(s for s in extract_pdf(plain).sections if s.title == "Plain Heading")
    assert mathy_sec.has_math is True
    assert plain_sec.has_math is False


def test_text_is_fully_partitioned(tmp_path: Path):
    pdf = _make_pdf(
        tmp_path / "partition.pdf",
        pages=[
            [("Alpha", HEAD), ("unique-token-alpha appears here.", BODY)],
            [("Beta", HEAD), ("unique-token-beta appears here.", BODY)],
        ],
    )
    doc = extract_pdf(pdf)
    joined = " ".join(s.text for s in doc.sections)
    assert "unique-token-alpha" in joined
    assert "unique-token-beta" in joined


def test_oversized_section_is_paginated(tmp_path: Path, monkeypatch):
    """A heading-poor document must never become one giant section; it is windowed by page."""
    import locus.extract.pdf as pdfmod

    monkeypatch.setattr(pdfmod, "MAX_SECTION_CHARS", 400)
    # Four pages of uniform body text (~165 chars each), no headings -> single -> paginated.
    line = ("uniform body content about signals and systems here.", BODY)
    pages = [[line, line, line] for _ in range(4)]
    pdf = _make_pdf(tmp_path / "big.pdf", pages=pages)
    doc = extract_pdf(pdf)

    assert doc.section_strategy == "paginated"
    assert len(doc.sections) > 1
    # No window exceeds the limit (each page here is well under it, so the bound is strict).
    assert all(len(s.text) <= 400 for s in doc.sections)
    # Split windows are page-labelled.
    assert all("pp " in (s.title or "") for s in doc.sections)


def test_title_strips_word_export_prefix(tmp_path: Path):
    pdf = _make_pdf(
        tmp_path / "word.pdf",
        pages=[[("Body.", BODY)]],
        title="Microsoft Word - My Real Title.docx",
    )
    assert extract_pdf(pdf).title == "My Real Title"


def test_missing_file_raises(tmp_path: Path):
    with pytest.raises(Exception):
        extract_pdf(tmp_path / "does-not-exist.pdf")
