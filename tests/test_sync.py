"""Step 10: repo sync + ingest lock + watcher repo drops.

ingest_repo / LLM passes are monkeypatched where the orchestration (not the pipeline) is
under test; synthetic git repos in tmp_path.
"""

import shutil
import subprocess
from pathlib import Path

import pytest

from locus import sync as sync_mod
from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.ingest_lock import IngestLockHeld, ingest_lock
from locus.ingest_pipeline import IngestResult
from locus.sync import sync_repos

HAS_GIT = shutil.which("git") is not None
needs_git = pytest.mark.skipif(not HAS_GIT, reason="git not installed")


def _git(repo: Path, *args: str) -> None:
    subprocess.run(
        ["git", "-C", str(repo), "-c", "user.email=t@t", "-c", "user.name=t", *args],
        check=True, capture_output=True,
    )


def _make_repo(root: Path, *, git: bool = True) -> Path:
    root.mkdir(parents=True, exist_ok=True)
    (root / "main.py").write_text("def main():\n    return 42\n")
    (root / "README.md").write_text("# Thing\n\nA useful readme with enough words in it.\n")
    if git:
        _git(root, "init", "-q")
        _git(root, "add", "-A")
        _git(root, "commit", "-qm", "initial")
    return root


@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "sync.db"
    migrate(db)
    c = get_connection(db)
    yield c
    c.close()


# --- sync_repos orchestration ----------------------------------------------------------------


@needs_git
def test_sync_skips_when_head_matches(tmp_path, conn, monkeypatch):
    repo = _make_repo(tmp_path / "proj")
    head = subprocess.run(
        ["git", "-C", str(repo), "rev-parse", "HEAD"], capture_output=True, text=True, check=True
    ).stdout.strip()
    conn.execute(
        "INSERT INTO documents (content_hash, source_type, source_uri, raw_path, ingest_model)"
        " VALUES (?, 'code', ?, 'm.json', 'test')",
        (head, str(repo.resolve())),
    )
    conn.commit()
    called = []
    monkeypatch.setattr(sync_mod, "reingest_repo_incremental", lambda *a, **k: called.append(a) or None)

    results = sync_repos(conn, [repo])
    assert [r.status for r in results] == ["skipped"]
    assert called == []  # no ingest attempted (cheap HEAD-match skip, before any blob hashing)


@needs_git
def test_sync_ingests_on_new_commit_and_force(tmp_path, conn, monkeypatch):
    repo = _make_repo(tmp_path / "proj")
    calls = []
    monkeypatch.setattr(
        sync_mod, "reingest_repo_incremental",
        lambda r, c, **k: calls.append(k) or IngestResult(str(r), "ingested", doc_id=1),
    )
    # No prior doc -> ingests.
    assert [r.status for r in sync_repos(conn, [repo])] == ["ingested"]
    # force passes through.
    sync_repos(conn, [repo], force=True)
    assert calls[-1]["force"] is True


def test_sync_skips_non_git_and_missing_paths(tmp_path, conn):
    plain = _make_repo(tmp_path / "plain", git=False)
    results = sync_repos(conn, [plain, tmp_path / "nope"])
    assert [r.status for r in results] == ["quarantined", "quarantined"]
    assert "not a git repository" in results[0].error
    assert "not a directory" in results[1].error


# --- ingest lock ------------------------------------------------------------------------------


def test_ingest_lock_contention(tmp_path, monkeypatch):
    # Point the lock at tmp via the config's db path parent.
    import locus.ingest_lock as lock_mod

    class _Paths:
        db = tmp_path / "vault" / "x.db"

    class _Cfg:
        paths = _Paths()

    monkeypatch.setattr(lock_mod, "load", lambda: _Cfg())

    with ingest_lock():
        # A second holder in the same process opens a new fd -> flock denies it.
        with pytest.raises(IngestLockHeld, match="Another ingest/sync is running"):
            with ingest_lock():
                pass
    # Released -> acquirable again.
    with ingest_lock():
        pass


# --- CLI registration -------------------------------------------------------------------------


def test_cli_sync_dispatch(monkeypatch, tmp_path):
    from locus import cli

    called = {}

    def fake_sync(conn, repos=None, force=False):
        called["repos"] = repos
        called["force"] = force
        return []

    monkeypatch.setattr("locus.sync.sync_repos", fake_sync)
    monkeypatch.setattr(cli, "_open", lambda: type("C", (), {"close": lambda self: None})())
    import locus.ingest_lock as lock_mod

    class _Paths:
        db = tmp_path / "vault" / "x.db"

    class _Cfg:
        paths = _Paths()

    monkeypatch.setattr(lock_mod, "load", lambda: _Cfg())

    cli.main(["sync", "--force", "/some/repo"])
    assert called["force"] is True
    assert called["repos"] == [Path("/some/repo")]
