"""figures as a first-class retrieval unit (CLAUDE.md §15.1, plan step 11)

Revision ID: 0007
Revises: 0006
Create Date: 2026-06-06

Figures (PDF raster images, vector diagrams, rendered slides) mirror the propositions
pattern (decision A): their VLM description + caption is embedded and directly searchable,
so a "block diagram of the closed-loop system" query can match the figure itself rather
than hoping nearby prose mentions it. The image file lives in the flat raw store
(`raw_path`); the DB stores text + provenance only.

`section_id` is nullable: a figure whose page could not be mapped to a section is still
retrievable (doc-level provenance suffices for citation).

Hand-authored with op.execute(raw SQL) because figure_vectors is a sqlite-vec vec0 virtual
table, which autogenerate does not model (same as 0001).
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0007"
down_revision: Union[str, None] = "0006"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.execute(
        """
        CREATE TABLE figures (
            id          INTEGER PRIMARY KEY,
            section_id  INTEGER          REFERENCES sections(id)  ON DELETE CASCADE,
            doc_id      INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
            position    INTEGER NOT NULL,
            page        INTEGER NOT NULL,
            kind        TEXT    NOT NULL CHECK (kind IN ('raster','vector','slide')),
            caption     TEXT,
            description TEXT,
            raw_path    TEXT    NOT NULL,
            embed_model TEXT    NOT NULL,
            UNIQUE (doc_id, position)
        )
        """
    )
    op.execute(
        "CREATE VIRTUAL TABLE figure_vectors USING vec0("
        " figure_id INTEGER PRIMARY KEY, embedding FLOAT[768])"
    )


def downgrade() -> None:
    for table in ("figure_vectors", "figures"):
        op.execute(f"DROP TABLE IF EXISTS {table}")
