"""discovery_candidates + profile vectors — the method-transfer discovery engine

Revision ID: 0018
Revises: 0017
Create Date: 2026-07-31

Phase 4 step 2. Citation mining can only ever find what the corpus already points at; this is the
half that finds work he has no edge to — a paper whose METHOD could transfer to a project, from a
field he has never read.

The mechanism is that his projects are ALREADY EMBEDDED. A harvested abstract goes through the
same nomic-embed-text (768-dim, unit-normalised) that every chunk and section went through, so
relevance is a cosine in one shared space rather than a keyword match. That is what makes it
conceptual adjacency instead of lexical overlap, and it is why no query text needs to be sent
anywhere (see `discover/arxiv.py` — the outbound query is a category and a date, nothing else).

`discovery_candidates` IS NOT `documents`, and that is the whole point. A candidate is a proposal,
not corpus (propose-never-mutate). Because it lives in its own table rather than behind an
exclusion flag, no retrieval arm can reach it by construction — a third-party abstract can never
surface in a generated answer dressed as his own material (invariant 5). This is the first
third-party text Locus stores locally, so the separation is structural, not a config setting
someone can flip.

`discovery_profiles` stores WHAT WAS EMBEDDED as well as the vector. A profile is a derived
summary of a project, and a ranking nobody can audit is a ranking nobody should trust: keeping the
text means "why was this proposed for the regime project" has an answer that can be read.
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0018"
down_revision: Union[str, None] = "0017"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    # --- harvested external metadata ---------------------------------------------------------
    op.execute(
        """
        CREATE TABLE IF NOT EXISTS discovery_candidates (
            id               INTEGER PRIMARY KEY,
            external_id      TEXT NOT NULL,        -- 'arxiv:2607.12345'
            dedupe_key       TEXT NOT NULL,        -- shared normaliser with reading_proposals
            title            TEXT NOT NULL,
            authors          TEXT,
            abstract         TEXT NOT NULL,
            primary_category TEXT,
            categories       TEXT,
            published        TEXT,
            url              TEXT,
            pdf_url          TEXT,                 -- arXiv is open access: the proposal can BE the paper
            source           TEXT NOT NULL DEFAULT 'arxiv',
            harvested_at     TEXT NOT NULL,
            embedded         INTEGER NOT NULL DEFAULT 0,
            UNIQUE (external_id)
        )
        """
    )
    op.execute(
        "CREATE INDEX IF NOT EXISTS idx_discovery_candidates_dedupe "
        "ON discovery_candidates(dedupe_key)"
    )
    op.execute(
        "CREATE INDEX IF NOT EXISTS idx_discovery_candidates_embedded "
        "ON discovery_candidates(embedded)"
    )
    op.execute(
        "CREATE VIRTUAL TABLE discovery_vectors USING vec0("
        " candidate_id INTEGER PRIMARY KEY, embedding FLOAT[768])"
    )

    # --- what we rank AGAINST ----------------------------------------------------------------
    op.execute(
        """
        CREATE TABLE IF NOT EXISTS discovery_profiles (
            id           INTEGER PRIMARY KEY,
            subject_kind TEXT NOT NULL CHECK (subject_kind IN ('project','gap')),
            subject_key  TEXT NOT NULL,   -- object id as text | canonical gap concept
            label        TEXT NOT NULL,
            text         TEXT NOT NULL,   -- the exact text embedded, kept so ranking is auditable
            -- The documents this profile was BUILT FROM, as a JSON id list. Ranking must exclude
            -- them when asking "do I already have this?", or the question answers itself: a
            -- project profile is derived from his own write-ups, those write-ups are in the
            -- corpus, so familiarity would equal fit for every candidate and every score would
            -- collapse to zero. Excluding them makes it the question actually worth asking —
            -- does anything OTHER than my own write-up already teach this?
            doc_ids      TEXT NOT NULL DEFAULT '[]',
            built_at     TEXT NOT NULL,
            UNIQUE (subject_kind, subject_key)
        )
        """
    )
    op.execute(
        "CREATE VIRTUAL TABLE discovery_profile_vectors USING vec0("
        " profile_id INTEGER PRIMARY KEY, embedding FLOAT[768])"
    )


def downgrade() -> None:
    for table in (
        "discovery_profile_vectors", "discovery_profiles",
        "discovery_vectors", "discovery_candidates",
    ):
        op.execute(f"DROP TABLE IF EXISTS {table}")
