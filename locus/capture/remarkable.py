"""rmapi plumbing shared by capture, and the item Loop A transcribes.

This module used to identify a `<uuid>.pdf` pushed by the agent on the tablet, by running
`rmapi find /` and then one `rmapi stat` per document and mapping the top folder to a category.
That agent stopped on or about 1 Aug 2026 and the identification went with it; since Slice 0.12
the tablet is read from the cloud in one call (`capture/device.py`), and a document's folder and
kind decide how it is read. What remains here is the runner and the two listing helpers that the
reader (`review.py`), Loop B's reading index and `reading/watch.py` still use.
"""

from __future__ import annotations

import json
import logging
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

log = logging.getLogger(__name__)

# argv AFTER the rmapi binary -> (returncode, stdout, stderr). Default shells out; tests fake it.
RmapiRunner = Callable[[list[str]], "tuple[int, str, str]"]


@dataclass(frozen=True)
class CaptureItem:
    uuid: str
    pdf_path: Path   # the notebook's render, <uuid>.pdf in the staging dir
    name: str        # human document name from reMarkable
    folder: str      # the device folders above it, "/Studies/B14 Information Engineering Systems"
    category: str    # the library category its area files under (capture/device.AREA_CATEGORY)
    # The device's lastModified — WHEN THE OWNER LAST WROTE IN THE NOTEBOOK, not when capture
    # ran. It becomes the note's `date:` frontmatter and hence documents.source_date, and hence
    # belief_positions.dated_at. Without it every captured note is dated "today" and the whole
    # understanding-evolution trajectory (§6.3) collapses to a flat line at the capture date —
    # which is exactly what happened to the first 12-document capture. None if the device did not
    # report one.
    modified: str | None = None
    area: str | None = None    # 'Studies' | 'Projects' | 'Careers', None for Jot and the root
    thing: str | None = None   # registry id of its course, repo or application, if matched


def _subprocess_runner(binary: str) -> RmapiRunner:
    def run(args: list[str]) -> tuple[int, str, str]:
        proc = subprocess.run([binary, *args], capture_output=True, text=True, timeout=120)
        return proc.returncode, proc.stdout, proc.stderr

    return run


def _find_file_paths(runner: RmapiRunner) -> list[str]:
    """Device paths of all documents (`[f]` entries) from `rmapi find /`."""
    code, out, err = runner(["find", "/"])
    if code != 0:
        raise RuntimeError(f"rmapi find failed: {err.strip() or out.strip()}")
    paths = []
    for line in out.splitlines():
        line = line.strip()
        if line.startswith("[f] "):
            paths.append(line[4:].strip())
    return paths


def _stat(runner: RmapiRunner, device_path: str) -> dict | None:
    """`rmapi stat <path>` parsed to a dict, or None if it failed/was unparseable."""
    code, out, err = runner(["stat", device_path])
    if code != 0:
        log.warning("rmapi stat %r failed: %s", device_path, err.strip() or out.strip())
        return None
    try:
        return json.loads(out)
    except json.JSONDecodeError:
        log.warning("rmapi stat %r returned non-JSON: %r", device_path, out[:200])
        return None
