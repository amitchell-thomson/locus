"""Extract structured content from a PDF with PyMuPDF.

Produces an ExtractedDoc: a title plus ordered, page-anchored sections. Section boundaries
come from, in order of preference:

  1. the PDF's embedded outline / table of contents ("outline"),
  2. a font-size + numbered-heading heuristic ("headings"),
  3. a single whole-document section ("single").

Text is preserved verbatim from the PDF's text layer (no lossy normalisation), so whatever
math survives in the text layer is kept intact. One deliberate exception: printed
table-of-contents pages (dense dotted-leader lines) are excised before sectioning — they are
navigation noise that otherwise ingests as content, competes for retrieval slots, and seeds
bogus headings (2026-06-04 evaluation; PLAN.md step 4). The structure they describe is
captured properly in the section map.

LIMITATION (documented, not a bug): PyMuPDF reads the *text layer*. It does not reconstruct
LaTeX from rendered equation images, and a scanned PDF with no text layer yields empty text
(it would need OCR). Sections that likely contain math are flagged via `has_math` so a future
math-OCR pass can target them; that pass is out of scope for phase 1.
"""

from __future__ import annotations

import bisect
import re
from collections import Counter
from datetime import date
from pathlib import Path

import pymupdf
from pydantic import BaseModel

# PyMuPDF span flag bit for bold text.
_FLAG_BOLD = 1 << 4

# Section size band. Both bounds are structural guarantees on the *output* of detection,
# independent of how noisy heading detection is on a given PDF:
#   - sections longer than MAX are split into page-aligned windows (no unsummarisable blob),
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

# A table-of-contents leader line: 4+ dots (consecutive or spaced — both occur in the wild),
# usually trailing into a page number on the same or next text-layer line.
_DOTTED_LEADER = re.compile(r"(?:\.[ \t]*){4,}")
# A page is treated as printed ToC when it has at least this many leader lines...
_TOC_MIN_LEADER_LINES = 5
# ...or when leader lines make up at least this fraction of its non-empty lines.
_TOC_LEADER_FRACTION = 0.3


class ExtractedSection(BaseModel):
    position: int  # 0-based order within the document
    title: str | None  # section heading, or None for front matter / unknown
    text: str  # verbatim text of this section
    page_start: int  # 1-based page where the section begins
    page_end: int  # 1-based page where the section ends
    has_math: bool  # heuristic: section likely contains mathematical content


class ExtractedDoc(BaseModel):
    title: str | None
    page_count: int
    section_strategy: str  # "outline" | "headings" | "single"
    sections: list[ExtractedSection]
    source_path: str
    source_date: str | None = None  # ISO 'YYYY-MM-DD' from PDF metadata; None if absent/invalid
    toc_pages: list[int] = []  # 1-based pages excised as printed ToC (audit trail)


def extract_pdf(path: str | Path) -> ExtractedDoc:
    """Extract a structured ExtractedDoc from the PDF at `path`."""
    path = Path(path)
    doc = pymupdf.open(path)
    try:
        page_texts = [page.get_text("text") for page in doc]
        # Excise printed-ToC pages before sectioning: blanking (rather than removing) keeps
        # page numbering and offsets intact for everything downstream.
        toc_idxs = {i for i, t in enumerate(page_texts) if _is_toc_page(t)}
        page_texts = ["" if i in toc_idxs else t for i, t in enumerate(page_texts)]
        full_text, page_offsets = _full_text_and_offsets(page_texts)
        title = _resolve_title(doc, path)

        headings = _headings_from_outline(doc, page_offsets, full_text)
        strategy = "outline"
        if headings is None:
            headings = _headings_from_fonts(doc, page_offsets, full_text, skip_pages=toc_idxs)
            strategy = "headings"
        if not headings:
            headings = [(None, 0)]
            strategy = "single"

        sections = _build_sections(headings, full_text, page_offsets, len(page_texts))
        # A heading-poor doc that had to be windowed by page is "paginated", not "single".
        if strategy == "single" and len(sections) > 1:
            strategy = "paginated"
        return ExtractedDoc(
            title=title,
            page_count=len(page_texts),
            section_strategy=strategy,
            sections=sections,
            source_path=str(path),
            source_date=_resolve_source_date(doc),
            toc_pages=sorted(i + 1 for i in toc_idxs),
        )
    finally:
        doc.close()


def _is_toc_page(text: str) -> bool:
    """True when a page reads as a printed table of contents (dense dotted-leader lines)."""
    lines = [ln for ln in text.splitlines() if ln.strip()]
    if not lines:
        return False
    leaders = sum(1 for ln in lines if _DOTTED_LEADER.search(ln))
    # The fraction rule needs a floor of 3 leader lines so a stray "...." on a near-empty
    # page cannot blank real content.
    return leaders >= _TOC_MIN_LEADER_LINES or (
        leaders >= 3 and leaders / len(lines) >= _TOC_LEADER_FRACTION
    )


# --- text assembly -----------------------------------------------------------------------


def _full_text_and_offsets(page_texts: list[str]) -> tuple[str, list[int]]:
    """Concatenate page texts; return (full_text, page_start_offsets) where
    page_start_offsets[i] is the char offset at which page i (0-based) begins."""
    offsets: list[int] = []
    pos = 0
    for t in page_texts:
        offsets.append(pos)
        pos += len(t)
    return "".join(page_texts), offsets


def _page_of_offset(offset: int, page_offsets: list[int]) -> int:
    """Map a character offset to its 1-based page number."""
    idx = bisect.bisect_right(page_offsets, offset) - 1
    return max(0, idx) + 1


def _find_heading_offset(full_text: str, title: str, search_from: int) -> int:
    """Locate `title` in full_text at/after search_from, tolerating whitespace differences.
    Falls back to search_from if not found."""
    title = title.strip()
    if not title:
        return search_from
    pattern = re.compile(r"\s+".join(re.escape(w) for w in title.split()), re.IGNORECASE)
    m = pattern.search(full_text, search_from)
    return m.start() if m else search_from


# --- section-boundary strategies ---------------------------------------------------------


def _headings_from_outline(doc, page_offsets, full_text) -> list[tuple[str | None, int]] | None:
    """Strategy 1: derive headings from the embedded outline/TOC. None if there is no TOC."""
    toc = doc.get_toc()
    if not toc:
        return None
    headings: list[tuple[str | None, int]] = []
    prev_off = 0
    for _level, title, page in toc:
        page_idx = max(0, min(page - 1, len(page_offsets) - 1))
        search_from = max(page_offsets[page_idx], prev_off)
        off = max(_find_heading_offset(full_text, title, search_from), prev_off)
        headings.append((title.strip() or None, off))
        prev_off = off + 1
    return _finalize_headings(headings)


# Prose/equation shapes that disqualify a heading candidate. The font heuristic alone accepts
# paragraph leads and display math set in a larger font, which explodes a document into
# mid-sentence "sections" (2026-06-04 evaluation). Real headings start with a letter or digit,
# stay short (longest legitimate heading measured in the corpus: 8 words), balance their
# parentheses, and contain neither sentence breaks nor equation glyphs. A rejected real
# heading is a benign failure (its text merges into the previous section); an accepted
# sentence fragment mis-titles a section, so the filter errs strict.
_MAX_HEADING_WORDS = 8
_MULTI_SENTENCE = re.compile(r"[a-z][.!?][ \t]+[A-Z]")
_EQUATION_GLYPHS = set("=∇∂∑∫∏√≤≥≈≠±·")


def _plausible_heading(text: str) -> bool:
    if not text[:1].isalnum() or text[:1].islower():
        return False
    if any(ord(c) < 32 for c in text):  # control chars: mangled math in the text layer
        return False
    # Demand one real word (3+ ASCII letters): rejects math fragments like 'ˆi' whose
    # modifier letters count as alphanumeric to str.isalnum().
    if not re.search(r"[A-Za-z]{3,}", text):
        return False
    if len(text.split()) > _MAX_HEADING_WORDS:
        return False
    if text.rstrip().endswith((",", ";", "-", "–")):
        return False
    if _MULTI_SENTENCE.search(text):
        return False
    if _EQUATION_GLYPHS & set(text):
        return False
    if text.count("(") != text.count(")"):
        return False
    return True


def _headings_from_fonts(
    doc, page_offsets, full_text, skip_pages: set[int] = frozenset()
) -> list[tuple[str | None, int]] | None:
    """Strategy 2: heuristic headings from font size (vs. body size) and numbered markers.

    `skip_pages` (0-based) are excised ToC pages: their lines must not seed headings — the
    text was blanked, so offsets could not be located and the titles would be ToC entries.
    """
    lines: list[tuple[int, float, bool, str]] = []  # (page_idx, max_size, bold, text)
    size_weight: Counter[int] = Counter()
    for pno, page in enumerate(doc):
        if pno in skip_pages:
            continue
        for block in page.get_text("dict")["blocks"]:
            for line in block.get("lines", []):
                text = "".join(s["text"] for s in line["spans"]).strip()
                if not text:
                    continue
                max_size = max((s["size"] for s in line["spans"]), default=0.0)
                bold = any(s["flags"] & _FLAG_BOLD for s in line["spans"])
                lines.append((pno, max_size, bold, text))
                # Weight by text length so the dominant (body) size wins.
                size_weight[round(max_size)] += len(text)
    if not lines:
        return None

    body_size = max(size_weight.items(), key=lambda kv: kv[1])[0]

    # Bold is only a heading cue when it is *rare*. Some documents set their entire body in
    # bold; there "short bold line" matches nearly everything and is meaningless, so we fall
    # back to font size alone. (Documents with noisy font data instead trip the cap below and
    # fall through to the single/paginated path, which guarantees bounded sections.)
    total_chars = sum(len(t) for _, _, _, t in lines)
    bold_chars = sum(len(t) for _, _, b, t in lines if b)
    bold_is_rare = total_chars > 0 and bold_chars / total_chars < 0.4

    # Heading signal = larger-than-body font, OR (when bold is rare) a short bold line.
    # We deliberately do NOT treat body-size *numbered* lines as headings: enumerated list
    # items ("1. ...", "2. ...") are indistinguishable from numbered headings and would
    # explode a document into hundreds of micro-sections.
    headings: list[tuple[str | None, int]] = []
    for pno, size, bold, text in lines:
        if len(text) > 120:
            continue
        # Skip alpha-less candidates (page numbers, "1", "1.1", equation labels): they are not
        # section titles. Merge would absorb their fragments anyway, but this keeps titles clean.
        if not any(c.isalpha() for c in text):
            continue
        if not _plausible_heading(text):
            continue
        is_heading = size >= body_size * 1.15 or (bold_is_rare and bold and len(text) <= 100)
        if is_heading:
            off = _find_heading_offset(full_text, text, page_offsets[pno])
            headings.append((text, off))
    if not headings:
        return None

    # Sanity guard: if the heuristic still fires implausibly often AFTER the shape filter
    # (heading-poor document, noisy font data), treat it as unreliable and let the caller fall
    # back to a single/paginated section. ~1.5 headings per page is already implausibly dense.
    if len(headings) > max(8, round(1.5 * len(page_offsets))):
        return None

    headings.sort(key=lambda h: h[1])
    return _finalize_headings(headings)


def _finalize_headings(headings: list[tuple[str | None, int]]) -> list[tuple[str | None, int]]:
    """Sort, drop near-duplicate offsets, and prepend a front-matter section if text
    precedes the first heading."""
    headings = sorted(headings, key=lambda h: h[1])
    deduped: list[tuple[str | None, int]] = []
    for title, off in headings:
        if deduped and off - deduped[-1][1] < 2:
            continue
        deduped.append((title, off))
    if deduped and deduped[0][1] > 0:
        deduped.insert(0, (None, 0))
    return deduped


def _build_sections(
    headings: list[tuple[str | None, int]],
    full_text: str,
    page_offsets: list[int],
    page_count: int,
) -> list[ExtractedSection]:
    """Slice full_text at heading offsets into page-anchored sections.

    Sections are forced into the [MIN, MAX] size band, independent of detection noise:
      - tiny/heading-only spans are merged into neighbours (no empty, fragment sections),
      - oversized spans are split into page-aligned windows (no giant, unsummarisable blob).
    This is what saves both over-segmented (noisy heading) and heading-poor documents.
    """
    # Raw spans between consecutive heading offsets (drop empties).
    raw: list[tuple[str | None, int, int]] = []
    for i, (title, start) in enumerate(headings):
        end = headings[i + 1][1] if i + 1 < len(headings) else len(full_text)
        if full_text[start:end].strip():
            raw.append((title, start, end))

    merged = _merge_small(raw, full_text, MIN_SECTION_CHARS)

    spans: list[tuple[str | None, int, int]] = []
    for title, start, end in merged:
        if end - start <= MAX_SECTION_CHARS:
            spans.append((title, start, end))
        else:
            spans.extend(_paginate_span(title, start, end, page_offsets))

    sections: list[ExtractedSection] = []
    for title, start, end in spans:
        text = full_text[start:end].strip()
        if not text:
            continue
        sections.append(
            ExtractedSection(
                position=len(sections),
                title=title,
                text=text,
                page_start=_page_of_offset(start, page_offsets),
                page_end=_page_of_offset(max(start, end - 1), page_offsets),
                has_math=_has_math(text),
            )
        )
    if not sections:
        # Nothing usable (e.g. empty/scanned PDF with no text layer): one whole-doc section.
        whole = full_text.strip()
        sections.append(
            ExtractedSection(
                position=0, title=None, text=whole, page_start=1,
                page_end=max(1, page_count), has_math=_has_math(whole),
            )
        )
    return sections


def _merge_small(
    spans: list[tuple[str | None, int, int]], full_text: str, min_chars: int
) -> list[tuple[str | None, int, int]]:
    """Merge consecutive spans until each has >= min_chars of body text.

    Collapses heading-only / fragment sections (the over-segmentation failure mode) into real,
    summarisable sections. A merged section keeps the first non-empty heading as its title, so a
    bare heading is absorbed into the content that follows it. Real sections already exceed
    min_chars and are emitted unchanged. A trailing under-sized span is folded into the previous
    section (or kept alone if it is the only one).
    """
    merged: list[tuple[str | None, int, int]] = []
    cur_title: str | None = None
    cur_start: int | None = None
    cur_end: int | None = None

    for title, start, end in spans:
        if cur_start is None:
            cur_title, cur_start, cur_end = title, start, end
        else:
            cur_end = end
            if cur_title is None:
                cur_title = title
        if len(full_text[cur_start:cur_end].strip()) >= min_chars:
            merged.append((cur_title, cur_start, cur_end))
            cur_title, cur_start, cur_end = None, None, None

    if cur_start is not None:  # trailing under-sized bucket
        if merged and len(full_text[cur_start:cur_end].strip()) < min_chars:
            pt, ps, _ = merged[-1]
            merged[-1] = (pt, ps, cur_end)
        else:
            merged.append((cur_title, cur_start, cur_end))
    return merged


def _paginate_span(
    title: str | None, start: int, end: int, page_offsets: list[int]
) -> list[tuple[str | None, int, int]]:
    """Split an oversized [start, end) span into page-aligned windows of <= MAX_SECTION_CHARS.

    Greedily groups whole pages so a window is never cut mid-page (unless a single page itself
    exceeds the limit). Titles get a "(pp X–Y)" suffix so the split is legible downstream.
    """
    boundaries = [start] + [o for o in page_offsets if start < o < end] + [end]
    windows: list[tuple[int, int]] = []
    seg_start, seg_end = boundaries[0], boundaries[0]
    for nxt in boundaries[1:]:
        if nxt - seg_start > MAX_SECTION_CHARS and seg_end > seg_start:
            windows.append((seg_start, seg_end))
            seg_start = seg_end
        seg_end = nxt
    windows.append((seg_start, seg_end))

    multi = len(windows) > 1
    result: list[tuple[str | None, int, int]] = []
    for s, e in windows:
        if multi:
            ps = _page_of_offset(s, page_offsets)
            pe = _page_of_offset(max(s, e - 1), page_offsets)
            label = f"{title or 'Section'} (pp {ps}–{pe})"
        else:
            label = title
        result.append((label, s, e))
    return result


# --- title + math heuristics -------------------------------------------------------------


_TITLE_PREFIX = re.compile(r"^(?:microsoft word|powerpoint|pdf)\s*[-:]\s*", re.IGNORECASE)
_TITLE_EXT = re.compile(r"\.(?:pdf|docx?|pptx?)$", re.IGNORECASE)


def _clean_title(t: str) -> str:
    """Strip common export artifacts (e.g. 'Microsoft Word - ', trailing extensions)."""
    return _TITLE_EXT.sub("", _TITLE_PREFIX.sub("", t.strip())).strip()


def _resolve_title(doc, path: Path) -> str | None:
    """Document metadata title, else the largest text on page 1, else the filename stem."""
    meta = _clean_title((doc.metadata or {}).get("title") or "")
    if len(meta) > 3:
        return meta
    if doc.page_count:
        best_text, best_size = "", 0.0
        for block in doc[0].get_text("dict")["blocks"]:
            for line in block.get("lines", []):
                text = "".join(s["text"] for s in line["spans"]).strip()
                size = max((s["size"] for s in line["spans"]), default=0.0)
                if text and len(text) > 4 and size > best_size:
                    best_text, best_size = text, size
        if best_text:
            return best_text
    return path.stem


def _has_math(text: str) -> bool:
    """Heuristic: True if the text shows several math indicators. A hint, not a guarantee."""
    return len(_MATH.findall(text)) >= 3


# PDF dates are 'D:YYYYMMDDHHmmSS...' (PDF spec); month/day/time are optional after the year.
_PDF_DATE = re.compile(r"D:(\d{4})(\d{2})?(\d{2})?")


def _parse_pdf_date(raw: str | None) -> str | None:
    """Parse a PDF date string into an ISO 'YYYY-MM-DD'. None if absent or not a real date.

    Missing month/day default to 01. The date is validated (via datetime.date) so a garbled
    value (e.g. month 13) yields None rather than an impossible date.
    """
    if not raw:
        return None
    m = _PDF_DATE.match(raw.strip())
    if not m:
        return None
    year, month, day = int(m.group(1)), int(m.group(2) or 1), int(m.group(3) or 1)
    try:
        return date(year, month, day).isoformat()
    except ValueError:
        return None


def _resolve_source_date(doc) -> str | None:
    """PDF creation date if present and valid, else modification date, else None."""
    meta = doc.metadata or {}
    return _parse_pdf_date(meta.get("creationDate")) or _parse_pdf_date(meta.get("modDate"))
