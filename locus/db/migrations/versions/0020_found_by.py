"""discovery_candidates: record WHICH concept search found each paper

Revision ID: 0020
Revises: 0019
Create Date: 2026-07-31

Candidates arrived two ways once targeted search existed: browsed from a category listing, or
returned by a search for a specific concept. Only the second knows WHY the paper is here, and
that reason is far stronger than any similarity score:

    "came up searching `liquidity-aware portfolio optimization` — a concept from your reading"

versus

    "closest to your work on Alpha Fund (fit 0.76)"

The first is a fact about a real query against real terminology he underlined by hand; the second
asks him to trust a number. Grounded-or-silent is better served by keeping the provenance than by
reconstructing a justification afterwards, so it is stored at harvest time rather than inferred.

`found_kind` is the CHANNEL (`reading` | `project` | `gap` | `browse`), which is also what the
flywheel needs: with judgments this scarce, learning "searches seeded from his reading get
accepted, category browsing does not" is a claim small numbers can actually support, while
per-paper learning is not.
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0020"
down_revision: Union[str, None] = "0019"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.execute("ALTER TABLE discovery_candidates ADD COLUMN found_term TEXT")
    op.execute("ALTER TABLE discovery_candidates ADD COLUMN found_kind TEXT")
    op.execute("ALTER TABLE discovery_candidates ADD COLUMN found_label TEXT")
    # Existing rows all came from the category browse, which is exactly what a NULL term means;
    # labelling them explicitly keeps the flywheel's channel counts honest rather than lumping
    # pre-search candidates in with searched ones.
    op.execute("UPDATE discovery_candidates SET found_kind = 'browse' WHERE found_kind IS NULL")
    op.execute(
        "CREATE INDEX IF NOT EXISTS idx_discovery_candidates_found "
        "ON discovery_candidates(found_kind)"
    )


def downgrade() -> None:
    op.execute("DROP INDEX IF EXISTS idx_discovery_candidates_found")
    # SQLite cannot drop a column before 3.35 and the data is harmless; left in place.
