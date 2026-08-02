"""pdf_annotations.intent — what he MEANT by a mark

Revision ID: 0025
Revises: 0024
Create Date: 2026-08-02

Step 4 of `docs/daily-use-refinement-plan.md`. Asked what an underline means, he gave three
answers, not one: "something I think is important, something I dont understand, or an idea I have
linking to the content of that passage (eg. I read part of a paper that I think describes a
concept or technique that would be useful to try in one of my projects)."

They want three different fates, and until now every mark got the same one — it became search
fuel and stopped there. 26 marks have accumulated, the `idea` object type has existed since
migration 0016, and NOTHING has ever created one.

  `intent`             'important' | 'not_understood' | 'idea', or NULL before the pass has run.
  `intent_confidence`  0-1. Below `[capture].intent_confidence_floor` the mark is not acted on and
                       becomes a `locus decide` item instead — his answer was "infer, then let me
                       correct", and a low-confidence guess acted on silently is the version of
                       that with the correction step removed.
  `intent_by`          'model' | 'owner'. An owner correction must be visibly durable, so a later
                       re-run cannot quietly overwrite what he said — the same asymmetry
                       `apply_owner_edit` enforces for objects.
  `intent_at`          when it was decided, so a re-run can skip what is already settled.

`object_id` (0016) stays what it always was: the mark has become something. It is now set by the
`idea` path specifically, which is what finally makes `build_marked`'s "finished with" condition
mean something.
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0025"
down_revision: Union[str, None] = "0024"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    for column, decl in (
        ("intent", "TEXT"),
        ("intent_confidence", "REAL"),
        ("intent_by", "TEXT"),
        ("intent_at", "TEXT"),
    ):
        op.execute(f"ALTER TABLE pdf_annotations ADD COLUMN {column} {decl}")
    op.execute(
        "CREATE INDEX IF NOT EXISTS idx_pdf_annotations_intent ON pdf_annotations(intent)"
    )


def downgrade() -> None:
    op.execute("DROP INDEX IF EXISTS idx_pdf_annotations_intent")
