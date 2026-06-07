"""widen source_type CHECK for slides (build step 9)

Revision ID: 0006
Revises: 0005
Create Date: 2026-06-05

Step 9 adds the .pptx extractor; 'slides' needs to be a legal `documents.source_type`
value (format lives in source_type, content-kind lives in category, §15.4). SQLite
cannot ALTER a CHECK constraint, so the table is rebuilt exactly as in 0004:
create-copy-drop-rename with an explicit column list, then the 0003 indexes are
recreated (DROP TABLE removed them).

The rebuild is safe without FK pragma games: the Alembic connection never sets
PRAGMA foreign_keys=ON (only locus/db/connection.py does, for the app), so child FKs
(sections/chunks/propositions/entities → documents.id) are not enforced mid-rebuild, and
they reference the table NAME, which the rename preserves. Row ids are copied verbatim,
so no child row is orphaned.
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0006"
down_revision: Union[str, None] = "0005"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None

_COLUMNS = (
    "id, content_hash, source_type, source_uri, raw_path, title, ingested_at, "
    "ingest_model, thesis, method, result, limitations, section_map, gap_flags, "
    "source_date, category"
)

_DDL = """
    CREATE TABLE documents_new (
        id           INTEGER PRIMARY KEY,
        content_hash TEXT    NOT NULL UNIQUE,
        source_type  TEXT    NOT NULL CHECK (source_type IN ({types})),
        source_uri   TEXT    NOT NULL,
        raw_path     TEXT    NOT NULL,
        title        TEXT,
        ingested_at  TEXT    NOT NULL DEFAULT (datetime('now')),
        ingest_model TEXT    NOT NULL,
        thesis       TEXT,
        method       TEXT,
        result       TEXT,
        limitations  TEXT,
        section_map  TEXT,
        gap_flags    TEXT,
        source_date  TEXT,
        category     TEXT
    )
"""

_WIDE = "'pdf','code','video','docx','markdown','text','notebook','slides'"
_NARROW = "'pdf','code','video','docx','markdown','text','notebook'"


def _rebuild(types: str) -> None:
    op.execute(_DDL.format(types=types))
    op.execute(f"INSERT INTO documents_new ({_COLUMNS}) SELECT {_COLUMNS} FROM documents")
    op.execute("DROP TABLE documents")
    op.execute("ALTER TABLE documents_new RENAME TO documents")
    # DROP TABLE took the 0003 indexes with it; recreate them on the rebuilt table.
    op.execute("CREATE INDEX idx_documents_source_date ON documents(source_date)")
    op.execute("CREATE INDEX idx_documents_category ON documents(category)")


def upgrade() -> None:
    _rebuild(_WIDE)


def downgrade() -> None:
    # Mirrors back to the 0004 CHECK; fails (by design) if rows with 'slides' exist.
    _rebuild(_NARROW)
