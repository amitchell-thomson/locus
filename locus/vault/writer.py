"""Atomic owned-block writer (agent-layer plan §10), used by capture's Related block.

  - `upsert_block()` — put/replace an agent-owned block INSIDE a human note. Reads the note,
    replaces (or appends) only the marked span, and writes atomically. If a two-writer conflict is
    detected it diverts to a sidecar (sidecar.py) rather than risk the owner's prose.
  - `_atomic_write()` — the write itself; Loop A and conversation capture use it directly.

`write_generated_note()` (whole agent-owned files: the daily page, `_generated/` notes) was
deleted with the daily page in Slice C (C2).

Atomicity (the never-corrupt-a-note guarantee): write to a temp file in the SAME directory, fsync,
then `os.replace` (atomic rename on POSIX). A crash mid-write leaves the original intact — never a
half-written note.
"""

from __future__ import annotations

import hashlib
import os
import tempfile
from dataclasses import dataclass
from pathlib import Path

from locus.vault import markers, sidecar


@dataclass(frozen=True)
class WriteResult:
    path: Path        # where the content was actually written (note or sidecar)
    to_sidecar: bool  # True if diverted to the sidecar because of a conflict
    created: bool     # True if `path` did not exist before


def _atomic_write(path: Path, content: str) -> None:
    """Write `content` to `path` atomically (temp in same dir -> fsync -> os.replace)."""
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=str(path.parent), prefix=".locus-", suffix=".tmp")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            f.write(content)
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, path)  # atomic rename; overwrites the target in one step
    except BaseException:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise


def content_hash(text: str) -> str:
    """SHA-256 of note text — the caller reads a note, passes this hash to `upsert_block`, and a
    later mismatch (someone else wrote in between) triggers the sidecar fallback."""
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def upsert_block(
    note_path: str | Path,
    kind: str,
    body: str,
    *,
    run_id: str,
    expected_hash: str | None = None,
) -> WriteResult:
    """Insert or wholesale-replace the agent-owned `kind` block in `note_path`.

    `body` is the block content (e.g. a `> [!ai] Related` callout); the markers are added here.
    Content outside the block is preserved byte-for-byte. Re-running with the same kind REPLACES
    the block (never accumulates), even as `run_id` changes.

    Conflict -> sidecar: if `expected_hash` is given and the note changed since (someone else
    wrote), or a Syncthing conflict copy exists, the block is written into `<note>.locus.md`
    instead of the note. `expected_hash=None` (the server-authored case) skips the change check.
    """
    markers.validate_kind(kind)
    path = Path(note_path)
    existed = path.exists()
    current = path.read_text(encoding="utf-8") if existed else ""

    changed = expected_hash is not None and existed and content_hash(current) != expected_hash
    if changed or sidecar.has_sync_conflict(path):
        target = sidecar.sidecar_path(path)
        base = target.read_text(encoding="utf-8") if target.exists() else ""
        target_existed = target.exists()
        _atomic_write(target, markers.upsert(base, kind, body, run_id))
        return WriteResult(path=target, to_sidecar=True, created=not target_existed)

    _atomic_write(path, markers.upsert(current, kind, body, run_id))
    return WriteResult(path=path, to_sidecar=False, created=not existed)
