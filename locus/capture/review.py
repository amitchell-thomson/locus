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
from dataclasses import dataclass, field, replace
from pathlib import Path

# 140dpi renders his handwriting legibly at a sane payload size. 150 (what `transcribe` uses for
# the vision pass) is the same picture ~15% heavier, and these ride in a chat context where the
# marginal page competes with the conversation for room.
DEFAULT_DPI = 140

# Total PNG bytes one call may return. Twelve inked pages of a marked-up A4 draft came to
# 3.5MB at 130dpi (measured on the HH-TTF draft), which is ~4.7MB once base64-encoded into a
# tool result — enough to risk a hard transport failure rather than a slow one. So pages are
# kept in ink-density order until the budget is spent and the rest are REPORTED as omitted.
# A caller that wants a specific page asks for it by number and always gets it.
DEFAULT_MAX_PNG_BYTES = 6 * 1024 * 1024

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

    def render(self, *, image_hint: bool = False) -> str:
        """The text register: every mark, grouped by page, in reading order.

        `image_hint` adds "ask for the page image" to a mark that covered nothing. It is OFF by
        default because `markups` returns the images alongside, and telling a reader to ask for
        what it has already been given is noise. Text-only callers pass True.
        """
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
                    lines.append(
                        "      (ink covering no text — usually a figure or a bracket"
                        + ("; ask for the page image to see what it marks)" if image_hint else ")")
                    )
                lines.append("")
        if self.blank_count and image_hint:
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
    return [(r["uri"], _readable_title(r["uri"], r["title"]), r["n"]) for r in rows]


def _readable_title(uri: str, title: str) -> str:
    """A name a person would recognise.

    A document with no corpus row and no reading_target falls back to its `source_uri`, which
    for an un-ingested draft is the full device path — so the register headed itself
    `"/Inbox/2026-09-05 HH-TTF spread null (first draft)"`. The basename is what he called it.
    """
    if title and title != uri:
        return title
    return Path(uri).name or uri


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
        title=_readable_title(source_uri, (meta["title"] if meta else "") or ""),
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
    margins: bool = True,
    fetch=None,
    read=None,
) -> dict[int, bytes]:
    """The named pages of the device copy, with his ink drawn on, as PNG bytes.

    `margins=True` (the default) renders on a canvas enlarged to hold ink that overhangs the
    paper. It is the default because the alternative silently truncates his marginalia, and
    because ONE renderer is the point: when the CLI and the MCP tool used different ones, the
    CLI looked right and was wrong.

    Pulls the CLOUD copy, so it works with the tablet asleep and shows the ink as it stands now
    rather than as the last sweep saw it. `fetch`/`read` are injection points for tests — the
    real ones shell out to rmapi and parse a bundle, neither of which belongs in a unit test.

    Raises on failure. The caller decides how to degrade, because the right degrade differs:
    the CLI prints the error, the MCP tool returns the text register plus an explanation.
    """
    from locus.capture.rmdoc import (
        composite_pages_with_margins,
        composite_pdf,
        fetch_rmdoc,
        read_rmdoc,
    )

    if not page_indexes:
        return {}
    fetch = fetch or (
        lambda path, dest: fetch_rmdoc(path, dest, rmapi_binary=rmapi_binary, timeout=timeout)
    )
    read = read or read_rmdoc

    with tempfile.TemporaryDirectory() as tmp:
        rmdoc = read(fetch(device_path, tmp))
        if margins:
            return composite_pages_with_margins(rmdoc, page_indexes, dpi=dpi)
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


# ---------------------------------------------------------------------------------------------
# One-call markup reading: resolve anywhere on the device, sweep if needed, render with margins.
# ---------------------------------------------------------------------------------------------

# A xochitl document id, as `rmapi stat` reports it.
_UUID_RE = __import__("re").compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")


@dataclass
class Target:
    """A document we can fetch and render, however it was named.

    `source_uri` is the key its marks are stored under and is None until something has swept it.
    Everything else is what the device knows.
    """

    device_path: str
    title: str
    doc_uuid: str = ""
    source_uri: str | None = None


def device_file_paths(runner) -> list[str]:
    """Every document path on the device. ONE `rmapi find /` — 0.37s over 79 files, measured."""
    from locus.capture.remarkable import _find_file_paths

    return _find_file_paths(runner)


def find_on_device(runner, query: str) -> list[str]:
    """Device paths whose basename matches `query`, WITHOUT stat-ing anything.

    The stat is the expensive call (0.14s each; 79 files is ~11s of the ~30s budget), and the
    name is already in the listing. So names are matched from the cheap listing and only the
    survivors are stat-ed for their uuid.
    """
    q = query.strip().casefold()
    if not q:
        return []
    hits = [p for p in device_file_paths(runner) if q in Path(p).name.casefold()]
    exact = [p for p in hits if Path(p).name.casefold() == q]
    return exact or hits


def device_index(runner, paths: list[str] | None = None) -> dict[str, tuple[str, str]]:
    """uuid -> (path, name) over the WHOLE device, not just the reading folders.

    `loop_b.reading_index` deliberately restricts to `Reading/`, which is right for Loop B and
    wrong here: `to_remarkable` writes to `[reading].send_folder` (/Inbox), so the folder this
    system puts documents in was the one place the renderer could not see them. Proved on the
    HH-TTF draft, 2026-09-06 — `rmapi` fetched it by path perfectly while `locate_device_copy`
    reported "not on the device under any reading folder".
    """
    from locus.capture.remarkable import _stat

    index: dict[str, tuple[str, str]] = {}
    for device_path in paths if paths is not None else device_file_paths(runner):
        meta = _stat(runner, device_path)
        if not meta or not meta.get("ID"):
            continue
        index[meta["ID"]] = (device_path, meta.get("Name") or Path(device_path).name)
    return index


def _marks_source_uri_for_uuid(conn: sqlite3.Connection, doc_uuid: str) -> str | None:
    """The source_uri this document's marks are already stored under, found BY UUID.

    Marks are keyed by `source_uri`, and for a document with no corpus row that is its device
    path — which changes the moment he moves it between folders. Looking up by the stable uuid
    is what keeps a swept document findable after a move (and stops a second sweep filing a
    duplicate set under the new path).
    """
    if not doc_uuid:
        return None
    row = conn.execute(
        "SELECT source_uri FROM pdf_annotations WHERE doc_uuid=? ORDER BY id LIMIT 1", (doc_uuid,)
    ).fetchone()
    return row["source_uri"] if row else None


def resolve_target(
    conn: sqlite3.Connection,
    document: str,
    *,
    rmapi_binary: str = "rmapi",
    runner=None,
) -> list[Target]:
    """Everything `document` could mean, cheapest lookup first.

    Order (the brief's, and it is an ordering by COST as much as by confidence):
      1. already-swept documents, by title or source_uri — a pure DB read;
      2. `reading_targets`, whose cached `device_path` may be stale but whose uuid is not;
      3. the whole device, by filename from one `find /`, stat-ing only the survivors.

    Returns every candidate rather than guessing. There are currently two documents on his
    device matching "HH-TTF" (a first and a second draft), so guessing would silently answer
    about the wrong one.
    """
    from locus.capture.remarkable import _subprocess_runner

    q = document.strip()
    if not q:
        return []

    # (1) already swept — the common case once a document has been read once.
    stored = resolve(conn, q)
    if stored:
        out = []
        for uri, title, _ in stored:
            doc = load(conn, uri)
            out.append(Target(
                device_path=doc.device_path or "", title=title,
                doc_uuid=doc.doc_uuid, source_uri=uri,
            ))
        return out

    runner = runner or _subprocess_runner(rmapi_binary)

    # (2) a uuid, or a reading_target we know by name but have never swept.
    if _UUID_RE.match(q.casefold()):
        hit = device_index(runner).get(q.casefold())
        if hit:
            return [Target(device_path=hit[0], title=hit[1], doc_uuid=q.casefold(),
                           source_uri=_marks_source_uri_for_uuid(conn, q.casefold()))]
        return []

    # (3) whole device by filename. An explicit path short-circuits the name match.
    paths = [q] if q.startswith("/") else find_on_device(runner, q)
    index = device_index(runner, paths=paths)
    return [
        Target(device_path=path, title=name, doc_uuid=uuid,
               source_uri=_marks_source_uri_for_uuid(conn, uuid))
        for uuid, (path, name) in sorted(index.items(), key=lambda kv: kv[1][0])
    ]


def _cache_dir(cfg) -> Path:
    return Path(cfg.paths.db).parent / "cache" / "rmdoc"


def load_rmdoc(target: Target, *, cfg, refresh: bool = False, fetch=None, read=None):
    """The parsed `.rmdoc`, from a vault cache when the ink has not changed.

    `locus annotate` left every download in a `locus-rmdoc-*` temp directory, so a second look
    at the same document refetched the whole bundle. Cached by `doc_uuid` here, which makes a
    repeat call in the same session nearly free and lets the renderer work with the cloud
    unreachable. `refresh=True` always refetches — it is what the caller passes when the point
    of the call is to pick up new ink.

    The cache is derived data: deleting `vault/cache/` costs one refetch and nothing else.
    """
    from locus.capture.rmdoc import fetch_rmdoc, read_rmdoc

    read = read or read_rmdoc
    # NO uuid, NO cache read. Keying on a placeholder would make every uuid-less document share
    # one cache entry and hand back somebody else's bundle — silently, and with a perfectly
    # valid render. The cache is an optimisation; identity is not negotiable.
    cached = _cache_dir(cfg) / f"{target.doc_uuid}.rmdoc" if target.doc_uuid else None
    if cached is not None and cached.exists() and not refresh:
        try:
            return read(cached), False
        except Exception:
            cached.unlink(missing_ok=True)      # a corrupt cache is not a reason to fail

    fetch = fetch or (
        lambda path, dest: fetch_rmdoc(
            path, dest, rmapi_binary=cfg.capture.rmapi_binary, timeout=FETCH_TIMEOUT
        )
    )
    with tempfile.TemporaryDirectory() as tmp:
        got = Path(fetch(target.device_path, tmp))
        doc = read(got)
        if target.doc_uuid or doc.doc_uuid:
            cached = _cache_dir(cfg) / f"{target.doc_uuid or doc.doc_uuid}.rmdoc"
            cached.parent.mkdir(parents=True, exist_ok=True)
            cached.write_bytes(got.read_bytes())
    return doc, True


def sweep(conn: sqlite3.Connection, target: Target, rmdoc) -> tuple[str, int]:
    """Run the FREE geometric pass and store the marks. Returns `(source_uri, count)`.

    Never transcribes: `capture/transcribe` is a billed vision call and this runs on a chat
    tool call, so a document with handwriting yields marks with `covered_text` and no `note`
    until `locus annotate --transcribe` or Loop B gets to it.

    The key is the corpus `source_uri` when these exact bytes are already ingested (the same
    content-hash identity `loop_b` uses), and the device path otherwise — the existing
    convention, kept deliberately so Loop B and this cannot file two sets of marks for one
    document under two different keys. `doc_uuid` goes on every row, which is what
    `_marks_source_uri_for_uuid` later uses to find them again after a folder move.
    """
    from locus.capture.annotate import marks_for_document, store_marks
    from locus.capture.loop_b import _corpus_uri_for

    uri = target.source_uri or _corpus_uri_for(conn, rmdoc.pdf_bytes) or target.device_path
    marks = marks_for_document(rmdoc)
    store_marks(conn, marks, source_uri=uri, doc_uuid=rmdoc.doc_uuid or target.doc_uuid)
    return uri, len(marks)


def pages_by_ink(rmdoc, cap: int | None = None) -> list[int]:
    """Inked pages, densest first, then back into reading order.

    Ink density (`total_points`) is the ranking because when a cap bites it should keep the
    pages he worked hardest on, not the ones that happen to come first. The RESULT is sorted by
    page so what comes back reads in document order.
    """
    ranked = sorted(rmdoc.pages, key=lambda p: (-p.total_points, p.pdf_page))
    kept = ranked[:cap] if cap else ranked
    return sorted(p.pdf_page for p in kept)


def locate(
    conn: sqlite3.Connection,
    target: Target,
    *,
    rmapi_binary: str = "rmapi",
    runner=None,
    index_fn=None,
) -> Target:
    """Fill in `target.device_path`, whatever it takes, and repair the cache when it was wrong.

    Four sources, cheapest first: the cached `reading_targets` path, the `source_uri` itself
    (for an un-ingested document that IS its device path), the uuid against a whole-device
    index, and finally the title. Only the first two are free; the rest cost a listing.

    This supersedes `locate_device_copy`'s Reading-only lookup for every caller that can be
    anywhere on the device. `reading_targets.device_path` is still written back when it exists
    and was wrong, because `reading/sweep.py` reads that column and stays broken until it is.
    """
    from locus.capture.remarkable import _subprocess_runner

    runner = runner or _subprocess_runner(rmapi_binary)

    for candidate in (target.device_path, target.source_uri):
        if candidate and candidate.startswith("/") and _device_path_works(candidate, rmapi_binary, runner):
            return replace(target, device_path=candidate)

    index = (index_fn or (lambda: device_index(runner)))()
    hit = index.get(target.doc_uuid) if target.doc_uuid else None
    if hit is None and target.title:
        by_name = [(u, v) for u, v in index.items() if v[1].casefold() == target.title.casefold()]
        hit = by_name[0][1] if len(by_name) == 1 else None
        if hit and not target.doc_uuid:
            target = replace(target, doc_uuid=by_name[0][0])

    if hit is None:
        raise RuntimeError(
            f'"{target.title or target.source_uri}" is not on the device anywhere (uuid '
            f"{target.doc_uuid[:8] or 'unknown'}…, cached path {target.device_path or '(none)'}). "
            "It may have been deleted."
        )

    found = hit[0]
    if target.source_uri and found != target.device_path:
        with conn:
            conn.execute(
                "UPDATE reading_targets SET device_path=? WHERE source_uri=?",
                (found, target.source_uri),
            )
    return replace(target, device_path=found)


@dataclass
class Markups:
    """Everything one call needs to read a marked-up document."""

    target: Target
    marks: DocumentMarks
    pages: dict[int, bytes] = field(default_factory=dict)   # 0-based page -> PNG
    inked_pages: list[int] = field(default_factory=list)    # every page with ink
    swept: int = 0                                          # marks stored by THIS call
    fetched: bool = False                                   # did it hit the CLOUD
    looked: bool = False                                    # did it read the bundle at all
    filtered: bool = False                                  # was a page/intent filter applied

    @property
    def omitted(self) -> list[int]:
        return [p for p in self.inked_pages if p not in self.pages]


def markups(
    conn: sqlite3.Connection,
    target: Target,
    *,
    cfg,
    pages: list[int] | None = None,
    intent: str | None = None,
    refresh: bool = False,
    images: bool = True,
    margins: bool = True,
    max_images: int = 12,
    dpi: int = 130,
    max_bytes: int = DEFAULT_MAX_PNG_BYTES,
    fetch=None,
    read=None,
) -> Markups:
    """Resolve, sweep if needed, render — the whole read-my-markups path in one call.

    The bundle is fetched at most ONCE and is reused for both the sweep and the render, which
    is what makes this affordable: the old route paid a fetch for `locus annotate` and another
    for `locus marks --images`.

    `pages` is 1-BASED, as a person reads it. Everything internal is 0-based (see the module
    note on the two conventions). `pages` narrows BOTH registers — asking for page 3 and getting
    every page's text back would not be answering the question. `intent` narrows the text and
    then decides which pages to render, so "show me what I did not understand" returns those
    pages rather than the most heavily inked ones.
    """
    from locus.capture.rmdoc import composite_pages_with_margins, composite_pdf

    need_sweep = refresh or target.source_uri is None or not load(conn, target.source_uri).marks
    doc = None
    fetched = False
    swept = 0

    if need_sweep or images:
        target = locate(conn, target, rmapi_binary=cfg.capture.rmapi_binary)
        doc, fetched = load_rmdoc(target, cfg=cfg, refresh=refresh, fetch=fetch, read=read)

    if need_sweep and doc is not None:
        uri, swept = sweep(conn, target, doc)
        target = replace(target, source_uri=uri)

    marks = load(conn, target.source_uri, intent=intent) if target.source_uri else DocumentMarks(
        source_uri="", title=target.title, doc_uuid=target.doc_uuid, device_path=target.device_path
    )
    if pages:
        keep = {p - 1 for p in pages}
        marks = replace(marks, marks=[m for m in marks.marks if m.page_index in keep])
    if target.title and marks.title == marks.source_uri:
        # `load` falls back to the source_uri for a document with no corpus row, which for an
        # un-ingested draft is its full device path. The device's own Name reads better and is
        # what he called it.
        marks = replace(marks, title=target.title)
    inked = pages_by_ink(doc) if doc is not None else marks.page_indexes

    rendered: dict[int, bytes] = {}
    if images and doc is not None:
        if pages:
            wanted = [p - 1 for p in pages]
        elif intent:
            # An intent filter is a question about particular marks, so show the pages those
            # marks are ON rather than the busiest pages in the document.
            wanted = marks.page_indexes[:max_images]
        else:
            wanted = pages_by_ink(doc, cap=max_images)
        if margins:
            rendered = composite_pages_with_margins(doc, wanted, dpi=dpi)
        else:
            with tempfile.TemporaryDirectory() as tmp:
                flat = composite_pdf(doc, Path(tmp) / "flat.pdf")
                rendered = _render_pages(flat, wanted, dpi=dpi)
        rendered = _within_budget(rendered, doc, max_bytes=max_bytes, explicit=bool(pages))

    return Markups(
        target=target, marks=marks, pages=rendered,
        inked_pages=inked, swept=swept, fetched=fetched,
        looked=doc is not None, filtered=bool(pages or intent),
    )


def _within_budget(
    rendered: dict[int, bytes], rmdoc, *, max_bytes: int, explicit: bool
) -> dict[int, bytes]:
    """Trim to `max_bytes`, dropping the least-inked pages first.

    Never applied when the caller named the pages: an explicit request for page 9 that silently
    returns page 3 instead is worse than a large reply. Whatever is dropped reappears in
    `Markups.omitted`, which the callers print — an image budget that quietly eats pages is the
    same silent-truncation failure this module exists to undo.
    """
    if explicit or sum(len(v) for v in rendered.values()) <= max_bytes:
        return rendered
    density = {p.pdf_page: p.total_points for p in rmdoc.pages}
    kept: dict[int, bytes] = {}
    spent = 0
    for idx in sorted(rendered, key=lambda i: -density.get(i, 0)):
        if spent + len(rendered[idx]) > max_bytes:
            continue
        kept[idx] = rendered[idx]
        spent += len(rendered[idx])
    return kept
