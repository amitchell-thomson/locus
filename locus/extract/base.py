"""Shared extraction types and section-building helpers for all extractors.

Every extractor (pdf, docx, markdown/text/notebook, later pptx/code) produces the same
`ExtractedDoc` shape so the ingest pipeline stays source-type-agnostic (CLAUDE.md §2.6).
PDF-specific fields (`toc_pages`, `page_flags`, `ocr_*`) default to empty; non-paginated
formats set `page_count=0` and `page_start=page_end=1` — "page" is vestigial for them.

`build_sections` gives non-paginated extractors the same structural guarantees pdf.py
enforces offset-wise: no heading-only fragments below MIN_SECTION_CHARS, no unsummarisable
blob above MAX_SECTION_CHARS (the summary pass interpolates full section text, untruncated).
"""

from __future__ import annotations

import re

from pydantic import BaseModel

# Section size band. Both bounds are structural guarantees on the *output* of detection,
# independent of how noisy heading detection is on a given source:
#   - sections longer than MAX are split into windows (no unsummarisable blob),
#   - sections shorter than MIN are merged into their neighbours (no heading-only fragments).
# ~12k chars ≈ 3k tokens; 400 chars cleanly separates real sections (measured 600-8000c) from
# heading/label fragments (<200c).
MAX_SECTION_CHARS = 12_000
MIN_SECTION_CHARS = 400

# Heuristic math indicators (LaTeX commands, common math unicode, inline $...$).
_MATH = re.compile(
    r"\\(?:frac|sum|int|prod|sqrt|partial|nabla|alpha|beta|gamma|theta|sigma|lambda|mu|"
    r"infty|begin\{)|[∑∫∏√≤≥≈≠∂∇∞±×÷πθλμσΣΩ]|\$[^$\n]{1,80}\$"
)
# Display-math markup: one hit is unambiguous (unlike the inline indicators above, which
# need several to outweigh prose noise). Covers $$...$$, LaTeX environments, ```math fences.
MATH_MARKUP = re.compile(r"\$\$|\\begin\{(?:equation|align|aligned|cases)\*?\}|```math")


def has_math(text: str) -> bool:
    """Heuristic: several inline math indicators, or any display-math markup. A hint."""
    return len(_MATH.findall(text)) >= 3 or MATH_MARKUP.search(text) is not None


# Thresholds for PageFlags (measured on the corpus; see PLAN.md step 6).
_MATH_DENSE_CHARS = 30  # chars set in math fonts on a page
_MATHUNI_CHARS = 20  # math-unicode chars in the text layer on a page
_SMALL_IMAGE_MIN = 3  # small raster images (formula-sized) on a page


class PageFlags(BaseModel):
    """Per-page extraction-damage and math-evidence signals (1-based page number)."""

    page: int
    ligature_hits: int = 0  # broken-ligature words in the text layer
    symbol_garbage: int = 0  # mis-mapped symbol glyphs ('H(!)', 'ei!t')
    mathfont_chars: int = 0  # characters set in TeX math fonts
    mathuni_chars: int = 0  # math-unicode chars in the text layer (𝑐, ℝ, ∂ ...)
    small_images: int = 0  # formula-sized raster images
    drawings: int = 0  # vector drawing groups (Colab formulas render as these)
    gap_hits: int = 0  # mid-sentence inline formula gaps

    @property
    def corrupted(self) -> bool:
        """Text layer is provably damaged (content already lost)."""
        return self.ligature_hits >= 2 or self.symbol_garbage >= 2

    @property
    def math_dense(self) -> bool:
        """Typeset math present, by font evidence or math-unicode density (the text-layer
        rendering of math is unreliable in both cases)."""
        return self.mathfont_chars >= _MATH_DENSE_CHARS or self.mathuni_chars >= _MATHUNI_CHARS

    @property
    def image_math(self) -> bool:
        """Formulas rendered as images/vector drawings — absent from the text layer."""
        return self.small_images >= _SMALL_IMAGE_MIN or (
            self.drawings >= 3 and self.gap_hits >= 2
        )

    @property
    def needs_ocr(self) -> bool:
        return self.corrupted or self.math_dense or self.image_math


class PreChunk(BaseModel):
    """An extractor-supplied chunk with provenance, bypassing the generic token splitter.

    Code uses these for function-granular chunks with real line spans; the future video
    extractor can carry `video_timestamp` the same way. All provenance fields optional.
    """

    text: str
    file_path: str | None = None
    line_start: int | None = None
    line_end: int | None = None
    video_timestamp: int | None = None  # seconds, for ?t= deep links


class PreEntity(BaseModel):
    """An extractor-supplied entity (deterministic, e.g. from a code AST — no LLM).

    `type` is a plain string here to keep extract/ independent of ingest/; the pipeline
    validates it against the closed EntityType vocabulary when converting.
    """

    name: str
    type: str


class ExtractedSection(BaseModel):
    position: int  # 0-based order within the document
    title: str | None  # section heading, or None for front matter / unknown
    text: str  # verbatim text of this section
    page_start: int  # 1-based page where the section begins (1 for unpaginated formats)
    page_end: int  # 1-based page where the section ends (1 for unpaginated formats)
    has_math: bool  # heuristic: section likely contains mathematical content
    # code provenance (None for prose formats)
    file_path: str | None = None  # repo-relative file path
    call_graph: dict | None = None  # {qualified_def: [callee_names]} for code files
    chunks: list[PreChunk] | None = None  # extractor-supplied chunks; None => chunk_text
    entities: list[PreEntity] | None = None  # extractor-supplied entities; None => LLM pass


class ExtractedDoc(BaseModel):
    title: str | None
    page_count: int  # 0 for unpaginated formats (md/txt/docx/ipynb)
    section_strategy: str  # pdf: "outline"|"headings"|"single"; others add "windowed"
    sections: list[ExtractedSection]
    source_path: str
    source_date: str | None = None  # ISO 'YYYY-MM-DD' from embedded metadata; None if absent
    toc_pages: list[int] = []  # 1-based pages excised as printed ToC (pdf audit trail)
    page_flags: list[PageFlags] = []  # per-page damage/math signals (pdf OCR routing)
    ocr_replaced: list[int] = []  # 1-based pages whose text the math-OCR pass replaced
    ocr_fallbacks: list[str] = []  # "page: reason" entries where QC kept the original text


def window_by_chars(
    title: str | None, text: str, *, max_chars: int = MAX_SECTION_CHARS
) -> list[tuple[str | None, str]]:
    """Split oversized text into <= max_chars windows on paragraph (blank-line) boundaries.

    The unpaginated analogue of pdf.py's `_paginate_span`: greedy packing of whole
    paragraphs, hard-splitting only a single paragraph that itself exceeds the limit.
    Titles get a "(part N)" suffix so the split is legible downstream. Text at or under
    the limit is returned unchanged as a single window.
    """
    if len(text) <= max_chars:
        return [(title, text)]
    paragraphs = re.split(r"(\n\s*\n)", text)  # keep separators so the text stays verbatim
    windows: list[str] = []
    cur = ""
    for piece in paragraphs:
        if cur and len(cur) + len(piece) > max_chars:
            windows.append(cur)
            cur = ""
        while len(piece) > max_chars:  # one pathological paragraph: hard-split
            if cur:
                windows.append(cur)
                cur = ""
            windows.append(piece[:max_chars])
            piece = piece[max_chars:]
        cur += piece
    if cur.strip():
        windows.append(cur)
    return [(f"{title or 'Section'} (part {i + 1})", w) for i, w in enumerate(windows)]


def build_sections(spans: list[tuple[str | None, str]]) -> list[ExtractedSection]:
    """Turn raw (title, text) spans into size-banded ExtractedSections.

    Mirrors pdf.py's `_build_sections` for unpaginated sources: drop empty spans, merge
    consecutive under-MIN spans (a merged section keeps the first non-empty heading — span
    text is verbatim and includes its own heading line, so absorbed headings stay visible),
    window over-MAX spans, then assign positions and the has_math flag. `page_start` and
    `page_end` are fixed at 1 (vestigial for unpaginated formats).
    """
    spans = [(t, x) for t, x in spans if x.strip()]

    merged: list[tuple[str | None, str]] = []
    cur_title: str | None = None
    cur_text = ""
    for title, text in spans:
        if not cur_text:
            cur_title, cur_text = title, text
        else:
            cur_text += "\n\n" + text
            if cur_title is None:
                cur_title = title
        if len(cur_text.strip()) >= MIN_SECTION_CHARS:
            merged.append((cur_title, cur_text))
            cur_title, cur_text = None, ""
    if cur_text:  # trailing under-sized bucket: fold into the previous section if any
        if merged and len(cur_text.strip()) < MIN_SECTION_CHARS:
            pt, px = merged[-1]
            merged[-1] = (pt, px + "\n\n" + cur_text)
        else:
            merged.append((cur_title, cur_text))

    windowed: list[tuple[str | None, str]] = []
    for title, text in merged:
        windowed.extend(window_by_chars(title, text))

    return [
        ExtractedSection(
            position=i,
            title=title,
            text=text.strip(),
            page_start=1,
            page_end=1,
            has_math=has_math(text),
        )
        for i, (title, text) in enumerate(windowed)
    ]
