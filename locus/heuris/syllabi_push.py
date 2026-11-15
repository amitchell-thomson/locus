"""The course syllabi reach the server (spec P, step P8): `~/files/Studies/**/syllabus.md`.

They are the department's text, written verbatim by `locus canvas syllabus` (P8 first half), so
nothing is scrubbed: no mail, no one's address. Only `syllabus.md` files cross, by rsync with
`--delete`, so a syllabus removed on the laptop leaves the index on the next full scan. A laptop
with no syllabi at all sends nothing (an unmounted or moved `~/files` must not empty the server's
copy; the indexer refuses to prune after reading nothing too).
"""

from __future__ import annotations

import os
import subprocess
from dataclasses import dataclass
from pathlib import Path

STUDIES = Path("~/files/Studies").expanduser()


@dataclass
class Report:
    found: int = 0
    sent: bool = False
    error: str | None = None


def push(send: bool = True, root: Path = STUDIES) -> Report:
    r = Report(found=sum(1 for _ in root.rglob("syllabus.md")) if root.is_dir() else 0)
    if not r.found:
        r.error = f"no syllabus.md under {root}; nothing sent, the server keeps its copy"
        return r
    if not send:
        return r
    host = os.environ.get("LOCUS_HOST", "compute-node")
    rd = f"{os.environ.get('LOCUS_DIR', '/home/alec/server-projects/locus')}/data/heuris-in/syllabi/"
    proc = subprocess.run(
        ["rsync", "-a", "--delete", "--prune-empty-dirs", "--include=*/", "--include=syllabus.md",
         "--exclude=*", "-e", "ssh -o ConnectTimeout=10 -o BatchMode=yes",
         "--rsync-path", f"mkdir -p {rd} && rsync", f"{root}/", f"{host}:{rd}"],
        capture_output=True, text=True, timeout=600,
    )
    r.sent = proc.returncode == 0
    if not r.sent:
        r.error = (proc.stderr.strip().splitlines() or ["rsync failed"])[-1]
    return r


def describe(r: Report) -> str:
    return f"{r.found} syllabi; " + ("sent to the server" if r.sent else f"NOT sent: {r.error}" if r.error else "not sent")
