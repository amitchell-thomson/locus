"""belief_positions.source_uri: stable provenance that survives a re-ingest

Revision ID: 0012
Revises: 0011
Create Date: 2026-07-29

0011 recorded a position's provenance as `source_doc_id` and deliberately made it NOT a foreign
key, so that "a recorded belief survives its source being re-ingested". It does survive — but as a
DANGLING POINTER, which is only half the requirement.

`notes_sync` replaces a changed note by DELETING its document row and inserting a new one with a
fresh id (replace-by-path). So editing a note at all — even its frontmatter — orphans every belief
position taken from it. Observed live on 2026-07-29: correcting the capture dates on 12 handwriting
notes re-ingested them as documents 477-488 and left both handwriting-derived positions pointing at
deleted ids 472 and 476. The stance and its date survived; where it came from did not, and a
trajectory that cannot say which note a view came from has lost half its worth.

`source_uri` is STABLE across re-ingest — it is the note's path, which replace-by-path preserves.
This is the same reasoning that made `object_links.target_key` a stable string rather than a row id
(0011), and objects came through the same re-ingest unharmed because of it. Positions should have
had that treatment from the start; this migration corrects the oversight.

`source_doc_id` is KEPT: it is the fast join for the common case where the document has not been
replaced. Consumers prefer `source_uri` and fall back to it.

Backfill: existing rows take the source_uri of their document where that document still exists;
already-orphaned rows keep NULL, because inventing a provenance we cannot verify is worse than
admitting we lost it.
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0012"
down_revision: Union[str, None] = "0011"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.execute("ALTER TABLE belief_positions ADD COLUMN source_uri TEXT")
    # Backfill from the live join while it still resolves.
    op.execute(
        """
        UPDATE belief_positions
           SET source_uri = (SELECT d.source_uri FROM documents d WHERE d.id = source_doc_id)
         WHERE source_doc_id IS NOT NULL
        """
    )
    # The trajectory reads by subject; provenance lookups go the other way (which positions came
    # from this note?) when a note is re-ingested or deleted.
    op.execute("CREATE INDEX idx_belief_source_uri ON belief_positions(source_uri)")


def downgrade() -> None:
    op.execute("DROP INDEX IF EXISTS idx_belief_source_uri")
    # SQLite supports DROP COLUMN from 3.35; the shipped runtime is well past that.
    op.execute("ALTER TABLE belief_positions DROP COLUMN source_uri")
