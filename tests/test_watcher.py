"""Stage: watcher — file disposition (ingest/skip remove; failures quarantined; settling/gitkeep ignored).

ingest_file is monkeypatched so these test the watcher's file-handling logic without models.
"""

import os
import time
from pathlib import Path

import pytest

from locus import watcher as watcher_mod
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


def test_unsettled_file_is_skipped(incoming, monkeypatch):
    monkeypatch.setattr(watcher_mod, "ingest_file", _fake_ingest("ingested"))
    fresh = incoming / "arriving.pdf"
    fresh.write_text("x")  # current mtime -> within settle window
    process_once(object(), incoming=incoming, settle=3.0)
    assert fresh.exists()  # not ingested yet — still settling
