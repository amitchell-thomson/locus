"""agent-state tables: objects, links, belief positions, review schedule, acceptance log

Revision ID: 0011
Revises: 0010
Create Date: 2026-07-28

Phase 2 of the agent layer (plan §6.2-6.4) — the state behind the value surfaces. Five tables:

  objects          structured overlays on the corpus (project|concept|question|reading), AGENT-
                   PROPOSED and HUMAN-BLESSED (status proposed -> active). Not purely regenerable:
                   they carry the owner's decisions (blessing, mastery), so they are backed up with
                   the DB rather than rebuilt.
  object_links     an object's grounding: which corpus doc / canonical entity / other object it
                   refers to. This is where "grounded or silent" (invariant 3) is enforced — an
                   object with no link cites nothing and should not have been proposed.
  belief_positions the headline capability (§6.3): a DATED chain of the owner's stances per
                   concept/project, ordered by `dated_at` = the SOURCE note/conversation's date
                   (NOT the extraction time), so the trajectory reads as when he actually thought
                   it, no matter when capture caught up.
  review_schedule  SM-2 spaced repetition over prompts (a proposition or a question object).
  acceptance_log   the flywheel substrate (§12.1): keep/reject of a proposal is a free relevance
                   label; derived/regenerable, never mutates ingested rows.

SCOPE (CLAUDE.md principles 7-9): agent state lives in its OWN tables in the SAME DB — `locus
backup` covers it, and the immutable documents/sections/chunks/propositions spine is untouched.
Nothing here has a foreign key INTO the spine except `belief_positions.source_doc_id`, which is
deliberately NOT a hard FK: a re-ingest replaces a document row, and a stance the owner recorded
must survive its source being re-ingested (it degrades to an unresolvable pointer, not a deleted
belief). The plan's migration numbering (0010 = all agent state) was split by the Phase-1 decision
that pulled `agent_runs` forward alone; this is the remainder. `annotations` stays Phase 3.
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0011"
down_revision: Union[str, None] = "0010"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    # --- objects -------------------------------------------------------------------------
    # `body` is JSON and type-specific by design (plan §6.2): Project {approach, open_threads[],
    # learnings[]}, Concept {mastery, last_engaged, engagement[]}, Question free-standing,
    # Reading {state, why}. A future Thesis/Trade-idea type (§14) is a new `type` value + body
    # shape with NO migration — that extensibility is the reason body is not columns.
    op.execute(
        """
        CREATE TABLE objects (
            id          INTEGER PRIMARY KEY,
            type        TEXT    NOT NULL
                        CHECK (type IN ('project','concept','question','reading')),
            title       TEXT    NOT NULL,
            status      TEXT    NOT NULL DEFAULT 'proposed'
                        CHECK (status IN ('proposed','active','archived')),
            maturity    TEXT    NOT NULL DEFAULT 'rough'
                        CHECK (maturity IN ('rough','tidy')),
            body        TEXT,                       -- JSON, type-specific
            created_at  TEXT    NOT NULL,
            updated_at  TEXT    NOT NULL,
            source_run  INTEGER REFERENCES agent_runs(id) ON DELETE SET NULL
        )
        """
    )
    # One object per (type, title): re-proposing an existing subject must UPDATE it, never
    # accumulate near-duplicates (failure mode #7 — suggestion fatigue is how this layer dies).
    # Titles are stored already-normalised by the proposer.
    op.execute("CREATE UNIQUE INDEX idx_objects_type_title ON objects(type, title)")
    # The two live listings: "what is proposed and awaiting blessing" and "my active projects".
    op.execute("CREATE INDEX idx_objects_status_type ON objects(status, type)")

    # --- object_links --------------------------------------------------------------------
    # target_key is a STABLE STRING, not a row id: a doc's `source_uri` (survives re-ingest,
    # which changes doc_id), a canonical entity's "name\x1ftype", or another object's id. The
    # same stability argument the eval labels learned the hard way when `retitle` broke every
    # title-substring label (CLAUDE.md §2, round-7).
    op.execute(
        """
        CREATE TABLE object_links (
            id          INTEGER PRIMARY KEY,
            object_id   INTEGER NOT NULL REFERENCES objects(id) ON DELETE CASCADE,
            target_kind TEXT    NOT NULL CHECK (target_kind IN ('doc','entity','object')),
            target_key  TEXT    NOT NULL,
            relation    TEXT    NOT NULL
                        CHECK (relation IN ('implements','about','raised_by','answered_by',
                                            'reads','relates')),
            UNIQUE (object_id, target_kind, target_key, relation)
        )
        """
    )
    op.execute("CREATE INDEX idx_object_links_target ON object_links(target_kind, target_key)")

    # --- belief_positions ----------------------------------------------------------------
    # subject_key mirrors object_links.target_key: a canonical "name\x1ftype" for a concept, or
    # an object id (as text) for a project. `stance` is the owner's position IN HIS WORDS —
    # the proposer quotes the source rather than paraphrasing, so the trajectory cannot drift
    # into the model's voice (failure mode #1).
    op.execute(
        """
        CREATE TABLE belief_positions (
            id            INTEGER PRIMARY KEY,
            subject_kind  TEXT    NOT NULL CHECK (subject_kind IN ('concept','project')),
            subject_key   TEXT    NOT NULL,
            stance        TEXT    NOT NULL,
            source_doc_id INTEGER,                  -- NOT a FK: must survive a re-ingest
            source_run    INTEGER REFERENCES agent_runs(id) ON DELETE SET NULL,
            dated_at      TEXT    NOT NULL,         -- the SOURCE note/convo date, not extraction
            created_at    TEXT    NOT NULL,
            UNIQUE (subject_kind, subject_key, stance, source_doc_id)
        )
        """
    )
    # THE trajectory query: positions for one subject, oldest first (§6.3).
    op.execute(
        "CREATE INDEX idx_belief_subject_dated "
        "ON belief_positions(subject_kind, subject_key, dated_at)"
    )

    # --- review_schedule -----------------------------------------------------------------
    # SM-2: `ease` is the E-factor (>=1.3), `interval` is in days, `due` an ISO date.
    # prompt_ref is (kind, key) so a schedule item can hang off a stored proposition or a
    # question object without a polymorphic FK.
    op.execute(
        """
        CREATE TABLE review_schedule (
            id           INTEGER PRIMARY KEY,
            prompt_kind  TEXT    NOT NULL CHECK (prompt_kind IN ('proposition','object')),
            prompt_ref   TEXT    NOT NULL,
            due          TEXT    NOT NULL,          -- ISO date
            ease         REAL    NOT NULL DEFAULT 2.5,
            interval     INTEGER NOT NULL DEFAULT 0, -- days
            reps         INTEGER NOT NULL DEFAULT 0,
            last_grade   INTEGER,                   -- 0-5, SM-2 quality of the last recall
            last_review  TEXT,
            UNIQUE (prompt_kind, prompt_ref)
        )
        """
    )
    op.execute("CREATE INDEX idx_review_due ON review_schedule(due)")

    # --- acceptance_log ------------------------------------------------------------------
    # Append-only. Every kept/rejected proposal is a free labelled relevance judgment that
    # later folds into related-doc ranking and grows the eval's links_recall labels (§12.1).
    op.execute(
        """
        CREATE TABLE acceptance_log (
            id            INTEGER PRIMARY KEY,
            surface       TEXT    NOT NULL
                          CHECK (surface IN ('link','connection','reading','recall','object')),
            candidate_key TEXT    NOT NULL,
            verdict       TEXT    NOT NULL CHECK (verdict IN ('kept','rejected')),
            at            TEXT    NOT NULL
        )
        """
    )
    op.execute("CREATE INDEX idx_acceptance_surface ON acceptance_log(surface, candidate_key)")


def downgrade() -> None:
    for table in (
        "acceptance_log",
        "review_schedule",
        "belief_positions",
        "object_links",
        "objects",
    ):
        op.execute(f"DROP TABLE IF EXISTS {table}")
