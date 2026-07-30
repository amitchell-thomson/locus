"""annotations.mark: distinguish a tick from a cross

Revision ID: 0015
Revises: 0014
Create Date: 2026-07-30

0013 recorded only `ticked` — a boolean for "is there a mark in the box". The extraction prompt
that fed it said "true if there is a tick/CROSS/mark inside it", meaning *any* mark counts as
affirmative. That is precisely backwards from how a person uses a checkbox: crossing something
out means NO, and Locus read it as yes and blessed the object.

There was also no way to refuse at all. The four outcomes were bless / correct+bless /
correct-and-stay-proposed / no-op, and none of them reached `archived`, so an object the owner
did not want was re-offered forever — it merely rotated to the back of the oldest-first queue.

`mark` carries the SHAPE of the mark ('tick' | 'cross' | 'none'), which is what lets a cross
mean "archive this, stop offering it" while an empty box keeps meaning "not today". `ticked`
stays as the derived affirmative flag so existing rows and readers are unaffected.

Nullable and additive: rows written before this migration have no recorded shape, and their
`ticked` value still says whether a mark was present.
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0015"
down_revision: Union[str, None] = "0014"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.execute("ALTER TABLE annotations ADD COLUMN mark TEXT")


def downgrade() -> None:
    # SQLite pre-3.35 cannot DROP COLUMN; the column is additive and harmless to leave.
    pass
