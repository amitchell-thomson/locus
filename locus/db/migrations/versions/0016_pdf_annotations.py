"""pdf_annotations (Loop B) + the `idea` object type

Revision ID: 0016
Revises: 0015
Create Date: 2026-07-30

TWO CHANGES, both from the same session's work on reading annotations.

`pdf_annotations` — one row per mark the owner made on a PDF. NOT the `annotations` table:
that one belongs to the daily page and is keyed `(page_date, anchor)`, which cannot express a
region on page 71 of a book. The plan had earmarked the name for Loop B and the daily page took
it first; rather than overload one table with two unrelated identities, Loop B gets its own.

Idempotency is `(source_uri, pdf_page, bbox_key)`. Re-reading the same `.rmdoc` must revise a
mark, never duplicate it, and the owner's ink does not move between syncs — so the rounded
bounding box is a stable identity for "this mark". Rounding to whole points absorbs the
float noise of the coordinate transform without merging genuinely separate marks.

`covered_text` is stored because it is expensive to recompute (it needs the source PDF, the
stroke geometry and the text layer) and because it is the evidence for anything derived from
the mark. Grounded-or-silent applies here as everywhere: a note claiming he marked a passage
must be able to show the passage.

THE `idea` OBJECT TYPE. `objects.type` was CHECKed to (project|concept|question|reading), and
none of them fits the thing reading actually produces. A `question` is something he does not
know; a `concept` is something that exists; a `project` is something he is building. An IDEA is
something he MIGHT build — half-formed, worth keeping, not yet a commitment. Marginalia is
where those come from ("this factor idea applies to the regime project"), so Loop B without an
`idea` type would have nowhere to put its most valuable output.

SQLite cannot ALTER a CHECK constraint, so `objects` is rebuilt. Every column, row, and the
foreign key from `object_links` are preserved; `object_links` is untouched because its FK
targets `objects(id)` and the ids are carried over unchanged.
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0016"
down_revision: Union[str, None] = "0015"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.execute(
        """
        CREATE TABLE IF NOT EXISTS pdf_annotations (
            id            INTEGER PRIMARY KEY,
            source_uri    TEXT NOT NULL,
            doc_uuid      TEXT,
            pdf_page      INTEGER NOT NULL,
            kind          TEXT NOT NULL,
            bbox_key      TEXT NOT NULL,
            bbox          TEXT,
            covered_text  TEXT,
            line_text     TEXT,
            in_margin     INTEGER NOT NULL DEFAULT 0,
            stroke_count  INTEGER,
            point_count   INTEGER,
            note          TEXT,
            object_id     INTEGER,
            source_run    INTEGER,
            captured_at   TEXT NOT NULL,
            UNIQUE(source_uri, pdf_page, bbox_key)
        )
        """
    )
    op.execute(
        "CREATE INDEX IF NOT EXISTS idx_pdf_annotations_doc "
        "ON pdf_annotations(source_uri, pdf_page)"
    )

    # --- widen objects.type to include 'idea' (SQLite: rebuild) ---
    op.execute("PRAGMA foreign_keys=OFF")
    op.execute(
        """
        CREATE TABLE objects_new (
            id         INTEGER PRIMARY KEY,
            type       TEXT NOT NULL
                       CHECK (type IN ('project','concept','question','reading','idea')),
            title      TEXT NOT NULL,
            status     TEXT NOT NULL DEFAULT 'proposed'
                       CHECK (status IN ('proposed','active','archived')),
            maturity   TEXT NOT NULL DEFAULT 'rough'
                       CHECK (maturity IN ('rough','tidy')),
            body       TEXT NOT NULL DEFAULT '{}',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            source_run INTEGER,
            UNIQUE(type, title)
        )
        """
    )
    op.execute(
        "INSERT INTO objects_new (id, type, title, status, maturity, body, created_at, "
        "updated_at, source_run) SELECT id, type, title, status, maturity, body, created_at, "
        "updated_at, source_run FROM objects"
    )
    op.execute("DROP TABLE objects")
    op.execute("ALTER TABLE objects_new RENAME TO objects")
    op.execute("PRAGMA foreign_keys=ON")


def downgrade() -> None:
    op.execute("DROP INDEX IF EXISTS idx_pdf_annotations_doc")
    op.execute("DROP TABLE IF EXISTS pdf_annotations")
    # The CHECK widening is not narrowed back: any 'idea' row would be destroyed by it.
