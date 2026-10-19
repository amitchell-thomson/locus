"""The data directory is `data/`, not `vault/`: rewrite the stored paths that name it

Revision ID: 0037
Revises: 0036
Create Date: 2026-10-19

The directory holding the database, the raw store, the drop zone, the notes, backups, cache and
logs was called `vault/`, which reads as an Obsidian vault (and holds one, `vault/obsidian/`, the
export). His call, 19 Oct: renamed to `data/`.

A rename of the directory alone would break things silently, because paths under it are stored
as IDENTITY, not just as locations. `documents.source_uri` is how a note is found again when he
edits it (`ingest_pipeline` replaces the document AT that path); under a new directory the
lookup misses and the edited note arrives as a second document beside the old one. The same
strings key the agent tables (links, annotations, connection notes, the shown-ledger), so each
of those would quietly point at nothing. Measured on the server before the rename: about 1,700
rows across the columns below.

Only the directory segment is rewritten, and only where it is followed by one of the data
directory's own entries, so `locus/vault/markers.py` (the notes-writer package, a code path)
is never touched. The corpus content (chunks, sections, propositions, pass caches) is not
rewritten: it is what the documents SAID, and invariant 2 keeps derived layers off it.
"""

import re
from typing import Sequence, Union

from alembic import op

revision: str = "0037"
down_revision: Union[str, None] = "0036"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None

# (table, column) pairs that store a path under the data directory as a key or a location.
COLUMNS = [
    ("documents", "source_uri"),
    ("object_links", "target_key"),
    ("belief_positions", "source_uri"),
    ("daily_pages", "md_path"),
    ("daily_pages", "pdf_path"),
    ("daily_anchors", "target_key"),
    ("pdf_annotations", "source_uri"),
    ("objects", "body"),
    ("reading_proposals", "local_path"),
    ("reading_targets", "source_uri"),
    ("acceptance_log", "candidate_key"),
    ("daily_shown", "item_key"),
    ("connection_notes", "src_uri"),
    ("connection_notes", "other_uri"),
]

_ENTRIES = r"(?:incoming|notes|raw|cache|backups|logs|obsidian)/|locus\.db"


def _rewriter(old: str, new: str):
    # The segment starts a string or follows a separator: `/` (absolute paths), `:` and `|`
    # (composite keys such as `conn:<a>|<b>` in daily_shown).
    pattern = re.compile(rf"(^|[/:|]){old}/(?={_ENTRIES})")
    return lambda s: pattern.sub(rf"\g<1>{new}/", s)


def _rewrite(old: str, new: str) -> None:
    conn = op.get_bind()
    sub = _rewriter(old, new)
    tables = {r[0] for r in conn.exec_driver_sql("SELECT name FROM sqlite_master WHERE type='table'")}
    for table, column in COLUMNS:
        if table not in tables:
            continue
        rows = conn.exec_driver_sql(
            f"SELECT rowid, {column} FROM {table} WHERE {column} LIKE ?", (f"%{old}/%",)
        ).fetchall()
        for rowid, value in rows:
            changed = sub(value)
            if changed != value:
                conn.exec_driver_sql(
                    f"UPDATE {table} SET {column} = ? WHERE rowid = ?", (changed, rowid)
                )


def upgrade() -> None:
    _rewrite("vault", "data")


def downgrade() -> None:
    _rewrite("data", "vault")
