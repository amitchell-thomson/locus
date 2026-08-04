"""mark_answers — the questions he wrote while reading, answered

Revision ID: 0034
Revises: 0033
Create Date: 2026-08-03

Nine marks carry a written question — "what is a factor covariance estimator r = Bf + e", "how
are information ratio and sharpe ratio different?", "no momentum in Japan??" — and not one of
them ever reached him again. `intent='not_understood'` earned exactly one thing: a corpus re-read
slot on the Read page, which `_explains` correctly rejects almost always (and which the gate log
showed rejecting 30 of 30). So a question he actually asked, at the moment he was confused, with
the passage that confused him attached, was a dead end.

This is the store for the answer. Written overnight, never at page-composition time, because the
page must render whether or not last night's runs succeeded (§18) — the same contract as
`connection_notes` and `belief_tensions`. The page prints; it does not think.

`evidence` keeps the citations the answer was built from, so page 4 can attribute it and so a
stored answer can be audited later against what it was actually shown. `dismissed_at` lets him
retire one he no longer cares about without deleting the record that he asked.

An answer is written ONLY when the corpus can ground one. "The corpus cannot answer this" is a
true and useful outcome — it is what the discovery channel exists to act on — so an ungrounded
question stores nothing rather than storing a plausible invention.
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0034"
down_revision: Union[str, None] = "0033"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.execute(
        """
        CREATE TABLE IF NOT EXISTS mark_answers (
            id           INTEGER PRIMARY KEY,
            mark_id      INTEGER NOT NULL REFERENCES pdf_annotations(id) ON DELETE CASCADE,
            question     TEXT NOT NULL,   -- copied, so the row survives a re-transcription
            answer       TEXT NOT NULL,
            evidence     TEXT,            -- JSON: the citations it was grounded in
            source       TEXT,            -- document title the passage came from
            written_at   TEXT NOT NULL,
            dismissed_at TEXT,
            source_run   INTEGER REFERENCES agent_runs(id) ON DELETE SET NULL,
            UNIQUE (mark_id)
        )
        """
    )
    op.execute(
        "CREATE INDEX IF NOT EXISTS idx_mark_answers_open "
        "ON mark_answers(dismissed_at, written_at)"
    )


def downgrade() -> None:
    op.execute("DROP INDEX IF EXISTS idx_mark_answers_open")
    op.execute("DROP TABLE IF EXISTS mark_answers")
