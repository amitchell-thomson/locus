"""`locus status` collection is model-free DB aggregation — assert the counts and the
warning logic on a seeded tiny corpus."""

from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest

from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.status import collect_status, format_status


@pytest.fixture()
def conn_and_paths(tmp_path: Path):
    db = tmp_path / "locus.db"
    migrate(db)
    conn = get_connection(db)
    with conn:
        for h, cat, st in (("h1", "paper", "pdf"), ("h2", "paper", "pdf"), ("h3", "code", "code")):
            conn.execute(
                "INSERT INTO documents (content_hash, source_type, source_uri, raw_path, title, "
                "ingested_at, ingest_model, category) VALUES (?,?,?,?,?,?,?,?)",
                (h, st, f"{h}.x", f"{h}.x", f"Doc {h}", "2026-01-01 00:00:00", "test", cat),
            )
    conn.close()
    return {
        "db": db,
        "incoming": tmp_path / "incoming",
        "backups": tmp_path / "backups",
        "root": tmp_path,
    }


def _collect(p, **kw):
    conn = get_connection(p["db"])
    try:
        return collect_status(
            conn, db_path=p["db"], incoming=p["incoming"], backup_root=p["backups"],
            project_root=p["root"], **kw,
        )
    finally:
        conn.close()


def test_counts_and_categories(conn_and_paths):
    r = _collect(conn_and_paths)
    assert r.doc_total == 3
    assert r.by_category == {"paper": 2, "code": 1}
    assert r.by_source_type == {"pdf": 2, "code": 1}
    assert r.last_ingest == "2026-01-01 00:00:00"


def test_warns_when_no_backup_and_no_aliases(conn_and_paths):
    r = _collect(conn_and_paths)
    joined = " ".join(r.warnings)
    assert "no backup snapshot found" in joined
    assert "alias substrate not built" in joined
    # format_status renders without error and echoes the warnings.
    out = format_status(r)
    assert "WARNINGS" in out and "documents" in out


def test_quarantine_count(conn_and_paths):
    qdir = conn_and_paths["incoming"] / ".quarantine" / "papers"
    qdir.mkdir(parents=True)
    (qdir / "bad.pdf").write_text("x")
    (qdir / ".gitkeep").write_text("")  # dotfiles ignored
    r = _collect(conn_and_paths)
    assert r.quarantine_count == 1
    assert any("quarantined" in w for w in r.warnings)


def test_stale_backup_warns(conn_and_paths, tmp_path):
    # Lay down a snapshot manifest dated 30 days before `now`.
    from locus import backup as bk
    snap = conn_and_paths["backups"] / f"{bk.SNAPSHOT_PREFIX}2026-01-01T00-00-00Z"
    snap.mkdir(parents=True)
    old = datetime(2026, 1, 1, tzinfo=timezone.utc)
    (snap / bk.MANIFEST_NAME).write_text(
        '{"created_at": "%s", "git_sha": null, "db_bytes": 1, "db_page_count": 1, '
        '"doc_count": 3, "raw_bytes": 0, "raw_files": 0, "notes_files": 0, "linked_from": null}'
        % old.isoformat()
    )
    r = _collect(conn_and_paths, now=old + timedelta(days=30))
    assert r.last_backup_age_days == pytest.approx(30, abs=0.1)
    assert any("days old" in w for w in r.warnings)
