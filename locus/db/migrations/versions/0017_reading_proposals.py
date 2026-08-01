"""reading_proposals + reading_targets (annotation reconciliation) + the 'discovery' surface

Revision ID: 0017
Revises: 0016
Create Date: 2026-07-31

Phase 4, step 1: the ACCEPT LOOP (docs/reading-discovery-plan.md). Three changes, all agent
state — the ingest spine is untouched (principles 7-9).

`reading_proposals` — one row per candidate work, carried through its whole life:

    candidate --score, cap--> proposed --moved out of Proposed--> accepted --> ingested
        |                        |
        |                        +-- TTL in Proposed --> rejected  (a WEAK negative)
        +-- already in corpus / already seen --> superseded  (never shown, never counted)

`accepted -> ingested` is the only transition that writes to the corpus, and it goes through the
ordinary ingest spine with no special-casing. A proposal is NOT a corpus document until the owner
moves the file on the device (propose-never-mutate); a book STUB is agent output and is never
ingested at all — the corpus gets the real book or nothing (invariant 5).

GROUNDED-OR-SILENT IS ENFORCED IN THE SCHEMA. `why` and `evidence_key` are NOT NULL because an
ungrounded "you might like this" is precisely the noise that teaches the owner to stop opening the
folder, and a rule that lives only in a code path is a rule that gets forgotten in the second
caller. `why_kind` is a stored COLUMN rather than a rendered string because the flywheel learns
per-CHANNEL: with 3 judgments on the whole reading surface today, per-item learning is not
statistically available, but "discovery 4/5 accepted, co_citation 0/6" is.

IDENTITY IS `dedupe_key`, NOT `external_id`. The same work arrives as an arXiv id from one citing
paper and a DOI from another, and proposing it twice is the suggestion fatigue that kills this
layer (failure mode #7). The key is a normalised title+author, and the normaliser must fold the
OCR manglings the entity pass produces — the corpus already holds `L¨utkepohl and Wo´zniak, 2020`
and `Lütkepohl and Woźniak, 2020` as separate surfaces of one work.

`reading_targets` — THE ANNOTATION JOIN KEY, and the reason marking a paper up is worth anything.
`pdf_annotations.source_uri` holds a reMarkable DEVICE PATH (`/reading_list/Advanced Portfolio
Management`, from `locus annotate`'s `device_path` argument), while `documents.source_uri` holds a
filesystem path. Those never match, so the 26 marks on the book do not join to the book even now
that it is being ingested, and any "how engaged is he with this document" term silently scores his
most-annotated reading ZERO.

The mapping keys on `doc_uuid` — the xochitl document id — because it is the only identifier that
survives both a folder move and a rename, which is exactly what the accept signal asks the owner
to do. `device_path` is recorded too, but only as a fallback for rows predating uuid capture and
as human context; it is deliberately NOT unique, since it changes every time a file is moved.
"""

from typing import Sequence, Union

from alembic import op

revision: str = "0017"
down_revision: Union[str, None] = "0016"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    # --- reading_proposals ------------------------------------------------------------------
    op.execute(
        """
        CREATE TABLE IF NOT EXISTS reading_proposals (
            id            INTEGER PRIMARY KEY,
            kind          TEXT NOT NULL CHECK (kind IN ('paper','book')),
            dedupe_key    TEXT NOT NULL,
            external_id   TEXT,
            title         TEXT NOT NULL,
            authors       TEXT,
            year          INTEGER,
            url           TEXT,
            abstract      TEXT,
            oa_pdf_url    TEXT,
            why           TEXT NOT NULL,
            why_kind      TEXT NOT NULL
                          CHECK (why_kind IN ('discovery','citation','co_citation','book_biblio',
                                              'annotation','gap','related_work','manual')),
            evidence_key  TEXT NOT NULL,
            score         REAL,
            status        TEXT NOT NULL DEFAULT 'candidate'
                          CHECK (status IN ('candidate','proposed','accepted','ingested',
                                            'rejected','superseded')),
            -- HOW it resolved, which is not the same as WHAT it resolved to. A proposal left
            -- sitting in Proposed for three weeks ('ttl') may mean it was wrong or may mean he was
            -- busy; one he DELETED off the device ('removed') is an unambiguous no. The flywheel
            -- must weight those differently, and `acceptance_log` has no column that can say so.
            resolution    TEXT CHECK (resolution IN ('moved','ttl','removed','manual')),
            device_uuid   TEXT,
            device_folder TEXT,
            filename      TEXT,
            local_path    TEXT,
            proposed_at   TEXT,
            resolved_at   TEXT,
            created_at    TEXT NOT NULL,
            source_run    INTEGER REFERENCES agent_runs(id) ON DELETE SET NULL,
            UNIQUE (dedupe_key)
        )
        """
    )
    # The two live listings: "what is sitting in Proposed" (the stock cap) and "what is still a
    # candidate awaiting a slot".
    op.execute(
        "CREATE INDEX IF NOT EXISTS idx_reading_proposals_status "
        "ON reading_proposals(status, kind)"
    )
    # The accept-signal lookup: match a device path back to the row that produced it.
    op.execute(
        "CREATE INDEX IF NOT EXISTS idx_reading_proposals_filename ON reading_proposals(filename)"
    )

    # --- reading_targets (annotation reconciliation) -----------------------------------------
    op.execute(
        """
        CREATE TABLE IF NOT EXISTS reading_targets (
            id          INTEGER PRIMARY KEY,
            doc_uuid    TEXT,
            device_path TEXT,
            source_uri  TEXT NOT NULL,
            proposal_id INTEGER REFERENCES reading_proposals(id) ON DELETE SET NULL,
            linked_by   TEXT NOT NULL CHECK (linked_by IN ('delivery','manual','name_match')),
            created_at  TEXT NOT NULL,
            UNIQUE (doc_uuid)
        )
        """
    )
    op.execute(
        "CREATE INDEX IF NOT EXISTS idx_reading_targets_source ON reading_targets(source_uri)"
    )
    op.execute(
        "CREATE INDEX IF NOT EXISTS idx_reading_targets_device ON reading_targets(device_path)"
    )

    # --- widen acceptance_log.surface to include 'discovery' ---------------------------------
    # 'reading' is ALREADY TAKEN: pull_daily.SURFACE_READING writes it for the daily page's
    # RE-READ slot (3 rows live, all corpus paths). Pooling discovery judgments into it would
    # conflate "stop telling me to re-read Mechanical Vibrations" with "this discovery channel is
    # not working" — two unrelated signals feeding one prior. SQLite cannot ALTER a CHECK, so the
    # table is rebuilt; every row is carried over.
    op.execute(
        """
        CREATE TABLE acceptance_log_new (
            id            INTEGER PRIMARY KEY,
            surface       TEXT    NOT NULL
                          CHECK (surface IN ('link','connection','reading','recall','object',
                                             'discovery')),
            candidate_key TEXT    NOT NULL,
            verdict       TEXT    NOT NULL CHECK (verdict IN ('kept','rejected')),
            at            TEXT    NOT NULL
        )
        """
    )
    op.execute(
        "INSERT INTO acceptance_log_new (id, surface, candidate_key, verdict, at) "
        "SELECT id, surface, candidate_key, verdict, at FROM acceptance_log"
    )
    op.execute("DROP TABLE acceptance_log")
    op.execute("ALTER TABLE acceptance_log_new RENAME TO acceptance_log")
    op.execute(
        "CREATE INDEX IF NOT EXISTS idx_acceptance_surface "
        "ON acceptance_log(surface, candidate_key)"
    )


def downgrade() -> None:
    op.execute("DROP INDEX IF EXISTS idx_reading_targets_device")
    op.execute("DROP INDEX IF EXISTS idx_reading_targets_source")
    op.execute("DROP TABLE IF EXISTS reading_targets")
    op.execute("DROP INDEX IF EXISTS idx_reading_proposals_filename")
    op.execute("DROP INDEX IF EXISTS idx_reading_proposals_status")
    op.execute("DROP TABLE IF EXISTS reading_proposals")
    # The surface CHECK is not narrowed back: any 'discovery' row would be destroyed by it.
