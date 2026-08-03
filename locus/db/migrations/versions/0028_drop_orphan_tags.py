"""drop tags / doc_tags — schema with no code on either side

Revision ID: 0028
Revises: 0027
Create Date: 2026-08-03

Both tables have existed since the original schema and have NEVER been written or read. There is
no extractor->tags path (`extract/textdoc.py` says so in a comment, and `ingest_pipeline.py`
records that "tags are not yet produced"); `doc_tags` does not appear anywhere in `locus/` at all,
not even in a docstring. Live at 218 documents: 0 rows in each.

Dropped rather than kept "in case", because empty scaffolding is indistinguishable from a feature
that is quietly broken — which is the exact confusion the 2026-08-03 readiness audit was called to
resolve. Tagging can come back as its own migration if it is ever built, and it would want a
different shape by then anyway.

Derived data only: nothing references these tables, so this loses nothing that is not
reconstructible from the corpus.
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0028"
down_revision: Union[str, None] = "0027"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.execute("DROP TABLE IF EXISTS doc_tags")
    op.execute("DROP TABLE IF EXISTS tags")


def downgrade() -> None:
    op.execute(
        """
        CREATE TABLE IF NOT EXISTS tags (
            id   INTEGER PRIMARY KEY,
            name TEXT NOT NULL UNIQUE
        )
        """
    )
    op.execute(
        """
        CREATE TABLE IF NOT EXISTS doc_tags (
            doc_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
            tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
            PRIMARY KEY (doc_id, tag_id)
        )
        """
    )
