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


def _body(tag: str, n: int = 12) -> list[tuple[str, float]]:
    """Body lines totalling > MIN_SECTION_CHARS so a section survives the small-merge."""
    return [(f"{tag}: sentence {i} contains several descriptive words about the topic.", BODY) for i in range(n)]


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
            [("Introduction", BODY), *_body("intro")],
            [("Methods", BODY), ("This describes the methods used in detail.", BODY), *_body("methods")],
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
        pages=[[("Section One", HEAD), *_body("one"), ("Section Two", HEAD), *_body("two")]],
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


def test_tiny_heading_fragments_are_merged(tmp_path: Path):
    """Over-segmentation guard: many tiny heading fragments collapse into real sections."""
    from locus.extract.pdf import MIN_SECTION_CHARS

    # 12 big-font "headings" each with only a short body line -> 12 tiny raw sections.
    pages = []
    for i in range(12):
        pages_line = [(f"Heading {i}", HEAD), (f"short body {i}.", BODY)]
        pages.append(pages_line)
    pdf = _make_pdf(tmp_path / "fragments.pdf", pages=pages)
    doc = extract_pdf(pdf)

    assert len(doc.sections) < 12  # merged, not one section per heading
    # No tiny fragment survives (all but possibly the trailing one meet the floor).
    assert all(len(s.text) >= 200 for s in doc.sections[:-1])
    assert MIN_SECTION_CHARS == 400


def test_alpha_less_headings_are_ignored(tmp_path: Path):
    # A bare number on its own line must not become a section title.
    pdf = _make_pdf(
        tmp_path / "numbered.pdf",
        pages=[[("1", HEAD), ("Real Heading", HEAD), ("Body content for the real heading.", BODY)]],
    )
    titles = [s.title for s in extract_pdf(pdf).sections]
    assert "1" not in titles


def test_missing_file_raises(tmp_path: Path):
    with pytest.raises(Exception):
        extract_pdf(tmp_path / "does-not-exist.pdf")


# --- ToC excision + heading-shape filter (plan step 4, eval phase B) ----------------------


def test_toc_pages_are_excised(tmp_path: Path):
    """A printed-ToC page (dense dotted-leader lines) must not ingest as content."""
    toc_lines = [(f"Chapter {i} heading text . . . . . . . . . . . {i * 7}", BODY) for i in range(1, 8)]
    pdf = _make_pdf(
        tmp_path / "with_toc.pdf",
        pages=[
            [("Contents", HEAD), *toc_lines],
            [("Real Heading", HEAD), *_body("content")],
        ],
    )
    doc = extract_pdf(pdf)
    assert doc.toc_pages == [1]
    joined = " ".join(s.text for s in doc.sections)
    assert ". . . . ." not in joined  # leader lines gone
    assert "content: sentence 1" in joined  # real content intact
    assert not any("Chapter 3" in (s.title or "") for s in doc.sections)  # no ToC-seeded titles


def test_sentence_lead_in_large_font_is_not_a_heading(tmp_path: Path):
    """Regression (PDE doc): paragraph leads set in a larger font are not section titles."""
    lead = "Applying boundary conditions specifies particular values of the dependent variable"
    pdf = _make_pdf(
        tmp_path / "lead.pdf",
        pages=[[("Real Heading", HEAD), *_body("alpha"), (lead, HEAD), *_body("beta")]],
    )
    doc = extract_pdf(pdf)
    titles = [s.title for s in doc.sections]
    assert "Real Heading" in titles
    assert lead not in titles


# --- page damage/math signals (plan step 6, eval phase D) ---------------------------------


def test_page_flags_properties():
    from locus.extract.pdf import PageFlags

    assert PageFlags(page=1, ligature_hits=2).corrupted
    assert PageFlags(page=1, symbol_garbage=2).corrupted
    assert not PageFlags(page=1, ligature_hits=1).corrupted  # threshold guards one-offs
    assert PageFlags(page=1, mathfont_chars=30).math_dense
    assert PageFlags(page=1, mathuni_chars=20).math_dense
    assert PageFlags(page=1, small_images=3).image_math
    assert PageFlags(page=1, drawings=3, gap_hits=2).image_math
    assert not PageFlags(page=1, drawings=16).image_math  # drawings alone: could be a plot
    assert not PageFlags(page=1).needs_ocr


def test_damage_signal_regexes():
    from locus.extract.pdf import _BROKEN_LIGATURES, _INLINE_GAP, _SYMBOL_GARBAGE, _mathuni_count

    # Broken ligatures: word-boundary only — 'first-order' must NOT match.
    assert len(_BROKEN_LIGATURES.findall("The rst step denes a xed eld.")) == 4
    assert _BROKEN_LIGATURES.findall("first-order systems are defined here") == []
    # Mis-mapped symbols: 'H(!)' and 'ei!t' (ω->!) match; prose '!' does not.
    assert len(_SYMBOL_GARBAGE.findall("the response H(!) equals ei!t here")) == 2
    assert _SYMBOL_GARBAGE.findall("this is surprising (usually!) but fine") == []
    # Inline formula gap: space on both sides of the newline (Colab exports).
    assert len(_INLINE_GAP.findall("combination of \n in a vector space")) == 1
    assert _INLINE_GAP.findall("an ordinary wrapped\nline of text") == []
    # Math-unicode density (doc-24-style mathematical alphanumeric symbols).
    assert _mathuni_count("min 𝑐(𝑤) over 𝑤 ∈ ℝ with ∇𝑐") >= 5


def test_plausible_heading_shapes():
    from locus.extract.pdf import _plausible_heading

    # Real headings survive.
    assert _plausible_heading("Introduction to Partial Differential Equations")
    assert _plausible_heading("First-order systems.")
    assert _plausible_heading("Laplace transforms of partial derivatives")
    # Prose / fragment / equation shapes are rejected (all observed in the corpus).
    assert not _plausible_heading("nary differential equation takes the form")  # lower start
    assert not _plausible_heading("(The kernel is defined as the")  # punct start, unbalanced
    assert not _plausible_heading("The Fourier coefficients (also")  # unbalanced parens
    assert not _plausible_heading("∇p = ˆi∂p")  # equation glyphs
    assert not _plausible_heading("ˆi")  # no real word
    assert not _plausible_heading("The inverse Laplace transformation operation, L \x08")  # ctrl char
    assert not _plausible_heading(
        "Next define a linear operator, which we will call the one-dimensional"
    )  # too many words
    assert not _plausible_heading("Waves are introduced. A wave equation is derived")  # 2 sentences


def test_dehyphenate_joins_line_wrapped_words():
    from locus.extract.pdf import _dehyphenate

    assert _dehyphenate("convec-\ntion is a transport process") == (
        "convection is a transport process"
    )
    assert _dehyphenate("the distribu-\ntion of returns") == "the distribution of returns"
    # Capitalized compounds and non-wrap hyphens are untouched.
    assert _dehyphenate("the Navier-\nStokes equations") == "the Navier-\nStokes equations"
    assert _dehyphenate("a well-known result") == "a well-known result"
    assert _dehyphenate("range 3-\n4 metres") == "range 3-\n4 metres"  # digits: not a wrap
    # Soft hyphens are typographic artifacts: removed, joining the word.
    assert _dehyphenate("convec­\ntion") == "convection"
    assert _dehyphenate("im­plicit") == "implicit"


def test_extraction_dehyphenates_page_text(tmp_path: Path):
    pdf = _make_pdf(
        tmp_path / "wrapped.pdf",
        pages=[
            [
                ("Introduction", HEAD),
                ("The process of convec-", BODY),
                ("tion moves heat through the fluid in a continuous manner.", BODY),
                *_body("intro"),
            ],
        ],
    )
    doc = extract_pdf(pdf)
    text = "".join(s.text for s in doc.sections)
    assert "convection" in text
    assert "convec-" not in text
    # Page mapping is intact after the joins shorten the text.
    assert all(s.page_start == 1 and s.page_end == 1 for s in doc.sections)


def test_artefact_labels_are_not_headings():
    from locus.extract.pdf import _plausible_heading

    # Bold/large captions mis-titled sections (2026-06-05 evaluation: "Figure 22:").
    for label in ("Figure 22: Results", "Figure 22:", "Fig. 3 overview", "Table 2",
                  "Equation 4", "Algorithm 1: training loop", "Eq. 7"):
        assert not _plausible_heading(label), label
    # Words that merely start with a label-word stay legitimate headings.
    for ok in ("Figure of merit", "Tables and joins in SQL", "3 Methods", "Figures"):
        assert _plausible_heading(ok), ok
