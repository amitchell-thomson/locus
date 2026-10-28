"""model_calls — one row per headless Claude call, for the budget (Slice 0.7)

Revision ID: 0039
Revises: 0038
Create Date: 2026-10-28

Since Slice 0.13 every model call in Locus runs on the subscription through `agent/claude.py`,
and the plan gives background work about 15% of the week. Nothing counted it: the old ledger
summed a notional dollar figure per journaled run, which says nothing about tokens, time, or
which job spent them. Written by the runner itself, so a call made outside a journaled run
(a chat tool, a one-off command) is counted too, and a call that FAILED is counted, since a
retry loop that fails is exactly the spend nobody sees.

`cost_usd` is the envelope's notional API-equivalent figure, kept because it is the one number
that weighs models against each other; it is not money spent.
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0039"
down_revision: Union[str, None] = "0038"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.execute(
        """
        CREATE TABLE model_calls (
            id              INTEGER PRIMARY KEY,
            at              TEXT NOT NULL,
            job             TEXT NOT NULL,
            model           TEXT NOT NULL DEFAULT '',
            ok              INTEGER NOT NULL,
            duration_ms     INTEGER NOT NULL DEFAULT 0,
            input_tokens    INTEGER NOT NULL DEFAULT 0,
            cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
            cache_read_tokens     INTEGER NOT NULL DEFAULT 0,
            output_tokens   INTEGER NOT NULL DEFAULT 0,
            thinking_tokens INTEGER NOT NULL DEFAULT 0,
            images          INTEGER NOT NULL DEFAULT 0,
            cost_usd        REAL NOT NULL DEFAULT 0,
            error           TEXT
        )
        """
    )
    op.execute("CREATE INDEX ix_model_calls_at ON model_calls(at)")


def downgrade() -> None:
    op.execute("DROP TABLE model_calls")
