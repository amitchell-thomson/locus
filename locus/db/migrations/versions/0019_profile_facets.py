"""discovery_profiles: many facets per subject, not one summary

Revision ID: 0019
Revises: 0018
Create Date: 2026-07-31

A project was represented by ONE vector built from its title, thesis and method — 289 characters
for `regime-ml`, against 34,856 available (88 section summaries, plus `result`, `limitations`, and
263 method/concept entities, all unused). Under 1% of what the corpus knows about the project, and
all of it the high-level pitch: "detects market regimes using Hidden Markov Models". Matching an
abstract against that finds something relevant roughly by luck.

The fix is not simply a longer string. nomic truncates around 2k tokens, and averaging a whole
repository into one vector makes it generic precisely where it needs to be specific — every
project collapses toward "a machine learning system with a data pipeline". So a subject now gets
MANY vectors, one per facet, and a candidate's fit is the MAX over them.

That is the same principle the retrieval spine already runs on: sections and chunks are separate
searchable units because a document is not one topic. It is also what makes method-transfer
possible at all — a section summary about tuning HMM state persistence can match a paper on sticky
HMM priors, while "regime-conditioned equity ML trading" never will.

`facet` identifies which slice a row holds ('synthesis', 'section:<n>', 'concepts'), so the
uniqueness constraint moves from (kind, key) to (kind, key, facet). Rows still carry the subject's
label, so grouping for display and for the per-project cap is a plain group-by.
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0019"
down_revision: Union[str, None] = "0018"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    # SQLite cannot alter a UNIQUE constraint, so the table is rebuilt. It is derived and
    # regenerable (principle 9) — but rebuilt rather than dropped so an existing DB stays
    # queryable until the next `rebuild()`, and the vec0 table keyed on profile_id is emptied
    # in step with it rather than left pointing at ids that no longer mean anything.
    op.execute(
        """
        CREATE TABLE discovery_profiles_new (
            id           INTEGER PRIMARY KEY,
            subject_kind TEXT NOT NULL CHECK (subject_kind IN ('project','gap')),
            subject_key  TEXT NOT NULL,
            facet        TEXT NOT NULL DEFAULT 'synthesis',
            label        TEXT NOT NULL,
            text         TEXT NOT NULL,
            doc_ids      TEXT NOT NULL DEFAULT '[]',
            built_at     TEXT NOT NULL,
            UNIQUE (subject_kind, subject_key, facet)
        )
        """
    )
    op.execute(
        "INSERT INTO discovery_profiles_new "
        "(id, subject_kind, subject_key, facet, label, text, doc_ids, built_at) "
        "SELECT id, subject_kind, subject_key, 'synthesis', label, text, doc_ids, built_at "
        "FROM discovery_profiles"
    )
    op.execute("DROP TABLE discovery_profiles")
    op.execute("ALTER TABLE discovery_profiles_new RENAME TO discovery_profiles")
    op.execute(
        "CREATE INDEX IF NOT EXISTS idx_discovery_profiles_subject "
        "ON discovery_profiles(subject_kind, subject_key)"
    )


def downgrade() -> None:
    # Collapsing many facets back to one row would silently discard all but an arbitrary facet,
    # so the narrowing is not performed; the extra column is simply left in place.
    op.execute("DROP INDEX IF EXISTS idx_discovery_profiles_subject")
