"""pdf_annotations.inserted — ink on a page he ADDED has no text beneath it, and never will

Revision ID: 0036
Revises: 0035
Create Date: 2026-09-14

The tablet lets him insert blank pages into a PDF, and he uses them: a two-page question sheet
came back with four appended pages carrying 3,334 strokes of answers. `rmdoc._page_order` now
keeps those pages instead of dropping them, so their marks reach this table for the first time.

They need a flag because of what they look like WITHOUT one. A mark on an inserted page covers
no words — there are none — so it stores with empty `covered_text` and empty `line_text`, which
is indistinguishable from the other blank mark this system knows about: a highlight drawn over a
figure. `review.MarkRow.is_blank` explains that one to the reader as "ink covering no text —
usually a figure or a bracket". Told that about a page of handwritten answers, a reader would
conclude the opposite of the truth. The flag is what lets the register say which it is.

Not derivable after the fact. "Is document position 4 backed by a PDF page" is a question only
the `.content` manifest can answer, and the manifest is in a bundle on the reMarkable cloud, not
in this database. Recording it at sweep time is the only honest option; inferring it later from
an empty `covered_text` is the guess this column exists to avoid.

Existing rows default to 0, which is correct for every one of them: the three annotated
documents in the corpus on 2026-09-14 were checked and none has an inserted page.
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0036"
down_revision: Union[str, None] = "0035"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.execute(
        "ALTER TABLE pdf_annotations ADD COLUMN inserted INTEGER NOT NULL DEFAULT 0"
    )


def downgrade() -> None:
    op.execute("ALTER TABLE pdf_annotations DROP COLUMN inserted")
