"""Incremental note ingest (agent-layer §6.7, Phase 1 ③). The diff/delete/manifest logic is
isolated from ingestion by injecting a fake `ingest_file` (recording calls + inserting a doc row
so deletions resolve), on a real migrated DB. Pure helpers are tested directly."""

from __future__ import annotations

from pathlib import Path

import pytest

from locus import notes_sync
from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.ingest_pipeline import IngestResult
from locus.notes_sync import _iter_notes, _read_maturity, normalized_hash, sync_notes


# ---------- pure helpers ----------

def test_normalized_hash_ignores_whitespace_only_changes():
    a = normalized_hash("# Note\n\nBody.\n")
    b = normalized_hash("# Note   \r\n\r\nBody.  \n\n\n")  # trailing ws, CRLF, extra blank lines
    d = normalized_hash("# Note\n\n\n\nBody.\n")            # internal blank-line run
    c = normalized_hash("# Note\n\nBody edited.\n")         # real change
    assert a == b
    assert a == d   # runs of blank lines collapse -> no churn
    assert a != c


def test_iter_notes_applies_exclusions(tmp_path: Path):
    (tmp_path / "keep.md").write_text("x", encoding="utf-8")
    (tmp_path / "sub").mkdir()
    (tmp_path / "sub" / "nested.md").write_text("x", encoding="utf-8")
    (tmp_path / "_home.md").write_text("x", encoding="utf-8")
    (tmp_path / "note.locus.md").write_text("x", encoding="utf-8")
    (tmp_path / "note.sync-conflict-20260101-120000-AB.md").write_text("x", encoding="utf-8")
    (tmp_path / ".hidden.md").write_text("x", encoding="utf-8")
    (tmp_path / "_generated").mkdir()
    (tmp_path / "_generated" / "surfaced.md").write_text("x", encoding="utf-8")
    (tmp_path / "not_markdown.txt").write_text("x", encoding="utf-8")

    names = {p.name for p in _iter_notes(tmp_path)}
    assert names == {"keep.md", "nested.md"}


def test_read_maturity_from_frontmatter(tmp_path: Path):
    rough = tmp_path / "r.md"
    rough.write_text("---\nmaturity: rough\n---\nbody\n", encoding="utf-8")
    tidy = tmp_path / "t.md"
    tidy.write_text("---\nmaturity: TIDY\n---\nbody\n", encoding="utf-8")
    plain = tmp_path / "p.md"
    plain.write_text("no frontmatter\n", encoding="utf-8")
    assert _read_maturity(rough, "tidy") == "rough"
    assert _read_maturity(tidy, "rough") == "tidy"       # case-insensitive
    assert _read_maturity(plain, "rough") == "rough"      # falls back to default


# ---------- sync orchestration ----------

@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "notes.db"
    migrate(db)
    c = get_connection(db)
    yield c
    c.close()


@pytest.fixture()
def fake_ingest(monkeypatch):
    calls: list[tuple[str, str | None]] = []

    def _fake(path, conn, *, reingest=False, maturity=None, category=None, replace_uri=None):
        # Faithful to real ingest_file: content-hash identity, but replace_uri supersedes the doc
        # at that path (an edit REPLACES, never duplicates). content_hash varies with content.
        calls.append((Path(path).name, maturity))
        if replace_uri is not None:
            conn.execute("DELETE FROM documents WHERE source_uri=?", (replace_uri,))
        h = normalized_hash(Path(path).read_text(encoding="utf-8"))[:12]
        cur = conn.execute(
            "INSERT INTO documents (content_hash, source_type, source_uri, raw_path, title, "
            "ingest_model, maturity, category) VALUES (?,?,?,?,?,?,?,?)",
            (f"{path}:{h}", "markdown", str(path), str(path), Path(path).stem, "test",
             maturity, category),
        )
        conn.commit()
        return IngestResult(str(path), "ingested", doc_id=cur.lastrowid)

    monkeypatch.setattr(notes_sync, "ingest_file", _fake)
    return calls


def _sync(conn, notes_dir, manifest, **kw):
    return sync_notes(conn, notes_dir, manifest_path=manifest, **kw)


def test_first_sync_ingests_all(tmp_path, conn, fake_ingest):
    nd = tmp_path / "notes"
    nd.mkdir()
    (nd / "a.md").write_text("A\n", encoding="utf-8")
    (nd / "b.md").write_text("---\nmaturity: tidy\n---\nB\n", encoding="utf-8")
    manifest = tmp_path / "m.json"

    r = _sync(conn, nd, manifest)
    assert (r.ingested, r.skipped, r.deleted, r.failed) == (2, 0, 0, 0)
    assert {name for name, _ in fake_ingest} == {"a.md", "b.md"}
    # a.md has no frontmatter -> default rough; b.md -> tidy from frontmatter
    assert dict(fake_ingest) == {"a.md": "rough", "b.md": "tidy"}
    # notes are filed as category='note' (outside the incoming/<cat>/ folder convention)
    cats = {r["category"] for r in conn.execute("SELECT DISTINCT category FROM documents")}
    assert cats == {"note"}


def test_unchanged_and_whitespace_only_resave_skip(tmp_path, conn, fake_ingest):
    nd = tmp_path / "notes"
    nd.mkdir()
    note = nd / "a.md"
    note.write_text("Body.\n", encoding="utf-8")
    manifest = tmp_path / "m.json"
    _sync(conn, nd, manifest)
    fake_ingest.clear()

    note.write_text("Body.  \n\n\n", encoding="utf-8")  # whitespace-only re-save
    r = _sync(conn, nd, manifest)
    assert (r.ingested, r.skipped) == (0, 1)
    assert fake_ingest == []  # ingest NOT called — the point of normalised hashing


def test_real_edit_reingests(tmp_path, conn, fake_ingest):
    nd = tmp_path / "notes"
    nd.mkdir()
    note = nd / "a.md"
    note.write_text("Body.\n", encoding="utf-8")
    manifest = tmp_path / "m.json"
    _sync(conn, nd, manifest)
    fake_ingest.clear()

    note.write_text("Body, revised.\n", encoding="utf-8")
    r = _sync(conn, nd, manifest)
    assert r.ingested == 1
    assert fake_ingest == [("a.md", "rough")]
    # the edit REPLACED the prior doc at this path — exactly one doc, no duplicate
    assert conn.execute("SELECT COUNT(*) FROM documents").fetchone()[0] == 1


def test_deleted_note_removes_document(tmp_path, conn, fake_ingest):
    nd = tmp_path / "notes"
    nd.mkdir()
    note = nd / "a.md"
    note.write_text("A\n", encoding="utf-8")
    manifest = tmp_path / "m.json"
    _sync(conn, nd, manifest)
    assert conn.execute("SELECT COUNT(*) FROM documents").fetchone()[0] == 1

    note.unlink()
    r = _sync(conn, nd, manifest)
    assert r.deleted == 1
    assert conn.execute("SELECT COUNT(*) FROM documents").fetchone()[0] == 0


def test_failed_ingest_stays_out_of_manifest_and_retries(tmp_path, conn, monkeypatch):
    nd = tmp_path / "notes"
    nd.mkdir()
    (nd / "a.md").write_text("A\n", encoding="utf-8")
    manifest = tmp_path / "m.json"

    calls = {"n": 0}

    def flaky(path, conn, *, reingest=False, maturity=None, category=None, replace_uri=None):
        calls["n"] += 1
        return IngestResult(str(path), "quarantined", error="boom")

    monkeypatch.setattr(notes_sync, "ingest_file", flaky)
    r1 = _sync(conn, nd, manifest)
    assert (r1.ingested, r1.failed) == (0, 1)
    r2 = _sync(conn, nd, manifest)  # still not in manifest -> retried
    assert r2.failed == 1
    assert calls["n"] == 2
