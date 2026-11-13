"""Migration 0011's agent-state tables, and `owner_authored_sql` (the one definition of his
writing). The objects store these tables were written for was deleted in Slice C (C2).
"""

from __future__ import annotations

from pathlib import Path

import pytest

from locus.agent import state
from locus.db.connection import get_connection
from locus.db.migrate import migrate


@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "state.db"
    migrate(db)
    c = get_connection(db)
    yield c
    c.close()


# ---------- migration ----------


def test_agent_state_tables_exist(conn):
    names = {r["name"] for r in conn.execute("SELECT name FROM sqlite_master WHERE type='table'")}
    for expected in (
        "objects", "object_links", "belief_positions", "review_schedule", "acceptance_log",
    ):
        assert expected in names, f"missing table: {expected}"


def test_spine_tables_untouched_by_0011(conn):
    # Principle 7-9: agent state is additive; the ingest spine keeps its shape.
    cols = {r["name"] for r in conn.execute("PRAGMA table_info(documents)")}
    assert "status" not in cols and "body" not in cols


def test_object_type_and_status_are_constrained(conn):
    import sqlite3

    with pytest.raises(sqlite3.IntegrityError):
        conn.execute(
            "INSERT INTO objects (type,title,created_at,updated_at) VALUES ('thesis','x','t','t')"
        )


# ---------- objects ----------


# ---------- links ----------


# ---------- belief positions ----------


# ---------- acceptance log ----------


# ---------- migration 0012: provenance that survives a re-ingest ----------


def test_owner_authored_sql_names_his_writing(conn):
    """ONE definition of "his writing" (heuris search reads it to tell his notes from sources).

    The Python predicate it was once checked against lived in the proposer, deleted in Slice C.
    """

    rows = [
        # (uri, category, source_type, expected)
        ("/home/alec/data/notes/optimisation.md", "coursework", "markdown", True),
        ("data/incoming/project/oxdaq.md", "project", "markdown", True),
        # Nothing dropped into an AREA folder is his writing (ADR-003), whatever its category or
        # format: a third-party .docx in personal/, careers/ or projects/ is a source.
        ("data/incoming/projects/oxdaq.md", "project", "markdown", False),
        ("data/incoming/careers/offer.docx", "career", "docx", False),
        ("data/incoming/Personal/tenancy.docx", "note", "docx", False),
        ("data/incoming/career/cv.docx", "career", "docx", True),
        ("data/incoming/projects/optibook.pdf", "project", "pdf", False),
        ("data/incoming/papers/x.pdf", "paper", "pdf", False),
        ("repos/regime-ml", "project", "code", False),
    ]
    for i, (uri, cat, st, _expected) in enumerate(rows, start=500):
        with conn:
            conn.execute(
                "INSERT INTO documents (id, content_hash, source_type, source_uri, raw_path, "
                "ingest_model, category) VALUES (?,?,?,?,?,'test',?)",
                (i, f"h{i}", st, uri, f"raw{i}", cat),
            )

    clause, params = state.owner_authored_sql("d")
    sql_ids = {r["id"] for r in conn.execute(f"SELECT d.id FROM documents d WHERE {clause}", params)}
    for i, (_uri, _cat, _st, expected) in enumerate(rows, start=500):
        assert (i in sql_ids) is expected
