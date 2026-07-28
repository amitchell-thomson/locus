"""Budget metering for agent-layer `claude -p` spend (agent-layer plan §10, Phase-0 finding d).

Phase 0 established that the `claude -p` JSON envelope reports `total_cost_usd` + full token
`usage` per call, so metering is a DIRECT LEDGER READ — no scraping of rate-limit errors. Two
layers:

  - `BudgetLedger` — an in-memory accumulator fed by the runner's `on_result` hook
    (`call(..., on_result=ledger.record)`). A loop calls `ledger.check()` between units of work
    to stop before blowing a session cap.
  - `spent_today()` — the cross-process daily total, summed from `agent_runs.stats.cost_usd`
    (every journaled run records its cost there), so a cap survives restarts and multiple runs.

NOT YET IMPLEMENTED — foreground yield. The plan wants background work to *yield to foreground
interactive use* (pause, don't starve it). The reliable trigger for that is the live
subscription-throttle error shape, which Phase 0 could not capture without a real throttle event
(finding d, still open). Until it's observed, this module meters and caps on COST; the
throttle-yield is deliberately absent rather than faked. The cost cap is the working guard today.
"""

from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timezone

from locus.agent.claude import ClaudeResult


class BudgetExceeded(RuntimeError):
    """Raised by `BudgetLedger.check()` when accumulated spend reaches the cap."""


@dataclass
class BudgetLedger:
    """Accumulates cost + tokens across a run's `claude -p` calls (fed via `on_result`)."""

    cap_usd: float | None = None
    spent_usd: float = 0.0
    input_tokens: int = 0
    output_tokens: int = 0
    calls: int = 0

    def record(self, result: ClaudeResult) -> None:
        """Add one call's cost/usage. Wire as `call(prompt, on_result=ledger.record)`."""
        self.spent_usd += result.cost_usd
        self.calls += 1
        usage = result.usage or {}
        self.input_tokens += int(usage.get("input_tokens") or 0)
        self.output_tokens += int(usage.get("output_tokens") or 0)

    def over_cap(self) -> bool:
        return self.cap_usd is not None and self.spent_usd >= self.cap_usd

    def check(self) -> None:
        """Raise `BudgetExceeded` if the cap is set and reached. Call between work units so the
        loop stops BEFORE the next spend, not after."""
        if self.over_cap():
            raise BudgetExceeded(
                f"agent spend ${self.spent_usd:.4f} reached cap ${self.cap_usd:.2f} "
                f"over {self.calls} call(s)"
            )

    def stats(self) -> dict:
        """A JSON-able summary for the run journal's `stats` blob."""
        return {
            "cost_usd": round(self.spent_usd, 6),
            "input_tokens": self.input_tokens,
            "output_tokens": self.output_tokens,
            "calls": self.calls,
        }


def _utc_midnight_iso(now: datetime | None = None) -> str:
    now = now or datetime.now(timezone.utc)
    return now.astimezone(timezone.utc).replace(hour=0, minute=0, second=0, microsecond=0).isoformat()


def spent_today(conn, *, now: datetime | None = None) -> float:
    """Total agent-layer cost (USD) journaled since UTC midnight — the cross-process daily total.

    Summed from `agent_runs.stats.cost_usd`; runs that recorded no cost contribute 0. Used to
    enforce a daily cap that survives restarts (a fresh `BudgetLedger` starts at 0 each run)."""
    row = conn.execute(
        "SELECT COALESCE(SUM(CAST(json_extract(stats,'$.cost_usd') AS REAL)), 0.0) "
        "FROM agent_runs WHERE started_at >= ?",
        (_utc_midnight_iso(now),),
    ).fetchone()
    return float(row[0] or 0.0)
