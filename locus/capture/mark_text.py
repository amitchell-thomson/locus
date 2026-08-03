"""Read the handwriting the owner wrote BESIDE a passage (Loop B, the other half).

`annotate.py` answers "which words did he mark?" by geometry, and deliberately never calls a
model. That left the reading loop half-captured: the system knew precisely what he marked and
nothing about what he SAID about it. Measured 2026-07-31 on *Advanced Portfolio Management* —
23 of 26 marks carried the book's text, ZERO carried his, and one page held 342 strokes of his
own writing stored as bare coordinates.

Transcribing that is the one part of Loop B a model genuinely has to do, so it lives here rather
than in the geometry module, and it is opt-in (`locus annotate --transcribe`).

**THE INK IS RENDERED ON ITS OWN, not cropped out of the page.** Two reasons, and the first is a
correctness bug rather than a preference:

  - marginalia legitimately falls OUTSIDE the page rectangle (the tablet screen is wider than a
    portrait page), and `composite_pdf` cannot draw there — pymupdf refuses, so those strokes are
    dropped from a composited render. Cropping a composite would silently lose exactly the notes
    written furthest into the margin, which tend to be the longest ones;
  - printed text under the handwriting is interference. For transcription the model wants the
    strokes and nothing else; the passage is already known by geometry and is supplied as text.

WHICH MARKS COST A CALL. Stroke count, not `kind`. On the real book the distribution has an empty
band — nine marks at 1-2 strokes (underlines, brackets, ticks: gestures, nothing to read) and
then a jump straight to 13+. `kind` would be the wrong filter: an `underline` there carries 41
strokes because a note was clustered with it, and a `mark` carries 113. The threshold sits in the
observed gap so it is robust to a stroke or two either way.
"""

from __future__ import annotations

import base64
import json
import logging
import sqlite3

from pydantic import BaseModel, Field

from locus.config import Config, load

log = logging.getLogger(__name__)

# Below this a mark is a GESTURE, not writing. Chosen from the empty band (2 -> 13) observed on
# the first real book; see the module docstring.
MIN_INK_STROKES = 6

# The rendered crop is scaled up to at least this width in pixels. Handwriting in a page margin
# is physically small — a 90pt-wide note rendered 1:1 is unreadable to a vision model.
_MIN_RENDER_PX = 900
_MAX_RENDER_PX = 2400
_PAD_PT = 6.0


class _Transcription(BaseModel):
    text: str = Field(
        default="",
        description="The handwriting, transcribed verbatim. Empty string if it is not writing.",
    )


_SYSTEM = (
    "You read handwriting. You transcribe only what is physically written; you never infer, "
    "complete, correct, or tidy. Illegible words are omitted rather than guessed."
)

_USER = (
    "This image shows ONLY the ink of a handwritten annotation, lifted off the page it was "
    "written on. Transcribe it verbatim.\n\n"
    "It may be a note, a question, an arrow, a tick, or an underline. If it carries no words at "
    "all — a bracket, a squiggle, a shape — return an empty string rather than describing it. "
    "Never invent words that are not there."
)


def has_ink(mark) -> bool:
    """True when a mark is handwriting worth paying a model to read."""
    return (mark.stroke_count or 0) >= MIN_INK_STROKES


def render_ink(mark) -> bytes:
    """A PNG of just this mark's strokes, on white, scaled up to be legible.

    Returns b"" when the mark carries no stroke geometry (it was loaded from the DB rather than
    parsed from a `.rmdoc`).
    """
    import pymupdf

    strokes = getattr(mark, "strokes", None) or []
    if not strokes:
        return b""

    x0, y0, x1, y1 = mark.bbox
    width = max(x1 - x0, 1.0) + 2 * _PAD_PT
    height = max(y1 - y0, 1.0) + 2 * _PAD_PT

    doc = pymupdf.open()
    try:
        page = doc.new_page(width=width, height=height)
        for stroke in strokes:
            pts = [
                pymupdf.Point(x - x0 + _PAD_PT, y - y0 + _PAD_PT) for x, y in stroke.points
            ]
            if len(pts) < 2:
                continue
            page.draw_polyline(pts, color=(0, 0, 0), width=0.9)
        zoom = min(max(_MIN_RENDER_PX / width, 1.0), _MAX_RENDER_PX / max(width, 1.0))
        pix = page.get_pixmap(matrix=pymupdf.Matrix(zoom, zoom))
        return pix.tobytes("png")
    finally:
        doc.close()


def transcribe_ink(png: bytes, *, client=None, model: str | None = None) -> str:
    """The words in one rendered annotation. '' when there are none.

    `client` is injectable so the tests stay model-free, matching `capture/transcribe.py`.
    """
    if not png:
        return ""
    cfg = load().capture
    model = model or cfg.transcribe_model
    if client is None:
        import anthropic

        client = anthropic.Anthropic(api_key=Config.anthropic_api_key())

    schema = json.dumps(_Transcription.model_json_schema())
    resp = client.messages.create(
        model=model,
        max_tokens=1024,
        system=[{"type": "text", "text": _SYSTEM}],
        messages=[{"role": "user", "content": [
            {"type": "image", "source": {
                "type": "base64", "media_type": "image/png",
                "data": base64.standard_b64encode(png).decode("utf-8"),
            }},
            {"type": "text", "text": f"{_USER}\n\nReply with ONLY JSON matching:\n{schema}"},
        ]}],
    )
    body = "".join(b.text for b in resp.content if getattr(b, "type", None) == "text")
    start, end = body.find("{"), body.rfind("}")
    if start == -1 or end <= start:
        return ""
    try:
        return " ".join(_Transcription.model_validate_json(body[start : end + 1]).text.split())
    except Exception:
        # A malformed reply loses one transcription; it must never abort the batch or, worse,
        # write a guess into the note column.
        log.warning("mark transcription: unparseable reply, skipping")
        return ""


def transcribe_marks(
    conn: sqlite3.Connection,
    marks,
    *,
    source_uri: str,
    client=None,
    model: str | None = None,
    limit: int | None = None,
) -> int:
    """Transcribe every un-transcribed handwritten mark. Returns how many were written.

    Skips marks that already carry a `note`: re-running `locus annotate --transcribe` must not
    re-pay for ink it has already read, and `store_marks` deliberately never overwrites `note`
    for the same reason.
    """
    from locus.capture.annotate import bbox_key

    done = {
        row["bbox_key"]
        for row in conn.execute(
            "SELECT bbox_key FROM pdf_annotations WHERE source_uri=? AND note IS NOT NULL",
            (source_uri,),
        )
    }

    written = 0
    for mark in marks:
        if limit is not None and written >= limit:
            break
        key = bbox_key(mark.bbox)
        if not has_ink(mark) or key in done:
            continue
        text = transcribe_ink(render_ink(mark), client=client, model=model)
        if not text:
            continue
        with conn:
            conn.execute(
                "UPDATE pdf_annotations SET note=? WHERE source_uri=? AND pdf_page=? "
                "AND bbox_key=?",
                (text, source_uri, mark.pdf_page, key),
            )
            # THE EVIDENCE JUST CHANGED, so a GUESSED intent has to be guessed again. An intent
            # inferred before the handwriting was read was inferred from the passage alone, and
            # `intent.pending_marks` never revisits a mark that already has one — so it would
            # keep that verdict permanently. Live: mark 25 was classified `not_understood` with
            # no note at all; the ink says "interesting systematic portfolio construction".
            # An intent HE set is untouched, on the same reasoning that protects it elsewhere.
            conn.execute(
                "UPDATE pdf_annotations SET intent=NULL, intent_confidence=NULL, "
                "intent_by=NULL, intent_at=NULL "
                "WHERE source_uri=? AND pdf_page=? AND bbox_key=? "
                "AND COALESCE(intent_by,'') = 'model'",
                (source_uri, mark.pdf_page, key),
            )
        written += 1
    return written
