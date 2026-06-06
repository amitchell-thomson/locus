"""Figure-region detection for PDFs (plan step 11, tier 1 — preserve).

Detects two kinds of figure on a page and renders each region to PNG:

  - raster: embedded images (plots, photos, scanned diagrams) via `page.get_image_info`,
  - vector: drawing clusters (block diagrams, schematics — common in the engineering
    corpus) via `page.cluster_drawings`.

Both are rendered with `page.get_pixmap(clip=...)` rather than raw xref extraction: the
clip render captures exactly what a reader sees (masks, composites, overlaid annotations),
and vector clusters have no xref at all — one render path for both kinds.

Filters lean STRICT (the `_plausible_heading` risk asymmetry): a missed figure is benign,
a junk figure burns a VLM call at ingest and a retrieval slot at query time. Formula-sized
images are excluded by the same size box the math detector uses (`_SMALL_IMAGE_MAX_W/H`),
so figure detection and math detection share one definition of "too small to be a figure".

Deterministic and model-free, like `_page_flags`: the VLM description pass (ingest/figures.py)
runs later, in the pipeline.
"""

from __future__ import annotations

import logging
import re

import pymupdf

from locus.config import FiguresConfig
from locus.extract.base import (
    SMALL_IMAGE_MAX_H,
    SMALL_IMAGE_MAX_W,
    ExtractedFigure,
    ExtractedSection,
)

log = logging.getLogger(__name__)

# Caption labels — same family as pdf._HEADING_LABEL, re-declared locally on purpose (that
# one rejects captions as headings; this one *finds* them). Split by what the label proves:
# a figure-class caption is the author asserting "this region is a figure" (it overrides the
# text-density filter — text-boxy flowcharts like 'Figure 2: Pipeline architecture' are real
# figures at prose-level density); a table-class caption proves the region is a table, whose
# content the text layer already carries — rejected outright.
_FIGURE_LABEL = re.compile(
    r"^\s*(?:figure|fig\.?|chart|diagram|scheme|plate|exhibit)\s*\.?\s*\d", re.IGNORECASE
)
_TABLE_LABEL = re.compile(r"^\s*(?:table|tbl\.?)\s*\.?\s*\d", re.IGNORECASE)
_CAPTION_MAX_CHARS = 500  # a caption is a sentence or two, not a column of prose

_RENDER_ZOOM = 2.0  # render at 2x: enough detail for the VLM without huge PNGs
_MAX_ASPECT = 20.0  # beyond this it's a rule line / border strip, not a figure


def detect_figures(
    doc: "pymupdf.Document",
    cfg: FiguresConfig,
    *,
    skip_pages: set[int] = frozenset(),
) -> list[ExtractedFigure]:
    """Detect + render figure regions across all pages of an open pymupdf document.

    `skip_pages` (0-based) are excised ToC pages — anything on them is navigation chrome.
    Raster xrefs are deduped across the whole document so a recurring per-page logo is
    considered at most once. Figures are returned in reading order (page, then top-to-bottom);
    `section_position` is left unset (the caller maps pages to sections, see
    `assign_sections`).
    """
    figures: list[ExtractedFigure] = []
    seen_xrefs: set[int] = set()
    for pno in range(doc.page_count):
        if pno in skip_pages:
            continue
        page = doc[pno]
        try:
            regions = _page_regions(page, cfg, seen_xrefs)
        except Exception as exc:  # one pathological page must not sink the document
            log.warning("figure detection failed on p%d: %s", pno + 1, exc)
            continue
        for rect, kind, caption in regions:
            try:
                png = page.get_pixmap(
                    clip=rect, matrix=pymupdf.Matrix(_RENDER_ZOOM, _RENDER_ZOOM)
                ).tobytes("png")
            except Exception as exc:
                log.warning("figure render failed on p%d %s: %s", pno + 1, rect, exc)
                continue
            figures.append(
                ExtractedFigure(
                    page=pno + 1,
                    bbox=(rect.x0, rect.y0, rect.x1, rect.y1),
                    image_bytes=png,
                    caption=caption,
                    kind=kind,
                )
            )
    return figures


def _page_regions(
    page: "pymupdf.Page", cfg: FiguresConfig, seen_xrefs: set[int]
) -> list[tuple["pymupdf.Rect", str, str | None]]:
    """Figure regions on one page: filtered rasters + vector clusters, merged + captioned.

    Caption pairing happens here (not after) because the caption class decides survival:
    figure-class caption => keep (density-exempt), table-class => reject, no caption =>
    keep only under the text-density ceiling.
    """
    page_area = abs(page.rect)  # width * height in pt²
    if page_area <= 0:
        return []

    def area_ok(r: pymupdf.Rect) -> bool:
        frac = abs(r) / page_area
        return cfg.min_area_frac <= frac <= cfg.max_area_frac

    def shape_ok(r: pymupdf.Rect) -> bool:
        if r.is_empty or r.width <= 0 or r.height <= 0:
            return False
        if max(r.width / r.height, r.height / r.width) > _MAX_ASPECT:
            return False  # rule line / border strip
        # Formula-sized rasters belong to the math detector, not the figures pipeline.
        if r.width <= SMALL_IMAGE_MAX_W and r.height <= SMALL_IMAGE_MAX_H:
            return False
        return True

    rasters: list[pymupdf.Rect] = []
    for info in page.get_image_info(xrefs=True):
        xref = info.get("xref") or 0
        if xref and xref in seen_xrefs:
            continue  # recurring image (per-page logo/banner) — consider once, doc-wide
        r = pymupdf.Rect(info["bbox"]) & page.rect  # clamp to the page
        if not (shape_ok(r) and area_ok(r)):
            continue
        if xref:
            seen_xrefs.add(xref)
        rasters.append(r)

    drawings = page.get_drawings()
    vectors: list[pymupdf.Rect] = []
    for cluster in page.cluster_drawings():
        r = pymupdf.Rect(cluster) & page.rect
        if not (shape_ok(r) and area_ok(r)):
            continue
        # A real diagram has many strokes; an underline or a single box does not.
        paths = sum(1 for d in drawings if pymupdf.Rect(d["rect"]).intersects(r))
        if paths < cfg.min_vector_paths:
            continue
        vectors.append(r)

    # Merge raster/vector candidates that are substantially the same region (a plot exported
    # as an image plus its vector axes, etc.): keep the union rect, labelled by the larger part.
    merged: list[tuple[pymupdf.Rect, str]] = [(r, "raster") for r in rasters]
    for v in vectors:
        for i, (m, kind) in enumerate(merged):
            if _iou(v, m) > 0.5:
                union = pymupdf.Rect(m) | v
                merged[i] = (union, kind if abs(m) >= abs(v) else "vector")
                break
        else:
            merged.append((v, "vector"))

    # Survival by caption class, then text density. A figure-class caption is the author's
    # own assertion the region is a figure — it overrides the density ceiling (text-boxy
    # flowcharts read 5-7 chars/1000pt², prose-level). A table-class caption proves a table:
    # rejected, the text layer already carries its content. Uncaptioned regions survive only
    # under the ceiling (corpus-measured: real diagrams <= 1.6, prose-with-drawings and
    # gridline tables >= 2.2).
    def text_density(r: pymupdf.Rect) -> float:
        chars = len("".join(page.get_text(clip=r).split()))
        return 1000.0 * chars / abs(r)

    survivors: list[tuple[pymupdf.Rect, str, str | None]] = []
    for r, kind in merged:
        caption = _pair_caption(page, r, cfg)
        if caption and _TABLE_LABEL.match(caption):
            continue
        if caption is None and text_density(r) > cfg.max_text_density:
            continue
        survivors.append((r, kind, caption))

    # Junk guard: at most max_per_page, largest first, then back to reading order.
    survivors.sort(key=lambda rkc: abs(rkc[0]), reverse=True)
    kept = survivors[: cfg.max_per_page]
    kept.sort(key=lambda rkc: (rkc[0].y0, rkc[0].x0))
    return kept


def _iou(a: "pymupdf.Rect", b: "pymupdf.Rect") -> float:
    inter = pymupdf.Rect(a) & b
    if inter.is_empty:
        return 0.0
    union_area = abs(a) + abs(b) - abs(inter)
    return abs(inter) / union_area if union_area > 0 else 0.0


def _pair_caption(page: "pymupdf.Page", rect: "pymupdf.Rect", cfg: FiguresConfig) -> str | None:
    """The nearest caption-shaped text block vertically adjacent to `rect` (below, then above).

    A caption block starts with a "Figure N"-style label, sits within `caption_max_gap_pt`
    of the figure, and horizontally overlaps it. A caption block CONTAINED in the rect ranks
    best of all — vector clusters often swallow their own caption text (observed: doc 54 p7).
    None when nothing qualifies — captions are a bonus, not a requirement.
    """
    best: tuple[float, str] | None = None  # (rank, text); contained < below < above
    for x0, y0, x1, y1, text, *_rest in page.get_text("blocks"):
        text = " ".join(text.split())
        if not text or len(text) > _CAPTION_MAX_CHARS:
            continue
        if not (_FIGURE_LABEL.match(text) or _TABLE_LABEL.match(text)):
            continue
        if x1 <= rect.x0 or x0 >= rect.x1:
            continue  # no horizontal overlap
        if rect.y0 <= y0 and y1 <= rect.y1:  # caption inside the figure region
            gap_rank = -1.0
        elif 0 <= y0 - rect.y1 <= cfg.caption_max_gap_pt:  # below the figure
            gap_rank = y0 - rect.y1
        elif 0 <= rect.y0 - y1 <= cfg.caption_max_gap_pt:  # above the figure
            gap_rank = (rect.y0 - y1) + cfg.caption_max_gap_pt  # below always wins over above
        else:
            continue
        if best is None or gap_rank < best[0]:
            best = (gap_rank, text)
    return best[1] if best else None


def assign_sections(
    figures: list[ExtractedFigure], sections: list[ExtractedSection]
) -> None:
    """Set each figure's `section_position` to its enclosing section, in place.

    A figure belongs to a section whose `[page_start, page_end]` contains its page. When a
    page is shared by several sections, the caption (whitespace-normalised) breaks the tie —
    it appears verbatim in exactly one section's text; otherwise the narrowest page span wins
    (most specific section), then document order. No candidate => None (kept, doc-level
    provenance only).
    """
    for fig in figures:
        cands = [s for s in sections if s.page_start <= fig.page <= s.page_end]
        if not cands:
            continue
        if len(cands) > 1 and fig.caption:
            probe = " ".join(fig.caption.split()[:8])  # the label + a few words is enough
            pattern = re.compile(r"\s+".join(re.escape(w) for w in probe.split()), re.IGNORECASE)
            matches = [s for s in cands if pattern.search(s.text)]
            if matches:
                cands = matches
        cands.sort(key=lambda s: (s.page_end - s.page_start, s.position))
        fig.section_position = cands[0].position
