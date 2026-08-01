"""Put a proposal on the device, into `Locus/Reading/Proposed`.

Thin layer over `deliver_remarkable.deliver_pdf` (the same push channel the daily page uses), with
the three things a reading proposal needs on top:

  1. **A unique filename.** `rmapi put` REFUSES a same-named re-upload rather than duplicating it,
     which is how a scheduled delivery works once and then fails silently every run afterwards
     (the daily page hit exactly this on the 2026-07-30 deploy). Every proposal is date-prefixed.
  2. **Nested folder creation.** `rmapi mkdir` makes one level at a time, so `Locus/Reading/
     Proposed` needs three calls, not one.
  3. **The uuid, recorded once.** A single `rmapi stat` after upload gives the xochitl document id,
     which is the only identifier that survives a rename. The watch matches on filename first
     because `find` returns paths and not ids; the uuid is the fallback that turns "he renamed it"
     into something we can notice rather than silently reject.
"""

from __future__ import annotations

import json
import logging
import re
import sqlite3
from dataclasses import dataclass
from datetime import date as _date
from pathlib import Path

from locus.reading import proposals as P
from locus.reading.deliver_remarkable import (
    RmapiRunner,
    _ensure_folder,
    _subprocess_runner,
    deliver_pdf,
)

log = logging.getLogger(__name__)

DEFAULT_ROOT = "Locus/Reading"
# Characters the device (and rmapi's path handling) are happier without.
_UNSAFE = re.compile(r'[/\\:*?"<>|]+')


def safe_filename(title: str, *, on: _date | None = None, max_len: int = 70) -> str:
    """`YYYY-MM-DD Title.pdf` — date-prefixed so a re-delivery never collides (see module note)."""
    stamp = (on or _date.today()).isoformat()
    clean = _UNSAFE.sub(" ", title).strip()
    clean = " ".join(clean.split())[:max_len].strip() or "Reading"
    return f"{stamp} {clean}.pdf"


def ensure_reading_folders(runner: RmapiRunner, *, root: str = DEFAULT_ROOT) -> None:
    """Create the reading root and its three folders, tolerating any that already exist.

    `Proposed` is a holding pen; `In-Progress` and `Finished` exist so that moving a file OUT has
    somewhere obvious to go — the accept signal only works if the destination is already there.
    """
    parts = [p for p in root.split("/") if p]
    for i in range(len(parts)):
        _ensure_folder(runner, "/".join(parts[: i + 1]))
    for folder in P.READING_FOLDERS:
        _ensure_folder(runner, f"{root}/{folder}")


def _device_uuid(runner: RmapiRunner, device_path: str) -> str | None:
    """`rmapi stat` for the uploaded document's id. Best-effort: a failure is not fatal."""
    code, out, err = runner(["stat", device_path])
    if code != 0:
        log.warning("rmapi stat %r failed: %s", device_path, err.strip() or out.strip())
        return None
    try:
        return json.loads(out).get("ID")
    except (json.JSONDecodeError, AttributeError):
        log.warning("rmapi stat %r returned non-JSON", device_path)
        return None


@dataclass
class Delivered:
    proposal_id: int
    filename: str
    device_uuid: str | None


def fetch_open_access(url: str, dest: Path, *, timeout: int = 120) -> Path:
    """Download an open-access PDF so the proposal can BE the paper rather than describe it.

    Only ever called with a `oa_pdf_url` the metadata itself advertised as open access — this
    fetches what the publisher offers freely, and nothing is sent but the request. A proposal with
    no such URL stays a stub and waits for him to supply the file (invariant 5).

    Verifies the payload really is a PDF: an arXiv rate-limit or maintenance page returns 200 with
    HTML, and silently delivering that to the device would put an error page in the reading folder
    wearing a paper's title.
    """
    import urllib.request

    req = urllib.request.Request(url, headers={"User-Agent": "locus-discovery/1.0"})
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        payload = resp.read()
    if not payload.startswith(b"%PDF"):
        raise RuntimeError(f"{url} did not return a PDF ({len(payload)} bytes, {payload[:16]!r})")
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_bytes(payload)
    return dest


def deliver_proposal(
    conn: sqlite3.Connection,
    proposal: P.Proposal,
    pdf_path: str | Path,
    *,
    root: str = DEFAULT_ROOT,
    runner: RmapiRunner | None = None,
    rmapi_binary: str = "rmapi",
    on: _date | None = None,
    is_real: bool,
) -> Delivered:
    """Upload `pdf_path` into `Proposed` and move the proposal to `status='proposed'`.

    `pdf_path` is the rendered artifact; `is_real` says what it IS — the actual work (an
    open-access paper) or a stub page describing one. Only a real file records a `local_path`,
    because `local_path` is what `accept.ingest_accepted` will later feed to the ingest spine, and
    a stub is agent output that must never enter the corpus as though it were the work itself
    (invariant 5). `Proposal.is_stub` keys off that emptiness, so getting this wrong would ingest
    our own description of a book as if it were the book.
    """
    runner = runner or _subprocess_runner(rmapi_binary)
    pdf_path = Path(pdf_path)
    filename = safe_filename(proposal.title, on=on)

    staged = pdf_path.parent / filename
    if staged != pdf_path:
        staged.write_bytes(pdf_path.read_bytes())

    ensure_reading_folders(runner, root=root)
    target = f"{root}/{P.FOLDER_PROPOSED}"
    deliver_pdf(staged, remote_folder=target, runner=runner)

    uuid = _device_uuid(runner, f"/{target}/{Path(filename).stem}")
    P.mark_proposed(
        conn,
        proposal.id,
        filename=filename,
        device_uuid=uuid,
        local_path=str(pdf_path) if is_real else None,
    )
    return Delivered(proposal.id, filename, uuid)
