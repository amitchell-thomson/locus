"""pass-output cache for repo re-ingest (build step 10)

Revision ID: 0005
Revises: 0004
Create Date: 2026-06-05

Commit-triggered repo sync re-ingests a whole repo per commit, but most files are
unchanged. The cache stores LLM pass outputs keyed by *content* —
sha256(file_blob_sha + pass_kind + ingest_model + PROMPT_VERSION) — so unchanged files
skip their LLM calls on re-ingest. Content-keyed means it survives delete_document and
is shared across doc rebuilds. It is an optimization table, not corpus data: rows are
written outside the per-document transaction and can be dropped at any time.
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0005"
down_revision: Union[str, None] = "0004"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.execute(
        """
        CREATE TABLE pass_cache (
            key        TEXT PRIMARY KEY,
            payload    TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        )
        """
    )


def downgrade() -> None:
    op.execute("DROP TABLE pass_cache")
