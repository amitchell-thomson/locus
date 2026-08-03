"""documents.structured_at — which documents the structurer has already looked at

Revision ID: 0027
Revises: 0026
Create Date: 2026-08-03

THE FAILURE THIS CLOSES. `locus-maintain` runs `locus structure --ingested-since "2 days ago"`,
and there was no record anywhere of what had actually been structured. A window is not a ledger:
a document that arrives while the nightly run is broken — and `locus-maintain` was broken for six
consecutive nights (§22) — falls out of the window two days later and is then skipped FOREVER,
silently, with nothing able to name which documents were lost.

Found live at 218 documents: doc 482, his own handwritten `Optimisation` note, was ingested
2026-07-29 09:57 and had produced no objects at all. A dry run proved it had two concepts to
give; it had simply never been offered to the structurer, and nothing could have said so.

`structured_at` is stamped whenever a document is PLANNED, including when the plan proposes
nothing. That is the important case: "the structurer looked and found nothing" and "the
structurer never looked" are different facts, and a ledger that only records successes would
retry the empty ones every night forever, re-billing each time.

NULL means never structured. The nightly run selects on that instead of on a date window, so a
missed night is caught up rather than lost. Existing rows are left NULL deliberately — the
honest state is "unknown, treat as not done", and the alternative (back-stamping everything)
would bake in exactly the silent loss this exists to prevent.
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0027"
down_revision: Union[str, None] = "0026"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def _columns() -> set[str]:
    bind = op.get_bind()
    return {row[1] for row in bind.exec_driver_sql("PRAGMA table_info(documents)")}


def upgrade() -> None:
    if "structured_at" not in _columns():
        op.execute("ALTER TABLE documents ADD COLUMN structured_at TEXT")
    op.execute(
        "CREATE INDEX IF NOT EXISTS idx_documents_structured_at "
        "ON documents(structured_at, ingested_at)"
    )


def downgrade() -> None:
    op.execute("DROP INDEX IF EXISTS idx_documents_structured_at")
    # SQLite pre-3.35 cannot DROP COLUMN; the column is nullable and inert, so it is left.
