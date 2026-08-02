"""daily_shown + read_at + a written reason: the four-page daily page

Revision ID: 0023
Revises: 0022
Create Date: 2026-08-02

Step 2 of `docs/daily-use-refinement-plan.md`. Three additions, each answering something the
owner said in the requirements interview.

  `daily_shown`                THE NO-REPEAT LEDGER. "I dont want to see the same thing twice on
                               different daily pages, even if I missed one." A page is now built
                               every morning regardless of whether the last one was read, so
                               without this a skipped day would re-offer its whole contents.

                               `item_key` carries the item's VERSION, not just its identity —
                               `object:41:2026-08-02T09:11` rather than `object:41`. That is what
                               makes the rule mean the right thing in both directions: a thread he
                               DEVELOPED has changed and is worth showing again, while an
                               untouched one is the literal repeat he objected to. Same for a
                               re-scheduled recall (a new `due` is a new key, which is exactly
                               what spaced repetition is) and for a proposal whose `why` has been
                               rewritten.

  `daily_pages.read_at`        WHEN INK FIRST APPEARED on the page. Drives the /Daily inbox: a
                               page stays loose at the root until it has been written on, then
                               archives to /Daily/YYYY-MM. Opening the folder is then a true
                               statement of what he has not been through — a state he can see
                               rather than a count we would have to print at him.

                               Distinct from the existing `pulled_at`/`pulled_hash`, which record
                               when we last LOOKED. A page pulled ten times and never written on
                               has ten pulls and no `read_at`.

  `reading_proposals.why_long` The written reason a paper was proposed — what in his project it
  `.why_written_at`            bears on and what he could do with it — composed once at proposal
                               time and stored, so the page stays aggregate-only and renders
                               identically whether or not last night's model run succeeded.
                               `why_written_at` drives the 7-day rewrite: a proposal still sitting
                               in `Proposed` a week later was justified against threads he has
                               moved past, and a stale reason is the failure mode that made the
                               old read-next slot worthless.

  `review_schedule.question`  THE RECALL PAGE HAD NO QUESTION. `resolve_prompt` returns the
                              PROPOSITION TEXT as the prompt, so what was printed under "Recall"
                              was the claim itself — he was being shown the answer and asked to
                              recall it. That is why his response to the recall section was "how
                              does it work if I answer correctly/incorrectly?": the loop had no
                              answerable step in it.

                              `learn/practice.py` already turns a proposition into a real question
                              and keeps the proposition verbatim as the reference answer, but
                              nothing persisted the question, so the schedule could not use it.
                              Storing it lets the page ask on page 3 and answer on page 4, and
                              keeps composition model-free (the generation happens overnight).
                              Nullable: with no question stored the page degrades to the old
                              behaviour and prints no answer, rather than printing an "answer"
                              identical to the prompt.

`why` keeps its NOT NULL deterministic role (the citation or gap that grounds the proposal).
`why_long` is prose ABOUT that grounding and is nullable: no model run, no prose, and the page
falls back to the deterministic line rather than showing nothing.
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0023"
down_revision: Union[str, None] = "0022"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.execute(
        """
        CREATE TABLE IF NOT EXISTS daily_shown (
            id         INTEGER PRIMARY KEY,
            item_key   TEXT NOT NULL,
            kind       TEXT NOT NULL,
            page_date  TEXT NOT NULL,
            shown_at   TEXT NOT NULL,
            UNIQUE (item_key)
        )
        """
    )
    # Lookup is always "have I shown this key before", never "what was on page N" — the page
    # itself is already recorded by `daily_anchors`.
    op.execute("CREATE INDEX IF NOT EXISTS idx_daily_shown_key ON daily_shown(item_key)")

    op.execute("ALTER TABLE daily_pages ADD COLUMN read_at TEXT")
    op.execute("ALTER TABLE reading_proposals ADD COLUMN why_long TEXT")
    op.execute("ALTER TABLE reading_proposals ADD COLUMN why_written_at TEXT")
    op.execute("ALTER TABLE review_schedule ADD COLUMN question TEXT")


def downgrade() -> None:
    # `daily_shown` is dropped because it is pure bookkeeping; the columns stay, since SQLite
    # drops are awkward and both are inert without the code that reads them.
    op.execute("DROP TABLE IF EXISTS daily_shown")
