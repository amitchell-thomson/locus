"""answer_attempts — stop a question the corpus cannot answer from being retried forever

Revision ID: 0035
Revises: 0034
Create Date: 2026-08-06

`learn/answers.pending_questions` selects marks with no row in `mark_answers`, and
grounded-or-silent means a question the corpus cannot answer stores NOTHING. Those two correct
rules compose into a loop: the mark stays selectable, is re-attempted every night, fails the same
way, and stores nothing again. Mark 11 ("is a factor like a feature in ML? is performance of a
factor just how factor changes...") has done exactly that every night since 2026-08-04 — it is the
only sample the `answers.grounded` gate has recorded rejecting on four separate days — and it
takes one of the four nightly answer slots with it.

That is the same shape as the two starvation bugs fixed earlier today: a permanent occupant at the
head of a bounded queue. The counter below ends it. After `_MAX_ANSWER_ATTEMPTS` failures the mark
stops being offered, and because the count is stored rather than inferred, `locus status` can say
how many questions are parked and why — the alternative is silence, which is how a question he
asked would simply stop existing.

NOT A DISMISSAL. Nothing is deleted and no answer is invented. New evidence arrives constantly
(every ingest, every `locus link`), so a parked question is a candidate for re-attempt, which is
what `locus review --answer-marks --retry-parked` is for: reset the counters and let the pass try
again against the corpus as it now stands.
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0035"
down_revision: Union[str, None] = "0034"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    # SQLite ALTER TABLE ADD COLUMN is safe and rewrites nothing; the default keeps every existing
    # mark eligible, so this migration cannot retire a question by arriving.
    op.execute("ALTER TABLE pdf_annotations ADD COLUMN answer_attempts INTEGER NOT NULL DEFAULT 0")


def downgrade() -> None:
    op.execute("ALTER TABLE pdf_annotations DROP COLUMN answer_attempts")
