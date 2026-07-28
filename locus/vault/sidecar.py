"""Sidecar fallback for two-writer conflicts (agent-layer plan §10).

reMarkable- and conversation-originated notes are server-authored, so there is no concurrent
writer and the agent edits the note in place. But a TYPED note the owner edits in Obsidian (with
Syncthing enabled) could be written by both sides. If the note changed since we read it, or a
Syncthing `*.sync-conflict-*` copy exists, the agent must NOT risk clobbering the owner's prose —
it writes its block into a sidecar `<note>.locus.md` instead. Losing an enrichment to a sidecar is
recoverable; overwriting the owner's words is not (invariant 2 / failure mode #2).
"""

from __future__ import annotations

from pathlib import Path


def sidecar_path(note_path: str | Path) -> Path:
    """`foo.md` -> `foo.locus.md`. The `.locus` infix marks it as agent-owned output."""
    p = Path(note_path)
    return p.with_name(f"{p.stem}.locus{p.suffix}")


def has_sync_conflict(note_path: str | Path) -> bool:
    """True if a Syncthing conflict copy of the note exists next to it
    (`<name>.sync-conflict-<date>-<time>-<id>.<ext>`) — a signal that two writers diverged."""
    p = Path(note_path)
    if not p.parent.exists():
        return False
    return any(p.parent.glob(f"{p.stem}.sync-conflict-*{p.suffix}"))
