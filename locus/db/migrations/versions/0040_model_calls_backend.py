"""model_calls.backend — which route a call took: the Max API credit or the subscription

Revision ID: 0040
Revises: 0039
Create Date: 2026-11-03

Calls go to the API credit first and fall back to the subscription when it refuses (3 Nov,
agent/claude.py). Without the route on each row, `locus budget` could not say how much of the
month's credit is gone, nor show that a fallback happened at all. Earlier rows were all made on
the subscription (Slice 0.13 to 3 Nov), which is the default.
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0040"
down_revision: Union[str, None] = "0039"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.execute(
        "ALTER TABLE model_calls ADD COLUMN backend TEXT NOT NULL DEFAULT 'subscription'"
    )


def downgrade() -> None:
    op.execute("ALTER TABLE model_calls DROP COLUMN backend")
