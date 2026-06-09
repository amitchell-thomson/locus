"""Backup/restore round-trips the durable corpus state without a live model.

Seeds a tiny real DB (via migrations) plus raw/notes trees, snapshots them, mutates the
live state, then restores and asserts the snapshot state is back. Exercises the shutil
fallback path of _copy_tree by construction (no model, small trees); the rsync path is the
same contract."""

from datetime import datetime, timezone
from pathlib import Path

import pytest

from locus import backup as bk
from locus.db.connection import get_connection
from locus.db.migrate import migrate


@pytest.fixture()
def vault(tmp_path: Path) -> dict:
    db = tmp_path / "locus.db"
    migrate(db)
    conn = get_connection(db)
    with conn:
        conn.execute(
            "INSERT INTO documents (content_hash, source_type, source_uri, raw_path, title, ingest_model) "
            "VALUES ('h1', 'text', 'a.txt', 'h1.txt', 'Doc One', 'test')"
        )
    conn.close()
    raw = tmp_path / "raw"
    raw.mkdir()
    (raw / "h1.txt").write_text("original raw bytes")
    notes = tmp_path / "notes"
    notes.mkdir()
    (notes / "note.md").write_text("an authored note")
    return {"db": db, "raw": raw, "notes": notes, "root": tmp_path / "backups"}


def _doc_count(db: Path) -> int:
    conn = get_connection(db)
    try:
        return conn.execute("SELECT COUNT(*) FROM documents").fetchone()[0]
    finally:
        conn.close()


def test_backup_writes_manifest_and_data(vault):
    snap = bk.create_backup(
        db=vault["db"], raw_store=vault["raw"], notes=vault["notes"],
        backup_root=vault["root"], now=datetime(2026, 1, 2, 3, 4, 5, tzinfo=timezone.utc),
    )
    assert snap.name == "locus-backup-2026-01-02T03-04-05Z"
    assert (snap / "locus.db").exists()
    assert (snap / "raw" / "h1.txt").read_text() == "original raw bytes"
    assert (snap / "notes" / "note.md").exists()
    m = bk.read_manifest(snap)
    assert m.doc_count == 1
    assert m.raw_files == 1
    assert m.db_page_count > 0


def test_restore_reverts_live_state(vault):
    snap = bk.create_backup(
        db=vault["db"], raw_store=vault["raw"], notes=vault["notes"],
        backup_root=vault["root"], now=datetime(2026, 1, 2, 3, 4, 5, tzinfo=timezone.utc),
    )
    # Mutate live state after the snapshot: add a doc, rewrite a raw file.
    conn = get_connection(vault["db"])
    with conn:
        conn.execute(
            "INSERT INTO documents (content_hash, source_type, source_uri, raw_path, title, ingest_model) "
            "VALUES ('h2', 'text', 'b.txt', 'h2.txt', 'Doc Two', 'test')"
        )
    conn.close()
    (vault["raw"] / "h1.txt").write_text("CORRUPTED")
    assert _doc_count(vault["db"]) == 2

    bk.restore_backup(
        snap, db=vault["db"], raw_store=vault["raw"], notes=vault["notes"]
    )
    assert _doc_count(vault["db"]) == 1  # the post-snapshot insert is gone
    assert (vault["raw"] / "h1.txt").read_text() == "original raw bytes"


def test_incremental_snapshot_links_against_prior(vault):
    first = bk.create_backup(
        db=vault["db"], raw_store=vault["raw"], notes=vault["notes"],
        backup_root=vault["root"], now=datetime(2026, 1, 2, 3, 4, 5, tzinfo=timezone.utc),
    )
    second = bk.create_backup(
        db=vault["db"], raw_store=vault["raw"], notes=vault["notes"],
        backup_root=vault["root"], now=datetime(2026, 1, 3, 3, 4, 5, tzinfo=timezone.utc),
    )
    assert first != second
    m = bk.read_manifest(second)
    assert m.linked_from == first.name
