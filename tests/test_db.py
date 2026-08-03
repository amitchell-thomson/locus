"""Stage 1: sqlite-vec loads, migrations apply idempotently, KNN works, FKs cascade."""

import struct
from pathlib import Path

import pytest

from locus.db.connection import get_connection, vec_version
from locus.db.migrate import current_revision, head_revision, migrate

DIM = 768


def _vec(head: list[float]) -> bytes:
    """Pack a 768-dim float vector whose leading dims are `head`, rest zero."""
    vals = head + [0.0] * (DIM - len(head))
    return struct.pack(f"{DIM}f", *vals)


@pytest.fixture()
def db(tmp_path: Path) -> Path:
    path = tmp_path / "test.db"
    migrate(path)
    return path


def test_extension_loads(db: Path):
    conn = get_connection(db)
    assert vec_version(conn).startswith("v0.")
    conn.close()


def test_migration_records_head_and_is_idempotent(db: Path):
    # After migrate, the DB is at the head revision Alembic knows about.
    head = head_revision(db)
    assert current_revision(db) == head == "0032"
    # Re-running is a no-op and leaves the revision unchanged.
    migrate(db)
    assert current_revision(db) == head


def test_all_core_tables_exist(db: Path):
    conn = get_connection(db)
    names = {
        r["name"]
        for r in conn.execute("SELECT name FROM sqlite_master WHERE type='table'")
    }
    for expected in (
        "documents", "sections", "chunks", "propositions",
        "entities", "alembic_version",
        "section_vectors", "chunk_vectors", "proposition_vectors",
    ):
        assert expected in names, f"missing table: {expected}"
    # `tags` / `doc_tags` were dropped in 0028: schema with no code on either side since the
    # original design, 0 rows at 218 documents. Empty scaffolding is indistinguishable from a
    # feature that is quietly broken, which is the confusion the readiness audit existed to end.
    assert "tags" not in names and "doc_tags" not in names
    conn.close()


def test_knn_orders_by_distance(db: Path):
    conn = get_connection(db)
    conn.execute("INSERT INTO chunk_vectors(chunk_id, embedding) VALUES (?, ?)", (1, _vec([1.0, 0.0])))
    conn.execute("INSERT INTO chunk_vectors(chunk_id, embedding) VALUES (?, ?)", (2, _vec([0.0, 1.0])))
    conn.commit()
    q = _vec([0.9, 0.1])  # leans toward chunk 1
    rows = conn.execute(
        "SELECT chunk_id FROM chunk_vectors WHERE embedding MATCH ? ORDER BY distance LIMIT 2",
        (q,),
    ).fetchall()
    assert [r["chunk_id"] for r in rows] == [1, 2]
    conn.close()


def test_source_type_check_widened(db: Path):
    """0004: the new step-8 formats are legal source_type values; junk is still rejected."""
    conn = get_connection(db)
    for i, st in enumerate(("docx", "markdown", "text", "notebook"), start=10):
        conn.execute(
            "INSERT INTO documents(id, content_hash, source_type, source_uri, raw_path, ingest_model)"
            f" VALUES ({i}, 'h{i}', ?, 'u', 'p', 'qwen2.5:7b')",
            (st,),
        )
    conn.commit()
    import sqlite3

    with pytest.raises(sqlite3.IntegrityError):
        conn.execute(
            "INSERT INTO documents(id, content_hash, source_type, source_uri, raw_path, ingest_model)"
            " VALUES (99, 'h99', 'bogus', 'u', 'p', 'qwen2.5:7b')"
        )
    conn.close()


def test_0004_rebuild_preserves_existing_rows(tmp_path: Path):
    """A pre-0004 document survives the table rebuild with its id and children intact."""
    from alembic import command

    from locus.db.migrate import _config

    path = tmp_path / "old.db"
    command.upgrade(_config(path), "0003")  # build a pre-rebuild DB
    conn = get_connection(path)
    conn.execute(
        "INSERT INTO documents(id, content_hash, source_type, source_uri, raw_path,"
        " ingest_model, title, source_date, category)"
        " VALUES (7, 'h7', 'pdf', 'u', 'p', 'qwen2.5:7b', 'Old Doc', '2024-01-02', 'paper')"
    )
    conn.execute("INSERT INTO sections(id, doc_id, position) VALUES (3, 7, 0)")
    conn.commit()
    conn.close()

    migrate(path)  # 0003 -> 0004 rebuild
    conn = get_connection(path)
    row = conn.execute("SELECT * FROM documents WHERE id = 7").fetchone()
    assert row["title"] == "Old Doc"
    assert row["source_date"] == "2024-01-02"
    assert row["category"] == "paper"
    # Child still attached and the FK cascade still works on the rebuilt table.
    assert conn.execute("SELECT doc_id FROM sections WHERE id = 3").fetchone()["doc_id"] == 7
    conn.execute("DELETE FROM documents WHERE id = 7")
    conn.commit()
    assert conn.execute("SELECT COUNT(*) AS c FROM sections").fetchone()["c"] == 0
    conn.close()


def test_foreign_keys_cascade(db: Path):
    conn = get_connection(db)
    conn.execute(
        "INSERT INTO documents(id, content_hash, source_type, source_uri, raw_path, ingest_model)"
        " VALUES (1, 'h1', 'pdf', 'u', 'p', 'qwen2.5:7b')"
    )
    conn.execute(
        "INSERT INTO sections(id, doc_id, position) VALUES (1, 1, 0)"
    )
    conn.execute(
        "INSERT INTO propositions(id, section_id, doc_id, position, text, embed_model)"
        " VALUES (1, 1, 1, 0, 'claim', 'nomic-embed-text')"
    )
    conn.commit()
    # Deleting the document cascades to sections and propositions.
    conn.execute("DELETE FROM documents WHERE id = 1")
    conn.commit()
    assert conn.execute("SELECT COUNT(*) AS c FROM sections").fetchone()["c"] == 0
    assert conn.execute("SELECT COUNT(*) AS c FROM propositions").fetchone()["c"] == 0
    conn.close()
