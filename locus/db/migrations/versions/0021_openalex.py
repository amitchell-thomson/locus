"""discovery_candidates: citation counts and a venue, for the OpenAlex channel

Revision ID: 0021
Revises: 0020
Create Date: 2026-08-01

arXiv is preprints, and skewed to CS, physics and maths. A great deal of what he actually reads —
the portfolio-management canon, the finance journals, the books — is not on it at all, which caps
both the quality and the SIZE of the backlog. OpenAlex indexes journals, books and chapters, needs
no key, and returns two things arXiv does not:

  `cited_by`  how many works cite this one. Until now a 500-citation canonical treatment and a
              two-week-old preprint with none ranked identically on similarity alone. For method
              transfer the canonical treatment is usually the one worth reading, and this is the
              cheapest available proxy for it.
  `venue`     where it appeared, which is context he can judge at a glance on the page.

Both are nullable: arXiv-sourced rows have neither, and a missing citation count must read as
"unknown" rather than "zero" — treating unknown as zero would systematically demote every arXiv
preprint the moment the prior was switched on.
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0021"
down_revision: Union[str, None] = "0020"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.execute("ALTER TABLE discovery_candidates ADD COLUMN cited_by INTEGER")
    op.execute("ALTER TABLE discovery_candidates ADD COLUMN venue TEXT")
    op.execute("ALTER TABLE discovery_candidates ADD COLUMN doi TEXT")
    op.execute(
        "CREATE INDEX IF NOT EXISTS idx_discovery_candidates_cited "
        "ON discovery_candidates(cited_by)"
    )


def downgrade() -> None:
    op.execute("DROP INDEX IF EXISTS idx_discovery_candidates_cited")
    # Columns are left in place: SQLite drops are awkward and the data is inert without them.
