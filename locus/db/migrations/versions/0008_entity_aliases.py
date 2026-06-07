"""entity_aliases: cross-document alias canonicalization (CLAUDE.md §15.4, plan step 12)

Revision ID: 0008
Revises: 0007
Create Date: 2026-06-06

Maps every distinct stored entity identity `(name, type)` to one canonical
`(canonical_name, canonical_type)`. The table is DERIVED, REGENERABLE data built by
`locus link` (locus/link/aliases.py): rebuild = delete + recompute. The `entities` table
is never mutated — per-section provenance is preserved (§14's name+type identity), and the
alias layer expresses cross-type merges ("fourier transform" stored as concept/method/theorem)
by pointing several variant rows at one canonical.

The mapping is TOTAL: singleton entities get a row with canonical = self (tier='identity'),
so consumers (the entity-anchored retrieval arm, related-documents) use plain inner joins.

`tier` records which pass justified the mapping: 'identity' (no merge), deterministic tiers
('casefold' | 'punct' | 'acronym' | 'plural'), or 'llm' (Claude-adjudicated fuzzy cluster).
Not re-ingest-bound.
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0008"
down_revision: Union[str, None] = "0007"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.execute(
        """
        CREATE TABLE entity_aliases (
            id             INTEGER PRIMARY KEY,
            variant_name   TEXT    NOT NULL,
            variant_type   TEXT    NOT NULL,
            canonical_name TEXT    NOT NULL,
            canonical_type TEXT    NOT NULL,
            cluster_id     INTEGER NOT NULL,
            tier           TEXT    NOT NULL CHECK (
                tier IN ('identity','casefold','punct','acronym','plural','llm')
            ),
            UNIQUE (variant_name, variant_type)
        )
        """
    )
    op.execute("CREATE INDEX idx_aliases_variant ON entity_aliases(variant_name)")
    op.execute(
        "CREATE INDEX idx_aliases_canonical ON entity_aliases(canonical_name, canonical_type)"
    )


def downgrade() -> None:
    op.execute("DROP TABLE IF EXISTS entity_aliases")
