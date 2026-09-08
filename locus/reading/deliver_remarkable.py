"""Push a rendered PDF to the reMarkable via `rmapi put`.

This is the server->device *push* channel — the reverse of the Phase-0 capture transport
(which pushes device renders to the server). Every reading loop and the daily page (Phases 1/3)
depend on this channel working, so `locus read` exercising it is the Phase-0.5 acceptance gate.

The `rmapi` subprocess is behind an injectable `RmapiRunner` so the delivery logic (ensure the
target folder exists, then upload) is unit-testable without a device or network. `rmapi` is the
same authed binary Phase 0 verified for the pull direction.
"""

from __future__ import annotations

import json
import logging
import subprocess
import tempfile
import zipfile
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

log = logging.getLogger(__name__)

# A runner takes the argv AFTER the binary (e.g. ["put", file, dir]) and returns
# (returncode, stdout, stderr). The default shells out; tests inject a fake.
RmapiRunner = Callable[[list[str]], "tuple[int, str, str]"]


@dataclass
class RemoteDoc:
    """What the device already holds under a name we are about to replace.

    `ok` is separate from the other two on purpose: a fetch that failed must never read as
    "no ink", because the only thing the ink flag gates is deletion.
    """

    ok: bool                  # the bundle was fetched AND parsed
    page_count: int | None    # from `.content`; None when absent or unreadable
    has_ink: bool             # any `.rm` layer in the bundle


# Takes the full device path ("Inbox/2026-09-08 Note.pdf") and reports what is there.
RemoteInspector = Callable[[str], RemoteDoc]


@dataclass
class DeliveryResult:
    remote_folder: str
    filename: str
    created_folder: bool  # whether `mkdir` reported creating the folder this run
    # How an existing entry was replaced, when one was: "content-only" or "delete+put".
    # None when nothing was replaced. Worth returning rather than logging only, because the
    # two paths differ in whether the device's page records survive (see `deliver_pdf`).
    replaced: str | None = None


def _subprocess_runner(binary: str) -> RmapiRunner:
    def run(args: list[str]) -> tuple[int, str, str]:
        proc = subprocess.run(
            [binary, *args], capture_output=True, text=True, timeout=120
        )
        return proc.returncode, proc.stdout, proc.stderr

    return run


def _pdf_page_count(pdf: Path) -> int | None:
    """Pages in the PDF about to be uploaded, or None if it cannot be read.

    None means "do not know", and every decision below treats not knowing as a reason to take
    the conservative branch rather than the clever one.
    """
    try:
        import fitz

        with fitz.open(pdf) as doc:
            return doc.page_count
    except Exception:
        return None


def _uninspectable(_device_path: str) -> RemoteDoc:
    """The "we cannot look" answer, which routes every decision to the conservative branch."""
    return RemoteDoc(ok=False, page_count=None, has_ink=False)


def _rmapi_inspector(binary: str) -> RemoteInspector:
    """Fetch the device bundle and report its page count and whether it carries ink.

    One download answers both questions, and it only ever runs on the replace path, so the cost
    is paid exactly when a wrong answer would corrupt something.
    """

    def inspect(device_path: str) -> RemoteDoc:
        with tempfile.TemporaryDirectory() as tmp:
            proc = subprocess.run(
                [binary, "get", device_path],
                capture_output=True, text=True, cwd=tmp, timeout=300,
            )
            if proc.returncode != 0:
                log.warning("rmapi get %r failed, treating the remote as uninspectable: %s",
                            device_path, (proc.stderr or proc.stdout).strip())
                return RemoteDoc(ok=False, page_count=None, has_ink=False)

            bundles = [p for p in Path(tmp).iterdir() if p.suffix in (".rmdoc", ".zip")]
            if not bundles:
                return RemoteDoc(ok=False, page_count=None, has_ink=False)
            try:
                with zipfile.ZipFile(bundles[0]) as bundle:
                    names = bundle.namelist()
                    has_ink = any(n.endswith(".rm") for n in names)
                    content = next((n for n in names if n.endswith(".content")), None)
                    pages = None
                    if content:
                        data = json.loads(bundle.read(content))
                        pages = data.get("pageCount")
                        if pages is None:
                            listed = (data.get("cPages") or {}).get("pages")
                            pages = len(listed) if listed else None
                    return RemoteDoc(ok=True, page_count=pages, has_ink=has_ink)
            except (zipfile.BadZipFile, json.JSONDecodeError, OSError) as exc:
                log.warning("could not read the bundle for %r: %s", device_path, exc)
                return RemoteDoc(ok=False, page_count=None, has_ink=False)

    return inspect


def _ensure_folder(runner: RmapiRunner, folder: str) -> bool:
    """`rmapi mkdir <folder>`, tolerating "already exists".

    rmapi mkdir is not idempotent (it errors if the folder exists), so we run it and treat an
    exists-style failure as success. Returns True only when the folder was newly created.
    """
    if not folder or folder == "/":
        return False
    code, out, err = runner(["mkdir", folder])
    if code == 0:
        return True
    blob = f"{out}\n{err}".lower()
    if "exist" in blob or "already" in blob:
        return False
    raise RuntimeError(f"rmapi mkdir {folder!r} failed: {err.strip() or out.strip()}")


def _replace_existing(
    pdf_path: Path,
    *,
    remote_folder: str,
    runner: RmapiRunner,
    inspect: RemoteInspector,
) -> tuple[str, int, str, str]:
    r"""Replace an entry that already exists, choosing the path that keeps the device coherent.

    `--content-only` swaps the PDF and KEEPS the device's per-page records. When the rebuild has
    a different number of pages those records no longer describe it, and the document is left
    broken in a way nothing reports: observed 2026-09-08, a 9-page brief rebuilt to 6 left
    `.content` reading `pageCount: 9`, and `rmapi geta` refused the document outright with
    "invalid page number (page count too short)". The daily page hits the same thing whenever a
    section is omitted, since its length varies by design.

    Deleting first fixes the records, and deleting is exactly what must not happen to a page he
    has written on. So the rule is: delete and re-put ONLY when the bundle was successfully read,
    carries no `.rm` ink layer, and its page count genuinely differs from what we are uploading.
    Every other case, including every case where the remote could not be inspected, keeps the old
    `--content-only` behaviour. Stale metadata is recoverable; his handwriting is not.
    """
    device_path = f"{remote_folder.rstrip('/')}/{pdf_path.name}"
    existing = inspect(device_path)
    new_pages = _pdf_page_count(pdf_path)

    records_are_stale = (
        existing.ok
        and new_pages is not None
        and existing.page_count is not None
        and existing.page_count != new_pages
    )

    if records_are_stale and not existing.has_ink:
        code, out, err = runner(["rm", device_path])
        if code != 0:
            log.warning("could not delete %r before re-uploading (%s); falling back to "
                        "--content-only", device_path, (err or out).strip())
        else:
            code, out, err = runner(["put", str(pdf_path), remote_folder])
            return "delete+put", code, out, err

    if records_are_stale and existing.has_ink:
        log.warning(
            "%r has ink and its page count changed (%s on the device, %s now). Replacing "
            "content only: the device's page records will be stale, which is the recoverable "
            "problem. Deleting would take his handwriting with it.",
            device_path, existing.page_count, new_pages,
        )

    code, out, err = runner(["put", "--content-only", str(pdf_path), remote_folder])
    return "content-only", code, out, err


def deliver_pdf(
    pdf_path: Path,
    *,
    remote_folder: str = "Locus",
    rmapi_binary: str = "rmapi",
    replace: bool = False,
    runner: RmapiRunner | None = None,
    inspect: RemoteInspector | None = None,
) -> DeliveryResult:
    """Upload `pdf_path` to `remote_folder` on the reMarkable, creating the folder if needed.

    `rmapi put <file> <dir>` uploads under the (already-existing) directory.

    `replace=True` makes re-delivery idempotent. The original note here assumed a same-named
    re-upload would create a device-side duplicate; it does not — rmapi REFUSES it outright
    ("entry already exists"), which surfaced the first time a scheduled unit tried to deliver
    the same filename twice (2026-07-30 deploy). Left alone, any recurring delivery works once
    and then fails every run after. With `replace`, an existing entry is re-uploaded with
    `--content-only`, which swaps the PDF while keeping the device-side document.

    Callers that must NOT clobber a page the owner may have annotated should give each
    delivery a distinct name instead (the daily page dates its filename) and use `replace`
    only for same-name rebuilds.

    **`--content-only` swaps the PDF but keeps the device-side PAGE RECORDS**, and those are
    per-page: `content.pageCount`, the `pages` UUID list, `redirectionPageMap` and `.pagedata`.
    If the rebuild has a DIFFERENT number of pages, they no longer describe it. Observed
    2026-08-06: a 4-page page was rebuilt to 5 (a Recall section came back), and the device
    document still read `pageCount: 4` with four page UUIDs — the back page carrying `Q1` and the
    recall answers had no page record at all. The daily page's length varies by design (an empty
    section is omitted, taking its page break with it), so any same-day rebuild can hit this.
    A fresh `put` under an unused name always produces correct records; only the replace path is
    affected, and it is invisible unless you download the bundle and read `.content`.

    Since 2026-09-08 that repair is automatic, and `_replace_existing` holds the rule: the
    existing entry is deleted and re-put ONLY when its bundle was successfully read, carries no
    `.rm` ink layer, and its page count actually differs. Anything else, including any failure to
    inspect, keeps `--content-only`. It is still not unconditional and must not become so: his
    handwriting is worth more than clean metadata and `rm` is not recoverable. `DeliveryResult
    .replaced` reports which path ran.

    `inspect` is the injectable that answers "what is on the device"; it defaults to fetching the
    bundle with `rmapi get`, which is one download and only on the replace path.
    """
    pdf_path = Path(pdf_path)
    if not pdf_path.is_file():
        raise FileNotFoundError(pdf_path)

    # The inspector is a SECOND transport (it shells out to `rmapi get`, which writes to cwd and
    # so cannot go through `runner`'s argv-only interface). A caller that injected a runner
    # injected the transport it wants used, and reaching past it to the network would make every
    # test that exercises replace hit the device. So: default to the real inspector only when we
    # are also building the real runner. Callers that inject a runner in PRODUCTION (`send.py`)
    # pass an inspector explicitly. Uninspectable is the safe answer, not a degraded one — it
    # simply keeps the old `--content-only` behaviour.
    if inspect is None:
        inspect = _rmapi_inspector(rmapi_binary) if runner is None else _uninspectable
    runner = runner or _subprocess_runner(rmapi_binary)

    created = _ensure_folder(runner, remote_folder)
    code, out, err = runner(["put", str(pdf_path), remote_folder])

    how: str | None = None
    if code != 0 and replace and "already exists" in (err + out):
        how, code, out, err = _replace_existing(
            pdf_path, remote_folder=remote_folder, runner=runner, inspect=inspect,
        )

    if code != 0:
        raise RuntimeError(
            f"rmapi put {pdf_path.name!r} -> {remote_folder!r} failed: "
            f"{err.strip() or out.strip()}"
        )
    return DeliveryResult(
        remote_folder=remote_folder, filename=pdf_path.name, created_folder=created,
        replaced=how,
    )
