"""agent_runs: crash-safe journal for agent-layer runs (agent-layer plan §6.4, Phase 1 ②)

Revision ID: 0010
Revises: 0009
Create Date: 2026-07-28

The capture/enrichment loops spend real subscription budget through `claude -p`. A run that
crashes mid-flight must not silently lose track of what it did or re-spend on the next attempt
(the same contract `link`/`retitle` already honour with per-verdict pass_cache commits). This
table is that journal: one row per agent run, opened `running` and closed `ok`/`error`/`degraded`,
carrying a JSON `stats` blob (counts, cost, what was processed).

SCOPE NOTE: the agent-layer plan grouped ALL agent-state tables (objects, object_links,
belief_positions, review_schedule, acceptance_log, agent_runs) into one migration "0010". Per
the Phase-1 decision (2026-07-28) `agent_runs` is PULLED FORWARD alone — the Phase-1 loops need
crash-safe journaling on day one, the rest are Phase-2 (structured objects / evolution). The
remaining agent-state tables land in a later migration, not renumbered here.

Agent-state, NOT the ingest spine: separate table, same DB (so `locus backup` covers it); the
immutable documents/sections/chunks spine is untouched (CLAUDE.md principles 7-9).
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0010"
down_revision: Union[str, None] = "0009"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.execute(
        """
        CREATE TABLE agent_runs (
            id          INTEGER PRIMARY KEY,
            kind        TEXT    NOT NULL,           -- 'capture' | 'enrich' | 'structure' | ...
            started_at  TEXT    NOT NULL,           -- ISO 8601 UTC
            finished_at TEXT,                       -- NULL while running
            status      TEXT    NOT NULL DEFAULT 'running'
                        CHECK (status IN ('running','ok','error','degraded')),
            stats       TEXT                        -- JSON: counts, cost_usd, tokens, notes
        )
        """
    )
    # Recent runs of a kind — the common query (last capture-sync, cost over a window).
    op.execute("CREATE INDEX idx_agent_runs_kind_started ON agent_runs(kind, started_at)")


def downgrade() -> None:
    op.execute("DROP TABLE IF EXISTS agent_runs")
