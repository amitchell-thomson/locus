"""reading_targets: track ink and folder after acceptance, so marks are actually captured

Revision ID: 0022
Revises: 0021
Create Date: 2026-08-01

THE HOLE THIS CLOSES, spotted by the owner: accepting a paper ingested it and then the system
stopped watching. `watch.scan` only ever revisits proposals at `status='proposed'`, so once a
paper was moved out of `Proposed` and ingested it left the loop entirely. He could read it,
underline half of it, write in the margins, move it to `Finished` — and none of that reached the
corpus, because `locus annotate` has to be invoked by hand with a device path and nothing invoked
it.

That is the wrong half of the loop to drop. The whole argument for delivering a real PDF rather
than a link is that he marks it up, and the marks are the highest-precision signal the system has:
they are what seeds the next round of concept searches (`discover/queries.reading_terms`) and what
becomes an `idea` object. A reading loop that ingests the paper but discards the reading is barely
better than a bookmark.

  `stroke_fingerprint`  `rmdoc.ink_hash` of the ink last seen. Compositing is not byte-reproducible
                        so a file hash would report "changed" every run and re-pay a billed
                        transcription pass each time; the ink is what the guard actually means.
  `device_folder`       where it now sits. `Finished` is a genuine signal — he read it through —
                        and is worth distinguishing from a paper still sitting in `In-Progress`.
  `last_swept`          when the ink was last checked, so a sweep can prioritise and so a stalled
                        one is visible rather than silent.
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0022"
down_revision: Union[str, None] = "0021"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.execute("ALTER TABLE reading_targets ADD COLUMN stroke_fingerprint TEXT")
    op.execute("ALTER TABLE reading_targets ADD COLUMN device_folder TEXT")
    op.execute("ALTER TABLE reading_targets ADD COLUMN last_swept TEXT")
    op.execute("ALTER TABLE reading_targets ADD COLUMN marks INTEGER NOT NULL DEFAULT 0")


def downgrade() -> None:
    # Columns left in place; SQLite drops are awkward and the data is inert without the sweep.
    pass
