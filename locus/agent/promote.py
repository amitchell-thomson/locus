"""Promote his margin notes on a document into the corpus, as a note `notes_sync` ingests.

**ONLY HIS WORDS ARE PROMOTED.** Agent-authored text must never re-enter the corpus as if it
were his (invariant 5). A margin note is his; the passage it was written against is the book's,
and is quoted under it as evidence, never presented as his writing.

Thread and project promotion (`promote_all`, the `locus promote` command) went with the daily
page in Slice C (C2): nothing writes development onto `objects` any more.
"""

from __future__ import annotations

import re
import sqlite3
from dataclasses import dataclass
from pathlib import Path

from locus.config import load

_SLUG_RE = re.compile(r"[^a-z0-9]+")


@dataclass
class Promotion:
    object_id: int
    path: Path
    status: str            # 'created' | 'updated' | 'unchanged'
    entries: int           # how many notes it carries

    @property
    def wrote(self) -> bool:
        return self.status != "unchanged"


def _slug(text: str, *, limit: int = 48) -> str:
    slug = _SLUG_RE.sub("-", text.lower()).strip("-")
    return (slug[:limit].rstrip("-") or "thread")


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
        "SELECT pdf_page, kind, note, covered_text, line_text FROM pdf_annotations "
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
        # What the note was about: the words the ink covered, or, for a note written in the
        # margin, which covers none, the line beside it. A margin note is deictic ("why here?"),
        # and without its line it reads as nothing at all (Slice 0.12).
        passage = " ".join((row["covered_text"] or row["line_text"] or "").split())
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
