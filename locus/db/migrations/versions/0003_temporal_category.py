"""temporal + category metadata (build step 1, CLAUDE.md §15.4/§16) [re-ingest-bound]

Personal/historical content has meaningful *dates* (when an achievement happened, when a
project ran) and *kinds* (paper / project / achievement / note / cv / code / video). Add
`documents.source_date` (ISO 'YYYY-MM-DD', from the source's embedded date with a file-mtime
fallback) and `documents.category` (drop-folder convention / heuristic) so temporal + category
retrieval facets work. Indexed because the facets filter on them.

Re-ingest-bound: existing rows keep NULL for both (a NULL date is simply excluded by a
`--since/--until` filter, which is the correct semantics for an unknown date). A `--reingest`
repopulates them from the source; this migration deliberately does not guess historical dates.
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0003"
down_revision: Union[str, None] = "0002"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.execute("ALTER TABLE documents ADD COLUMN source_date TEXT")  # ISO 'YYYY-MM-DD'
    op.execute("ALTER TABLE documents ADD COLUMN category TEXT")
    op.execute("CREATE INDEX idx_documents_source_date ON documents(source_date)")
    op.execute("CREATE INDEX idx_documents_category ON documents(category)")


def downgrade() -> None:
    op.execute("DROP INDEX IF EXISTS idx_documents_category")
    op.execute("DROP INDEX IF EXISTS idx_documents_source_date")
    # SQLite (3.35+) supports DROP COLUMN directly.
    op.execute("ALTER TABLE documents DROP COLUMN category")
    op.execute("ALTER TABLE documents DROP COLUMN source_date")
