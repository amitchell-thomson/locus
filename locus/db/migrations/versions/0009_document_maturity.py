"""documents.maturity: rough|tidy capture-maturity tag (agent-layer plan §6.1, Phase 1)

Revision ID: 0009
Revises: 0008
Create Date: 2026-07-28

The capture layer (agent-layer plan) ingests handwriting and conversations as `rough` notes —
freeform, un-curated, high-noise. Retrieval DOWN-WEIGHTS rough units (a penalty on the
cross-encoder score, retrieve/pipeline.py) so they neither drown authoritative sources nor get
buried — flag/down-weight, NEVER filter (CLAUDE.md principle 8: retrieval misses are
unrecoverable). Promotion of a note to `tidy` re-ingests it at full weight.

`tidy` is the default: every existing document, and every authoritative source (papers,
coursework, code), is full-weight. Only the capture loops write `maturity='rough'`. The column
is re-ingest-bound metadata, not derived — a document carries the maturity its ingest set.
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0009"
down_revision: Union[str, None] = "0008"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    # NOT NULL DEFAULT 'tidy' backfills every existing row to full-weight in one statement;
    # the CHECK constrains the domain the same way source_type/category are constrained.
    op.execute(
        "ALTER TABLE documents ADD COLUMN maturity TEXT NOT NULL DEFAULT 'tidy' "
        "CHECK (maturity IN ('rough','tidy'))"
    )


def downgrade() -> None:
    op.execute("ALTER TABLE documents DROP COLUMN maturity")
