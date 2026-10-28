"""The budget record (Slice 0.7): every headless call is one `model_calls` row, named by its
job, failures included, and `locus budget` sums them. No subprocess and no live DB."""

from __future__ import annotations

import json
import subprocess
from datetime import datetime, timezone
from pathlib import Path

import pytest

from locus.agent import claude, journal, usage
from locus.db.connection import get_connection
from locus.db.migrate import migrate


@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "u.db"
    migrate(db)
    c = get_connection(db)
    yield c
    c.close()


@pytest.fixture()
def recorded(monkeypatch):
    rows: list = []
    monkeypatch.setattr(usage, "SINK", rows.append)
    return rows


class _Proc:
    def __init__(self, stdout, returncode=0, stderr=""):
        self.stdout, self.returncode, self.stderr = stdout, returncode, stderr


def _reply(monkeypatch, *, is_error=False):
    line = json.dumps({"type": "result", "result": "ok", "is_error": is_error,
                       "total_cost_usd": 0.02,
                       "usage": {"input_tokens": 5, "cache_creation_input_tokens": 700,
                                 "cache_read_input_tokens": 100, "output_tokens": 40,
                                 "output_tokens_details": {"thinking_tokens": 0}}})
    monkeypatch.setattr(subprocess, "run", lambda cmd, **kw: _Proc(line + "\n"))


def test_a_call_is_recorded_with_its_tokens_images_and_job(monkeypatch, recorded):
    _reply(monkeypatch)
    with usage.job("capture"):
        claude.content_runner([claude.image_block(b"png"), {"type": "text", "text": "x"}],
                              "sonnet", None)
    (r,) = recorded
    assert (r.job, r.model, r.ok, r.images) == ("capture", "sonnet", True, 1)
    assert (r.input_tokens, r.cache_creation_tokens, r.cache_read_tokens, r.output_tokens) == (
        5, 700, 100, 40)
    assert r.cost_usd == pytest.approx(0.02)


def test_a_failed_call_is_recorded_too(monkeypatch, recorded):
    """A retry loop that keeps failing is exactly the spend nobody sees."""
    _reply(monkeypatch, is_error=True)
    with pytest.raises(claude.ClaudeError):
        claude.content_runner([{"type": "text", "text": "x"}], "haiku", None)
    assert [(r.ok, bool(r.error)) for r in recorded] == [(False, True)]


def test_a_journaled_run_names_the_job_of_every_call_inside_it(conn, monkeypatch, recorded):
    _reply(monkeypatch)
    usage.set_default_job("watch")
    claude.content_runner([{"type": "text", "text": "x"}], None, None)
    with journal.run(conn, "structure"):
        claude.content_runner([{"type": "text", "text": "x"}], None, None)
    claude.content_runner([{"type": "text", "text": "x"}], None, None)
    assert [r.job for r in recorded] == ["watch", "structure", "watch"]


def test_recording_never_costs_a_call(monkeypatch):
    def broken(rec):
        raise RuntimeError("database is locked")

    monkeypatch.setattr(usage, "SINK", broken)
    _reply(monkeypatch)
    assert claude.content_runner([{"type": "text", "text": "x"}], None, None).text == "ok"


def test_budget_sums_by_job_over_a_window(conn):
    rec = usage.CallRecord(job="capture", model="sonnet", ok=True, duration_ms=30_000,
                           input_tokens=10, cache_creation_tokens=90, output_tokens=50,
                           thinking_tokens=0, images=1, cost_usd=0.01)
    usage.write(conn, rec, at="2026-10-28T09:00:00+00:00")
    usage.write(conn, rec, at="2026-10-28T09:30:00+00:00")
    usage.write(conn, usage.CallRecord(job="watch", model="haiku", ok=False, duration_ms=60_000,
                                       error="timed out"), at="2026-10-28T10:00:00+00:00")
    usage.write(conn, rec, at="2026-10-20T09:00:00+00:00")          # outside the window
    rows = {r.job: r for r in usage.totals(conn, since="2026-10-28T00:00:00+00:00")}
    assert rows["capture"].calls == 2 and rows["capture"].input_tokens == 200
    assert rows["capture"].output_tokens == 100 and rows["capture"].minutes == pytest.approx(1.0)
    assert rows["watch"].failed == 1
    text = usage.format_totals("today", list(rows.values()))
    assert "capture" in text and "total" in text


def test_windows_are_today_and_the_last_seven_days():
    today, week = usage.windows(datetime(2026, 10, 28, 15, 0, tzinfo=timezone.utc))
    assert today == "2026-10-28T00:00:00+00:00"
    assert week == "2026-10-21T15:00:00+00:00"
