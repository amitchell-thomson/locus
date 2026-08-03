"""When retrieval returns one of his own threads, return the THREAD — not a flattened copy of it.

THE HALF-GAP THIS CLOSES. `objects` sits in no retrieval arm, and the bridge into search is
`locus promote` -> `vault/notes/threads/` -> `notes_sync` -> the ordinary spine. That makes the
TEXT findable, which was the important half. But what comes back is a note: the sentence he wrote,
stripped of the thing that made it worth storing as an object — the project it belongs to, the
other threads it touches, and how his view moved across successive passes. Ask "what have I
thought about regime detection" and you got his words with none of their connections, which is
the same flattening the Obsidian export exists to avoid.

WHY THIS IS NOT A NEW RETRIEVAL ARM, which was the obvious design and is the wrong one:

  - Every owner-authored thread is already promoted, so a parallel arm would put the same text in
    the candidate pool TWICE — competing with itself for the top-k, double-counting against the
    per-doc diversity cap, and making a thread look like corroboration from two sources when it is
    one thought written once.
  - `Candidate` is (doc_id, section_id); an object has neither. Giving threads a synthetic doc id
    would push a special case through rerank, the diversity rules, expansion and citation
    formatting — five modules changed to carry a unit that is already in the pool.
  - A second copy would need embedding and re-embedding on every development pass.

So this is an EXPANSION, in the same spirit as the hierarchical parent-summary/synthesis step: the
document was retrieved on its merits, and the object it came from is then joined on. Joins only,
no model, no vectors, no new kind.

The join is exact rather than inferred: `promote` records `body.promoted_path`, so a retrieved
note maps back to its object by path. A note whose thread has since been archived, or a note that
was never a thread, simply has no context and adds nothing.
"""

from __future__ import annotations

import json
import sqlite3
from dataclasses import dataclass, field
from pathlib import Path

from locus.agent import state
from locus.agent.promote import PROMOTED_PATH_KEY

# Enough to say "this connects", not enough to reprint his notebook into every answer.
_MAX_RELATED = 4
_MAX_PASSES = 4


@dataclass
class ThreadContext:
    """The live object behind a promoted thread note."""

    object_id: int
    type: str
    title: str
    project: str = ""
    related: tuple[str, ...] = ()          # other threads of his that touch this one
    development: tuple[str, ...] = ()      # his successive passes, oldest first
    status: str = "active"

    @property
    def is_empty(self) -> bool:
        return not (self.project or self.related or self.development)

    def render(self) -> str:
        """One indented block, appended under the note it belongs to."""
        lines = [f"    thread: {self.type} — {self.title}"]
        if self.project:
            lines.append(f"    part of: {self.project}")
        if self.related:
            lines.append("    also touches: " + "; ".join(self.related))
        for text in self.development:
            lines.append(f"    you later wrote: {text}")
        return "\n".join(lines)


def _by_promoted_path(conn: sqlite3.Connection) -> dict[str, int]:
    """`promoted note path -> object id`, for every thread that has been promoted.

    Scans `objects` rather than indexing a column, because `promoted_path` lives in the JSON body
    and the thread count is in the dozens. If that stops being true this becomes a column.
    """
    out: dict[str, int] = {}
    try:
        # A thread he RESOLVED is still promoted and still in the corpus, so excluding every
        # archived object did not hide it — it came back as a bare note, stripped of the project,
        # the sibling threads and the development that are the whole reason it is an object. A
        # question he ANSWERED is exactly the one whose answer is worth carrying back. One he
        # threw away is not, and `status` cannot tell those apart — see `dropped_object_ids`.
        rows = conn.execute(
            "SELECT id, body FROM objects WHERE type IN ('idea','question')"
        ).fetchall()
        dropped = state.dropped_object_ids(conn)
        rows = [r for r in rows if r["id"] not in dropped]
    except sqlite3.OperationalError:
        return out
    for row in rows:
        try:
            body = json.loads(row["body"] or "{}")
        except (TypeError, ValueError):
            continue
        path = str(body.get(PROMOTED_PATH_KEY) or "")
        if path:
            out[path] = row["id"]
            out[Path(path).name] = row["id"]      # match on basename too: source_uri may be relative
    return out


def context_for(conn: sqlite3.Connection, source_uri: str, index: dict[str, int] | None = None):
    """The `ThreadContext` for a retrieved document, or None if it is not a promoted thread."""
    if not source_uri:
        return None
    index = _by_promoted_path(conn) if index is None else index
    object_id = index.get(source_uri) or index.get(Path(source_uri).name)
    if object_id is None:
        return None

    obj = state.get_object(conn, object_id)
    if obj is None:
        return None

    ctx = ThreadContext(object_id=obj.id, type=obj.type, title=obj.title, status=obj.status)
    related: list[str] = []
    for link in obj.links:
        if link.target_kind != "object" or not link.target_key.isdigit():
            continue
        other = state.get_object(conn, int(link.target_key))
        if other is None:
            continue
        if other.type == "project":
            ctx.project = other.title
        elif other.type in ("idea", "question") and len(related) < _MAX_RELATED:
            related.append(other.title)
    ctx.related = tuple(related)

    # His successive passes. The note already carries them in prose, so only the LAST few are
    # repeated here — the point is to signal that the thought moved, not to duplicate the note.
    passes = [p.stance for p in state.development_positions(conn, "object", str(obj.id))]
    ctx.development = tuple(passes[-_MAX_PASSES:])
    return ctx


def contexts_for(conn: sqlite3.Connection, source_uris: list[str]) -> dict[str, ThreadContext]:
    """Batch form: one index scan for a whole result set."""
    index = _by_promoted_path(conn)
    if not index:
        return {}
    out: dict[str, ThreadContext] = {}
    for uri in set(source_uris):
        ctx = context_for(conn, uri, index=index)
        if ctx is not None and not ctx.is_empty:
            out[uri] = ctx
    return out
