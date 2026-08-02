"""reading_targets: relevance for the books he chooses himself

Revision ID: 0024
Revises: 0023
Create Date: 2026-08-02

His own request, and it closes a real asymmetry: "the books I add to in progress as well should
also have relevance calculated... it is interesting to see the links of books that I find myself/
am recommended by others to my projects."

A paper the discovery loop proposed carries `evidence_key` ("project:regime-ml") and a written
reason, so the daily page can say what it bears on. A book he bought, or one a colleague told him
to read, carries nothing — it appears in `reading_targets` with `proposal_id IS NULL` and is a
bare filename. That is backwards: material he sought out himself is the material he is most
committed to, and it is exactly where a cross-domain link is most interesting, because nothing in
the pipeline chose it to be relevant.

  `title`            the human name, so the page need not parse a device path (which the 2026-08-02
                     migration has already made stale for the one owner-added book).
  `subject_kind`     what it links to: the same `(kind, label)` pair `discovery_profiles` uses,
  `subject_label`    so a book and a proposed paper resolve links the identical way.
  `fit`              the cosine behind that link, kept because it is the checkable fact under the
                     prose — and because a weak fit should be visible as weak.
  `why_long`         the written reason (`discover/why.py`), same pass, same grounding rule.
  `why_written_at`   drives the 7-day rewrite, exactly as it does for a proposal.

WHAT IT IS SCORED ON, and why this is better than an abstract. An owner-added book usually is not
in the corpus and has no abstract — but if he has been reading it, `pdf_annotations` holds the
passages he underlined. Those are a stronger relevance signal than any publisher's summary: they
are the parts HE stopped on. *Advanced Portfolio Management* has 26 marks and ~4,300 characters of
marked text, which is what the first link will actually be computed from.
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0024"
down_revision: Union[str, None] = "0023"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    for column, decl in (
        ("title", "TEXT"),
        ("subject_kind", "TEXT"),
        ("subject_label", "TEXT"),
        ("fit", "REAL"),
        ("why_long", "TEXT"),
        ("why_written_at", "TEXT"),
    ):
        op.execute(f"ALTER TABLE reading_targets ADD COLUMN {column} {decl}")


def downgrade() -> None:
    # Columns left in place: SQLite drops are awkward and all of this is derived, regenerable
    # data that is inert without the code that reads it.
    pass
