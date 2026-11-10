"""home-live H7: what Home's system section reads, from the run journal."""

import sqlite3
from datetime import datetime, timedelta, timezone

from locus import home_system


def _db() -> sqlite3.Connection:
    c = sqlite3.connect(":memory:")
    c.execute("CREATE TABLE agent_runs (id INTEGER PRIMARY KEY, kind TEXT, started_at TEXT, "
              "finished_at TEXT, status TEXT, stats TEXT)")
    c.execute("CREATE TABLE model_calls (at TEXT, backend TEXT, cost_usd REAL)")
    return c


def test_running_done_failed_and_a_dead_run(monkeypatch):
    monkeypatch.setattr(home_system, "_active_units", lambda: ["capture"])
    now = datetime(2026, 11, 10, 12, tzinfo=timezone.utc)
    c = _db()
    iso = lambda d: (now - d).isoformat()  # noqa: E731
    c.executemany("INSERT INTO agent_runs (kind, started_at, finished_at, status, stats) VALUES (?,?,?,?,?)", [
        ("annotate", iso(timedelta(minutes=5)), None, "running", None),
        ("capture", iso(timedelta(hours=1)), iso(timedelta(hours=1)), "ok", None),
        ("review", iso(timedelta(hours=2)), iso(timedelta(hours=2)), "error", '{"error": "rmapi 429\\nmore"}'),
        ("maintain", iso(timedelta(hours=9)), None, "running", None),
    ])
    c.execute("INSERT INTO model_calls VALUES (?, 'api', 1.25)", (iso(timedelta(hours=1)),))
    out = home_system.collect(c, now=now)
    assert [r["kind"] for r in out["running"]] == ["annotate", "capture"]
    assert out["done_today"] == 1
    assert out["failed_today"] == 2  # the error, and the run that never finished
    causes = {r["kind"]: r["cause"] for r in out["recent"]}
    assert causes["review"] == "rmapi 429"
    assert "never finished" in causes["maintain"]
    assert out["credit_used"] == 1.25 and out["calls_today"] == 1
