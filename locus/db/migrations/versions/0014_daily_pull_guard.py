"""daily_pages.pulled_hash / pulled_at: don't pay for the same scan twice

Revision ID: 0014
Revises: 0013
Create Date: 2026-07-30

The device pushes EVERY changed document to the staging dir, and the pull-back timer runs on a
schedule, so the same annotated page is offered for reading again and again. Routing is already
idempotent (0013's UNIQUE(page_date, anchor) plus the side-effect guard), but extraction is not
free — it is one vision call per page, billed, every time.

`pulled_hash` is the SHA-256 of the PDF bytes the last successful pull read. An unchanged file
is skipped before the model is called, so a pull timer costs nothing on the days the owner did
not write, and costs one call on the days he did. This is the same hash-idempotency Loop A uses
for handwriting captures, applied to the page's return leg.

Nullable and additive: a page built before this migration simply has no recorded hash, so its
first pull reads it once and records one.
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0014"
down_revision: Union[str, None] = "0013"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.execute("ALTER TABLE daily_pages ADD COLUMN pulled_hash TEXT")
    op.execute("ALTER TABLE daily_pages ADD COLUMN pulled_at TEXT")


def downgrade() -> None:
    # SQLite pre-3.35 cannot DROP COLUMN; these are additive and harmless to leave.
    pass
