"""timer_failures — a run that dies before it can journal itself

Revision ID: 0026
Revises: 0025
Create Date: 2026-08-02

`agent_runs` is opened by the code and closed by the code, which makes it blind to exactly the
failures that matter most: a unit whose import fails, whose binary is missing, whose config will
not load, or which the OOM killer takes. None of those reach a `with journal.run(...)`, so they
leave no row at all — and "no row" is indistinguishable from "did not run tonight".

That is not hypothetical. `locus-maintain` failed six consecutive nights (2026-08-01) and nothing
said so; it was found by accident. `systemd` knew every time.

    `unit`    the systemd unit as systemd names it (`locus-maintain.service`), because the point
              is to record what SYSTEMD saw, not what the code thinks it is called.
    `detail`  the exit status / result string, so a triage starts with a cause.

Written by `locus record-failure`, invoked from `OnFailure=` on every locus unit. Consecutive
counts are DERIVED at read time from `failed_at` against the last successful `agent_runs` row for
that kind, rather than stored: a stored counter has to be reset by something, and the something
is exactly the code path that is failing.
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0026"
down_revision: Union[str, None] = "0025"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.execute(
        """
        CREATE TABLE IF NOT EXISTS timer_failures (
            id        INTEGER PRIMARY KEY,
            unit      TEXT NOT NULL,
            failed_at TEXT NOT NULL,
            detail    TEXT
        )
        """
    )
    op.execute(
        "CREATE INDEX IF NOT EXISTS idx_timer_failures_unit ON timer_failures(unit, failed_at)"
    )


def downgrade() -> None:
    op.execute("DROP TABLE IF EXISTS timer_failures")
