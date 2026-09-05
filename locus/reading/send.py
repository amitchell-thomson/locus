"""Send arbitrary markdown to the reMarkable, rendered with the device geometry.

WHY THIS EXISTS SEPARATELY FROM `locus read`
--------------------------------------------
`cli.cmd_read` renders markdown *files by path* and pushes them. That is the right shape on the
server, where the file is on disk — and the wrong shape everywhere else. The two clients that
matter for ad-hoc reading (the Claude desktop app, and a Claude Code session on the laptop) have
no access to this filesystem: what they hold is the markdown *text*. So the callable surface here
takes CONTENT, and the MCP tool built on it works identically from any machine. A path-taking
tool would have looked wired from the laptop and failed on every call.

WHERE IT LANDS ON THE DEVICE
----------------------------
NOT `/Daily` (that folder is the daily page's ink inbox — a page sits loose there until it has
handwriting on it, then archives to `/Daily/YYYY-MM`; dropping unrelated reading in it corrupts
that signal) and NOT `Reading/In-Progress` or `Reading/Finished`, because `capture/loop_b` treats
a document he MOVED into either of those with no corpus match as one he chose to read, and
ingests it from the bundle's own bytes. A doc that arrived there because a tool put it there was
never chosen, so it would auto-ingest a rendering of text usually already in the corpus. And
emphatically NOT `[capture].notes_root` (`/Notes` on the live device, which looked like the
obvious home for it): Loop A pulls that tree and ingests what it finds as the owner's OWN
handwriting, so agent prose filed there breaks invariant 5 by filing rather than by code.

The default (`[reading].send_folder`) is therefore its own top-level folder that no loop watches,
from which moving a document into `Reading/In-Progress` stays a deliberate act meaning exactly
what it has always meant.

Filenames are date-prefixed by `deliver.safe_filename`, so two sends of the same title on
different days coexist and a same-day resend replaces content rather than failing outright
(`rmapi put` REFUSES a same-name upload; see `deliver_pdf`). `deliver_pdf(replace=True)` keeps
the device-side per-page records, which is a real hazard for the daily page and not for these:
these carry no ink that anything pulls back, and a resend is a fresh render of the same document.

MARKDOWN TAKES CONTENT; A PDF TAKES A PATH — AND THAT ASYMMETRY IS DELIBERATE
-----------------------------------------------------------------------------
Everything above argues for a content-taking surface, and `send_markdown` has one. `send_pdf`
cannot: a PDF is binary, and the only content-shaped way to pass it through an MCP tool call is
base64 in a tool ARGUMENT — which the client model has to emit token by token. A 2 MB paper is
~2.7 MB of base64; it is not a slow path, it is an impossible one.

So `send_pdf` takes a path, and that path is resolved ON THE LOCUS SERVER. From a Claude Code
session running on this machine (where Claude writes the PDF, or where the repo lives) that is
exactly right. From the desktop app or a laptop session it cannot work at all, and the failure
must be unmistakable rather than a confusing rmapi error — which is why `resolve_pdf` raises
with the paths it actually tried. This is the "looks wired and isn't" class (CLAUDE.md §3)
sitting one argument away, and the error message is the guard.
"""

from __future__ import annotations

import tempfile
from dataclasses import dataclass
from pathlib import Path

from locus.config import load
from locus.reading.deliver import safe_filename
from locus.reading.deliver_remarkable import (
    RmapiRunner,
    _ensure_folder,
    _subprocess_runner,
    deliver_pdf,
)
from locus.reading.md2pdf import PageGeometry, render_markdown_to_pdf


@dataclass
class SentDoc:
    """What was sent, named as the device names it."""

    filename: str
    remote_folder: str
    pages: int

    @property
    def device_path(self) -> str:
        return f"/{self.remote_folder}/{self.filename}"


def reading_geometry(cfg=None) -> PageGeometry:
    """The device-tuned page geometry from `[reading]` (shared by every read-facing render)."""
    reading = (cfg or load()).reading
    return PageGeometry(
        width_in=reading.page_width_in,
        height_in=reading.page_height_in,
        margin_in=reading.margin_in,
        font_pt=reading.font_pt,
    )


def _ensure_folder_path(runner: RmapiRunner, folder: str) -> None:
    """mkdir every prefix of `folder`, parent first.

    `rmapi mkdir` does not create intermediate directories, so a configured nested
    `send_folder` ("Notes/Sent") needs one call per level — the same fix `deliver`'s
    `ensure_reading_folders` makes for the reading root.
    """
    parts = [p for p in folder.split("/") if p]
    for i in range(len(parts)):
        _ensure_folder(runner, "/".join(parts[: i + 1]))


def _page_count(pdf: Path) -> int:
    """Page count for the report line. Best effort: a send that worked is not a failure
    because the count could not be read."""
    try:
        import fitz

        with fitz.open(pdf) as doc:
            return doc.page_count
    except Exception:
        return 0


def send_markdown(
    markdown: str,
    *,
    title: str,
    folder: str | None = None,
    cfg=None,
    runner: RmapiRunner | None = None,
) -> SentDoc:
    """Render `markdown` to a device-tuned PDF and push it to the reMarkable.

    `title` names the document on the device and is prepended as an H1 when the markdown does
    not already open with one (`markdown_to_typst`), so the page is self-identifying on a screen
    that shows no filename.

    Raises rather than degrading: every caller (CLI, MCP tool) reports to a human who can act on
    it, and a silent "sent" for a document that never left the server is exactly the failure
    class this codebase is built to resist.
    """
    if not markdown.strip():
        raise ValueError("nothing to send: the markdown is empty")

    cfg = cfg or load()
    folder = (folder or cfg.reading.send_folder).strip("/")
    if not folder:
        raise ValueError("no target folder: set [reading].send_folder or pass one")

    runner = runner or _subprocess_runner(cfg.reading.rmapi_binary)
    filename = safe_filename(title)

    with tempfile.TemporaryDirectory() as tmp:
        pdf = render_markdown_to_pdf(
            markdown, Path(tmp) / filename, geometry=reading_geometry(cfg), title=title
        )
        pages = _page_count(pdf)
        _ensure_folder_path(runner, folder)
        deliver_pdf(pdf, remote_folder=folder, replace=True, runner=runner)

    return SentDoc(filename=filename, remote_folder=folder, pages=pages)


# A mistyped path is far likelier than a genuinely enormous document, and an rmapi upload that
# runs for minutes and then fails is the worst way to learn which one you have. Generous on
# purpose: this is a typo guard, not a policy on how long a paper may be.
_MAX_PDF_BYTES = 150 * 1024 * 1024

# `%PDF` is at byte 0 in a well-formed file, but readers tolerate leading junk and some
# generators emit it, so scan a small prefix rather than demanding an exact `startswith`.
_PDF_MAGIC_WINDOW = 1024


def _repo_root() -> Path:
    """The checkout root: `locus/reading/send.py` -> three parents up."""
    return Path(__file__).resolve().parents[2]


def resolve_pdf(path: str | Path) -> Path:
    """Find `path` on the SERVER's filesystem and prove it is really a PDF.

    Two lookups, in order: the path as given (absolute, or relative to the server process's
    working directory), then relative to the checkout root — because "a file in the project"
    is the second thing this tool is for and `docs/plan.pdf` is how a person says it.

    Every failure names what was tried. The reason is not politeness: this function is the one
    place that can distinguish "the file is not there" from "you are calling this from a machine
    that does not share a filesystem with the Locus server", and those have completely different
    fixes. An `rmapi` error, or a bare FileNotFoundError, tells the caller neither.

    The magic-byte check exists because `deliver_pdf` will happily upload any bytes under a
    `.pdf` name: a renamed .docx or a saved HTML error page becomes a document on the device
    that opens to nothing, which reads as a device fault rather than a send fault.
    """
    given = Path(path).expanduser()
    tried = [given]
    found = given if given.is_file() else None
    if found is None and not given.is_absolute():
        candidate = _repo_root() / given
        tried.append(candidate)
        found = candidate if candidate.is_file() else None

    if found is None:
        where = "\n  ".join(str(t) for t in tried)
        raise FileNotFoundError(
            f"no such PDF on the Locus server. Tried:\n  {where}\n"
            "This tool resolves paths on the SERVER, so a path from another machine "
            "(the desktop app, a laptop session) cannot resolve here — send markdown text "
            "instead, or copy the file to the server first."
        )

    size = found.stat().st_size
    if size == 0:
        raise ValueError(f"{found} is empty (0 bytes) — nothing to send")
    if size > _MAX_PDF_BYTES:
        raise ValueError(
            f"{found} is {size / 1024 / 1024:.0f} MB, over the {_MAX_PDF_BYTES // 1024 // 1024} MB "
            "send guard. If that is genuinely the document you meant, push it with "
            "`locus read` on the server."
        )

    with found.open("rb") as fh:
        head = fh.read(_PDF_MAGIC_WINDOW)
    if b"%PDF" not in head:
        raise ValueError(
            f"{found} is not a PDF (no %PDF header in the first {_PDF_MAGIC_WINDOW} bytes). "
            "Uploading it would put a document on the device that opens to nothing."
        )
    return found


def send_pdf(
    pdf: str | Path,
    *,
    title: str | None = None,
    folder: str | None = None,
    cfg=None,
    runner: RmapiRunner | None = None,
) -> SentDoc:
    """Push an existing PDF to the reMarkable unchanged.

    For a PDF that Claude generated on this machine, or one already in the repo/vault. Nothing
    is re-rendered: the device geometry in `[reading]` shapes what `send_markdown` PRODUCES and
    has no bearing on a document that already exists, so applying it here would mean re-flowing
    someone else's typesetting to no purpose.

    `title` defaults to the file's stem. It is applied by COPYING to `safe_filename(title)` in a
    temporary directory before upload, because `rmapi put` names the device document after the
    file it is given — renaming at the call site is the only way to control what appears on the
    tablet, and mutating the caller's file to do it would be indefensible.

    SAME-DAY RESEND CAVEAT, which differs from `send_markdown`'s. Both replace on a name
    collision, but a re-rendered markdown doc is by construction the same document, whereas two
    different PDFs sent under one title on one day are not. `deliver_pdf(replace=True)` keeps the
    device-side per-page records, so if the second PDF has a different page count those records
    no longer describe it (see `deliver_pdf`). Give the second send a distinct `title` when it is
    genuinely a different document.
    """
    cfg = cfg or load()
    folder = (folder or cfg.reading.send_folder).strip("/")
    if not folder:
        raise ValueError("no target folder: set [reading].send_folder or pass one")

    src = resolve_pdf(pdf)
    runner = runner or _subprocess_runner(cfg.reading.rmapi_binary)
    filename = safe_filename(title or src.stem)
    pages = _page_count(src)

    with tempfile.TemporaryDirectory() as tmp:
        staged = Path(tmp) / filename
        staged.write_bytes(src.read_bytes())
        _ensure_folder_path(runner, folder)
        deliver_pdf(staged, remote_folder=folder, replace=True, runner=runner)

    return SentDoc(filename=filename, remote_folder=folder, pages=pages)
