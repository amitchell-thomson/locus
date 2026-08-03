"""belief_tensions — stored contradictions, so the page can offer one

Revision ID: 0030
Revises: 0029
Create Date: 2026-08-03

The owner's request: the Think page should include something that "tells you when you're wrong".
`evolve/trajectory.find_tensions` is that pass and has existed since Phase 2 — it was inert (see
the note on `_MAX_DISTANCE`) and, more to the point, it calls a model, which page composition may
never do (§18: the page must render whether or not last night's runs succeeded).

So tensions are written overnight and stored here, exactly as `connection_notes` stores the
written reason two documents belong together. The page prints; it does not think.

Keyed on the POSITION plus the claim it conflicts with, because one stance can be in tension with
more than one stored claim and each is a separate thing to react to. `dismissed_at` exists so a
tension he has judged and rejected is not offered again — the equivalent of the acceptance
surfaces elsewhere, kept local because a tension is advisory and its dismissal means only "not
this one", never "stop looking".
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0030"
down_revision: Union[str, None] = "0029"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.execute(
        """
        CREATE TABLE IF NOT EXISTS belief_tensions (
            id             INTEGER PRIMARY KEY,
            position_id    INTEGER NOT NULL REFERENCES belief_positions(id) ON DELETE CASCADE,
            stance         TEXT NOT NULL,   -- copied, so the row survives a position rewrite
            conflicts_with TEXT NOT NULL,   -- the stored claim, verbatim (grounded-or-silent)
            reason         TEXT NOT NULL,
            source         TEXT,            -- document title the claim came from
            written_at     TEXT NOT NULL,
            dismissed_at   TEXT,
            source_run     INTEGER REFERENCES agent_runs(id) ON DELETE SET NULL,
            UNIQUE (position_id, conflicts_with)
        )
        """
    )
    op.execute(
        "CREATE INDEX IF NOT EXISTS idx_belief_tensions_open "
        "ON belief_tensions(dismissed_at, written_at)"
    )


def downgrade() -> None:
    op.execute("DROP TABLE IF EXISTS belief_tensions")
