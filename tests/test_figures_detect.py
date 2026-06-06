"""Step 11: figure detection — raster/vector regions, caption pairing, junk filters.

Synthetic PDFs built with PyMuPDF (same approach as test_extract_pdf.py) so the tests are
deterministic and committable. Calibration against the real corpus happened during
development (eval-artifacts/figures-calibration); these tests pin the decision rules:
area band, formula-box exclusion, text-density ceiling, caption-class survival
(figure-caption exempts density, table-caption rejects), per-page cap, section mapping.
"""

from pathlib import Path

import pymupdf

from locus.config import FiguresConfig
from locus.extract import figures_detect
from locus.extract.base import ExtractedSection
from locus.extract.pdf import extract_pdf

CFG = FiguresConfig()


def _diagram(page, rect: pymupdf.Rect, strokes: int = 12) -> None:
    """Draw a diagram-shaped vector cluster: many strokes inside `rect`, no text."""
    x0, y0, x1, y1 = rect
    step = (y1 - y0) / strokes
    for i in range(strokes):
        y = y0 + i * step
        page.draw_line((x0, y), (x1, y - step / 2))
    page.draw_rect(rect)


def _png(w: int = 300, h: int = 200) -> bytes:
    pix = pymupdf.Pixmap(pymupdf.csRGB, pymupdf.IRect(0, 0, w, h))
    pix.clear_with(90)
    return pix.tobytes("png")


def _prose(page, rect: pymupdf.Rect) -> None:
    """Fill `rect` with dense prose lines (the text-density junk signature)."""
    y = rect.y0 + 12
    while y < rect.y1 - 4:
        page.insert_text(
            (rect.x0 + 2, y),
            "the quick brown fox jumps over the lazy dog again and again today",
            fontsize=10,
        )
        y += 12


def test_vector_diagram_detected_with_caption_below(tmp_path: Path):
    doc = pymupdf.open()
    page = doc.new_page()  # 612x792 default
    _diagram(page, pymupdf.Rect(100, 150, 450, 400))
    page.insert_text((100, 430), "Figure 3: Closed-loop control system.", fontsize=10)
    figs = figures_detect.detect_figures(doc, CFG)
    assert len(figs) == 1
    f = figs[0]
    assert f.kind == "vector"
    assert f.page == 1
    assert f.caption is not None and f.caption.startswith("Figure 3")
    assert f.image_bytes[:8] == b"\x89PNG\r\n\x1a\n"


def test_raster_image_detected(tmp_path: Path):
    doc = pymupdf.open()
    page = doc.new_page()
    page.insert_image(pymupdf.Rect(100, 100, 520, 380), stream=_png())
    figs = figures_detect.detect_figures(doc, CFG)
    assert len(figs) == 1
    assert figs[0].kind == "raster"


def test_formula_sized_raster_excluded(tmp_path: Path):
    """Images inside the SMALL_IMAGE box belong to the math detector, not figures."""
    doc = pymupdf.open()
    page = doc.new_page()
    page.insert_image(pymupdf.Rect(100, 100, 350, 140), stream=_png(250, 40))  # 250x40pt
    assert figures_detect.detect_figures(doc, CFG) == []


def test_sparse_drawing_excluded(tmp_path: Path):
    """An underline / lone box has too few paths to be a diagram."""
    doc = pymupdf.open()
    page = doc.new_page()
    page.draw_rect(pymupdf.Rect(100, 150, 450, 400))  # 1 path < min_vector_paths
    assert figures_detect.detect_figures(doc, CFG) == []


def test_dense_text_region_excluded(tmp_path: Path):
    """Prose with decoration drawings (the doc-50 Colab signature) is not a figure."""
    doc = pymupdf.open()
    page = doc.new_page()
    rect = pymupdf.Rect(80, 100, 530, 500)
    _prose(page, rect)
    _diagram(page, rect)  # drawings overlaying prose
    assert figures_detect.detect_figures(doc, CFG) == []


def test_figure_caption_overrides_density(tmp_path: Path):
    """A figure-class caption is the author's assertion: text-boxy flowcharts survive."""
    doc = pymupdf.open()
    page = doc.new_page()
    rect = pymupdf.Rect(80, 100, 530, 500)
    _prose(page, rect)  # density way over the ceiling
    _diagram(page, rect)
    page.insert_text((80, 530), "Figure 1: Pipeline architecture overview.", fontsize=10)
    figs = figures_detect.detect_figures(doc, CFG)
    assert len(figs) == 1
    assert figs[0].caption.startswith("Figure 1")


def test_table_caption_rejects_region(tmp_path: Path):
    """A table-class caption proves a table — its content is already in the text layer."""
    doc = pymupdf.open()
    page = doc.new_page()
    _diagram(page, pymupdf.Rect(100, 150, 450, 400))  # gridline-shaped cluster
    page.insert_text((100, 430), "Table 2: Results across all configurations.", fontsize=10)
    assert figures_detect.detect_figures(doc, CFG) == []


def test_per_page_cap(tmp_path: Path):
    doc = pymupdf.open()
    page = doc.new_page()
    for i in range(6):
        x = 60 + (i % 3) * 180
        y = 80 + (i // 3) * 300
        _diagram(page, pymupdf.Rect(x, y, x + 150, y + 250))
    figs = figures_detect.detect_figures(doc, CFG)
    assert len(figs) == CFG.max_per_page


def test_recurring_xref_considered_once(tmp_path: Path):
    """A per-page logo (same xref on every page) is considered at most once doc-wide."""
    doc = pymupdf.open()
    png = _png()
    for _ in range(3):
        page = doc.new_page()
        page.insert_image(pymupdf.Rect(100, 100, 520, 380), stream=png)
    figs = figures_detect.detect_figures(doc, CFG)
    assert len(figs) == 1


def test_skip_pages_skipped(tmp_path: Path):
    doc = pymupdf.open()
    page = doc.new_page()
    _diagram(page, pymupdf.Rect(100, 150, 450, 400))
    assert figures_detect.detect_figures(doc, CFG, skip_pages={0}) == []


def test_assign_sections_by_page_and_caption():
    def sec(pos: int, title: str, text: str, p0: int, p1: int) -> ExtractedSection:
        return ExtractedSection(
            position=pos, title=title, text=text, page_start=p0, page_end=p1, has_math=False
        )

    sections = [
        sec(0, "Intro", "intro text", 1, 2),
        sec(1, "Methods", "see Figure 4: pipeline details here", 2, 3),
        sec(2, "Results", "results text", 3, 5),
    ]
    from locus.extract.base import ExtractedFigure

    by_page = ExtractedFigure(page=4, image_bytes=b"x", kind="raster")
    tie_by_caption = ExtractedFigure(
        page=2, image_bytes=b"x", kind="vector", caption="Figure 4: pipeline details"
    )
    unmapped = ExtractedFigure(page=9, image_bytes=b"x", kind="raster")
    figures_detect.assign_sections([by_page, tie_by_caption, unmapped], sections)
    assert by_page.section_position == 2  # only Results spans p4
    assert tie_by_caption.section_position == 1  # p2 is shared; caption text breaks the tie
    assert unmapped.section_position is None


def test_extract_pdf_figures_flag(tmp_path: Path):
    """extract_pdf(figures=True) attaches section-assigned figures; default leaves none."""
    doc = pymupdf.open()
    page = doc.new_page()
    y = 72.0
    for i in range(14):
        page.insert_text((72, y), f"Sentence {i} contains several descriptive words.", fontsize=11)
        y += 21
    _diagram(page, pymupdf.Rect(100, 420, 450, 650))
    page.insert_text((100, 680), "Figure 1: A diagram of the system.", fontsize=10)
    pdf = tmp_path / "fig.pdf"
    doc.save(str(pdf))
    doc.close()

    assert extract_pdf(pdf).figures == []  # ad-hoc default: no rendering
    extracted = extract_pdf(pdf, figures=True)
    assert len(extracted.figures) == 1
    assert extracted.figures[0].section_position == 0
