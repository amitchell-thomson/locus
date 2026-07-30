"""Turn reMarkable strokes into "he marked THIS passage" (Loop B, agent-layer plan §8.2).

`rmdoc.py` gives strokes in PDF page coordinates. This module answers the question that makes
them useful: **which words did each mark actually cover?**

That is deliberately geometry, not vision. A bracket down the margin spans a precise range of
lines; an underline sits on a precise run of words. Asking a model to look at a picture and
guess would be slower, billed, and less accurate than intersecting rectangles — and the model
is then free to do the one thing it is actually better at, which is saying why the passage
mattered.

MARK KINDS, and why they are separated:

  - `underline`  — a wide, flat stroke lying ON a line of text. The covered text is the words it
    runs beneath. This is the most common "this sentence matters" gesture.
  - `bracket`    — a tall, narrow stroke beside the text. It selects the LINES it spans, not the
    words it touches, because it deliberately sits in the margin next to them.
  - `margin_note`— strokes outside the page rectangle, or a dense cluster in the margin. There
    is no covered text to speak of; the value is the handwriting itself, and the passage it
    belongs to is the text at the same height. Often the most valuable annotation on the page.
  - `mark`       — anything else (circles, arrows, ticks). Covered text is whatever it overlaps.

The nearest-line rule matters more than it looks. An underline drawn by hand sits slightly BELOW
the glyph boxes it refers to, so a naive `intersects` test returns nothing at all, or the line
underneath. Words are therefore matched against a band around the stroke, and a stroke that is
flat and wide is attributed to the nearest text line ABOVE it.
"""

from __future__ import annotations

from dataclasses import dataclass, field

from locus.capture.rmdoc import AnnotatedPage, Stroke

# A stroke wider than this multiple of its height is a rule, not a shape.
_FLAT_RATIO = 4.0
# ...and one taller than this multiple of its width is a bracket/margin bar.
_TALL_RATIO = 3.0
# Vertical slack when matching an underline to the line it belongs to, in points. Hand-drawn
# underlines sit below the glyph box; without this they match nothing.
_UNDERLINE_BAND = 9.0
# Strokes closer than this vertically are treated as one annotation (a bracket plus its note).
_CLUSTER_GAP = 26.0


@dataclass
class Mark:
    """One annotation: a cluster of strokes, and the text it covers."""

    kind: str                       # 'underline' | 'bracket' | 'margin_note' | 'mark'
    pdf_page: int                   # 0-based
    bbox: tuple[float, float, float, float]
    covered_text: str = ""
    line_text: str = ""             # the full line(s) the mark sits on, for context
    in_margin: bool = False
    stroke_count: int = 1
    point_count: int = 0

    @property
    def has_text(self) -> bool:
        return bool(self.covered_text.strip())


def _cluster(strokes: list[Stroke]) -> list[list[Stroke]]:
    """Group strokes that belong to one gesture, by vertical proximity.

    A bracket and the words written beside it are one annotation, not seven; handwriting is
    dozens of short strokes that mean a single thing.
    """
    if not strokes:
        return []
    ordered = sorted(strokes, key=lambda s: s.bbox[1])
    groups: list[list[Stroke]] = [[ordered[0]]]
    for s in ordered[1:]:
        prev_bottom = max(x.bbox[3] for x in groups[-1])
        if s.bbox[1] - prev_bottom <= _CLUSTER_GAP:
            groups[-1].append(s)
        else:
            groups.append([s])
    return groups


def _union(strokes: list[Stroke]) -> tuple[float, float, float, float]:
    xs0 = min(s.bbox[0] for s in strokes)
    ys0 = min(s.bbox[1] for s in strokes)
    xs1 = max(s.bbox[2] for s in strokes)
    ys1 = max(s.bbox[3] for s in strokes)
    return xs0, ys0, xs1, ys1


def classify(bbox: tuple[float, float, float, float], *, page_width: float) -> tuple[str, bool]:
    """(kind, in_margin) for a clustered mark."""
    x0, y0, x1, y1 = bbox
    w, h = max(x1 - x0, 0.01), max(y1 - y0, 0.01)
    # Outside the page, or hugging an outer edge: written beside the text rather than on it.
    in_margin = x0 > page_width * 0.82 or x1 > page_width or x1 < page_width * 0.18

    # SHAPE decides the gesture; position only decides `in_margin`. A bracket drawn down the
    # left margin is still a bracket — it selects the lines it spans — and calling it a margin
    # note because of where it sits would lose that. Only a mark whose shape says nothing falls
    # back to being read as handwriting beside the text.
    if w / h >= _FLAT_RATIO:
        return "underline", in_margin
    if h / w >= _TALL_RATIO:
        return "bracket", in_margin
    if in_margin:
        return "margin_note", True
    return "mark", False


def _words(page) -> list[tuple[float, float, float, float, str]]:
    return [(w[0], w[1], w[2], w[3], w[4]) for w in page.get_text("words")]


def _lines(page) -> list[tuple[float, float, float, float, str]]:
    """(x0, y0, x1, y1, text) per text line, via the raw dict — used for context."""
    out = []
    for block in page.get_text("dict").get("blocks", []):
        for line in block.get("lines", []):
            text = "".join(span.get("text", "") for span in line.get("spans", []))
            if text.strip():
                x0, y0, x1, y1 = line["bbox"]
                out.append((x0, y0, x1, y1, text))
    return out


def _covered(
    kind: str, bbox: tuple[float, float, float, float], words, lines
) -> tuple[str, str]:
    """(covered_text, line_context) for a mark."""
    x0, y0, x1, y1 = bbox

    if kind == "underline":
        # A hand-drawn underline sits below its words: search a band ABOVE the stroke.
        band_top, band_bottom = y0 - _UNDERLINE_BAND, y1 + 2.0
        hits = [
            w for w in words
            if w[3] > band_top and w[1] < band_bottom and w[2] > x0 - 2 and w[0] < x1 + 2
        ]
    elif kind in ("bracket", "margin_note"):
        # Selects the LINES it spans — it sits beside the text, not on it.
        hits = [w for w in words if w[3] > y0 and w[1] < y1]
    else:
        hits = [
            w for w in words
            if w[3] > y0 and w[1] < y1 and w[2] > x0 and w[0] < x1
        ]

    hits.sort(key=lambda w: (round(w[1], 1), w[0]))
    covered = " ".join(w[4] for w in hits)

    ctx = [ln[4] for ln in lines if ln[3] > y0 - _UNDERLINE_BAND and ln[1] < y1 + 2.0]
    return covered.strip(), " ".join(ctx).strip()


def marks_for_page(page, annotated: AnnotatedPage) -> list[Mark]:
    """Every annotation on one page, with the text each covers.

    `page` is the pymupdf page for `annotated.pdf_page`.
    """
    words, lines = _words(page), _lines(page)
    out: list[Mark] = []
    for group in _cluster(annotated.strokes):
        bbox = _union(group)
        kind, in_margin = classify(bbox, page_width=page.rect.width)
        covered, ctx = _covered(kind, bbox, words, lines)
        out.append(
            Mark(
                kind=kind,
                pdf_page=annotated.pdf_page,
                bbox=bbox,
                covered_text=covered,
                line_text=ctx,
                in_margin=in_margin,
                stroke_count=len(group),
                point_count=sum(len(s.points) for s in group),
            )
        )
    return out


def marks_for_document(rmdoc) -> list[Mark]:
    """Every annotation in a parsed `.rmdoc`, page order."""
    import pymupdf

    doc = pymupdf.open(stream=rmdoc.pdf_bytes, filetype="pdf")
    try:
        out: list[Mark] = []
        for annotated in rmdoc.pages:
            if annotated.pdf_page < doc.page_count:
                out.extend(marks_for_page(doc[annotated.pdf_page], annotated))
        return out
    finally:
        doc.close()


# --- persistence ------------------------------------------------------------------------------


def bbox_key(bbox: tuple[float, float, float, float]) -> str:
    """Stable identity for a mark: its bounding box rounded to whole points.

    The owner's ink does not move between syncs, so the box is a durable key. Rounding absorbs
    the float noise of the coordinate transform without merging genuinely separate marks.
    """
    return ",".join(str(int(round(v))) for v in bbox)


def store_marks(conn, marks, *, source_uri: str, doc_uuid: str = "", source_run=None) -> int:
    """Upsert marks for one document. Returns the number of rows written.

    Idempotent by (source_uri, pdf_page, bbox_key): re-reading the same `.rmdoc` revises a mark
    rather than duplicating it. `note` and `object_id` are NOT overwritten — those are derived
    or owner-supplied, and a re-sync of the same ink must not discard them.
    """
    import json
    from datetime import datetime, timezone

    stamp = datetime.now(timezone.utc).isoformat()
    written = 0
    with conn:
        for m in marks:
            conn.execute(
                "INSERT INTO pdf_annotations (source_uri, doc_uuid, pdf_page, kind, bbox_key, "
                "bbox, covered_text, line_text, in_margin, stroke_count, point_count, "
                "source_run, captured_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?) "
                "ON CONFLICT(source_uri, pdf_page, bbox_key) DO UPDATE SET "
                "kind=excluded.kind, covered_text=excluded.covered_text, "
                "line_text=excluded.line_text, in_margin=excluded.in_margin, "
                "stroke_count=excluded.stroke_count, point_count=excluded.point_count",
                (
                    source_uri, doc_uuid, m.pdf_page, m.kind, bbox_key(m.bbox),
                    json.dumps([round(v, 2) for v in m.bbox]), m.covered_text, m.line_text,
                    int(m.in_margin), m.stroke_count, m.point_count, source_run, stamp,
                ),
            )
            written += 1
    return written
