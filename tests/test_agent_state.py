"""Phase-2 agent state: migration 0011 + the objects/links/positions/acceptance store.

Model-free — pure SQLite against a seeded tmp DB. The behaviours asserted here are the
invariants, not the CRUD: propose-never-mutate (an agent may not un-bless), additive body merge
(a re-proposal may not delete the owner's threads), and position dedup (a trajectory must not
stack repeats of one stance).
"""

from __future__ import annotations

from pathlib import Path

import pytest

from locus.agent import state
from locus.agent.state import ObjectLink, entity_key, parse_entity_key
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


def test_upsert_creates_proposed_then_updates_in_place(conn):
    oid, created = state.upsert_object(conn, type_="project", title="tanker-flow", body={"approach": "AIS"})
    assert created is True
    again, created2 = state.upsert_object(conn, type_="project", title="tanker-flow", body={"approach": "AIS"})
    assert (again, created2) == (oid, False)
    assert len(state.list_objects(conn, type_="project")) == 1
    assert state.get_object(conn, oid).status == "proposed"


def test_reproposal_never_overwrites_a_blessing(conn):
    """Invariant 2: the owner blesses; a later agent pass must not knock it back to proposed."""
    oid, _ = state.upsert_object(conn, type_="concept", title="regime detection")
    assert state.set_status(conn, oid, "active") is True
    state.upsert_object(conn, type_="concept", title="regime detection", body={"mastery": "thin"})
    obj = state.get_object(conn, oid)
    assert obj.status == "active"
    assert obj.body["mastery"] == "thin"  # body still updates


def test_body_merge_is_additive_and_never_drops_owner_threads(conn):
    oid, _ = state.upsert_object(
        conn, type_="project", title="regime-ml",
        body={"approach": "HMM", "open_threads": ["validate on 2020"], "learnings": []},
    )
    state.upsert_object(
        conn, type_="project", title="regime-ml",
        body={"approach": "changepoint", "open_threads": ["compare to Brevan view"]},
    )
    body = state.get_object(conn, oid).body
    assert body["open_threads"] == ["validate on 2020", "compare to Brevan view"]
    assert body["approach"] == "HMM"  # a non-empty scalar is the owner's; not clobbered


def test_merge_body_fills_empty_scalars_and_recurses():
    merged = state.merge_body(
        {"why": "", "state": "queued", "meta": {"a": 1}},
        {"why": "cited by Ang", "state": "done", "meta": {"b": 2}},
    )
    assert merged == {"why": "cited by Ang", "state": "queued", "meta": {"a": 1, "b": 2}}


def test_unknown_type_or_relation_is_rejected(conn):
    with pytest.raises(ValueError):
        state.upsert_object(conn, type_="thesis", title="x")
    oid, _ = state.upsert_object(conn, type_="question", title="q")
    with pytest.raises(ValueError):
        state.add_links(conn, oid, [ObjectLink("doc", "vault/raw/x.pdf", "cites")])


# ---------- links ----------


def test_links_are_idempotent_and_reverse_lookupable(conn):
    oid, _ = state.upsert_object(conn, type_="project", title="tanker-flow")
    links = [ObjectLink("doc", "repos/tanker-flow", "implements"),
             ObjectLink("entity", entity_key("laden ton-miles", "concept"), "about")]
    assert state.add_links(conn, oid, links) == 2
    assert state.add_links(conn, oid, links) == 0  # re-run adds nothing
    assert len(state.links_for(conn, oid)) == 2
    found = state.objects_linking_to(conn, "doc", "repos/tanker-flow")
    assert [o.id for o in found] == [oid]


def test_entity_key_roundtrips_names_containing_punctuation():
    key = entity_key("Ornstein-Uhlenbeck (OU) process", "concept")
    assert parse_entity_key(key) == ("Ornstein-Uhlenbeck (OU) process", "concept")


def test_deleting_an_object_cascades_its_links(conn):
    oid, _ = state.upsert_object(conn, type_="reading", title="Ang - Asset Management")
    state.add_links(conn, oid, [ObjectLink("doc", "papers/ang.pdf", "reads")])
    with conn:
        conn.execute("DELETE FROM objects WHERE id=?", (oid,))
    assert conn.execute("SELECT COUNT(*) c FROM object_links").fetchone()["c"] == 0


# ---------- belief positions ----------


def test_positions_dedupe_and_order_by_source_date(conn):
    key = entity_key("portfolio construction", "concept")
    first = state.record_position(
        conn, subject_kind="concept", subject_key=key,
        stance="equal weight beats mean-variance out of sample", dated_at="2026-03-01", source_doc_id=7,
    )
    dup = state.record_position(
        conn, subject_kind="concept", subject_key=key,
        stance="equal weight beats mean-variance out of sample", dated_at="2026-03-01", source_doc_id=7,
    )
    state.record_position(
        conn, subject_kind="concept", subject_key=key,
        stance="shrinkage covariance recovers most of the gap", dated_at="2026-01-15", source_doc_id=9,
    )
    assert first is not None and dup is None  # re-run records nothing new
    chain = state.positions_for(conn, "concept", key)
    assert [p.dated_at for p in chain] == ["2026-01-15", "2026-03-01"]  # oldest first
    assert state.subjects_with_positions(conn) == [("concept", key, 2)]


def test_position_survives_its_source_document_being_deleted(conn):
    """source_doc_id is deliberately NOT a FK: a re-ingest replaces the doc row, and a recorded
    stance must not vanish with it."""
    with conn:
        conn.execute(
            "INSERT INTO documents (id, content_hash, source_type, source_uri, raw_path, title, "
            "ingested_at, ingest_model) VALUES (5,'h','markdown','n.md','r','N','t','m')"
        )
    state.record_position(
        conn, subject_kind="project", subject_key="12", stance="pivoting to flow features",
        dated_at="2026-05-05", source_doc_id=5,
    )
    with conn:
        conn.execute("DELETE FROM documents WHERE id=5")
    assert len(state.positions_for(conn, "project", "12")) == 1


# ---------- acceptance log ----------


def test_acceptance_counts_group_by_candidate(conn):
    state.log_acceptance(conn, surface="link", candidate_key="a->b", verdict="kept")
    state.log_acceptance(conn, surface="link", candidate_key="a->b", verdict="kept")
    state.log_acceptance(conn, surface="link", candidate_key="a->c", verdict="rejected")
    state.log_acceptance(conn, surface="object", candidate_key="obj:9", verdict="kept")
    counts = state.acceptance_counts(conn, surface="link")
    assert counts == {"a->b": {"kept": 2}, "a->c": {"rejected": 1}}
    with pytest.raises(ValueError):
        state.log_acceptance(conn, surface="link", candidate_key="x", verdict="maybe")


# ---------- migration 0012: provenance that survives a re-ingest ----------


def test_position_provenance_survives_the_note_being_reingested(conn):
    """notes_sync replaces a changed note by DELETING its row and inserting a NEW id, so
    source_doc_id goes stale on any edit — even a frontmatter one. source_uri does not."""
    with conn:
        conn.execute(
            "INSERT INTO documents (id, content_hash, source_type, source_uri, raw_path, title, "
            "ingested_at, ingest_model) VALUES (40,'h','markdown','notes/rates.md','r','Rates v1','t','m')"
        )
    key = entity_key("discount factors", "concept")
    state.record_position(
        conn, subject_kind="concept", subject_key=key, stance="discount factors are the atoms",
        dated_at="2026-07-10", source_doc_id=40, source_uri="notes/rates.md",
    )
    # The note is edited -> replace-by-path: old row deleted, NEW id inserted, same path.
    with conn:
        conn.execute("DELETE FROM documents WHERE id=40")
        conn.execute(
            "INSERT INTO documents (id, content_hash, source_type, source_uri, raw_path, title, "
            "ingested_at, ingest_model) VALUES (41,'h2','markdown','notes/rates.md','r','Rates v2','t','m')"
        )
    pos = state.positions_for(conn, "concept", key)[0]
    assert pos.source_doc_id == 40          # stale by design, kept as a fast path
    assert pos.source_uri == "notes/rates.md"  # still resolves

    from locus.evolve.trajectory import build_trajectory

    entry = build_trajectory(conn, "concept", key).entries[0]
    assert entry.source_title == "Rates v2"  # provenance recovered through the stable path


def test_position_without_a_source_uri_still_resolves_by_doc_id(conn):
    """Rows predating 0012 (and any the backfill could not resolve) keep working."""
    with conn:
        conn.execute(
            "INSERT INTO documents (id, content_hash, source_type, source_uri, raw_path, title, "
            "ingested_at, ingest_model) VALUES (50,'h','markdown','notes/old.md','r','Old note','t','m')"
        )
    key = entity_key("legacy", "concept")
    state.record_position(conn, subject_kind="concept", subject_key=key, stance="a stance",
                          dated_at="2026-01-01", source_doc_id=50)

    from locus.evolve.trajectory import build_trajectory

    assert build_trajectory(conn, "concept", key).entries[0].source_title == "Old note"


def test_owner_authored_sql_agrees_with_the_python_predicate(conn):
    """ONE definition of "his writing", asserted across the two expressions of it.

    Three layers ask this question — the proposer, the daily page's connection source, and
    re-read ranking — and they used to answer it three different ways, all `category='note'`.
    """
    from locus.structure.propose import _is_owner_authored

    rows = [
        # (uri, category, source_type, expected)
        ("/home/alec/vault/notes/optimisation.md", "coursework", "markdown", True),
        ("vault/incoming/projects/oxdaq.md", "project", "markdown", True),
        ("vault/incoming/projects/optibook.pdf", "project", "pdf", False),
        ("vault/incoming/papers/x.pdf", "paper", "pdf", False),
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
    py_ids = {r["id"] for r in conn.execute("SELECT * FROM documents") if _is_owner_authored(r)}
    assert sql_ids == py_ids, "the SQL and Python definitions of owner-authored have drifted"
    for i, (_uri, _cat, _st, expected) in enumerate(rows, start=500):
        assert (i in sql_ids) is expected
