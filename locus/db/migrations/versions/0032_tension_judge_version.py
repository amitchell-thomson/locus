"""belief_tensions.judge_version — a cached "no" must not outlive the judge that said it

Revision ID: 0032
Revises: 0031
Create Date: 2026-08-03

`store_tensions` caches a verdict so the nightly pass does not re-pay for the same "no", and
re-judges only when a NEW DOCUMENT has arrived since — because a contradiction can only be found
against material that exists.

That is right about the corpus and silent about the JUDGE. On 2026-08-03 the tension prompt was
found to be over-tuned toward silence ("most of the time the correct answer is an empty list") and
was returning 0 tensions from 16 positions, while position 5's own neighbour list contained a
claim that plainly conflicted with it. The prompt was rebalanced and immediately found 2 — but
every position already carried a "judged, none found" marker, and the last ingest predated those
markers, so the improved judge would never have run. The fix would have shipped, passed its tests,
and changed nothing on the page: the same class of silent-inert failure it was fixing.

So the cache is now keyed on the judge as well as the corpus. A marker written by a different
`_JUDGE_VERSION` — or by none, which is every row that existed before this migration — is stale,
and the position is re-judged. Bump the constant whenever the prompt or the neighbour rule
changes; that is the whole protocol.
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0032"
down_revision: Union[str, None] = "0031"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    # Nullable on purpose: NULL means "judged by a prompt we can no longer identify", which is
    # exactly the condition that must trigger a re-judge.
    op.execute("ALTER TABLE belief_tensions ADD COLUMN judge_version TEXT")


def downgrade() -> None:
    # SQLite cannot drop a column without a table rebuild; the column is nullable and unread by
    # older code, so leaving it is the safe downgrade.
    pass
