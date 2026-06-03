"""initial schema (CLAUDE.md §5, propositions promoted to first-class — decision A)

Revision ID: 0001
Revises:
Create Date: 2026-06-03

Hand-authored with op.execute(raw SQL) because the schema uses sqlite-vec vec0 virtual
tables, which SQLAlchemy/Alembic autogenerate does not model. The sqlite-vec extension is
loaded on the connection by env.py, so CREATE VIRTUAL TABLE ... USING vec0 succeeds.
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0001"
down_revision: Union[str, None] = None
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    # L1 — document
    op.execute(
        """
        CREATE TABLE documents (
            id           INTEGER PRIMARY KEY,
            content_hash TEXT    NOT NULL UNIQUE,
            source_type  TEXT    NOT NULL CHECK (source_type IN ('pdf','code','video')),
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
            gap_flags    TEXT
        )
        """
    )

    # L2 — section (propositions are first-class rows below, not a JSON blob here)
    op.execute(
        """
        CREATE TABLE sections (
            id                 INTEGER PRIMARY KEY,
            doc_id             INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
            position           INTEGER NOT NULL,
            title              TEXT,
            summary            TEXT,
            cross_section_deps TEXT,
            file_path          TEXT,
            call_graph         TEXT,
            UNIQUE (doc_id, position)
        )
        """
    )
    op.execute(
        "CREATE VIRTUAL TABLE section_vectors USING vec0("
        " section_id INTEGER PRIMARY KEY, embedding FLOAT[768])"
    )

    # L3 — chunk
    op.execute(
        """
        CREATE TABLE chunks (
            id              INTEGER PRIMARY KEY,
            section_id      INTEGER NOT NULL REFERENCES sections(id)  ON DELETE CASCADE,
            doc_id          INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
            position        INTEGER NOT NULL,
            raw_text        TEXT    NOT NULL,
            embed_model     TEXT    NOT NULL,
            file_path       TEXT,
            line_start      INTEGER,
            line_end        INTEGER,
            video_timestamp INTEGER,
            UNIQUE (section_id, position)
        )
        """
    )
    op.execute(
        "CREATE VIRTUAL TABLE chunk_vectors USING vec0("
        " chunk_id INTEGER PRIMARY KEY, embedding FLOAT[768])"
    )

    # propositions — first-class, embedded, directly retrievable (decision A)
    op.execute(
        """
        CREATE TABLE propositions (
            id          INTEGER PRIMARY KEY,
            section_id  INTEGER NOT NULL REFERENCES sections(id)  ON DELETE CASCADE,
            doc_id      INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
            position    INTEGER NOT NULL,
            text        TEXT    NOT NULL,
            embed_model TEXT    NOT NULL,
            UNIQUE (section_id, position)
        )
        """
    )
    op.execute(
        "CREATE VIRTUAL TABLE proposition_vectors USING vec0("
        " proposition_id INTEGER PRIMARY KEY, embedding FLOAT[768])"
    )

    # entities — typed + section-anchored provenance
    op.execute(
        """
        CREATE TABLE entities (
            id         INTEGER PRIMARY KEY,
            doc_id     INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
            section_id INTEGER          REFERENCES sections(id)  ON DELETE CASCADE,
            name       TEXT    NOT NULL,
            type       TEXT    NOT NULL,
            UNIQUE (doc_id, section_id, name, type)
        )
        """
    )
    op.execute("CREATE INDEX idx_entities_name ON entities(name)")

    # tags — doc-level, normalised
    op.execute("CREATE TABLE tags (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE)")
    op.execute(
        """
        CREATE TABLE doc_tags (
            doc_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
            tag_id INTEGER NOT NULL REFERENCES tags(id)      ON DELETE CASCADE,
            PRIMARY KEY (doc_id, tag_id)
        )
        """
    )


def downgrade() -> None:
    for table in (
        "doc_tags",
        "tags",
        "entities",
        "proposition_vectors",
        "propositions",
        "chunk_vectors",
        "chunks",
        "section_vectors",
        "sections",
        "documents",
    ):
        op.execute(f"DROP TABLE IF EXISTS {table}")
