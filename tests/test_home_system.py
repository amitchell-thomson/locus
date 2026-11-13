"""home-live H7: what Home's system section reads, from the run journal."""

import sqlite3
from datetime import datetime, timedelta, timezone

from locus import home_system


def _db() -> sqlite3.Connection:
    c = sqlite3.connect(":memory:")
    c.execute("CREATE TABLE agent_runs (id INTEGER PRIMARY KEY, kind TEXT, started_at TEXT, "
              "finished_at TEXT, status TEXT, stats TEXT)")
    c.execute("CREATE TABLE model_calls (at TEXT, job TEXT, backend TEXT, ok INTEGER DEFAULT 1, "
              "input_tokens INT DEFAULT 0, cache_creation_tokens INT DEFAULT 0, cache_read_tokens INT DEFAULT 0, "
              "output_tokens INT DEFAULT 0, thinking_tokens INT DEFAULT 0, duration_ms INT DEFAULT 0, cost_usd REAL)")
    return c


def test_running_done_failed_and_a_dead_run(monkeypatch):
    monkeypatch.setattr(home_system, "_active_units", lambda: ["capture"])
    monkeypatch.setattr(home_system, "_timers", lambda: [])
    now = datetime(2026, 11, 10, 12, tzinfo=timezone.utc)
    c = _db()
    iso = lambda d: (now - d).isoformat()  # noqa: E731
    c.executemany("INSERT INTO agent_runs (kind, started_at, finished_at, status, stats) VALUES (?,?,?,?,?)", [
        ("annotate", iso(timedelta(minutes=5)), None, "running", None),
        ("capture", iso(timedelta(hours=1)), iso(timedelta(hours=1)), "ok", None),
        ("review", iso(timedelta(hours=2)), iso(timedelta(hours=2)), "error", '{"error": "rmapi 429\\nmore"}'),
        ("maintain", iso(timedelta(hours=9)), None, "running", None),
    ])
    c.execute("INSERT INTO model_calls (at, job, backend, cost_usd) VALUES (?, 'heuris-facts', 'api', 1.25)",
              (iso(timedelta(hours=1)),))
    c.execute("INSERT INTO model_calls (at, job, backend, cost_usd) VALUES (?, 'heuris-records', 'api', 0.5)",
              (iso(timedelta(days=3)),))
    out = home_system.collect(c, now=now)
    assert [r["kind"] for r in out["running"]] == ["annotate", "capture"]
    assert out["done_today"] == 1
    assert out["failed_today"] == 2  # the error, and the run that never finished
    causes = {r["kind"]: r["cause"] for r in out["recent"]}
    assert causes["review"] == "rmapi 429"
    assert "never finished" in causes["maintain"]
    assert out["credit_used"] == 1.75 and out["calls_today"] == 1


def test_the_budget_page_reads_the_ledger(monkeypatch):
    """Cleanup C3: the Budget page is the credit this cycle, today's allowance and each day by job."""
    monkeypatch.setattr(home_system, "_active_units", lambda: [])
    monkeypatch.setattr(home_system, "_timers", lambda: [{"name": "heuris", "next": None, "last": None,
                                                         "result": "success"}])
    now = datetime(2026, 11, 10, 12, tzinfo=timezone.utc)
    c = _db()
    for at, job, usd in [(now - timedelta(hours=1), "heuris-facts", 1.0),
                         (now - timedelta(days=2), "heuris-records", 2.0),
                         (now - timedelta(days=40), "old", 9.0)]:
        c.execute("INSERT INTO model_calls (at, job, backend, cost_usd) VALUES (?, ?, 'api', ?)",
                  (at.isoformat(), job, usd))
    b = home_system.collect(c, now=now)["budget"]
    assert b["used"] == 3.0 and b["today"] == 1.0 and b["since"] == "2026-11-01"
    assert b["daily"] == round(97.0 / 20.5, 2) and b["bulk_daily"] == round(b["daily"] * 0.7, 2)
    assert len(b["days"]) == 7 and b["days"][-1]["by_job"] == [{"job": "heuris-facts", "usd": 1.0}]
    assert [j["job"] for j in b["jobs"]] == ["heuris-records", "heuris-facts"]
