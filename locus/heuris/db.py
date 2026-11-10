"""heuris.db: items, their chunks, full-text and vectors, and each source's freshness (H1.3).

A separate file from `locus.db` (H1 D1): the library's schema is Alembic-managed and frozen, and
nothing here is a source of truth. Every row is derived from mail, kairos, transcripts or git and
is rebuilt by deleting the file and re-indexing (invariant 3), so the schema is created in place
and versioned by `PRAGMA user_version`; a version change rebuilds rather than migrates.

Search units are CHUNKS, not items: a mail message is usually one chunk, a long chat answer or a
README several. Filters (kind, thing, area, date) are vec0 metadata columns, so the KNN applies
them before ranking instead of over-fetching and filtering after, which would starve a narrow
filter (a ten-item `thing` buried under 40,000 mails returns nothing at k=50).
"""

from __future__ import annotations

import os
import sqlite3
from pathlib import Path

from locus.db.connection import get_connection

SCHEMA_VERSION = 1
DIM = 768

_SCHEMA = f"""
CREATE TABLE IF NOT EXISTS items (
    id          INTEGER PRIMARY KEY,
    ref         TEXT NOT NULL UNIQUE,     -- the pointer `open` follows: mail:personal/<thread>/<id>
    source      TEXT NOT NULL,            -- the indexer that wrote it: mail, calendar, chats, repos
    kind        TEXT NOT NULL,            -- mail, event, task, chat, commit, doc
    at          TEXT NOT NULL,            -- ISO 8601 with offset
    at_ts       INTEGER NOT NULL,         -- the same, epoch seconds, for filters
    area        TEXT,
    thing       TEXT,                     -- registry id, when it belongs to one
    parent      TEXT,                     -- the thread, session or repo it sits in
    title       TEXT NOT NULL DEFAULT '',
    text        TEXT NOT NULL DEFAULT '',
    meta        TEXT NOT NULL DEFAULT '{{}}',
    hash        TEXT NOT NULL,
    indexed_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS items_at ON items(at_ts);
CREATE INDEX IF NOT EXISTS items_thing ON items(thing, at_ts);
CREATE INDEX IF NOT EXISTS items_parent ON items(parent);
CREATE INDEX IF NOT EXISTS items_source ON items(source);

CREATE TABLE IF NOT EXISTS item_people (
    item_id INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    person  TEXT NOT NULL,                -- lowercased address, or lowercased name
    role    TEXT NOT NULL,                -- from, to, cc, attendee, name
    PRIMARY KEY (item_id, person, role)
);
CREATE INDEX IF NOT EXISTS people_person ON item_people(person);

CREATE TABLE IF NOT EXISTS chunks (
    id      INTEGER PRIMARY KEY,
    item_id INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    ord     INTEGER NOT NULL,
    text    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS chunks_item ON chunks(item_id);

CREATE VIRTUAL TABLE IF NOT EXISTS chunk_fts USING fts5(title, text, tokenize='porter unicode61');

CREATE VIRTUAL TABLE IF NOT EXISTS chunk_vec USING vec0(
    chunk_id  INTEGER PRIMARY KEY,
    embedding float[{DIM}] distance_metric=cosine,
    kind      text,
    thing     text,
    area      text,
    at_ts     integer
);

-- Files a source has already read (chats): re-read only when size or mtime moved, so a
-- 30-minute refresh does not parse 1.6 GB of transcripts to find the three that changed.
CREATE TABLE IF NOT EXISTS file_state (
    source TEXT NOT NULL,
    path   TEXT NOT NULL,
    size   INTEGER NOT NULL,
    mtime  REAL NOT NULL,
    PRIMARY KEY (source, path)
);

CREATE TABLE IF NOT EXISTS sources (
    name       TEXT PRIMARY KEY,
    last_run   TEXT,
    last_ok    TEXT,
    items      INTEGER NOT NULL DEFAULT 0,
    added      INTEGER NOT NULL DEFAULT 0,
    unchanged  INTEGER NOT NULL DEFAULT 0,
    removed    INTEGER NOT NULL DEFAULT 0,
    error      TEXT,
    note       TEXT
);
"""


def path() -> Path:
    """`$LOCUS_HEURIS_DB`, else `heuris.db` beside `locus.db`."""
    env = os.environ.get("LOCUS_HEURIS_DB")
    if env:
        return Path(env)
    from locus.config import load

    return load().paths.db.parent / "heuris.db"


def connect(p: Path | str | None = None) -> sqlite3.Connection:
    p = Path(p) if p else path()
    p.parent.mkdir(parents=True, exist_ok=True)
    conn = get_connection(p)
    conn.execute("PRAGMA journal_mode=WAL")
    conn.execute("PRAGMA busy_timeout=10000")
    have = conn.execute("PRAGMA user_version").fetchone()[0]
    if have not in (0, SCHEMA_VERSION):
        raise RuntimeError(
            f"{p} is schema {have}, this code is {SCHEMA_VERSION}. heuris.db is derived: delete it "
            "and run `locus heuris index all` to rebuild."
        )
    conn.executescript(_SCHEMA)
    conn.execute(f"PRAGMA user_version={SCHEMA_VERSION}")
    return conn
