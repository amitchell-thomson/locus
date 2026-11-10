"""Career files reach the server: read and redacted on the laptop, one JSON file (H1.12).

Two places only, because files are where his identity papers live too (documents.py):

* **iCloud `Career/`**: applications, contracts and offer letters, cover letters, his CVs,
  course and research material. Code folders inside it are skipped (`alpha-fund/.venv` alone
  holds hundreds of licence files).
* **`~/Downloads`**, only names that look like an offer, a contract, an offer pack or a
  handbook: the Millennium offer letters and contract (test question 24) are there and nowhere
  else. Everything else in Downloads is lecture PDFs the library already has, bank statements
  and installers.

Each file becomes one entry `{path, root, mtime, sha, text}`; the same bytes under two names
(`Offer Letter … -1.pdf`, `-2.pdf`) are sent once. `rsync --delete` and the indexer's full scan
take a deleted file out of the index.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
from collections import Counter
from dataclasses import dataclass, field
from pathlib import Path

from locus.heuris import documents
from locus.scrub import load_known
from locus.transcripts_push import state_dir

ICLOUD_CAREER = Path("~/Library/Mobile Documents/com~apple~CloudDocs/Career").expanduser()
DOWNLOADS = Path("~/Downloads").expanduser()
DOWNLOADS_KEEP = re.compile(r"offer|contract|handbook|letter of engagement|onboarding", re.I)
SKIP_DIRS = {".venv", "venv", "node_modules", "site-packages", "__pycache__", ".git", "Downloads Archive"}

ROOTS: list[tuple[str, Path, re.Pattern[str] | None]] = [
    ("career", ICLOUD_CAREER, None),
    ("downloads", DOWNLOADS, DOWNLOADS_KEEP),
]


@dataclass
class Report:
    read: int = 0
    empty: int = 0
    duplicates: int = 0
    skipped: Counter = field(default_factory=Counter)
    redacted: dict[str, int] = field(default_factory=dict)
    sent: bool = False
    error: str = ""


def candidates(root: Path, keep: re.Pattern[str] | None) -> list[Path]:
    if not root.is_dir():
        return []
    out = []
    if keep is not None:  # Downloads: the top level only, by name
        return sorted(p for p in root.iterdir() if p.is_file() and keep.search(p.name))
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS and not d.startswith(".")]
        out += [Path(dirpath) / f for f in filenames]
    return sorted(out)


def build(stage: Path | None = None, roots=None) -> Report:
    r = Report()
    stage = stage or state_dir() / "files"
    stage.mkdir(parents=True, exist_ok=True)
    known = load_known()
    cache = documents.Cache(state_dir() / "files-cache.json")
    seen: set[str] = set()
    rows = []
    for label, root, keep in roots or ROOTS:
        for p in candidates(root, keep):
            if why := documents.denied(p.name):
                if why != "not a readable kind" and not why.startswith("a lock"):
                    r.skipped[why] += 1
                continue
            try:
                st = p.stat()
                if st.st_size > documents.MAX_BYTES:
                    r.skipped["too large"] += 1
                    continue
                data = p.read_bytes()
            except OSError:
                r.skipped["unreadable"] += 1
                continue
            sha, text = cache.text(p.name, data, known, r.redacted)
            if not text:
                r.empty += 1
                continue
            if sha in seen:
                r.duplicates += 1
                continue
            seen.add(sha)
            r.read += 1
            rows.append({"root": label, "path": str(p.relative_to(root)), "mtime": st.st_mtime,
                         "sha": sha, "text": text})
    cache.save()
    doc = json.dumps({"files": rows}, ensure_ascii=False, sort_keys=True)
    f = stage / "files.json"
    if not (f.exists() and f.read_text() == doc):
        tmp = f.with_suffix(".part")
        tmp.write_text(doc)
        os.chmod(tmp, 0o600)
        tmp.replace(f)
    return r


def push(send: bool = True) -> Report:
    stage = state_dir() / "files"
    r = build(stage)
    if r.skipped["unreadable"]:
        # Sending a partial set would delete the rest on the server (rsync --delete, then the full
        # scan prunes). Under launchd macOS refused iCloud and Downloads (10 Nov: 47 unreadable),
        # so a partial read is a failed read: keep the server's last good copy and say so.
        r.error = (f"{r.skipped['unreadable']} files unreadable (macOS privacy permission for the "
                   f"push job?); nothing sent, the server keeps its last copy")
        return r
    if not send:
        return r
    host = os.environ.get("LOCUS_HOST", "compute-node")
    rd = f"{os.environ.get('LOCUS_DIR', '/home/alec/server-projects/locus')}/data/heuris-in/files/"
    proc = subprocess.run(
        ["rsync", "-a", "--delete", "--chmod=Du=rwx,Dgo=,Fu=rw,Fgo=", "-e",
         "ssh -o ConnectTimeout=10 -o BatchMode=yes", "--rsync-path", f"mkdir -p {rd} && rsync",
         f"{stage}/", f"{host}:{rd}"],
        capture_output=True, text=True, timeout=1800,
    )
    r.sent = proc.returncode == 0
    if not r.sent:
        r.error = (proc.stderr.strip().splitlines() or ["rsync failed"])[-1]
    return r


def describe(r: Report) -> str:
    skipped = ", ".join(f"{why} {n}" for why, n in r.skipped.most_common()) or "none"
    red = ", ".join(f"{k} {v}" for k, v in sorted(r.redacted.items())) or "nothing"
    sent = "sent to the server" if r.sent else (f"NOT sent: {r.error}" if r.error else "not sent")
    return (f"{r.read} read, {r.empty} without text, {r.duplicates} duplicates; skipped {skipped}; "
            f"redacted: {red}; {sent}")
