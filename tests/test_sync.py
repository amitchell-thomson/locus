"""Step 10: repo sync + ingest lock + watcher repo drops.

ingest_repo / LLM passes are monkeypatched where the orchestration (not the pipeline) is
under test; synthetic git repos in tmp_path.
"""

import shutil
import subprocess
import tarfile
import time
from pathlib import Path

import pytest

from locus import sync as sync_mod
from locus import watcher as watcher_mod
from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.ingest_lock import IngestLockHeld, ingest_lock
from locus.ingest_pipeline import IngestResult
from locus.sync import sync_repos
from locus.watcher import QUARANTINE_DIRNAME, process_once

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


# --- watcher repo drops -----------------------------------------------------------------------


@pytest.fixture()
def incoming(tmp_path: Path) -> Path:
    d = tmp_path / "incoming"
    (d / "projects").mkdir(parents=True)
    return d


def _age_tree(tree: Path) -> None:
    """ctime cannot be back-dated; patch time.time forward instead in tests that need it."""


def test_repo_drop_ingested_as_one_unit(incoming, tmp_path, monkeypatch):
    drop = _make_repo(incoming / "projects" / "mytool", git=False)
    tarballs = []
    monkeypatch.setattr(
        watcher_mod, "_tarball_tree", lambda tree, dest: tarballs.append(dest)
    )
    seen = []
    monkeypatch.setattr(
        watcher_mod, "ingest_repo",
        lambda tree, conn, **k: seen.append((Path(tree).name, k))
        or IngestResult(str(tree), "ingested", doc_id=1),
    )
    # The tree's ctime is fresh; advance the watcher's clock past the settle window.
    real_time = time.time()
    monkeypatch.setattr(watcher_mod.time, "time", lambda: real_time + 60)

    results = process_once(object(), incoming=incoming)
    assert [r.status for r in results] == ["ingested"]
    assert seen[0][0] == "mytool"
    assert seen[0][1]["source_uri"] == "locusdrop:mytool"
    assert seen[0][1]["raw_name"].endswith(".tar.gz")
    assert len(tarballs) == 1
    assert not drop.exists()  # disposed after ingest


def test_unsettled_repo_drop_waits(incoming, monkeypatch):
    _make_repo(incoming / "projects" / "fresh", git=False)
    monkeypatch.setattr(
        watcher_mod, "ingest_repo",
        lambda *a, **k: pytest.fail("unsettled tree must not be ingested"),
    )
    # Real clock: ctimes are 'now', inside the 30s settle window.
    results = process_once(object(), incoming=incoming)
    assert results == []
    assert (incoming / "projects" / "fresh").exists()


def test_files_inside_repo_drop_not_picked_individually(incoming, monkeypatch):
    _make_repo(incoming / "projects" / "mytool", git=False)
    picked = []
    monkeypatch.setattr(
        watcher_mod, "ingest_file",
        lambda p, c: picked.append(p) or IngestResult(str(p), "ingested"),
    )
    monkeypatch.setattr(
        watcher_mod, "ingest_repo",
        lambda tree, conn, **k: IngestResult(str(tree), "ingested", doc_id=1),
    )
    monkeypatch.setattr(watcher_mod, "_tarball_tree", lambda tree, dest: None)
    real_time = time.time()
    monkeypatch.setattr(watcher_mod.time, "time", lambda: real_time + 60)

    process_once(object(), incoming=incoming)
    assert picked == []  # the README.md inside the tree was claimed by the repo unit


def test_loose_file_under_projects_still_per_file(incoming, monkeypatch):
    loose = incoming / "projects" / "writeup.md"
    loose.write_text("# Writeup\n\nProse about a project, ingested as a normal file.\n")
    picked = []
    monkeypatch.setattr(
        watcher_mod, "ingest_file",
        lambda p, c: picked.append(Path(p).name) or IngestResult(str(p), "ingested"),
    )
    real_time = time.time()
    monkeypatch.setattr(watcher_mod.time, "time", lambda: real_time + 60)

    process_once(object(), incoming=incoming)
    assert picked == ["writeup.md"]


def test_dir_under_other_category_is_not_a_repo(incoming, monkeypatch):
    conf = incoming / "papers" / "conference2026"
    conf.mkdir(parents=True)
    (conf / "talk.md").write_text("# Talk\n\nNotes from a conference talk, plain file.\n")
    picked = []
    monkeypatch.setattr(
        watcher_mod, "ingest_file",
        lambda p, c: picked.append(Path(p).name) or IngestResult(str(p), "ingested"),
    )
    monkeypatch.setattr(
        watcher_mod, "ingest_repo",
        lambda *a, **k: pytest.fail("papers/ dirs are organization, not repo drops"),
    )
    real_time = time.time()
    monkeypatch.setattr(watcher_mod.time, "time", lambda: real_time + 60)

    process_once(object(), incoming=incoming)
    assert picked == ["talk.md"]


def test_failed_repo_drop_quarantined_with_tree(incoming, monkeypatch):
    _make_repo(incoming / "projects" / "bad", git=False)
    monkeypatch.setattr(watcher_mod, "_tarball_tree", lambda tree, dest: None)
    monkeypatch.setattr(
        watcher_mod, "ingest_repo",
        lambda tree, conn, **k: IngestResult(str(tree), "quarantined", error="boom"),
    )
    real_time = time.time()
    monkeypatch.setattr(watcher_mod.time, "time", lambda: real_time + 60)

    process_once(object(), incoming=incoming)
    assert not (incoming / "projects" / "bad").exists()
    assert (incoming / QUARANTINE_DIRNAME / "projects" / "bad" / "main.py").exists()


def test_tarball_tree_roundtrip(tmp_path):
    tree = _make_repo(tmp_path / "t", git=False)
    dest = tmp_path / "out" / "x.tar.gz"
    watcher_mod._tarball_tree(tree, dest)
    assert dest.exists()
    with tarfile.open(dest) as tar:
        names = tar.getnames()
    assert "t/main.py" in names


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
