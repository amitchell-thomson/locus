"""Writing items into heuris.db, incrementally by content hash (H1.3).

Every source (mail, calendar, chats, repos) yields `Item`s; `index` stores the ones whose hash
changed, chunks and embeds them, and records the run in `sources`. A second run over unchanged
data embeds nothing and adds 0 (the H1.3 check), which is what makes a 15-minute refresh free.

`prune=True` removes this source's items that the run did not yield. A source passes it only
when it saw EVERYTHING it owns (a full scan), never on a partial or failed read: pruning after a
read that silently returned half the mail would delete the other half (CLAUDE.md §3).
"""

from __future__ import annotations

import hashlib
import json
import sqlite3
import struct
from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import Callable, Iterable

# nomic-embed-text is trained with task prefixes; documents and queries take different ones.
DOC_PREFIX = "search_document: "
QUERY_PREFIX = "search_query: "

CHUNK_CHARS = 1800
CHUNK_OVERLAP = 200
_COMMIT_EVERY = 200  # items per transaction, so a first run of 30,000 can stop and resume

Embed = Callable[[list[str]], list[list[float]]]


@dataclass
class Item:
    ref: str
    kind: str
    at: datetime
    title: str = ""
    text: str = ""
    area: str | None = None
    thing: str | None = None
    parent: str | None = None
    people: list[tuple[str, str]] = field(default_factory=list)  # (person, role)
    meta: dict = field(default_factory=dict)

    def digest(self) -> str:
        blob = json.dumps(
            [self.ref, self.kind, self.at.isoformat(), self.title, self.text, self.area, self.thing,
             self.parent, sorted(self.people), self.meta],
            sort_keys=True, ensure_ascii=False, default=str,
        )
        return hashlib.sha256(blob.encode()).hexdigest()


@dataclass
class Stats:
    added: int = 0
    unchanged: int = 0
    removed: int = 0
    chunks: int = 0


def chunk_text(text: str, size: int = CHUNK_CHARS, overlap: int = CHUNK_OVERLAP) -> list[str]:
    """Split on paragraph, then line, then space boundaries near `size`; never return empty."""
    text = text.strip()
    if len(text) <= size:
        return [text]
    out, start = [], 0
    while start < len(text):
        end = min(start + size, len(text))
        if end < len(text):
            window = text[start:end]
            for sep in ("\n\n", "\n", ". ", " "):
                cut = window.rfind(sep)
                if cut > size // 2:
                    end = start + cut + len(sep)
                    break
        piece = text[start:end].strip()
        if piece:
            out.append(piece)
        if end >= len(text):
            break
        start = max(end - overlap, start + 1)
    return out


def _vec(v: list[float]) -> bytes:
    return struct.pack(f"{len(v)}f", *v)


def _default_embed(texts: list[str]) -> list[list[float]]:
    from locus.ingest.embed import embed_texts

    return embed_texts(texts)


def _delete_item(conn: sqlite3.Connection, item_id: int) -> None:
    ids = [r[0] for r in conn.execute("SELECT id FROM chunks WHERE item_id=?", (item_id,))]
    for cid in ids:
        conn.execute("DELETE FROM chunk_fts WHERE rowid=?", (cid,))
        conn.execute("DELETE FROM chunk_vec WHERE chunk_id=?", (cid,))
    conn.execute("DELETE FROM chunks WHERE item_id=?", (item_id,))
    conn.execute("DELETE FROM item_people WHERE item_id=?", (item_id,))
    conn.execute("DELETE FROM items WHERE id=?", (item_id,))


def _write(conn: sqlite3.Connection, source: str, batch: list[tuple[Item, str]], embed: Embed) -> int:
    pieces: list[tuple[int, int, str, str]] = []  # (batch index, ord, chunk text, embed text)
    for i, (it, _) in enumerate(batch):
        body = it.text or it.title
        for o, c in enumerate(chunk_text(body) if body else [""]):
            pieces.append((i, o, c, f"{DOC_PREFIX}{it.title}\n\n{c}".strip()))
    vectors = embed([p[3] for p in pieces]) if pieces else []
    now = datetime.now(timezone.utc).isoformat(timespec="seconds")
    ids: dict[int, int] = {}
    for i, (it, h) in enumerate(batch):
        old = conn.execute("SELECT id FROM items WHERE ref=?", (it.ref,)).fetchone()
        if old:
            _delete_item(conn, old[0])
        at = it.at if it.at.tzinfo else it.at.replace(tzinfo=timezone.utc)
        cur = conn.execute(
            "INSERT INTO items(ref, source, kind, at, at_ts, area, thing, parent, title, text, meta, "
            "hash, indexed_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
            (it.ref, source, it.kind, at.isoformat(), int(at.timestamp()), it.area, it.thing,
             it.parent, it.title, it.text, json.dumps(it.meta, ensure_ascii=False, default=str), h, now),
        )
        ids[i] = cur.lastrowid
        conn.executemany(
            "INSERT OR IGNORE INTO item_people(item_id, person, role) VALUES (?,?,?)",
            [(cur.lastrowid, p.strip().lower(), r) for p, r in it.people if p and p.strip()],
        )
    for (i, o, c, _), v in zip(pieces, vectors):
        it = batch[i][0]
        at = it.at if it.at.tzinfo else it.at.replace(tzinfo=timezone.utc)
        cid = conn.execute("INSERT INTO chunks(item_id, ord, text) VALUES (?,?,?)", (ids[i], o, c)).lastrowid
        conn.execute("INSERT INTO chunk_fts(rowid, title, text) VALUES (?,?,?)", (cid, it.title, c))
        conn.execute(
            "INSERT INTO chunk_vec(chunk_id, embedding, kind, thing, area, at_ts) VALUES (?,?,?,?,?,?)",
            (cid, _vec(v), it.kind, it.thing or "", it.area or "", int(at.timestamp())),
        )
    return len(pieces)


def index(conn: sqlite3.Connection, source: str, items: Iterable[Item], *,
          embed: Embed | None = None, prune: bool = False) -> Stats:
    """Store what changed; with `prune`, remove this source's items the run did not yield."""
    embed = embed or _default_embed
    st = Stats()
    known = {ref: h for ref, h in conn.execute("SELECT ref, hash FROM items WHERE source=?", (source,))}
    seen: set[str] = set()
    batch: list[tuple[Item, str]] = []
    for it in items:
        if it.ref in seen:
            continue
        seen.add(it.ref)
        h = it.digest()
        if known.get(it.ref) == h:
            st.unchanged += 1
            continue
        batch.append((it, h))
        if len(batch) >= _COMMIT_EVERY:
            with conn:
                st.chunks += _write(conn, source, batch, embed)
            st.added += len(batch)
            batch = []
    if batch:
        with conn:
            st.chunks += _write(conn, source, batch, embed)
        st.added += len(batch)
    if prune:
        gone = [ref for ref in known if ref not in seen]
        with conn:
            for ref in gone:
                row = conn.execute("SELECT id FROM items WHERE ref=?", (ref,)).fetchone()
                if row:
                    _delete_item(conn, row[0])
        st.removed = len(gone)
    return st


def record_run(conn: sqlite3.Connection, source: str, st: Stats | None, error: str | None = None,
               note: str | None = None) -> None:
    """One row per source: when it last ran, last succeeded, and what it did."""
    now = datetime.now(timezone.utc).isoformat(timespec="seconds")
    total = conn.execute("SELECT COUNT(*) FROM items WHERE source=?", (source,)).fetchone()[0]
    st = st or Stats()
    with conn:
        conn.execute(
            "INSERT INTO sources(name, last_run, last_ok, items, added, unchanged, removed, error, note) "
            "VALUES (?,?,?,?,?,?,?,?,?) ON CONFLICT(name) DO UPDATE SET last_run=excluded.last_run, "
            "last_ok=COALESCE(excluded.last_ok, sources.last_ok), items=excluded.items, "
            "added=excluded.added, unchanged=excluded.unchanged, removed=excluded.removed, "
            "error=excluded.error, note=COALESCE(excluded.note, sources.note)",
            (source, now, None if error else now, total, st.added, st.unchanged, st.removed, error, note),
        )
