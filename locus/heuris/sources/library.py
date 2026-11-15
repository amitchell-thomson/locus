"""The library as heuris items (spec P, step P7; P-D8): one search over everything.

Until P7 the library (locus.db: lectures, papers, Canvas files, his transcribed notes) was
searched by its own engine and glued onto heuris's hits (`library_candidates`). A filter it could
not apply in its KNN was applied AFTER it: `thing=b14` took the best library hits over every
course, then threw away the ones not about B14, so a narrow course filter starved. Now every
section of every library document is a heuris item and the filters are vec0 metadata like any
other item's.

- **One item per section**, ref `src:<doc>#s<position>`, the pointer `open` already follows.
  Its text is the section's chunks, in order, as the library extracted them.
- **kind** `note` when the document is his writing (`owner_authored_sql`, the one definition),
  else `source`. **thing** from `doc_thing` (a note's header, else a registry name in its title
  or path). **area** from a note's header, else the category, else the thing's area.
- **Re-embedded, not copied.** P-D8 proposed reusing the library's vectors. They were embedded
  without nomic's `search_document:` prefix and without the title, while every heuris chunk has
  both and every heuris query has `search_query:`; copying them would score library sections on
  a different scale from everything they are ranked against. Re-embedding costs a few minutes of
  GPU once (5,023 sections, 15 Nov) and nothing after: an unchanged section hashes the same.
- **Never while ingest runs.** The library is mid-write then, and two Ollama users at once is
  what produces spurious quarantines (CLAUDE.md invariant 6). `run_source` takes the ingest lock
  non-blocking and skips the tick if it is held.
- **One line per document in a timeline**: the first section is the document's `lead`
  (meta), which timeline shows with the document's own pointer.

The library's ingest (extract, LLM passes, figures) is untouched: this reads it.
"""

from __future__ import annotations

import sqlite3
from datetime import datetime, timezone
from typing import Iterator

from locus.heuris import registry
from locus.heuris.index import Item

CATEGORY_AREA = {"coursework": "Studies", "career": "Careers", "project": "Projects"}


def _at(source_date: str | None, ingested_at: str | None) -> datetime:
    for v in (source_date, ingested_at):
        if not v:
            continue
        try:
            d = datetime.fromisoformat(str(v).replace("Z", "+00:00"))
        except ValueError:
            try:
                d = datetime.strptime(str(v)[:10], "%Y-%m-%d")
            except ValueError:
                continue
        return d if d.tzinfo else d.replace(tzinfo=timezone.utc)
    return datetime(1970, 1, 1, tzinfo=timezone.utc)


def items(conn: sqlite3.Connection, his_sql: tuple[str, list] | None = None) -> Iterator[Item]:
    """Every library section as an Item. `conn` is locus.db; `his_sql` overrides the "his writing"
    predicate (tests, which must not read the live config)."""
    from locus.heuris.tools import doc_thing, note_header

    if his_sql is None:
        from locus.agent.state import owner_authored_sql

        his_sql = owner_authored_sql("d")
    clause, params = his_sql
    docs = conn.execute(
        f"SELECT d.id, d.title, d.source_uri, d.source_date, d.ingested_at, d.category, ({clause}) AS his "
        f"FROM documents d ORDER BY d.id", params).fetchall()
    for d in docs:
        doc_id, title, uri, sdate, ingested, category, his = d
        title = title or f"document {doc_id}"
        header = note_header(uri or "")
        thing = doc_thing(title, uri or "")
        area = header.get("area") or CATEGORY_AREA.get(category or "") or (registry.area_of(thing) if thing else None)
        folder = (header.get("remarkable_folder") or "").strip("/")
        shown = title + (f" (tablet: {folder})" if folder else "")
        at = _at(sdate, ingested)
        secs = conn.execute("SELECT id, position, title FROM sections WHERE doc_id=? ORDER BY position",
                            (doc_id,)).fetchall()
        lead = True
        for sid, pos, stitle in secs:
            text = "\n".join(r[0] for r in conn.execute(
                "SELECT raw_text FROM chunks WHERE section_id=? ORDER BY position", (sid,)) if r[0])
            if not text.strip():
                continue
            yield Item(
                ref=f"src:{doc_id}#s{pos}",
                kind="note" if his else "source",
                at=at,
                title=f"{shown} · {stitle}" if stitle and stitle.strip() and stitle.strip() != title else shown,
                text=text,
                area=area,
                thing=thing,
                parent=f"src:{doc_id}",
                meta={"category": category, "unit": "section", "doc": doc_id, "doc_title": shown,
                      **({"lead": 1} if lead else {})},
            )
            lead = False
