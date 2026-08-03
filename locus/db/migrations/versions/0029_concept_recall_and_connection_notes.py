"""concept-based recall + stored connection prose

Revision ID: 0029
Revises: 0028
Create Date: 2026-08-03

Two changes, both from the owner's review of the first real daily pages.

**`review_schedule.prompt_kind` gains `concept`.** Recall enrolled PROPOSITIONS from blessed
objects, so the questions were project-status facts asked back verbatim — "What analytical
capabilities does the tanker-flow project provide for vessel activity analysis?" — and 29 of 40
scheduled items came from three documents. His verdict: "way too broad and just regurgitating
material that I may have read verbatim - not actually useful". What he wants is the concept
itself: "how is covariance different to correlation", "what is covered interest rate parity",
"how does a hidden markov model work". Those are questions about a CONCEPT, not about a document,
so the schedule has to be able to point at one.

**`connection_notes`** stores the written reason two documents belong together. The daily page is
aggregate-only by design (§18) — it must render whether or not last night's runs succeeded — so
prose it prints has to be composed earlier and stored, exactly as `discover/why.py` already does
for reading proposals. The deterministic phrasing ("both develop regime detection") was the thing
he called obscure; what he asked for is "this paper discusses X this way, your note talks about it
that way, could you use the paper's methods to improve the project" — which no join can produce.

Keyed on the ORDERED document pair plus the shared concept, because the same two documents can be
worth connecting for two different reasons, and the prose is about the reason.
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0029"
down_revision: Union[str, None] = "0028"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    # --- review_schedule: widen the prompt_kind CHECK (SQLite needs a table rebuild) ----------
    op.execute(
        """
        CREATE TABLE review_schedule_new (
            id           INTEGER PRIMARY KEY,
            prompt_kind  TEXT    NOT NULL CHECK (prompt_kind IN ('proposition','object','concept')),
            prompt_ref   TEXT    NOT NULL,
            due          TEXT    NOT NULL,
            ease         REAL    NOT NULL DEFAULT 2.5,
            interval     INTEGER NOT NULL DEFAULT 0,
            reps         INTEGER NOT NULL DEFAULT 0,
            last_grade   INTEGER,
            last_review  TEXT,
            question     TEXT,
            UNIQUE (prompt_kind, prompt_ref)
        )
        """
    )
    op.execute(
        "INSERT INTO review_schedule_new (id, prompt_kind, prompt_ref, due, ease, interval, "
        "reps, last_grade, last_review, question) "
        "SELECT id, prompt_kind, prompt_ref, due, ease, interval, reps, last_grade, "
        "last_review, question FROM review_schedule"
    )
    op.execute("DROP TABLE review_schedule")
    op.execute("ALTER TABLE review_schedule_new RENAME TO review_schedule")

    # --- connection_notes: the written reason two documents belong together ------------------
    op.execute(
        """
        CREATE TABLE IF NOT EXISTS connection_notes (
            id          INTEGER PRIMARY KEY,
            src_uri     TEXT NOT NULL,
            other_uri   TEXT NOT NULL,
            shared      TEXT NOT NULL,   -- the canonical concept both develop
            prose       TEXT NOT NULL,   -- what to DO about it, written overnight
            written_at  TEXT NOT NULL,
            source_run  INTEGER REFERENCES agent_runs(id) ON DELETE SET NULL,
            UNIQUE (src_uri, other_uri, shared)
        )
        """
    )


def downgrade() -> None:
    op.execute("DROP TABLE IF EXISTS connection_notes")
    op.execute("DELETE FROM review_schedule WHERE prompt_kind='concept'")
