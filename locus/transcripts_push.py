"""Laptop Claude transcripts reach the server, scrubbed (Slice 0.10).

`~/.claude/projects/` on the laptop holds every Claude Code session run there (241 sessions and
1.2 GB on 10 Nov), and nothing on the server could read them: the knowledge questions heuris
answers ("what did I decide about X in that chat") were about chats it never saw. This copies
them across every 30 minutes (launchd `com.locus.transcripts`):

1. Each transcript that changed since the last run (size or mtime, kept in
   `$LOCUS_STATE_DIR/transcripts.json`) is scrubbed (locus/scrub.py) into a staging tree,
   `$LOCUS_STATE_DIR/transcripts/`, with the same relative paths.
2. The staging tree is rsynced to `data/transcripts/laptop/` on the server.

Only the scrubbed copy is ever written off the laptop. The staging tree exists so rsync sends
deltas; it is derived and safe to delete (the next run rebuilds it, slowly).

The server's own transcripts are read in place there; they are never copied here.
"""

from __future__ import annotations

import json
import os
import subprocess
from dataclasses import dataclass, field
from pathlib import Path

from locus.scrub import load_known, scrub_file

SOURCE = Path("~/.claude/projects").expanduser()


def state_dir() -> Path:
    base = os.environ.get("LOCUS_STATE_DIR") or os.path.join(
        os.environ.get("XDG_STATE_HOME", os.path.expanduser("~/.local/state")), "locus")
    return Path(base)


def remote_dir() -> str:
    d = os.environ.get("LOCUS_DIR", "/home/alec/server-projects/locus")
    return f"{d}/data/transcripts/laptop/"


@dataclass
class Report:
    scrubbed: int = 0
    unchanged: int = 0
    redacted: dict[str, int] = field(default_factory=dict)
    sent: bool = False
    error: str = ""


def changed(source: Path, seen: dict[str, list[float]]) -> list[Path]:
    """Transcripts whose size or mtime moved since they were last scrubbed."""
    out = []
    for p in sorted(source.rglob("*.jsonl")):
        st = p.stat()
        rel = str(p.relative_to(source))
        if seen.get(rel) != [st.st_size, st.st_mtime]:
            out.append(p)
    return out


def push(source: Path = SOURCE, dry_run: bool = False, send: bool = True) -> Report:
    r = Report()
    stage = state_dir() / "transcripts"
    ledger = state_dir() / "transcripts.json"
    try:
        seen = json.loads(ledger.read_text())
    except (OSError, ValueError):
        seen = {}
    todo = changed(source, seen)
    r.unchanged = sum(1 for _ in source.rglob("*.jsonl")) - len(todo)
    if dry_run:
        r.scrubbed = len(todo)
        return r
    known = load_known()
    for p in todo:
        rel = p.relative_to(source)
        st = p.stat()
        for k, v in scrub_file(p, stage / rel, known).items():
            r.redacted[k] = r.redacted.get(k, 0) + v
        seen[str(rel)] = [st.st_size, st.st_mtime]
        r.scrubbed += 1
    ledger.parent.mkdir(parents=True, exist_ok=True)
    ledger.write_text(json.dumps(seen))
    if not send:
        return r
    host = os.environ.get("LOCUS_HOST", "compute-node")
    rd = remote_dir()
    proc = subprocess.run(
        ["rsync", "-a", "--chmod=Du=rwx,Dgo=,Fu=rw,Fgo=", "-e", "ssh -o ConnectTimeout=10 -o BatchMode=yes",
         "--rsync-path", f"mkdir -p {rd} && rsync", f"{stage}/", f"{host}:{rd}"],
        capture_output=True, text=True, timeout=3600,
    )
    r.sent = proc.returncode == 0
    if not r.sent:
        r.error = (proc.stderr.strip().splitlines() or ["rsync failed"])[-1]
    return r


def describe(r: Report) -> str:
    red = ", ".join(f"{k} {v}" for k, v in sorted(r.redacted.items())) or "nothing"
    sent = "sent to the server" if r.sent else (f"NOT sent: {r.error}" if r.error else "not sent")
    return f"{r.scrubbed} scrubbed, {r.unchanged} unchanged; redacted: {red}; {sent}"
