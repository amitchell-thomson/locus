"""Atomic owned-block writer + generated-note writer (agent-layer plan §10).

Every agent write into the vault goes through here. Two entry points:

  - `upsert_block()` — put/replace an agent-owned block INSIDE a human note. Reads the note,
    replaces (or appends) only the marked span, and writes atomically. If a two-writer conflict is
    detected it diverts to a sidecar (sidecar.py) rather than risk the owner's prose.
  - `write_generated_note()` — write a WHOLE agent-owned file (a `_generated/` note, the daily
    page) with provenance frontmatter (`author: agent`, so it is glanceably not the owner's, and
    so note-ingest can corpus-exclude it — invariant 5).

Atomicity (the never-corrupt-a-note guarantee): write to a temp file in the SAME directory, fsync,
then `os.replace` (atomic rename on POSIX). A crash mid-write leaves the original intact — never a
half-written note.
"""

from __future__ import annotations

import hashlib
import os
import tempfile
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Callable

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


def _frontmatter(fields: dict[str, str]) -> str:
    lines = ["---"]
    lines += [f"{k}: {v}" for k, v in fields.items()]
    lines.append("---")
    return "\n".join(lines)


def write_generated_note(
    path: str | Path,
    body: str,
    *,
    run_id: str,
    generated_at: Callable[[], str] | None = None,
    extra: dict[str, str] | None = None,
) -> WriteResult:
    """Write a fully agent-owned note with provenance frontmatter (invariant 4/5).

    The frontmatter (`author: agent`, `generated: true`, `source_run`, `generated_at`) marks the
    file as not the owner's and lets note-ingest corpus-exclude `_generated/`. Overwrites wholesale
    (these files are regenerable, never read back). `extra` adds fields (e.g. `title`)."""
    now = (generated_at or (lambda: datetime.now(timezone.utc).isoformat()))()
    fields = {"author": "agent", "generated": "true", "source_run": str(run_id), "generated_at": now}
    fields.update(extra or {})
    path = Path(path)
    existed = path.exists()
    _atomic_write(path, f"{_frontmatter(fields)}\n\n{body.strip()}\n")
    return WriteResult(path=path, to_sidecar=False, created=not existed)
