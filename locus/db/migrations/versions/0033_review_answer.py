"""review_schedule.answer — the answer to THE QUESTION, not the sentence it grew from

Revision ID: 0033
Revises: 0032
Create Date: 2026-08-03

THE DEFECT, found on the page (2026-08-03). Recall page 3 asked:

    "Why might higher volatility - which causes Bollinger Bands to expand - simultaneously
     reduce the reliability of using band touches as mean-reversion signals?"

and page 4 answered:

    "In Task B, a model receives a sliding window of 30 daily OHLCV data points, together with
     pre-computed values of RSI (14-period), MACD (12/26/9)..."

which is not an answer to that question. R3 and R4 printed the SAME text, because both concepts
occur in one section and `concept_answer` returned `ORDER BY LENGTH(text) DESC LIMIT 1` — the
longest sentence near the concept, chosen without ever seeing the question.

This was coherent in the original design, where the prompt WAS the proposition: question and
answer were the same object, so they could not disagree. When questions became model-written
concept questions (§26 era), the two were produced by different mechanisms and silently stopped
matching. Nothing tied them together, and nothing could have noticed — the page renders, the
tests pass, and only turning the page reveals it.

So the answer is now written WITH the question, from the same evidence, in the same call, and
stored here. Mismatch stops being a thing that can happen rather than a thing to be checked for.
`grounded` records whether the stored answer cleared `ingest.summarize.is_grounded` against the
propositions it was shown; an ungrounded answer is never stored, and a row with no answer falls
back to the old proposition behaviour, so pre-existing rows degrade instead of breaking.
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0033"
down_revision: Union[str, None] = "0032"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.execute("ALTER TABLE review_schedule ADD COLUMN answer TEXT")
    # The document the evidence came from, so page 4 can still attribute the answer.
    op.execute("ALTER TABLE review_schedule ADD COLUMN answer_source TEXT")


def downgrade() -> None:
    # SQLite cannot drop a column without a table rebuild; both are nullable and unread by older
    # code, so leaving them is the safe downgrade.
    pass
