"""Stage: watcher — file disposition (ingest/skip remove; failures quarantined; settling/gitkeep ignored).

ingest_file is monkeypatched so these test the watcher's file-handling logic without models.
"""

import os
import time
from contextlib import contextmanager
from pathlib import Path
from types import SimpleNamespace

import pytest

from locus import watcher as watcher_mod
from locus.ingest_lock import IngestLockHeld
from locus.ingest_pipeline import IngestResult
from locus.watcher import QUARANTINE_DIRNAME, process_once


@pytest.fixture()
def incoming(tmp_path: Path) -> Path:
    d = tmp_path / "incoming"
    d.mkdir()
    (d / ".gitkeep").touch()
    return d


def _drop(incoming: Path, name: str, *, age: float = 100.0) -> Path:
    p = incoming / name
    p.write_text("content")
    old = time.time() - age
    os.utime(p, (old, old))  # make it "settled"
    return p


def _fake_ingest(status: str):
    return lambda path, conn: IngestResult(str(path), status)


def test_ingested_file_is_removed(incoming, monkeypatch):
    monkeypatch.setattr(watcher_mod, "ingest_file", _fake_ingest("ingested"))
    f = _drop(incoming, "paper.pdf")
    results = process_once(object(), incoming=incoming)
    assert [r.status for r in results] == ["ingested"]
    assert not f.exists()


def test_skipped_file_is_removed(incoming, monkeypatch):
    monkeypatch.setattr(watcher_mod, "ingest_file", _fake_ingest("skipped"))
    f = _drop(incoming, "dup.pdf")
    process_once(object(), incoming=incoming)
    assert not f.exists()


def test_quarantined_file_is_set_aside(incoming, monkeypatch):
    monkeypatch.setattr(watcher_mod, "ingest_file", _fake_ingest("quarantined"))
    _drop(incoming, "bad.pdf")
    process_once(object(), incoming=incoming)
    assert (incoming / QUARANTINE_DIRNAME / "bad.pdf").exists()
    assert not (incoming / "bad.pdf").exists()


def test_unsupported_file_is_set_aside(incoming, monkeypatch):
    monkeypatch.setattr(watcher_mod, "ingest_file", _fake_ingest("unsupported"))
    _drop(incoming, "notes.txt")
    process_once(object(), incoming=incoming)
    assert (incoming / QUARANTINE_DIRNAME / "notes.txt").exists()


def test_gitkeep_and_quarantine_dir_ignored(incoming, monkeypatch):
    calls = []
    monkeypatch.setattr(watcher_mod, "ingest_file", lambda p, c: calls.append(p) or IngestResult(str(p), "ingested"))
    (incoming / QUARANTINE_DIRNAME).mkdir()
    process_once(object(), incoming=incoming)
    assert calls == []  # only .gitkeep + the hidden quarantine dir present


def test_subfolder_drops_are_seen(incoming, monkeypatch):
    """Category subfolders (incoming/notes/x.md) are the laptop-outbox convention — the
    watcher must recurse, or categorized drops sit forever (step-8 verification finding)."""
    seen = []
    monkeypatch.setattr(
        watcher_mod, "ingest_file",
        lambda p, c: seen.append(Path(p)) or IngestResult(str(p), "ingested"),
    )
    (incoming / "notes").mkdir()
    f = _drop(incoming / "notes", "note.md")
    process_once(object(), incoming=incoming)
    assert seen == [f]
    assert not f.exists()


def test_drained_subfolder_is_pruned(incoming, monkeypatch):
    """An ingested file's now-empty parent folder is removed — no remnant husks in incoming
    (incoming/career/career-documents/ left behind after its files ingest)."""
    monkeypatch.setattr(watcher_mod, "ingest_file", _fake_ingest("ingested"))
    sub = incoming / "career" / "career-documents"
    sub.mkdir(parents=True)
    _drop(sub, "cv.pdf")
    process_once(object(), incoming=incoming)
    assert not sub.exists()  # nested folder pruned
    assert not (incoming / "career").exists()  # and its now-empty category parent
    assert incoming.exists() and (incoming / ".gitkeep").exists()  # root anchor untouched


def test_quarantining_last_file_prunes_drained_subfolder(incoming, monkeypatch):
    """When the only file in a drop subfolder is unsupported/quarantined, the move empties
    the source folder — prune it too, or it lingers as a remnant (observed: an empty
    notebook quarantined out of notes/alpha-fund/ left the folder behind)."""
    monkeypatch.setattr(watcher_mod, "ingest_file", _fake_ingest("unsupported"))
    sub = incoming / "notes" / "alpha-fund"
    sub.mkdir(parents=True)
    _drop(sub, "empty.ipynb")
    process_once(object(), incoming=incoming)
    assert (incoming / QUARANTINE_DIRNAME / "notes" / "alpha-fund" / "empty.ipynb").exists()  # set aside
    assert not sub.exists() and not (incoming / "notes").exists()  # source side pruned
    assert (incoming / ".gitkeep").exists()  # root anchor untouched


def test_prune_stops_at_pending_sibling(incoming, monkeypatch):
    """A folder still holding a settle-pending sibling is NOT pruned when one file ingests —
    rmdir is self-guarding, so the folder survives to a later tick."""
    monkeypatch.setattr(watcher_mod, "ingest_file", _fake_ingest("ingested"))
    sub = incoming / "papers"
    sub.mkdir()
    _drop(sub, "settled.pdf")  # ingested this tick
    pending = sub / "arriving.pdf"
    pending.write_text("x")  # current mtime -> within settle window, not a candidate
    process_once(object(), incoming=incoming, settle=3.0)
    assert sub.exists() and pending.exists()  # folder kept; pending file still there


def test_subfolder_quarantine_preserves_subpath(incoming, monkeypatch):
    monkeypatch.setattr(watcher_mod, "ingest_file", _fake_ingest("unsupported"))
    (incoming / "projects").mkdir()
    _drop(incoming / "projects", "blob.bin")
    process_once(object(), incoming=incoming)
    assert (incoming / QUARANTINE_DIRNAME / "projects" / "blob.bin").exists()


def test_quarantine_subtree_is_not_rescanned(incoming, monkeypatch):
    calls = []
    monkeypatch.setattr(
        watcher_mod, "ingest_file",
        lambda p, c: calls.append(p) or IngestResult(str(p), "ingested"),
    )
    (incoming / QUARANTINE_DIRNAME / "papers").mkdir(parents=True)
    _drop(incoming / QUARANTINE_DIRNAME / "papers", "old.bin")
    process_once(object(), incoming=incoming)
    assert calls == []  # quarantined files stay quarantined


def test_unsettled_file_is_skipped(incoming, monkeypatch):
    monkeypatch.setattr(watcher_mod, "ingest_file", _fake_ingest("ingested"))
    fresh = incoming / "arriving.pdf"
    fresh.write_text("x")  # current mtime -> within settle window
    process_once(object(), incoming=incoming, settle=3.0)
    assert fresh.exists()  # not ingested yet — still settling


# --- watch / watch-repo separation + shared-lock mutual exclusion --------------------------


def _patch_loop(monkeypatch, tmp_path, *, repos=("/r",)):
    """Stub config/db so watch() and watch_repos() loops run in isolation. Returns a dict
    recording process_once / sync_repos calls and the lock-acquired flag."""
    rec = {"process": 0, "sync": [], "locked": 0}
    cfg = SimpleNamespace(
        paths=SimpleNamespace(db=tmp_path / "x.db", incoming=tmp_path / "in"),
        repos=SimpleNamespace(paths=list(repos), check_interval=0.01),
    )
    monkeypatch.setattr(watcher_mod, "load", lambda: cfg)
    monkeypatch.setattr(watcher_mod, "migrate", lambda db: None)
    monkeypatch.setattr(watcher_mod, "get_connection", lambda db: SimpleNamespace(close=lambda: None))
    monkeypatch.setattr(watcher_mod, "process_once", lambda conn, **k: rec.__setitem__("process", rec["process"] + 1) or [])
    monkeypatch.setattr(watcher_mod, "sync_repos", lambda conn, repos: rec["sync"].append(repos) or [])

    @contextmanager
    def _lock():
        rec["locked"] += 1
        yield
    monkeypatch.setattr(watcher_mod, "ingest_lock", _lock)
    return rec, cfg


def test_watch_is_incoming_only(monkeypatch, tmp_path):
    rec, _ = _patch_loop(monkeypatch, tmp_path)
    watcher_mod.watch(once=True)
    assert rec["process"] == 1 and rec["locked"] == 1
    assert rec["sync"] == []  # repo sync no longer rides the incoming watcher


def test_watch_repos_syncs_tracked_repos_under_lock(monkeypatch, tmp_path):
    rec, _ = _patch_loop(monkeypatch, tmp_path, repos=("/a", "/b"))
    watcher_mod.watch_repos(once=True)
    assert rec["sync"] == [[Path("/a"), Path("/b")]]  # synced the configured repos
    assert rec["locked"] == 1 and rec["process"] == 0  # under the lock; no incoming work


def test_watch_repo_tick_skips_when_lock_held(monkeypatch, tmp_path):
    """Mutual exclusion: if the shared ingest lock is held (the pour mid-tick), a watch-repo
    tick skips rather than interrupting it — no sync runs."""
    rec, _ = _patch_loop(monkeypatch, tmp_path)

    @contextmanager
    def _held():
        raise IngestLockHeld("held by locus watch")
        yield  # pragma: no cover

    monkeypatch.setattr(watcher_mod, "ingest_lock", _held)
    watcher_mod.watch_repos(once=True)  # must not raise; tick skipped
    assert rec["sync"] == []  # the pour was not interrupted
