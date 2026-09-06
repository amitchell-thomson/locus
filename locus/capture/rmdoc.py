"""Read reMarkable annotation geometry out of a `.rmdoc` bundle (Loop B transport).

**This replaces the device-side render entirely, and needs no device at all.** `rmapi get`
returns a `.rmdoc` — a zip holding the original PDF, a `.content` manifest, and one `.rm` file
per annotated page. The annotations are vector strokes with coordinates, which is exactly what
Loop B wants: the goal is to know WHICH TEXT was marked, and that is a geometry question, not a
rendering one.

Three earlier approaches failed, and it is worth recording why so nobody retries them:

  - `rmapi geta` (cloud-side annotated render) fails on this document with
    "no uuid pagemap". The pagemap it cannot find is in fact right there in `.content`
    (`cPages.pages[].redir.value`); this module reads it directly.
  - the device's own `/download/<uuid>/pdf` endpoint — the transport Loop A relies on — returns
    the ORIGINAL file for an uploaded PDF. Verified 2026-07-30 by inspecting all 211 pages of
    the staged Advanced Portfolio Management: zero handwriting, and the "ink-like" strokes were
    the book's own typographic rules and chart lines. It composites ink for NOTEBOOKS, which is
    why Loop A works and why this looked like it should.
  - rendering `.rm` v6 to an image (rmscene as a renderer) was the documented dead end. Loop B
    does not need pixels.

COORDINATE MAPPING (established empirically 2026-07-30 by overlaying strokes on the page and
looking at the result; a width-fit assumption put an underline a full line too high):

    scale  = max(page_width / SCREEN_W, page_height / SCREEN_H)
    pdf_x  = page_width / 2 + rm_x * scale        # rm_x is 0 at the PAGE's centre
    pdf_y  = rm_y * scale                         # rm_y is 0 at the top

The page is fit to the screen, so the constraining dimension sets the scale. Strokes may fall
OUTSIDE the page rectangle and that is not an error: the screen is wider than a portrait page,
so marginalia written beside the page has no page coordinates. Those strokes are kept and
flagged rather than clipped — a margin note is often the most valuable annotation on the page.
"""

from __future__ import annotations

import io
import json
import zipfile
from dataclasses import dataclass, field
from pathlib import Path

# reMarkable Paper Pro panel, in the units `.rm` stroke coordinates use.
SCREEN_W = 1620.0
SCREEN_H = 2160.0


@dataclass
class Stroke:
    """One pen stroke, in PDF page points."""

    points: list[tuple[float, float]]
    tool: int | None = None
    color: int | None = None

    @property
    def bbox(self) -> tuple[float, float, float, float]:
        xs = [p[0] for p in self.points]
        ys = [p[1] for p in self.points]
        return min(xs), min(ys), max(xs), max(ys)

    @property
    def width(self) -> float:
        x0, _, x1, _ = self.bbox
        return x1 - x0

    @property
    def height(self) -> float:
        _, y0, _, y1 = self.bbox
        return y1 - y0


@dataclass
class AnnotatedPage:
    """The strokes on one PDF page."""

    pdf_page: int                 # 0-based index into the source PDF
    page_uuid: str
    strokes: list[Stroke] = field(default_factory=list)

    @property
    def total_points(self) -> int:
        return sum(len(s.points) for s in self.strokes)


@dataclass
class RmDoc:
    doc_uuid: str
    pdf_bytes: bytes
    pages: list[AnnotatedPage] = field(default_factory=list)


def _page_index(content: dict) -> dict[str, int]:
    """page uuid -> 0-based PDF page index, from `.content`.

    TWO SCHEMAS, both live on the same account (found 2026-07-30 when the daily page returned
    zero annotated pages while visibly covered in ink):

      - `formatVersion` 2+ — `cPages.pages[]`, each with `id` and `redir.value`. This is the
        "uuid pagemap" rmapi reports as missing; it is present, just not where that tool looks.
      - `formatVersion` 1 — a flat `pages` list of uuids with a PARALLEL `redirectionPageMap`
        of PDF page indices. Older documents, and anything uploaded by a client that still
        writes v1 (which is what `rmapi put` produces, so every Locus-delivered PDF lands here).

    A page mapped to -1 is an INSERTED page with no PDF behind it; it is left out rather than
    guessed onto page 0.
    """
    out: dict[str, int] = {}
    for page in (content.get("cPages") or {}).get("pages") or []:
        pid = page.get("id")
        redir = (page.get("redir") or {}).get("value")
        if pid is not None and isinstance(redir, int) and redir >= 0:
            out[pid] = redir
    if out:
        return out

    pages = content.get("pages") or []
    redirect = content.get("redirectionPageMap") or []
    for i, pid in enumerate(pages):
        idx = redirect[i] if i < len(redirect) else i
        if isinstance(pid, str) and isinstance(idx, int) and idx >= 0:
            out[pid] = idx
    return out


def _parse_rm(data: bytes) -> list[tuple[list[tuple[float, float]], int | None, int | None]]:
    """Raw strokes from one `.rm` file, still in reMarkable screen coordinates.

    rmscene warns that a newer format version wrote data it cannot read; that warning is about
    trailing blocks, and the scene line items we need parse fine. Any block that does not yield
    points is skipped rather than guessed at.
    """
    from rmscene import read_blocks

    out = []
    for block in read_blocks(io.BytesIO(data)):
        if type(block).__name__ != "SceneLineItemBlock":
            continue
        value = getattr(getattr(block, "item", None), "value", None)
        pts = getattr(value, "points", None)
        if not pts:
            continue
        out.append(
            (
                [(p.x, p.y) for p in pts],
                getattr(value, "tool", None),
                getattr(value, "color", None),
            )
        )
    return out


def to_page_coords(
    points: list[tuple[float, float]], *, page_width: float, page_height: float
) -> list[tuple[float, float]]:
    """Map reMarkable screen coordinates onto PDF page points (see the module docstring)."""
    scale = max(page_width / SCREEN_W, page_height / SCREEN_H)
    return [(page_width / 2 + x * scale, y * scale) for x, y in points]


def read_rmdoc(path: str | Path) -> RmDoc:
    """Parse a `.rmdoc` into its source PDF plus per-page strokes in PDF coordinates."""
    import pymupdf

    path = Path(path)
    with zipfile.ZipFile(path) as z:
        names = z.namelist()
        pdf_name = next((n for n in names if n.endswith(".pdf")), None)
        content_name = next((n for n in names if n.endswith(".content")), None)
        if pdf_name is None or content_name is None:
            raise ValueError(f"{path.name}: not a PDF-backed rmdoc (no .pdf/.content)")

        doc_uuid = Path(pdf_name).stem
        pdf_bytes = z.read(pdf_name)
        pagemap = _page_index(json.loads(z.read(content_name)))

        doc = pymupdf.open(stream=pdf_bytes, filetype="pdf")
        try:
            pages: list[AnnotatedPage] = []
            for name in sorted(n for n in names if n.endswith(".rm")):
                page_uuid = Path(name).stem
                idx = pagemap.get(page_uuid)
                if idx is None or idx >= doc.page_count:
                    continue  # a stroke layer we cannot place is dropped, never guessed onto a page
                rect = doc[idx].rect
                strokes = [
                    Stroke(
                        to_page_coords(pts, page_width=rect.width, page_height=rect.height),
                        tool=tool,
                        color=color,
                    )
                    for pts, tool, color in _parse_rm(z.read(name))
                ]
                if strokes:
                    pages.append(AnnotatedPage(idx, page_uuid, strokes))
        finally:
            doc.close()

    pages.sort(key=lambda p: p.pdf_page)
    return RmDoc(doc_uuid=doc_uuid, pdf_bytes=pdf_bytes, pages=pages)


def ink_hash(rmdoc: RmDoc) -> str:
    """A stable fingerprint of the STROKES in a document.

    Compositing is not byte-reproducible — pymupdf stamps each save — so hashing the rendered
    PDF would report "changed" on every run and re-pay a billed vision pass every time a timer
    fires. The ink is the thing the guard actually means: unchanged handwriting is unchanged
    handwriting however it happens to be drawn.
    """
    import hashlib

    h = hashlib.sha256()
    for page in sorted(rmdoc.pages, key=lambda p: p.pdf_page):
        h.update(f"p{page.pdf_page}:".encode())
        for stroke in page.strokes:
            h.update(b";")
            for x, y in stroke.points:
                h.update(f"{x:.2f},{y:.2f}|".encode())
    return h.hexdigest()


def composite_pdf(rmdoc: RmDoc, out_path: str | Path, *, width: float = 1.4) -> Path:
    """Draw the strokes onto their PDF pages and write the result. Returns `out_path`.

    Loop B does not need this — it asks which words a mark covers, which is geometry. The DAILY
    PAGE does: it asks what the owner WROTE, and reading handwriting needs pixels for the vision
    pass. This is the missing half of the pull-back, and it is why the device-render route was
    ever attempted: the tablet composites ink for notebooks but hands back the ORIGINAL file for
    an uploaded PDF (proved 2026-07-30), and every Locus-delivered page is an uploaded PDF.

    Compositing here instead means the whole path runs off the CLOUD copy, so it works with the
    tablet asleep and needs nothing installed on the device.
    """
    import pymupdf

    doc = pymupdf.open(stream=rmdoc.pdf_bytes, filetype="pdf")
    try:
        for annotated in rmdoc.pages:
            if annotated.pdf_page >= doc.page_count:
                continue
            page = doc[annotated.pdf_page]
            for stroke in annotated.strokes:
                if len(stroke.points) < 2:
                    continue
                # Clipped to the page: ink written beside a portrait page has no page
                # coordinates, and pymupdf refuses to draw outside the rect.
                pts = [pymupdf.Point(x, y) for x, y in stroke.points]
                try:
                    page.draw_polyline(pts, color=(0, 0, 0), width=width)
                except (ValueError, RuntimeError):
                    continue
        out_path = Path(out_path)
        out_path.parent.mkdir(parents=True, exist_ok=True)
        doc.save(str(out_path))
        return out_path
    finally:
        doc.close()


def composite_pages_with_margins(
    rmdoc: RmDoc,
    page_indexes: list[int] | None = None,
    *,
    dpi: int = 130,
    pad: float = 12.0,
    width: float = 1.6,
) -> dict[int, bytes]:
    """Render inked pages to PNG on a canvas ENLARGED to hold the margin ink. 0-based keys.

    WHY THIS EXISTS BESIDE `composite_pdf`. That function draws onto the original page, and
    pymupdf refuses to draw outside the page rect — so every stroke in the margin is silently
    cut at the paper edge. It is invisible in the API (no error, a valid PNG comes back) and
    devastating in practice: on the HH-TTF draft of 2026-09-06, 12 of 27 marks were margin
    notes and most of their words fell outside an A4 page. The reader saw the first few
    characters of each and no indication anything was missing.

    Nothing is lost before the draw. `to_page_coords` centres the tablet canvas on the page, so
    a stroke beside a 595pt-wide page legitimately carries x from about -85 to about 680; those
    coordinates survive parsing, storage and `Stroke.bbox` intact. Only the draw clipped them.

    So: build a NEW page whose rect is the union of the source page and the stroke bounding box,
    stamp the original into it with `show_pdf_page`, and draw the strokes at the same offset. A
    faint grey rectangle marks where the paper actually ended, because margin writing that is
    not visibly outside the page reads as writing ON the page, in the wrong place.

    `composite_pdf` is kept and unchanged: the daily page is written between ruled lines and has
    no margin ink, and it wants one PDF rather than per-page images.
    """
    import pymupdf

    src = pymupdf.open(stream=rmdoc.pdf_bytes, filetype="pdf")
    try:
        out: dict[int, bytes] = {}
        for annotated in rmdoc.pages:
            if annotated.pdf_page >= src.page_count or annotated.pdf_page < 0:
                continue
            if page_indexes is not None and annotated.pdf_page not in page_indexes:
                continue
            points = [pt for stroke in annotated.strokes for pt in stroke.points]
            if not points:
                continue

            rect = src[annotated.pdf_page].rect
            # The union of paper and ink. min(0, ...) / max(width, ...) keep the whole page
            # visible even when every stroke sits inside it.
            x0 = min(0.0, min(p[0] for p in points) - pad)
            y0 = min(0.0, min(p[1] for p in points) - pad)
            x1 = max(rect.width, max(p[0] for p in points) + pad)
            y1 = max(rect.height, max(p[1] for p in points) + pad)

            canvas = pymupdf.open()
            try:
                page = canvas.new_page(width=x1 - x0, height=y1 - y0)
                where = pymupdf.Rect(-x0, -y0, -x0 + rect.width, -y0 + rect.height)
                page.show_pdf_page(where, src, annotated.pdf_page)
                page.draw_rect(where, color=(0.7, 0.7, 0.7), width=0.5)
                for stroke in annotated.strokes:
                    if len(stroke.points) < 2:
                        continue
                    page.draw_polyline(
                        [pymupdf.Point(x - x0, y - y0) for x, y in stroke.points],
                        color=(0, 0, 0), width=width,
                    )
                out[annotated.pdf_page] = page.get_pixmap(dpi=dpi).tobytes("png")
            finally:
                canvas.close()
        return out
    finally:
        src.close()


def fetch_rmdoc(
    device_path: str, dest_dir: str | Path, *, rmapi_binary: str = "rmapi",
    timeout: int = 1800,
) -> Path:
    """`rmapi get` a document into `dest_dir` and return the downloaded `.rmdoc`.

    The cloud copy is the source of truth here, so this works with the tablet powered off —
    unlike every device-side route.
    """
    import subprocess

    dest = Path(dest_dir)
    dest.mkdir(parents=True, exist_ok=True)
    before = set(dest.glob("*.rmdoc"))
    # stdin CLOSED, and the timeout is a caller's decision.
    #
    # Both learned from one stall on 2026-08-01: the hourly reading job sat blocked for half an
    # hour in `do_poll` holding the INGEST LOCK, so nothing else could ingest either. rmapi with
    # an inherited stdin will wait forever on a re-auth prompt that no scheduled job can ever
    # answer, and the 1800s default — sized for pulling a large notebook interactively — turned
    # that into a thirty-minute outage per run rather than a fast, visible failure.
    proc = subprocess.run(
        [rmapi_binary, "get", device_path], cwd=str(dest), capture_output=True, text=True,
        timeout=timeout, stdin=subprocess.DEVNULL,
    )
    if proc.returncode != 0:
        raise RuntimeError(
            f"rmapi get {device_path!r} failed: {proc.stderr.strip() or proc.stdout.strip()}"
        )
    new = sorted(set(dest.glob("*.rmdoc")) - before)
    if new:
        return new[0]
    existing = sorted(dest.glob("*.rmdoc"), key=lambda p: p.stat().st_mtime, reverse=True)
    if not existing:
        raise RuntimeError(f"rmapi get {device_path!r} reported success but wrote no .rmdoc")
    return existing[0]
