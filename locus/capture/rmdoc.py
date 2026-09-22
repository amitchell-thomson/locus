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

PAGES HE ADDED ON THE TABLET ARE PART OF THE DOCUMENT (2026-09-14). The tablet lets him insert
blank pages into a PDF, and it records them in `.content` with no `redir` at all (or `-1`). They
carry `.rm` files like any other page. This module used to drop every stroke layer it could not
place on a PDF page, so a two-page question sheet with four appended answer pages parsed as
ZERO annotated pages: 3,334 strokes and 80,358 points of his handwriting, reported to him as an
unmarked document. Silence, not an error — the failure class of CLAUDE.md §3 exactly. An
inserted page now parses with `source_page=None` and renders on a blank canvas.

`AnnotatedPage.pdf_page` is therefore the page's position in the DOCUMENT as the tablet
paginates it, and `source_page` is the PDF page behind it. For every document without inserted
pages the two are identical, which is why no stored mark changed meaning when this landed (the
three annotated documents in the corpus on 2026-09-14 were checked). Position is the number he
reads off the tablet, so it is the number to key a mark by and the number to print.

COORDINATE MAPPING (established empirically 2026-07-30 by overlaying strokes on the page and
looking at the result; a width-fit assumption put an underline a full line too high):

    scale  = max(page_width / SCREEN_W, page_height / SCREEN_H)
    pdf_x  = page_width / 2 + rm_x * scale        # rm_x is 0 at the PAGE's centre
    pdf_y  = rm_y * scale                         # rm_y is 0 at the top

The page is fit to the screen, so the constraining dimension sets the scale. Strokes may fall
OUTSIDE the page rectangle and that is not an error: the screen is wider than a portrait page,
so marginalia written beside the page has no page coordinates. Those strokes are kept and
flagged rather than clipped — a margin note is often the most valuable annotation on the page.

A NATIVE NOTEBOOK IS A DOCUMENT WITH NO PDF AT ALL (2026-09-22). A notebook he opened on the
tablet and wrote in from scratch ("Jump call", in `Rough notes/`) ships as a bundle with a
`.content` manifest and `.rm` layers and no `.pdf`. `read_rmdoc` used to raise
"not a PDF-backed rmdoc" on it, so the one kind of document that is ALL his handwriting was the
one kind the reader could not open, and a session asked to read his call notes had nothing to
read. It is the inserted-page case with every page inserted: each page parses with
`source_page=None`, renders on the blank tablet canvas, and `pdf_bytes` is empty. `is_notebook`
says so, and `open_source` is the one way to open the source document, so no caller hands
empty bytes to pymupdf, which rejects them.
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

# A page he INSERTED has no PDF behind it and so has no page rectangle, but the coordinate
# transform needs one. The tablet's own canvas is the honest choice: the same aspect ratio as
# the screen, sized to A4's width, so `to_page_coords` maps the whole writing surface onto the
# page with no distortion and nothing is pushed into a notional margin that does not exist.
INSERTED_PAGE_WIDTH = 595.0
INSERTED_PAGE_HEIGHT = SCREEN_H * (INSERTED_PAGE_WIDTH / SCREEN_W)

# `AnnotatedPage.source_page` default: "this page IS its PDF page". Distinct from None, which
# means the page was inserted on the tablet and has no PDF page at all. A sentinel rather than a
# plain default because a dataclass cannot otherwise tell "not given" from "given as None", and
# the two mean opposite things here.
SAME_AS_PAGE = -1


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
    """The strokes on one page of the document, as the tablet paginates it."""

    pdf_page: int                 # 0-based position in the DOCUMENT (see the module docstring)
    page_uuid: str
    strokes: list[Stroke] = field(default_factory=list)
    # 0-based index into the source PDF, or None for a page he INSERTED on the tablet. The
    # sentinel default means "the same page", which is what every page of an un-inserted-into
    # document is and what a directly-constructed page has always meant.
    source_page: int | None = SAME_AS_PAGE

    def __post_init__(self) -> None:
        if self.source_page == SAME_AS_PAGE:
            self.source_page = self.pdf_page

    @property
    def inserted(self) -> bool:
        """Was this page added on the tablet, with no PDF page behind it?"""
        return self.source_page is None

    @property
    def total_points(self) -> int:
        return sum(len(s.points) for s in self.strokes)


@dataclass
class RmDoc:
    doc_uuid: str
    pdf_bytes: bytes
    pages: list[AnnotatedPage] = field(default_factory=list)
    # Document position -> source PDF page (None = inserted). EVERY page, not just the inked
    # ones: a renderer asked for the whole document needs to know what is behind a page it has
    # no strokes for, and an empty list means the manifest was unreadable rather than that the
    # document is empty.
    page_map: list[int | None] = field(default_factory=list)

    @property
    def is_notebook(self) -> bool:
        """A native notebook: written on the tablet from scratch, no PDF underneath."""
        return not self.pdf_bytes

    @property
    def page_count(self) -> int:
        """Pages as the TABLET counts them, inserted ones included."""
        return len(self.page_map)

    def source_page_for(self, position: int) -> int | None:
        """The PDF page behind a document position, or None when there is none."""
        if 0 <= position < len(self.page_map):
            return self.page_map[position]
        return position if position >= 0 else None


def _page_order(content: dict) -> list[tuple[str, int | None]]:
    """`(page uuid, source PDF page or None)` for every page, in the tablet's page order.

    TWO SCHEMAS, both live on the same account (found 2026-07-30 when the daily page returned
    zero annotated pages while visibly covered in ink):

      - `formatVersion` 2+ — `cPages.pages[]`, each with `id` and `redir.value`. This is the
        "uuid pagemap" rmapi reports as missing; it is present, just not where that tool looks.
      - `formatVersion` 1 — a flat `pages` list of uuids with a PARALLEL `redirectionPageMap`
        of PDF page indices. Older documents, and anything uploaded by a client that still
        writes v1 (which is what `rmapi put` produces, so every Locus-delivered PDF lands here).

    A page with no `redir`, or one mapped to -1, is one he INSERTED on the tablet. It is kept,
    at its place in the order, with no PDF page behind it — NOT dropped, and never guessed onto
    page 0. Dropping it is what made a sheet of handwritten answers read as an unmarked
    two-page document (see the module docstring). `redir` absent entirely is the shape his own
    device writes; -1 is the shape the v1 schema uses, and both occur.

    A page carrying a `deleted` marker is left out: he removed it, its `.rm` may still be in
    the bundle, and rendering it would put ink back into a document he had cleared it from.

    ORDER IS THE RETURN VALUE. A dict keyed by uuid cannot express where an inserted page sits,
    and position is the page number he reads off the tablet.
    """
    out: list[tuple[str, int | None]] = []

    pages_v2 = (content.get("cPages") or {}).get("pages") or []
    if pages_v2:
        for page in pages_v2:
            pid = page.get("id")
            if not isinstance(pid, str) or page.get("deleted"):
                continue
            redir = (page.get("redir") or {}).get("value")
            out.append((pid, redir if isinstance(redir, int) and redir >= 0 else None))
        return out

    pages = content.get("pages") or []
    redirect = content.get("redirectionPageMap") or []
    for i, pid in enumerate(pages):
        if not isinstance(pid, str):
            continue
        # No map at all means a straight PDF: page i IS page i. That is the identity fallback,
        # not a guess about an inserted page, which the map states explicitly when it exists.
        idx = redirect[i] if i < len(redirect) else i
        out.append((pid, idx if isinstance(idx, int) and idx >= 0 else None))
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


def open_source(rmdoc: RmDoc):
    """The document behind the ink as a pymupdf doc — an EMPTY one for a native notebook.

    Every page of a notebook has `source_page=None`, so nothing ever indexes into the empty
    document; it exists so the callers keep one code path. The caller closes it.
    """
    import pymupdf

    if rmdoc.is_notebook:
        return pymupdf.open()
    return pymupdf.open(stream=rmdoc.pdf_bytes, filetype="pdf")


def to_page_coords(
    points: list[tuple[float, float]], *, page_width: float, page_height: float
) -> list[tuple[float, float]]:
    """Map reMarkable screen coordinates onto PDF page points (see the module docstring)."""
    scale = max(page_width / SCREEN_W, page_height / SCREEN_H)
    return [(page_width / 2 + x * scale, y * scale) for x, y in points]


def read_rmdoc(path: str | Path) -> RmDoc:
    """Parse a `.rmdoc` into its source PDF plus per-page strokes in PDF coordinates.

    A bundle with a manifest and no PDF is a native notebook (see the module docstring): every
    page is a blank tablet page and `pdf_bytes` is empty. Only a bundle with no manifest at all
    is unreadable, because without it there is no page order to put the ink in.
    """
    import pymupdf

    path = Path(path)
    with zipfile.ZipFile(path) as z:
        names = z.namelist()
        pdf_name = next((n for n in names if n.endswith(".pdf")), None)
        content_name = next((n for n in names if n.endswith(".content")), None)
        if content_name is None:
            raise ValueError(f"{path.name}: not a reMarkable document (no .content manifest)")

        doc_uuid = Path(pdf_name or content_name).stem
        pdf_bytes = z.read(pdf_name) if pdf_name else b""
        order = _page_order(json.loads(z.read(content_name)))

        doc = (
            pymupdf.open(stream=pdf_bytes, filetype="pdf") if pdf_bytes else pymupdf.open()
        )
        try:
            # position -> source page, and uuid -> position. A source page the PDF does not have
            # is treated as INSERTED rather than dropped: a manifest that disagrees with its own
            # PDF is a reason to render the ink on a blank sheet, never a reason to lose it. For
            # a notebook the PDF has no pages, so this makes every page a blank one.
            page_map: list[int | None] = [
                src if src is not None and 0 <= src < doc.page_count else None
                for _, src in order
            ]
            position_of = {pid: i for i, (pid, _) in enumerate(order)}

            pages: list[AnnotatedPage] = []
            for name in sorted(n for n in names if n.endswith(".rm")):
                page_uuid = Path(name).stem
                position = position_of.get(page_uuid)
                if position is None:
                    continue  # a stroke layer the manifest does not list: a page he deleted
                source = page_map[position]
                rect = (
                    doc[source].rect if source is not None
                    else pymupdf.Rect(0, 0, INSERTED_PAGE_WIDTH, INSERTED_PAGE_HEIGHT)
                )
                strokes = [
                    Stroke(
                        to_page_coords(pts, page_width=rect.width, page_height=rect.height),
                        tool=tool,
                        color=color,
                    )
                    for pts, tool, color in _parse_rm(z.read(name))
                ]
                if strokes:
                    pages.append(AnnotatedPage(position, page_uuid, strokes, source_page=source))
        finally:
            doc.close()

    pages.sort(key=lambda p: p.pdf_page)
    return RmDoc(doc_uuid=doc_uuid, pdf_bytes=pdf_bytes, pages=pages, page_map=page_map)


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

    def _draw(page, strokes) -> None:
        for stroke in strokes:
            if len(stroke.points) < 2:
                continue
            # Clipped to the page: ink written beside a portrait page has no page
            # coordinates, and pymupdf refuses to draw outside the rect.
            pts = [pymupdf.Point(x, y) for x, y in stroke.points]
            try:
                page.draw_polyline(pts, color=(0, 0, 0), width=width)
            except (ValueError, RuntimeError):
                continue

    doc = open_source(rmdoc)
    try:
        # PDF-backed pages FIRST, indexed by their source page, because that index is only
        # valid while nothing has been inserted into `doc`. Ink drawn on the wrong page is the
        # one outcome worse than ink not drawn at all.
        for annotated in rmdoc.pages:
            if annotated.source_page is None or annotated.source_page >= doc.page_count:
                continue
            _draw(doc[annotated.source_page], annotated.strokes)

        # ...then the pages he added on the tablet, which have nothing to draw on until one is
        # made for them. Placed at their document position so the result reads in his order.
        for annotated in sorted(
            (pg for pg in rmdoc.pages if pg.inserted), key=lambda pg: pg.pdf_page
        ):
            _draw(
                doc.new_page(
                    pno=min(annotated.pdf_page, doc.page_count),
                    width=INSERTED_PAGE_WIDTH,
                    height=INSERTED_PAGE_HEIGHT,
                ),
                annotated.strokes,
            )
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
    gray: bool = False,
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

    `gray` renders one channel instead of three. On his own pages that is not a compromise at
    all — black ink, black print, a grey paper outline, no colour anywhere — and it halves the
    payload: six pages of the question sheet measure 1,161KB in colour and 620KB in grey at the
    SAME 130dpi (2026-09-14). It is a real loss only for a source page with a colour figure, so
    the caller turns it on as a budget lever rather than it being the default here.

    WHAT `page_indexes` MEANS. Omitted, it renders the inked pages and only those — the cheap
    default for a 211-page book of which he marked nine. NAMED, it renders exactly those pages
    whether they carry ink or not, because a page he asked for by number and did not get back
    is the silent-omission failure again, and because an answer written on an inserted page is
    unreadable without the question on the page before it. Positions are DOCUMENT positions,
    so an inserted page is addressable like any other; one the document does not have is
    skipped.
    """
    import pymupdf

    src = open_source(rmdoc)
    try:
        inked = {pg.pdf_page: pg for pg in rmdoc.pages}
        explicit = page_indexes is not None
        wanted = (
            [i for i in dict.fromkeys(page_indexes) if i >= 0] if explicit
            else sorted(inked)
        )

        out: dict[int, bytes] = {}
        for position in wanted:
            annotated = inked.get(position)
            source = (
                annotated.source_page if annotated is not None
                else rmdoc.source_page_for(position)
            )
            if source is not None and not 0 <= source < src.page_count:
                continue                    # a position this document does not have
            strokes = annotated.strokes if annotated is not None else []
            points = [pt for stroke in strokes for pt in stroke.points]
            if not points and not explicit:
                continue

            rect = (
                src[source].rect if source is not None
                else pymupdf.Rect(0, 0, INSERTED_PAGE_WIDTH, INSERTED_PAGE_HEIGHT)
            )
            # The union of paper and ink. min(0, ...) / max(width, ...) keep the whole page
            # visible even when every stroke sits inside it.
            x0 = min(0.0, min((p[0] for p in points), default=0.0) - pad)
            y0 = min(0.0, min((p[1] for p in points), default=0.0) - pad)
            x1 = max(rect.width, max((p[0] for p in points), default=0.0) + pad)
            y1 = max(rect.height, max((p[1] for p in points), default=0.0) + pad)

            canvas = pymupdf.open()
            try:
                page = canvas.new_page(width=x1 - x0, height=y1 - y0)
                where = pymupdf.Rect(-x0, -y0, -x0 + rect.width, -y0 + rect.height)
                if source is not None:
                    page.show_pdf_page(where, src, source)
                page.draw_rect(where, color=(0.7, 0.7, 0.7), width=0.5)
                for stroke in strokes:
                    if len(stroke.points) < 2:
                        continue
                    page.draw_polyline(
                        [pymupdf.Point(x - x0, y - y0) for x, y in stroke.points],
                        color=(0, 0, 0), width=width,
                    )
                pixmap_args = {"colorspace": pymupdf.csGRAY} if gray else {}
                out[position] = page.get_pixmap(dpi=dpi, **pixmap_args).tobytes("png")
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
