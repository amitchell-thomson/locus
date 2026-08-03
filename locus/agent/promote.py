"""Promote a developed thread into the corpus, so his own thinking compounds.

THE LOOP THIS CLOSES. Until now the daily page ran one way into a side table:

    corpus -> daily page -> his writing -> `objects` -> dead end

`objects` is not embedded and sits in no retrieval arm, so a question he asked and then
developed over a week was invisible to `locus query`, invisible to `locus link`, and could
never come back as a connection. Everything he wrote accumulated somewhere the rest of the
system could not read. Promotion writes a thread out as an ordinary note in `vault/notes/`,
where `notes_sync` ingests it like any other note — after which it embeds, links, and can
surface as a connection exactly like the papers do.

**ONLY HIS WORDS ARE PROMOTED, and this is the invariant that matters.** Agent-authored text
must never re-enter the corpus as if it were his (invariant 5 — the `_generated/` rule exists
for precisely this failure). A thread's body can hold both: the title and development entries
are his, while `why`/`summary` may be an agent rationale from the days when the object was a
proposal. So promotion reads ONLY fields carrying an `_owner_edits` marker, which is the
durable record of authorship `apply_owner_edit` leaves. A thread with no owner-authored
development is not promoted at all: a bare question is a question, not a note, and writing it
to the corpus would put a stub in retrieval that says nothing.

Re-promotion is an UPDATE, never a second note. The file path is derived from the object id, so
further development rewrites the same file; `notes_sync` sees a real content change and
re-ingests, which is the ordinary edit path rather than a special case.
"""

from __future__ import annotations

import re
import sqlite3
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path

from locus.agent import compose_daily as cd
from locus.agent import state
from locus.config import load

# Threads live here. A subdirectory of the notes dir, so `notes_sync` picks them up with no new
# wiring — and NOT under `_generated/`, which is corpus-excluded by design: this is his writing,
# and the whole point is that it enters the corpus.
THREADS_SUBDIR = "threads"

PROMOTED_PATH_KEY = "promoted_path"
PROMOTED_AT_KEY = "promoted_at"

_SLUG_RE = re.compile(r"[^a-z0-9]+")


@dataclass
class Promotion:
    object_id: int
    path: Path
    status: str            # 'created' | 'updated' | 'unchanged'
    entries: int           # how many development passes it carries

    @property
    def wrote(self) -> bool:
        return self.status != "unchanged"


def _slug(text: str, *, limit: int = 48) -> str:
    slug = _SLUG_RE.sub("-", text.lower()).strip("-")
    return (slug[:limit].rstrip("-") or "thread")


def owner_fields(body: dict) -> set[str]:
    """The body fields the OWNER authored, per the durable marker."""
    return set((body or {}).get(state.OWNER_EDITS_KEY, {}))


def _owner_development(obj) -> list[dict]:
    """His development passes, oldest first — only if he authored them."""
    if cd.DEVELOPMENT_KEY not in owner_fields(obj.body):
        return []
    out = []
    for entry in (obj.body or {}).get(cd.DEVELOPMENT_KEY) or []:
        if isinstance(entry, dict) and str(entry.get("text", "")).strip():
            out.append(entry)
        elif isinstance(entry, str) and entry.strip():
            out.append({"at": "", "text": entry})
    return out


def _owner_statement(obj) -> str:
    """The opening statement he wrote, if he wrote it — a mark's idea, or a question he asked."""
    owner = owner_fields(obj.body)
    for key in ("question", "idea"):
        if key in owner and str((obj.body or {}).get(key, "")).strip():
            return str(obj.body[key]).strip()
    return ""


def render_thread(conn: sqlite3.Connection, obj) -> str | None:
    """The note's markdown, or None when there is nothing of his to promote.

    THE BAR IS HIS WORDS, NOT HIS PERSISTENCE. The first cut required DEVELOPMENT passes, which
    meant a thread had to be written on twice before it could reach the corpus — and every idea
    born from a margin note fails that test by construction: it carries the sentence he wrote in
    `body["idea"]` and nothing else, because he wrote it once, in a book, and moved on. All four
    of the first real mark-born ideas were therefore permanently unreachable by `locus query`,
    which is the opposite of the point. An idea he had once is still an idea he had.

    What has NOT changed is the invariant that matters: `owner_fields` gates everything, so a
    thread carrying only the proposer's rationale still promotes nothing. The rule is "none of his
    words", not "not enough of them".
    """
    development = _owner_development(obj)
    statement = _owner_statement(obj)
    if not development and not statement:
        return None

    owner = owner_fields(obj.body)
    first_date = next((str(e.get("at") or "") for e in development if e.get("at")), "")
    date = first_date or (obj.created_at or "")[:10]

    front = {
        "date": date,
        "title": obj.title,
        "category": "note",
        # ROUGH, always. A thread is working thinking, and `maturity` measures polish; claiming
        # `tidy` for a page of jottings would misdescribe it to every consumer that reads the
        # field. (The retrieval penalty on `rough` was taken to 0.0 on 2026-07-29 precisely
        # because polish is not value, so this costs it nothing in ranking.)
        "maturity": "rough",
        "origin": "locus-thread",
        "thread_type": obj.type,
        "thread_object": obj.id,
        # NO promotion timestamp. The first draft stamped one here and it would have re-ingested
        # and re-embedded every thread note on every hourly run: `notes_sync` keys on a
        # whitespace-normalised content hash, and a fresh timestamp is a real content change.
        # The rendering must depend ONLY on the thread's content.
    }
    lines = ["---"]
    lines += [f"{k}: {v}" for k, v in front.items() if v not in (None, "")]
    lines += ["---", "", f"# {obj.title}", ""]

    # The opening statement, when he wrote it. `question`/`idea` hold the original text.
    if statement:
        lines += [statement, ""]

    # Omitted entirely when he has not written on it yet: a heading with nothing under it says
    # the thread is unfinished, which is a judgement about him rather than a fact about it.
    if development:
        lines += ["## Working notes", ""]
    for entry in development:
        stamp = str(entry.get("at") or "").strip()
        lines += [f"**{stamp}** — {str(entry['text']).strip()}" if stamp
                  else str(entry["text"]).strip(), ""]

    if "resolution" in owner and str(obj.body.get("resolution", "")).strip():
        lines += ["## Where it landed", "", str(obj.body["resolution"]).strip(), ""]

    links = state.links_for(conn, obj.id)
    if links:
        # Provenance, not inference: these are the units the thread was raised against, printed
        # so the note carries its own grounding into the corpus.
        lines += ["## Raised against", ""]
        for link in links:
            lines += [f"- {cd._format_grounding(link, conn)}"]
        lines += [""]

    return "\n".join(lines)


def promote_thread(
    conn: sqlite3.Connection, object_id: int, *, notes_dir: str | Path | None = None
) -> Promotion | None:
    """Write one thread out as a note. None when it has nothing of his in it yet."""
    obj = state.get_object(conn, object_id)
    if obj is None:
        return None
    body = render_thread(conn, obj)
    if body is None:
        return None

    root = Path(notes_dir or load().paths.notes) / THREADS_SUBDIR
    root.mkdir(parents=True, exist_ok=True)
    # Keyed on the object id so further development REWRITES rather than adding a second note.
    path = root / f"{_slug(obj.title)}-{obj.id}.md"

    if path.exists() and path.read_text(encoding="utf-8") == body:
        # Byte-identical: touching it would cost a re-ingest and a re-embed for nothing.
        return Promotion(object_id, path, "unchanged", len(_owner_development(obj)))

    status = "updated" if path.exists() else "created"
    tmp = path.with_suffix(".md.tmp")
    tmp.write_text(body, encoding="utf-8")
    tmp.replace(path)

    _record_promotion(conn, obj, path)
    return Promotion(object_id, path, status, len(_owner_development(obj)))


def _record_promotion(conn: sqlite3.Connection, obj, path: Path) -> None:
    """Note where the thread was written, WITHOUT claiming the owner wrote it.

    Deliberately not `apply_owner_edit`: promotion is the agent's bookkeeping, and marking these
    keys owner-authored would both misrecord authorship and enlarge `owner_fields`, which is the
    very set that decides what may enter the corpus.
    """
    import json

    body = dict(obj.body or {})
    body[PROMOTED_PATH_KEY] = str(path)
    body[PROMOTED_AT_KEY] = datetime.now(timezone.utc).isoformat()
    # `updated_at` is left alone on purpose: it orders the "Still open" fair queue, and a
    # bookkeeping write must not push a stale thread to the back as though he had touched it.
    with conn:
        conn.execute("UPDATE objects SET body=? WHERE id=?", (json.dumps(body), obj.id))


def promote_all(
    conn: sqlite3.Connection, *, notes_dir: str | Path | None = None, limit: int = 50
) -> list[Promotion]:
    """Promote every thread that has development he wrote. Idempotent.

    Both `active` and `archived` threads qualify: a resolved thread is exactly the one most
    worth having in the corpus, and an open one that he has worked on twice is already saying
    something retrieval should be able to find.
    """
    rows = conn.execute(
        "SELECT id FROM objects WHERE type IN ('question','idea') "
        "AND status IN ('active','archived') ORDER BY updated_at DESC, id LIMIT ?",
        (limit,),
    ).fetchall()

    out: list[Promotion] = []
    for row in rows:
        promoted = promote_thread(conn, row["id"], notes_dir=notes_dir)
        if promoted is not None:
            out.append(promoted)
    return out


READING_SUBDIR = "reading"


def promote_reading_notes(
    conn: sqlite3.Connection, source_uri: str, *, title: str = "",
    notes_dir: str | Path | None = None,
) -> Promotion | None:
    """Write the owner's margin notes on one document out as a note. None if he wrote none.

    The passages he marked are the BOOK's words and belong to the book; the notes beside them
    are HIS, and until now they lived only in `pdf_annotations` — a table no retrieval arm
    reads. Reading a 200-page book and writing sixteen notes in it produced nothing the corpus
    could see.

    Each note is emitted WITH the passage it was written against, because that pairing is the
    content: "no momentum in Japan??" is meaningless without the claim it objects to. The
    passage is quoted as evidence under his note, not presented as his writing.
    """
    rows = conn.execute(
        "SELECT pdf_page, kind, note, covered_text FROM pdf_annotations "
        "WHERE source_uri=? AND TRIM(COALESCE(note,''))!='' ORDER BY pdf_page, id",
        (source_uri,),
    ).fetchall()
    if not rows:
        return None

    doc = conn.execute(
        "SELECT title FROM documents WHERE source_uri=?", (source_uri,)
    ).fetchone()
    book = title or (doc["title"] if doc else "") or source_uri.rsplit("/", 1)[-1]

    first = conn.execute(
        "SELECT MIN(captured_at) a FROM pdf_annotations WHERE source_uri=?", (source_uri,)
    ).fetchone()["a"] or ""

    lines = [
        "---",
        f"date: {first[:10]}" if first[:10] else "",
        f"title: Reading notes — {book}",
        "category: note",
        "maturity: rough",
        "origin: locus-reading",
        f"reading_source: {source_uri}",
        "---",
        "",
        f"# Reading notes — {book}",
        "",
    ]
    for row in rows:
        note = " ".join((row["note"] or "").split())
        passage = " ".join((row["covered_text"] or "").split())
        lines += [f"## p.{row['pdf_page'] + 1}", "", note, ""]
        if passage:
            lines += [f"> {passage}", ""]

    root = Path(notes_dir or load().paths.notes) / READING_SUBDIR
    root.mkdir(parents=True, exist_ok=True)
    path = root / f"{_slug(book)}.md"
    body = "\n".join(line for line in lines if line is not None)

    if path.exists() and path.read_text(encoding="utf-8") == body:
        return Promotion(0, path, "unchanged", len(rows))
    status = "updated" if path.exists() else "created"
    tmp = path.with_suffix(".md.tmp")
    tmp.write_text(body, encoding="utf-8")
    tmp.replace(path)
    return Promotion(0, path, status, len(rows))


def unpromoted_count(conn: sqlite3.Connection) -> int:
    """Threads carrying his development that are not yet notes. For `locus status`, not the page.

    Deliberately NOT surfaced on the daily page: a count of what is outstanding is the guilt
    metric §9 forbids. An operator command is a different audience.
    """
    n = 0
    for row in conn.execute(
        "SELECT id FROM objects WHERE type IN ('question','idea') "
        "AND status IN ('active','archived')"
    ):
        obj = state.get_object(conn, row["id"])
        if obj is None:
            continue
        if _owner_development(obj) and PROMOTED_PATH_KEY not in (obj.body or {}):
            n += 1
    return n
