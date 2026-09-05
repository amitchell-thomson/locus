"""Read his annotations back — as structured text, and as the marked-up pages themselves.

WHY THIS EXISTS
---------------
Loop B and the hourly sweep already store every mark: which words the ink covered, the whole
line it sat on, the transcribed marginalia, and the intent a classifier assigned. 159 of them
across two documents by 2026-09-05. Nothing could READ that back. `locus status` counts them,
the daily page surfaces the questions among them, and the corpus absorbs the notes — but there
was no way to ask "what did I mark in this paper, and what did I write next to it".

This module is that surface, for the CLI and for the MCP tool of the same name.

TWO REGISTERS, AND THE SECOND ONE IS THE POINT
-----------------------------------------------
The stored marks are text, and text is cheap, exact and instant — it is a DB read. But three
things are lost in it, all of them measured on his live documents:

  * 29 of 109 highlights captured NO text at all. Highlights over a figure cover no words, so
    `covered_text` is empty and the mark reads as content-free (CLAUDE.md §14, open limits).
  * marginalia is deictic. "what does this mean here?" needs the page to mean anything, which
    is why `annotate` stores `line_text` beside `covered_text` — and even that is a line, not
    a diagram, an arrow, or a bracket spanning a paragraph.
  * transcription is a vision pass over handwriting and is not perfect.

So `annotated_page_pngs` composites the ink back onto the PDF and rasterises just the pages
that carry marks, and the MCP tool attaches them as image blocks. The client model then reads
the page as he sees it, ink and all, instead of reading a lossy description of it.

PAGE NUMBERS ARE 0-BASED IN THE DATABASE AND 1-BASED ON A PAGE
---------------------------------------------------------------
`pdf_annotations.pdf_page` is 0-based (`annotate.Mark.pdf_page`, and `composite_pdf` indexes
`doc[...]` with it directly). Every number a human types or reads is 1-based. `MarkRow` carries
BOTH — `page` to print and `page_index` to render with — because a silent off-by-one here shows
the wrong page with total confidence, which is worse than showing none.

WHAT EACH REGISTER IS ACTUALLY BETTER AT (measured 2026-09-05 on his own pages)
-------------------------------------------------------------------------------
Page 54 of *Advanced Portfolio Management* is a figure. The text register has it as a mark
covering `"→σ)"` — true, and nearly meaningless. The IMAGE shows an arrow drawn from "what do
they mean by 'estimator'" to the *Idio Vol Estimator* box, and his own `r = Bf + ε` written
above the diagram. Text could not have carried that, and this is the §14 open limit exactly.

Page 71 shows the opposite. His marginalia runs off the right edge of the PDF page, and
`composite_pdf` clips to the page rect because ink written beside a portrait page HAS no page
coordinates. The rendered image shows the note truncated; the stored `note` has it in full,
because `capture/mark_text.py` transcribes the STROKES rather than the composited page.

So neither register subsumes the other: images win for figures, diagrams, arrows and anything
positional; text wins for margin writing that overflows the page. Both are returned together
and that is deliberate — a caller offered only one of them is being quietly misinformed.

FRESHNESS
---------
The text comes from the last sweep; the images are fetched from the CLOUD COPY at call time, so
they show the ink as it is now. When those disagree, the image is right. Fetching is the only
slow, failure-prone part of this module, and it degrades to text-with-an-explanation rather than
to silence: an images request that quietly returns no images is the failure class this codebase
is built to resist.
"""

from __future__ import annotations

import sqlite3
import tempfile
from dataclasses import dataclass, field
from pathlib import Path

# 140dpi renders his handwriting legibly at a sane payload size. 150 (what `transcribe` uses for
# the vision pass) is the same picture ~15% heavier, and these ride in a chat context where the
# marginal page competes with the conversation for room.
DEFAULT_DPI = 140

# rmapi `get` defaults to a 1800s timeout, sized for pulling a large notebook interactively. A
# tool call answering a person in a chat must fail fast and say so instead: three minutes is
# already long enough to be annoying and far longer than his largest book has ever taken.
FETCH_TIMEOUT = 180


@dataclass
class MarkRow:
    """One stored annotation, in both page conventions (see the module note)."""

    page: int                # 1-based, for printing
    page_index: int          # 0-based, for indexing the PDF
    kind: str
    intent: str | None
    covered_text: str
    line_text: str
    note: str
    in_margin: bool

    @property
    def is_blank(self) -> bool:
        """Covered nothing and says nothing — almost always a highlight over a figure.

        These are not noise to be filtered: they are exactly the marks whose meaning is only
        visible in the image, so they are reported and counted rather than dropped.
        """
        return not self.covered_text.strip() and not self.note.strip()


@dataclass
class DocumentMarks:
    source_uri: str
    title: str
    doc_uuid: str
    device_path: str | None
    marks: list[MarkRow] = field(default_factory=list)
    swept_at: str = ""

    @property
    def page_indexes(self) -> list[int]:
        """0-based pages carrying at least one mark, in reading order."""
        return sorted({m.page_index for m in self.marks})

    @property
    def blank_count(self) -> int:
        return sum(m.is_blank for m in self.marks)

    def render(self) -> str:
        """The text register: every mark, grouped by page, in reading order."""
        if not self.marks:
            return f'No stored annotations for "{self.title}".'

        head = (
            f'"{self.title}" — {len(self.marks)} mark(s) on {len(self.page_indexes)} page(s)'
            + (f", last swept {self.swept_at[:10]}" if self.swept_at else "")
        )
        lines = [head, ""]
        for idx in self.page_indexes:
            for m in [x for x in self.marks if x.page_index == idx]:
                bits = [m.kind]
                if m.intent:
                    bits.append(m.intent)
                if m.in_margin:
                    bits.append("margin")
                lines.append(f"p.{m.page}  {' · '.join(bits)}")
                if m.covered_text.strip():
                    lines.append(f'      marked: "{_clip(m.covered_text)}"')
                if m.line_text.strip() and m.line_text.strip() != m.covered_text.strip():
                    lines.append(f'      line:   "{_clip(m.line_text)}"')
                if m.note.strip():
                    lines.append(f'      wrote:  "{_clip(m.note)}"')
                if m.is_blank:
                    lines.append("      (ink covering no text — usually a figure; ask for the "
                                 "page image to see what it marks)")
                lines.append("")
        if self.blank_count:
            lines.append(
                f"{self.blank_count} of {len(self.marks)} mark(s) covered no text. Their meaning "
                "is only in the ink — request the page images to read them."
            )
        return "\n".join(lines).rstrip() + "\n"


def _clip(text: str, limit: int = 240) -> str:
    text = " ".join(text.split())
    return text if len(text) <= limit else text[: limit - 1] + "…"


def documents_with_marks(conn: sqlite3.Connection) -> list[tuple[str, str, int]]:
    """`(source_uri, title, mark_count)` for everything he has annotated, most-marked first.

    Title comes from `documents` when the marks are corpus-mapped and from `reading_targets`
    when they are not — a document still sitting in `Reading/Proposed` keys its marks by device
    path and has no corpus row at all (CLAUDE.md §14), and those are precisely the ones he is
    reading right now.
    """
    rows = conn.execute(
        """
        SELECT a.source_uri AS uri,
               COALESCE(d.title, t.title, a.source_uri) AS title,
               COUNT(*) AS n
        FROM pdf_annotations a
        LEFT JOIN documents d ON d.source_uri = a.source_uri
        LEFT JOIN reading_targets t ON t.source_uri = a.source_uri
        GROUP BY a.source_uri
        ORDER BY n DESC
        """
    ).fetchall()
    return [(r["uri"], r["title"], r["n"]) for r in rows]


def resolve(conn: sqlite3.Connection, query: str) -> list[tuple[str, str, int]]:
    """Candidate documents for a human string. Exact `source_uri` wins outright.

    Returns every match rather than guessing, so an ambiguous ask is answered with a choice
    instead of with the wrong document's annotations.
    """
    known = documents_with_marks(conn)
    q = query.strip().casefold()
    if not q:
        return known
    exact = [k for k in known if k[0].casefold() == q]
    if exact:
        return exact
    return [k for k in known if q in k[1].casefold() or q in k[0].casefold()]


def load(
    conn: sqlite3.Connection,
    source_uri: str,
    *,
    page: int | None = None,
    intent: str | None = None,
) -> DocumentMarks:
    """Stored marks for one document. `page` is 1-BASED, as a person reads it."""
    meta = conn.execute(
        """
        SELECT COALESCE(d.title, t.title, ?) AS title,
               COALESCE(t.device_path, '')   AS device_path,
               COALESCE(t.last_swept, '')    AS swept
        FROM (SELECT 1) x
        LEFT JOIN documents d ON d.source_uri = ?
        LEFT JOIN reading_targets t ON t.source_uri = ?
        """,
        (source_uri, source_uri, source_uri),
    ).fetchone()

    sql = [
        "SELECT pdf_page, kind, intent, COALESCE(covered_text,'') ct, COALESCE(line_text,'') lt,",
        "       COALESCE(note,'') note, in_margin, doc_uuid",
        "FROM pdf_annotations WHERE source_uri = ?",
    ]
    args: list = [source_uri]
    if page is not None:
        sql.append("AND pdf_page = ?")
        args.append(page - 1)            # the 1-based -> 0-based hop; see the module note
    if intent:
        sql.append("AND intent = ?")
        args.append(intent)
    sql.append("ORDER BY pdf_page, id")

    rows = conn.execute(" ".join(sql), args).fetchall()
    marks = [
        MarkRow(
            page=r["pdf_page"] + 1,
            page_index=r["pdf_page"],
            kind=r["kind"],
            intent=r["intent"],
            covered_text=r["ct"],
            line_text=r["lt"],
            note=r["note"],
            in_margin=bool(r["in_margin"]),
        )
        for r in rows
    ]
    return DocumentMarks(
        source_uri=source_uri,
        title=(meta["title"] if meta else source_uri) or source_uri,
        doc_uuid=(rows[0]["doc_uuid"] if rows else "") or "",
        device_path=(meta["device_path"] if meta else "") or None,
        marks=marks,
        swept_at=(meta["swept"] if meta else "") or "",
    )


def _render_pages(pdf_path: Path, page_indexes: list[int], *, dpi: int) -> dict[int, bytes]:
    """Rasterise ONLY the named 0-based pages.

    `transcribe.render_pdf_pages` does the whole document, which is right for a 4-page daily
    page and absurd for a 300-page book of which he marked nine pages.
    """
    import pymupdf

    doc = pymupdf.open(str(pdf_path))
    try:
        return {
            i: doc[i].get_pixmap(dpi=dpi).tobytes("png")
            for i in page_indexes
            if 0 <= i < doc.page_count
        }
    finally:
        doc.close()


def annotated_page_pngs(
    device_path: str,
    page_indexes: list[int],
    *,
    dpi: int = DEFAULT_DPI,
    rmapi_binary: str = "rmapi",
    timeout: int = FETCH_TIMEOUT,
    fetch=None,
    read=None,
) -> dict[int, bytes]:
    """The named pages of the device copy, with his ink drawn on, as PNG bytes.

    Pulls the CLOUD copy, so it works with the tablet asleep and shows the ink as it stands now
    rather than as the last sweep saw it. `fetch`/`read` are injection points for tests — the
    real ones shell out to rmapi and parse a bundle, neither of which belongs in a unit test.

    Raises on failure. The caller decides how to degrade, because the right degrade differs:
    the CLI prints the error, the MCP tool returns the text register plus an explanation.
    """
    from locus.capture.rmdoc import composite_pdf, fetch_rmdoc, read_rmdoc

    if not page_indexes:
        return {}
    fetch = fetch or (
        lambda path, dest: fetch_rmdoc(path, dest, rmapi_binary=rmapi_binary, timeout=timeout)
    )
    read = read or read_rmdoc

    with tempfile.TemporaryDirectory() as tmp:
        rmdoc = read(fetch(device_path, tmp))
        marked = composite_pdf(rmdoc, Path(tmp) / "annotated.pdf")
        return _render_pages(marked, page_indexes, dpi=dpi)


def locate_device_copy(
    conn: sqlite3.Connection,
    doc: DocumentMarks,
    *,
    rmapi_binary: str = "rmapi",
    index_fn=None,
    runner=None,
) -> tuple[str, bool]:
    """Where this document actually lives on the device now. Returns `(path, repaired)`.

    `reading_targets.device_path` is a CACHED POINTER and it goes stale. Proved live on
    2026-09-05: the book he has been reading all month was still recorded at
    `/reading_list/Advanced Portfolio Management`, a path the 2026-08 device reorganisation
    deleted, so `rmapi get` answered "file doesn't exist". Marks kept arriving only because
    `capture/loop_b` never trusts that column — it rebuilds a uuid -> path index from a live
    listing every run, which is exactly the redundancy CLAUDE.md §9 calls deliberate.

    So: try the cached path, and on failure resolve by `doc_uuid` against a live index. The
    corrected path is written back, because it is derived data whose whole purpose is to save
    the slow lookup (invariant 3) — and because `reading/sweep.py` reads the same column and
    fails the same way until something fixes it.

    Raises when the document cannot be found at all, naming what it tried: a document he
    deleted from the tablet is a real answer, not an error to paper over.
    """
    from locus.capture.remarkable import _subprocess_runner

    if doc.device_path and _device_path_works(doc.device_path, rmapi_binary, runner):
        return doc.device_path, False

    if not doc.doc_uuid:
        raise RuntimeError(
            f'cannot locate "{doc.title}" on the device: the cached path '
            f"{doc.device_path or '(none)'} does not resolve and no doc_uuid was stored with "
            "its marks, so there is nothing left to look it up by."
        )

    index_fn = index_fn or (lambda: _live_index(runner or _subprocess_runner(rmapi_binary)))
    index = index_fn()
    hit = index.get(doc.doc_uuid)
    if hit is None:
        raise RuntimeError(
            f'"{doc.title}" is not on the device under any reading folder (uuid '
            f"{doc.doc_uuid[:8]}…). The cached path was {doc.device_path or '(none)'}. "
            "It may have been deleted or moved outside Reading/."
        )

    found = hit[0]
    with conn:
        conn.execute(
            "UPDATE reading_targets SET device_path=? WHERE source_uri=?", (found, doc.source_uri)
        )
    return found, True


def _device_path_works(device_path: str, rmapi_binary: str, runner=None) -> bool:
    """One cheap `rmapi stat` — far cheaper than a failed `get` of a whole book bundle."""
    from locus.capture.remarkable import _stat, _subprocess_runner

    try:
        return bool(_stat(runner or _subprocess_runner(rmapi_binary), device_path))
    except Exception:
        return False


def _live_index(runner) -> dict:
    from locus.capture.loop_b import reading_index

    return reading_index(runner)
