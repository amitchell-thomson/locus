"""Push a rendered PDF to the reMarkable via `rmapi put`.

This is the server->device *push* channel — the reverse of the Phase-0 capture transport
(which pushes device renders to the server). Every reading loop and the daily page (Phases 1/3)
depend on this channel working, so `locus read` exercising it is the Phase-0.5 acceptance gate.

The `rmapi` subprocess is behind an injectable `RmapiRunner` so the delivery logic (ensure the
target folder exists, then upload) is unit-testable without a device or network. `rmapi` is the
same authed binary Phase 0 verified for the pull direction.
"""

from __future__ import annotations

import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

# A runner takes the argv AFTER the binary (e.g. ["put", file, dir]) and returns
# (returncode, stdout, stderr). The default shells out; tests inject a fake.
RmapiRunner = Callable[[list[str]], "tuple[int, str, str]"]


@dataclass
class DeliveryResult:
    remote_folder: str
    filename: str
    created_folder: bool  # whether `mkdir` reported creating the folder this run


def _subprocess_runner(binary: str) -> RmapiRunner:
    def run(args: list[str]) -> tuple[int, str, str]:
        proc = subprocess.run(
            [binary, *args], capture_output=True, text=True, timeout=120
        )
        return proc.returncode, proc.stdout, proc.stderr

    return run


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


def deliver_pdf(
    pdf_path: Path,
    *,
    remote_folder: str = "Locus",
    rmapi_binary: str = "rmapi",
    runner: RmapiRunner | None = None,
) -> DeliveryResult:
    """Upload `pdf_path` to `remote_folder` on the reMarkable, creating the folder if needed.

    `rmapi put <file> <dir>` uploads under the (already-existing) directory. A same-named doc
    re-uploaded creates a device-side duplicate rather than overwriting — the caller keeps the
    PDF filename stable per source so re-reads land predictably; true idempotent replace is a
    Phase-1 concern once the capture loop drives delivery.
    """
    pdf_path = Path(pdf_path)
    if not pdf_path.is_file():
        raise FileNotFoundError(pdf_path)
    runner = runner or _subprocess_runner(rmapi_binary)

    created = _ensure_folder(runner, remote_folder)
    code, out, err = runner(["put", str(pdf_path), remote_folder])
    if code != 0:
        raise RuntimeError(
            f"rmapi put {pdf_path.name!r} -> {remote_folder!r} failed: "
            f"{err.strip() or out.strip()}"
        )
    return DeliveryResult(
        remote_folder=remote_folder, filename=pdf_path.name, created_folder=created
    )
