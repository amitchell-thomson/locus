"""daily_pages / daily_anchors / annotations: the two-way reMarkable surface

Revision ID: 0013
Revises: 0012
Create Date: 2026-07-30

Phase 3 of the agent layer (plan §9). The daily page is the one surface the owner touches, and
it is two-way: it goes out as a PDF and comes back annotated. Three tables, each earning its
place from a property the pull-back needs.

`daily_pages` — one row per page built, keyed by its DATE. The page is the unit the owner
annotates, so the date is the natural identity; UNIQUE on it means rebuilding a day's page
(the timer fired twice, or the owner asked for a rebuild) revises that day rather than
accumulating duplicates.

`daily_anchors` — what was physically printed at each numbered region. This is the table that
makes pull-back possible at all: a handwritten answer next to "R3" means nothing unless we
recorded that R3 was review item 91 on that date. It stores a STABLE STRING key
(`target_kind`/`target_key`) rather than a row id, for the same reason `object_links` and
`belief_positions.source_uri` do (0011, 0012) — a re-ingest changes doc ids, and an anchor
that outlives its target's id is the whole point of writing it down.

`annotations` — one row per (page_date, anchor) region that came back with something in it.
**UNIQUE(page_date, anchor) is the idempotency contract the plan asks for**: re-pulling the
same page UPDATES the region's record rather than inserting a second one, so scanning a page
twice cannot double-grade a recall answer or re-bless an object. `ticked` is a nullable
tri-state (1 ticked / 0 explicitly unticked / NULL nothing there) because the four-way
blessing outcome distinguishes "ticked with no writing" from "writing but not ticked" — a
two-state boolean cannot carry that, and "wrote corrections but didn't tick" is the
interesting case (it means *keep working on this*, not yes and not no).

`outcome` records what the router DID with the region, so a re-pull is auditable and a region
whose routing failed can be retried without guessing which ones already applied.

No foreign key from `annotations` to `daily_pages`: an annotation is evidence the owner
produced, and it must survive its page row being rebuilt — the same
survives-the-source-being-replaced reasoning as 0012, learned there the hard way.
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0013"
down_revision: Union[str, None] = "0012"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.execute(
        """
        CREATE TABLE IF NOT EXISTS daily_pages (
            id          INTEGER PRIMARY KEY,
            page_date   TEXT NOT NULL UNIQUE,
            built_at    TEXT NOT NULL,
            source_run  INTEGER,
            md_path     TEXT,
            pdf_path    TEXT
        )
        """
    )
    op.execute(
        """
        CREATE TABLE IF NOT EXISTS daily_anchors (
            id          INTEGER PRIMARY KEY,
            page_date   TEXT NOT NULL,
            anchor      TEXT NOT NULL,
            kind        TEXT NOT NULL,
            target_kind TEXT NOT NULL,
            target_key  TEXT NOT NULL,
            label       TEXT,
            UNIQUE(page_date, anchor)
        )
        """
    )
    op.execute(
        "CREATE INDEX IF NOT EXISTS idx_daily_anchors_date ON daily_anchors(page_date)"
    )
    op.execute(
        """
        CREATE TABLE IF NOT EXISTS annotations (
            id           INTEGER PRIMARY KEY,
            page_date    TEXT NOT NULL,
            anchor       TEXT NOT NULL,
            ticked       INTEGER,
            text         TEXT,
            outcome      TEXT,
            source_run   INTEGER,
            captured_at  TEXT NOT NULL,
            processed_at TEXT,
            UNIQUE(page_date, anchor)
        )
        """
    )
    op.execute(
        "CREATE INDEX IF NOT EXISTS idx_annotations_date ON annotations(page_date)"
    )


def downgrade() -> None:
    op.execute("DROP INDEX IF EXISTS idx_annotations_date")
    op.execute("DROP TABLE IF EXISTS annotations")
    op.execute("DROP INDEX IF EXISTS idx_daily_anchors_date")
    op.execute("DROP TABLE IF EXISTS daily_anchors")
    op.execute("DROP TABLE IF EXISTS daily_pages")
