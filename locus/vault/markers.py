"""Owned-block sentinel markers (agent-layer plan §10).

An agent-owned block inside a human note is delimited by HTML-comment sentinels keyed by a
`kind` (e.g. 'related', 'tension'):

    <!-- locus:ai:related:start run=<run_id> -->
    > [!ai] Related
    > - ...
    <!-- locus:ai:related:end -->

The markers make provenance structural and glanceable (invariant 4), and let a block be
regenerated WHOLESALE — located by its marker pair and replaced, so re-runs never accumulate.
HTML comments render invisibly in Obsidian/Markdown previews, so the note stays clean to read.
"""

from __future__ import annotations

import re

# A block kind: a short slug, safe to embed in a marker and a regex.
_KIND_RE = re.compile(r"^[a-z0-9][a-z0-9_-]*$")
# A run id: safe token (int / uuid / short slug) — no newline or '>' that could break a marker.
_RUN_ID_RE = re.compile(r"^[A-Za-z0-9_.:-]+$")


def validate_kind(kind: str) -> None:
    if not _KIND_RE.match(kind):
        raise ValueError(f"invalid block kind {kind!r}: must match {_KIND_RE.pattern}")


def _validate_run_id(run_id: str) -> str:
    run_id = str(run_id)
    if not _RUN_ID_RE.match(run_id):
        raise ValueError(f"invalid run_id {run_id!r}: must match {_RUN_ID_RE.pattern}")
    return run_id


def start_marker(kind: str, run_id: str) -> str:
    validate_kind(kind)
    return f"<!-- locus:ai:{kind}:start run={_validate_run_id(run_id)} -->"


def end_marker(kind: str) -> str:
    validate_kind(kind)
    return f"<!-- locus:ai:{kind}:end -->"


def _block_pattern(kind: str) -> re.Pattern[str]:
    """Match a whole block of `kind` (both markers + everything between), any run id."""
    validate_kind(kind)
    k = re.escape(kind)
    return re.compile(
        rf"<!-- locus:ai:{k}:start run=[^\n>]*-->.*?<!-- locus:ai:{k}:end -->",
        re.DOTALL,
    )


def find_block(text: str, kind: str) -> re.Match[str] | None:
    """The first block of `kind` in `text` (markers inclusive), or None."""
    return _block_pattern(kind).search(text)


def render_block(kind: str, body: str, run_id: str) -> str:
    """The full marked block (no trailing newline). `body` is inserted verbatim, stripped of
    surrounding blank lines so regeneration is byte-stable."""
    return f"{start_marker(kind, run_id)}\n{body.strip()}\n{end_marker(kind)}"


def upsert(text: str, kind: str, body: str, run_id: str) -> str:
    """Return `text` with the `kind` block replaced (if present) or appended (if not).

    Replacement is in place — surrounding content is preserved byte-for-byte — so a human note's
    prose is never touched. Appended blocks get one blank-line separator from prior content.
    """
    block = render_block(kind, body, run_id)
    existing = find_block(text, kind)
    if existing is not None:
        return text[: existing.start()] + block + text[existing.end() :]
    if text.strip() == "":
        return block + "\n"
    separator = "\n" if text.endswith("\n\n") else ("\n" if text.endswith("\n") else "\n\n")
    return text + separator + block + "\n"
