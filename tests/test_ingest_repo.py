"""Step 10: repo ingest orchestration — write shape, idempotency, cache, atomic replace.

LLM passes + embeddings are monkeypatched (no Ollama); synthetic git repos in tmp_path.
"""

import shutil
import subprocess
from pathlib import Path

import pytest

from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.ingest import embed, entities, gaps, propositions, summarize, synthesis
from locus.ingest.synthesis import DocSynthesis
from locus import ingest_pipeline
from locus.ingest_pipeline import ingest_file, ingest_repo

HAS_GIT = shutil.which("git") is not None
needs_git = pytest.mark.skipif(not HAS_GIT, reason="git not installed")

PY_A = '''\
def alpha(x):
    """Doubles x."""
    return x * 2


class Engine:
    def run(self):
        return alpha(1)
'''

PY_B = "def beta(y):\n    return y + 1\n"


def _git(repo: Path, *args: str) -> None:
    subprocess.run(
        ["git", "-C", str(repo), "-c", "user.email=t@t", "-c", "user.name=t", *args],
        check=True, capture_output=True,
    )


def _make_repo(root: Path, *, git: bool = True) -> Path:
    root.mkdir(parents=True, exist_ok=True)
    (root / "a.py").write_text(PY_A)
    (root / "b.py").write_text(PY_B)
    (root / "README.md").write_text("# Repo\n\nDoes things, extensively documented here.\n")
    if git:
        _git(root, "init", "-q")
        _git(root, "add", "-A")
        _git(root, "commit", "-qm", "initial")
    return root


def _count(conn, table: str) -> int:
    return conn.execute(f"SELECT COUNT(*) AS c FROM {table}").fetchone()["c"]


@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "repo.db"
    migrate(db)
    c = get_connection(db)
    yield c
    c.close()


@pytest.fixture()
def fake_passes(monkeypatch):
    """Deterministic stand-ins for the LLM passes + embeddings (mirrors test_ingest_pipeline)."""
    calls = {"summaries": [], "props": 0, "llm_entities": 0}

    def _summary(title, text, **k):
        calls["summaries"].append(title)
        return summarize.SectionSummary(summary=f"summary::{title}", title="Semantic Title")

    def _props(title, text, **k):
        calls["props"] += 1
        return ["claim"]

    def _ents(title, text, **k):
        calls["llm_entities"] += 1
        return []

    monkeypatch.setattr(summarize, "summarize_section", _summary)
    monkeypatch.setattr(propositions, "extract_propositions", _props)
    monkeypatch.setattr(entities, "extract_entities", _ents)
    monkeypatch.setattr(
        synthesis, "synthesize_document",
        lambda title, summaries, **k: DocSynthesis(
            thesis="T", method="M", result="R", limitations="L", title="Repo Title"
        ),
    )
    monkeypatch.setattr(gaps, "flag_gaps", lambda title, context, **k: [])
    monkeypatch.setattr(embed, "embed_text", lambda text: [0.1] * 768)
    monkeypatch.setattr(embed, "embed_texts", lambda texts: [[0.1] * 768 for _ in texts])
    # Keep test repos out of the real vault/raw.
    monkeypatch.setattr(
        ingest_pipeline, "_write_repo_manifest",
        lambda snap, repo: f"{snap.content_hash}.manifest.json",
    )
    return calls


@needs_git
def test_repo_ingests_with_full_provenance(tmp_path, conn, fake_passes):
    repo = _make_repo(tmp_path / "proj")
    result = ingest_repo(repo, conn)

    assert result.status == "ingested"
    assert result.sections == 3  # README.md, a.py, b.py
    assert result.propositions == 0  # pass skipped for code
    assert fake_passes["props"] == 0  # the LLM propositions pass never ran
    assert fake_passes["llm_entities"] == 0  # entities came from the AST, not the LLM

    doc = conn.execute("SELECT * FROM documents").fetchone()
    assert doc["source_type"] == "code"
    assert doc["category"] == "project"
    assert doc["source_uri"] == str(repo.resolve())
    assert len(doc["content_hash"]) == 40  # git HEAD sha
    assert doc["raw_path"].endswith(".manifest.json")
    assert doc["title"] == "Repo Title"  # 'proj' is suspect -> synthesis arbitration

    # Sections carry file_path; the .py ones carry a call graph.
    a = conn.execute("SELECT * FROM sections WHERE file_path='a.py'").fetchone()
    assert a is not None
    assert "Engine.run" in a["call_graph"]

    # Chunks carry function-level line provenance.
    rows = conn.execute(
        "SELECT raw_text, file_path, line_start, line_end FROM chunks WHERE file_path='a.py'"
    ).fetchall()
    alpha = next(r for r in rows if r["raw_text"].startswith("def alpha"))
    assert alpha["line_start"] == 1 and alpha["line_end"] >= 3

    # AST entities, not LLM ones.
    ents = {r["name"] for r in conn.execute("SELECT name FROM entities")}
    assert {"alpha", "Engine", "Engine.run"} <= ents


@needs_git
def test_same_head_skips_and_new_commit_replaces(tmp_path, conn, fake_passes):
    repo = _make_repo(tmp_path / "proj")
    first = ingest_repo(repo, conn)
    assert first.status == "ingested"

    again = ingest_repo(repo, conn)
    assert again.status == "skipped"
    assert again.doc_id == first.doc_id
    assert _count(conn, "documents") == 1

    old_hash = conn.execute("SELECT content_hash FROM documents").fetchone()["content_hash"]
    (repo / "b.py").write_text("def beta(y):\n    return y + 2\n")
    _git(repo, "add", "-A")
    _git(repo, "commit", "-qm", "tweak")
    replaced = ingest_repo(repo, conn)
    assert replaced.status == "ingested"
    assert _count(conn, "documents") == 1  # replaced, not duplicated
    row = conn.execute("SELECT content_hash FROM documents").fetchone()
    assert row["content_hash"] != old_hash  # the new commit's sha
    new_b = conn.execute("SELECT raw_text FROM chunks WHERE file_path='b.py'").fetchone()
    assert "y + 2" in new_b["raw_text"]  # content actually refreshed
    assert _count(conn, "section_vectors") == replaced.sections  # no orphaned vectors


@needs_git
def test_summary_cache_skips_unchanged_files(tmp_path, conn, fake_passes):
    repo = _make_repo(tmp_path / "proj")
    ingest_repo(repo, conn)
    assert len(fake_passes["summaries"]) == 3  # cold: all files summarized

    (repo / "b.py").write_text("def beta(y):\n    return y + 3\n")
    _git(repo, "add", "-A")
    _git(repo, "commit", "-qm", "change b only")
    ingest_repo(repo, conn)
    # Only the changed file misses the cache.
    assert fake_passes["summaries"][3:] == ["b.py"]


@needs_git
def test_failed_prepare_leaves_old_doc_intact(tmp_path, conn, fake_passes, monkeypatch):
    repo = _make_repo(tmp_path / "proj")
    first = ingest_repo(repo, conn)
    assert first.status == "ingested"

    (repo / "b.py").write_text("def beta(y):\n    return y + 9\n")
    _git(repo, "add", "-A")
    _git(repo, "commit", "-qm", "doomed")

    def boom(doc, source_type, summary_cache=None):
        raise RuntimeError("prepare exploded")
    monkeypatch.setattr(ingest_pipeline, "_prepare_doc", boom)

    result = ingest_repo(repo, conn)
    assert result.status == "quarantined"
    # The prior document survived the failed re-ingest (prepare-first ordering).
    assert _count(conn, "documents") == 1
    assert conn.execute("SELECT id FROM documents").fetchone()["id"] == first.doc_id


def test_ingest_file_reingest_failure_keeps_old_doc(tmp_path, conn, fake_passes, monkeypatch):
    """Regression for the latent ordering bug: a failed file re-ingest must not destroy
    the existing document (the delete used to commit before prepare ran)."""
    md = tmp_path / "note.md"
    md.write_text("# Note\n\n" + "Sentence with several words. " * 30)
    monkeypatch.setattr(
        ingest_pipeline, "_copy_to_raw", lambda path, content_hash: f"{content_hash}.md"
    )
    first = ingest_file(md, conn)
    assert first.status == "ingested"

    def boom(path, source_type):
        raise RuntimeError("prepare exploded")
    monkeypatch.setattr(ingest_pipeline, "_prepare", boom)

    result = ingest_file(md, conn, reingest=True)
    assert result.status == "quarantined"
    assert _count(conn, "documents") == 1
    assert conn.execute("SELECT id FROM documents").fetchone()["id"] == first.doc_id


def test_non_git_tree_ingests_with_manifest_hash(tmp_path, conn, fake_passes):
    repo = _make_repo(tmp_path / "snapshot", git=False)
    result = ingest_repo(repo, conn, source_uri="locusdrop:snapshot", raw_name="x.tar.gz")
    assert result.status == "ingested"
    doc = conn.execute("SELECT * FROM documents").fetchone()
    assert doc["source_uri"] == "locusdrop:snapshot"
    assert len(doc["content_hash"]) == 64  # manifest hash, not a commit sha
    assert doc["raw_path"] == "x.tar.gz"

    # Unchanged tree re-dropped -> skipped via the same locusdrop key.
    again = ingest_repo(repo, conn, source_uri="locusdrop:snapshot", raw_name="x.tar.gz")
    assert again.status == "skipped"
