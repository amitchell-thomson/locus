"""gate_log — what each threshold silently rejected, so a dead gate can be seen

Revision ID: 0031
Revises: 0030
Create Date: 2026-08-03

THE FAILURE THIS EXISTS FOR. `evolve/trajectory.find_tensions` was documented as the headline
capability, had tests either side of it, and could not possibly fire: `_MAX_DISTANCE` was 0.55,
so the only claims it ever saw were paraphrases of the owner's own view. Nothing was wrong with
the code. The constant simply admitted nothing, and a threshold that admits nothing looks exactly
like a subject with nothing to say.

That class cannot be caught by tests (a fixture is built to pass the gate) or by reading code (the
number looks reasonable). It is only visible in the REJECTS, and nothing recorded them.

WHAT THIS IS NOT. Not a metrics system and not a debug log. It answers one question per gate —
"over the last week, what did you throw away, and does that look right?" — which is why it stores
an aggregate plus a handful of verbatim samples rather than a row per rejection. The retrieval
floor alone rejects thousands of candidates a day; a row each would make the table the biggest in
the database and the question no easier to answer.

Keyed (gate, day) so a week is seven rows per gate. `samples` is a JSON array capped by the
recorder — the first N distinct values seen that day, which is enough to judge a gate by eye.
`passed` is counted alongside `rejected` because a rejection count on its own cannot distinguish
a gate that is working hard from one that is rejecting everything.

Derived and disposable (principle 9): DELETE the table's contents and nothing is lost but the
window. Never read by any pass — only by `locus gates`.
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0031"
down_revision: Union[str, None] = "0030"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.execute(
        """
        CREATE TABLE IF NOT EXISTS gate_log (
            id         INTEGER PRIMARY KEY,
            gate       TEXT NOT NULL,   -- stable dotted name, e.g. 'trajectory.max_distance'
            day        TEXT NOT NULL,   -- UTC date, so a week is seven rows per gate
            rejected   INTEGER NOT NULL DEFAULT 0,
            passed     INTEGER NOT NULL DEFAULT 0,
            samples    TEXT NOT NULL DEFAULT '[]',  -- JSON array of rejected values, capped
            updated_at TEXT NOT NULL,
            UNIQUE (gate, day)
        )
        """
    )
    op.execute("CREATE INDEX IF NOT EXISTS idx_gate_log_day ON gate_log(day, gate)")


def downgrade() -> None:
    op.execute("DROP INDEX IF EXISTS idx_gate_log_day")
    op.execute("DROP TABLE IF EXISTS gate_log")
