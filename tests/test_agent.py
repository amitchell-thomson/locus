"""Agent-layer foundation (agent-layer plan §10, Phase 1 ②): the claude -p runner, the
agent_runs journal, and the budget ledger. All model-free — the runner is injected, so no
subprocess or API is spawned."""

from __future__ import annotations

import os
from datetime import datetime, timezone
from pathlib import Path

import pytest
from pydantic import BaseModel

from locus.agent import budget, claude, journal
from locus.agent.budget import BudgetExceeded, BudgetLedger, spent_today
from locus.agent.claude import ClaudeError, ClaudeResult, call, run_structured, run_text
from locus.db.connection import get_connection
from locus.db.migrate import migrate


# ---------- claude.py runner ----------

class _Verdict(BaseModel):
    ok: bool
    n: int


def _result(text: str, cost: float = 0.01) -> ClaudeResult:
    return ClaudeResult(text=text, cost_usd=cost, usage={"input_tokens": 100, "output_tokens": 20})


def test_run_structured_parses_and_validates():
    runner = lambda prompt, model: _result('{"ok": true, "n": 3}')
    out = run_structured("p", schema=_Verdict, runner=runner)
    assert out == _Verdict(ok=True, n=3)


def test_run_structured_tolerates_prose_and_fences():
    runner = lambda prompt, model: _result('Sure!\n```json\n{"ok": false, "n": 0}\n```\nDone.')
    assert run_structured("p", schema=_Verdict, runner=runner).n == 0


def test_call_retries_transient_then_succeeds():
    calls = {"n": 0}

    def flaky(prompt, model):
        calls["n"] += 1
        if calls["n"] < 3:
            raise ClaudeError("transient connector warning")
        return _result("ok")

    res = call("p", runner=flaky, retries=2, backoff_s=0)
    assert res.text == "ok"
    assert calls["n"] == 3


def test_call_raises_after_exhausting_retries():
    def always_fail(prompt, model):
        raise ClaudeError("boom")

    with pytest.raises(ClaudeError, match="boom"):
        call("p", runner=always_fail, retries=2, backoff_s=0)


def test_run_structured_retries_on_invalid_then_degrades():
    def bad(prompt, model):
        return _result("not json at all")

    with pytest.raises(ClaudeError):
        run_structured("p", schema=_Verdict, runner=bad, retries=1, backoff_s=0)


def test_on_result_sink_receives_successful_result():
    seen: list[ClaudeResult] = []
    call("p", runner=lambda p, m: _result("hi", cost=0.02), on_result=seen.append, backoff_s=0)
    assert len(seen) == 1 and seen[0].cost_usd == 0.02


def test_run_text_returns_text():
    assert run_text("p", runner=lambda p, m: _result("transcribed")) == "transcribed"


def test_scrubbed_env_drops_metered_keys(monkeypatch):
    monkeypatch.setenv("ANTHROPIC_API_KEY", "sk-metered")
    monkeypatch.setenv("ANTHROPIC_AUTH_TOKEN", "tok")
    monkeypatch.setenv("PATH_KEEP", "keep")
    env = claude._scrubbed_env()
    assert "ANTHROPIC_API_KEY" not in env
    assert "ANTHROPIC_AUTH_TOKEN" not in env
    assert env.get("PATH_KEEP") == "keep"  # unrelated vars survive


# ---------- journal.py ----------

@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "agent.db"
    migrate(db)
    c = get_connection(db)
    yield c
    c.close()


def test_start_run_commits_running_row(conn):
    rid = journal.start_run(conn, "capture")
    row = conn.execute("SELECT kind, status, finished_at FROM agent_runs WHERE id=?", (rid,)).fetchone()
    assert row["kind"] == "capture"
    assert row["status"] == "running"  # visible before finish -> crash leaves a visible orphan
    assert row["finished_at"] is None


def test_run_context_records_ok_and_stats(conn):
    with journal.run(conn, "enrich") as h:
        h.stats["notes"] = 4
    row = journal.recent_runs(conn, "enrich")[0]
    assert row["status"] == "ok"
    assert row["stats"]["notes"] == 4
    assert row["finished_at"] is not None


def test_run_context_records_degraded(conn):
    with journal.run(conn, "capture") as h:
        h.status = "degraded"
        h.stats["skipped"] = 1
    assert journal.recent_runs(conn, "capture")[0]["status"] == "degraded"


def test_run_context_records_error_and_reraises(conn):
    with pytest.raises(ValueError, match="kaboom"):
        with journal.run(conn, "structure"):
            raise ValueError("kaboom")
    row = journal.recent_runs(conn, "structure")[0]
    assert row["status"] == "error"
    assert "kaboom" in row["stats"]["error"]


# ---------- budget.py ----------

def test_ledger_accumulates_and_caps():
    led = BudgetLedger(cap_usd=0.05)
    led.record(_result("a", cost=0.02))
    led.record(_result("b", cost=0.02))
    led.check()  # 0.04 < 0.05 -> fine
    led.record(_result("c", cost=0.02))
    assert led.over_cap()  # 0.06 >= 0.05
    with pytest.raises(BudgetExceeded):
        led.check()
    assert led.calls == 3
    assert led.stats()["input_tokens"] == 300


def test_spent_today_sums_from_agent_runs(conn):
    now = datetime(2026, 7, 28, 15, 0, tzinfo=timezone.utc)
    midnight = "2026-07-28T00:00:00+00:00"
    yesterday = "2026-07-27T23:00:00+00:00"
    # Two runs today (cost 0.10 + 0.25) and one yesterday (0.99, excluded).
    conn.execute(
        "INSERT INTO agent_runs (kind, started_at, status, stats) VALUES "
        "('capture', ?, 'ok', '{\"cost_usd\": 0.10}'),"
        "('enrich',  ?, 'ok', '{\"cost_usd\": 0.25}'),"
        "('capture', ?, 'ok', '{\"cost_usd\": 0.99}')",
        (midnight, now.isoformat(), yesterday),
    )
    conn.commit()
    assert spent_today(conn, now=now) == pytest.approx(0.35)
