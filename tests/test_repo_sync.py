"""Incremental tracked-repo re-ingest (locus/repo_sync.py).

LLM passes + embeddings are monkeypatched (no Ollama); synthetic git repos in tmp_path.
Unlike test_ingest_repo, the manifest is written for real (to a tmp raw_store) because the
incremental diff reads it back. The fakes count calls so tests can assert that only changed
files are re-prepared.
"""

import shutil
import subprocess
from pathlib import Path

import pytest

from locus import repo_sync
from locus.config import load
from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.ingest import embed, entities, gaps, propositions, summarize, synthesis
from locus.ingest.synthesis import DocSynthesis
from locus.ingest_pipeline import ingest_repo

HAS_GIT = shutil.which("git") is not None
needs_git = pytest.mark.skipif(not HAS_GIT, reason="git not installed")

PY_A = "def alpha(x):\n    return x + 1\n\n\nclass Engine:\n    def run(self):\n        return alpha(1)\n"
PY_B = "def beta(y):\n    return y * 2\n"


def _git(repo: Path, *args: str) -> None:
    subprocess.run(
        ["git", "-C", str(repo), "-c", "user.email=t@t", "-c", "user.name=t", *args],
        check=True, capture_output=True,
    )


def _make_repo(root: Path) -> Path:
    root.mkdir(parents=True, exist_ok=True)
    (root / "a.py").write_text(PY_A)
    (root / "b.py").write_text(PY_B)
    (root / "README.md").write_text("# Repo\n\nDoes things, documented here at length.\n")
    _git(root, "init", "-q")
    _git(root, "add", "-A")
    _git(root, "commit", "-qm", "initial")
    return root


def _count(conn, table: str, where: str = "") -> int:
    return conn.execute(f"SELECT COUNT(*) c FROM {table} {where}").fetchone()["c"]


@pytest.fixture()
def conn(tmp_path: Path, monkeypatch):
    # Real manifests, written to a tmp raw_store (load() is a cached singleton, so both
    # ingest_pipeline and repo_sync see the override; monkeypatch restores it after).
    raw = tmp_path / "raw"
    raw.mkdir()
    monkeypatch.setattr(load().paths, "raw_store", raw)
    db = tmp_path / "repo.db"
    migrate(db)
    c = get_connection(db)
    yield c
    c.close()


@pytest.fixture()
def fakes(monkeypatch):
    calls = {"summaries": [], "embed_texts": 0, "embed_text": 0, "synth": 0}

    def _summary(title, text, **k):
        calls["summaries"].append(title)
        # Echo the text so chunk/summary content is searchable & change-sensitive.
        return summarize.SectionSummary(summary=f"summary of {title}", title="Semantic Title")

    def _synth(title, summaries, **k):
        calls["synth"] += 1
        return DocSynthesis(thesis="T", method="M", result="R", limitations="L", title="Repo Title")

    monkeypatch.setattr(summarize, "summarize_section", _summary)
    monkeypatch.setattr(synthesis, "synthesize_document", _synth)
    monkeypatch.setattr(propositions, "extract_propositions", lambda *a, **k: [])
    monkeypatch.setattr(entities, "extract_entities", lambda *a, **k: [])
    monkeypatch.setattr(gaps, "flag_gaps", lambda *a, **k: [])
    monkeypatch.setattr(embed, "embed_text", lambda t: calls.__setitem__("embed_text", calls["embed_text"] + 1) or [0.1] * 768)
    monkeypatch.setattr(embed, "embed_texts", lambda ts: calls.__setitem__("embed_texts", calls["embed_texts"] + 1) or [[0.1] * 768 for _ in ts])
    return calls


def _reset(calls):
    calls["summaries"].clear()
    calls["embed_texts"] = calls["embed_text"] = calls["synth"] = 0


@needs_git
def test_modify_one_file_reprepares_only_it(tmp_path, conn, fakes):
    repo = _make_repo(tmp_path / "proj")
    ingest_repo(repo, conn)
    doc_id = conn.execute("SELECT id FROM documents").fetchone()["id"]
    b_sid_before = conn.execute("SELECT id FROM sections WHERE doc_id=? AND file_path='b.py'", (doc_id,)).fetchone()["id"]
    old_hash = conn.execute("SELECT content_hash FROM documents").fetchone()["content_hash"]

    (repo / "a.py").write_text(PY_A + "\n\ndef gamma():\n    return 99\n")
    _git(repo, "commit", "-aqm", "edit a")
    _reset(fakes)
    result = repo_sync.reingest_repo_incremental(repo, conn)

    assert result.status == "ingested" and result.doc_id == doc_id  # same document
    assert fakes["summaries"] == ["a.py"]  # ONLY the changed file was re-summarised
    assert fakes["synth"] == 1  # doc synthesis re-ran once
    # b.py's section row is the very same one — untouched.
    assert conn.execute("SELECT id FROM sections WHERE doc_id=? AND file_path='b.py'", (doc_id,)).fetchone()["id"] == b_sid_before
    # content_hash advanced to the new HEAD.
    assert conn.execute("SELECT content_hash FROM documents").fetchone()["content_hash"] != old_hash
    # The new function landed in a chunk.
    assert conn.execute("SELECT 1 FROM chunks WHERE file_path='a.py' AND raw_text LIKE '%gamma%'").fetchone()


@needs_git
def test_add_and_delete_files(tmp_path, conn, fakes):
    repo = _make_repo(tmp_path / "proj")
    ingest_repo(repo, conn)
    doc_id = conn.execute("SELECT id FROM documents").fetchone()["id"]
    n_before = _count(conn, "sections", f"WHERE doc_id={doc_id}")

    (repo / "c.py").write_text("def delta():\n    return 7\n")
    (repo / "b.py").unlink()
    _git(repo, "add", "-A")
    _git(repo, "commit", "-aqm", "add c, drop b")
    _reset(fakes)
    repo_sync.reingest_repo_incremental(repo, conn)

    assert set(fakes["summaries"]) == {"c.py"}  # only the new file re-summarised (b.py just dropped)
    assert _count(conn, "sections", f"WHERE doc_id={doc_id}") == n_before  # +c.py, -b.py
    assert conn.execute("SELECT 1 FROM sections WHERE doc_id=? AND file_path='c.py'", (doc_id,)).fetchone()
    assert conn.execute("SELECT id FROM sections WHERE doc_id=? AND file_path='b.py'", (doc_id,)).fetchone() is None
    # positions are contiguous 0..N-1 with no gaps or collisions
    positions = [r["position"] for r in conn.execute("SELECT position FROM sections WHERE doc_id=? ORDER BY position", (doc_id,))]
    assert positions == list(range(len(positions)))


@needs_git
def test_no_orphan_vectors_after_delete(tmp_path, conn, fakes):
    repo = _make_repo(tmp_path / "proj")
    ingest_repo(repo, conn)
    (repo / "b.py").unlink()
    _git(repo, "commit", "-aqm", "drop b")
    repo_sync.reingest_repo_incremental(repo, conn)
    # vec0 tables carry no FK — a correct delete leaves exactly one vector per row.
    assert _count(conn, "chunk_vectors") == _count(conn, "chunks")
    assert _count(conn, "section_vectors") == _count(conn, "sections")


@needs_git
def test_fts_index_tracks_incremental_edit(tmp_path, conn, fakes):
    repo = _make_repo(tmp_path / "proj")
    ingest_repo(repo, conn)
    # a.py initially has 'alpha'; replace it with a fresh token, drop the old one.
    (repo / "a.py").write_text("def zephyrus(x):\n    return x\n")
    _git(repo, "commit", "-aqm", "rewrite a")
    repo_sync.reingest_repo_incremental(repo, conn)
    # FTS must find the new token and NOT the removed one (trigger fired on explicit delete).
    assert conn.execute("SELECT 1 FROM chunks_fts WHERE chunks_fts MATCH 'zephyrus'").fetchone()
    assert conn.execute("SELECT 1 FROM chunks_fts WHERE chunks_fts MATCH 'alpha'").fetchone() is None


@needs_git
def test_noneligible_commit_skips_without_synthesis(tmp_path, conn, fakes):
    repo = _make_repo(tmp_path / "proj")
    ingest_repo(repo, conn)
    old_hash = conn.execute("SELECT content_hash FROM documents").fetchone()["content_hash"]
    (repo / "notes.txt").write_text("not an ingestible source file\n")  # .txt not eligible in a repo
    _git(repo, "add", "-A")
    _git(repo, "commit", "-aqm", "add non-source")
    _reset(fakes)
    result = repo_sync.reingest_repo_incremental(repo, conn)

    assert result.status == "skipped"
    assert fakes["synth"] == 0 and fakes["summaries"] == []  # no synthesis, no re-summarise
    # but the diff base advanced so the next sync diffs from the right commit
    assert conn.execute("SELECT content_hash FROM documents").fetchone()["content_hash"] != old_hash


@needs_git
def test_first_ingest_falls_back_to_full(tmp_path, conn, fakes):
    repo = _make_repo(tmp_path / "proj")
    result = repo_sync.reingest_repo_incremental(repo, conn)  # no prior doc
    assert result.status == "ingested" and result.sections == 3
