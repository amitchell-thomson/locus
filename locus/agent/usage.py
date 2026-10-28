"""What the subscription is spent on: one row per headless Claude call (Slice 0.7).

`agent/claude.content_runner` is the one place every model call passes through (Slice 0.13), so
it records each call here, success or failure: the job, the model, the tokens (input, cache
written, cache read, output, thinking), the time, how many images, and the envelope's notional
cost. `locus budget` reads it back by job, for today and the last seven days.

WHICH JOB. A journaled run (`journal.run(conn, "capture")`) names the job for every call made
inside it; outside one, the CLI names it after its subcommand. A call that knows neither is
`unknown`, which is itself worth seeing.

Recording never breaks a call: a failure to write is logged and dropped. Tests replace the sink
(tests/conftest.py), so no test writes to the live database.
"""

from __future__ import annotations

import contextvars
import logging
import sqlite3
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone

log = logging.getLogger(__name__)

_JOB: contextvars.ContextVar[str] = contextvars.ContextVar("locus_job", default="unknown")


@dataclass(frozen=True)
class CallRecord:
    job: str
    model: str
    ok: bool
    duration_ms: int
    input_tokens: int = 0
    cache_creation_tokens: int = 0
    cache_read_tokens: int = 0
    output_tokens: int = 0
    thinking_tokens: int = 0
    images: int = 0
    cost_usd: float = 0.0
    error: str | None = None


def current_job() -> str:
    return _JOB.get()


def set_default_job(name: str) -> None:
    """Name the job for the rest of this process, unless a journaled run names it more closely."""
    _JOB.set(name)


@contextmanager
def job(name: str) -> Iterator[None]:
    token = _JOB.set(name)
    try:
        yield
    finally:
        _JOB.reset(token)


def record_from_usage(
    *, model: str | None, ok: bool, duration_ms: int, usage: dict | None, images: int,
    cost_usd: float = 0.0, error: str | None = None,
) -> CallRecord:
    u = usage or {}
    details = u.get("output_tokens_details") or {}
    return CallRecord(
        job=current_job(), model=model or "", ok=ok, duration_ms=duration_ms,
        input_tokens=int(u.get("input_tokens") or 0),
        cache_creation_tokens=int(u.get("cache_creation_input_tokens") or 0),
        cache_read_tokens=int(u.get("cache_read_input_tokens") or 0),
        output_tokens=int(u.get("output_tokens") or 0),
        thinking_tokens=int(details.get("thinking_tokens") or 0),
        images=images, cost_usd=cost_usd, error=error,
    )


def write(conn: sqlite3.Connection, rec: CallRecord, *, at: str | None = None) -> None:
    with conn:
        conn.execute(
            "INSERT INTO model_calls (at, job, model, ok, duration_ms, input_tokens, "
            "cache_creation_tokens, cache_read_tokens, output_tokens, thinking_tokens, images, "
            "cost_usd, error) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
            (at or datetime.now(timezone.utc).isoformat(), rec.job, rec.model, int(rec.ok),
             rec.duration_ms, rec.input_tokens, rec.cache_creation_tokens, rec.cache_read_tokens,
             rec.output_tokens, rec.thinking_tokens, rec.images, rec.cost_usd,
             (rec.error or None) and rec.error[:500]),
        )


def _live_sink(rec: CallRecord) -> None:
    from locus.config import load
    from locus.db.connection import get_connection

    conn = get_connection(load().paths.db)
    try:
        write(conn, rec)
    finally:
        conn.close()


# Replaced in tests (tests/conftest.py). Never raises into the caller: see `record`.
SINK: Callable[[CallRecord], None] = _live_sink


def record(rec: CallRecord) -> None:
    try:
        SINK(rec)
    except Exception as exc:                      # counting must never cost a call
        log.warning("model-call usage not recorded: %s", exc)


# --- reading it back ------------------------------------------------------------------------------

@dataclass(frozen=True)
class JobTotals:
    job: str
    calls: int
    failed: int
    input_tokens: int          # fresh input + cache written + cache read
    output_tokens: int
    thinking_tokens: int
    minutes: float
    cost_usd: float


def totals(conn: sqlite3.Connection, *, since: str) -> list[JobTotals]:
    rows = conn.execute(
        "SELECT job, COUNT(*) calls, SUM(1-ok) failed, "
        "SUM(input_tokens+cache_creation_tokens+cache_read_tokens) tin, SUM(output_tokens) tout, "
        "SUM(thinking_tokens) think, SUM(duration_ms) ms, SUM(cost_usd) cost "
        "FROM model_calls WHERE at >= ? GROUP BY job ORDER BY tout DESC, calls DESC",
        (since,),
    ).fetchall()
    return [
        JobTotals(r[0], r[1], r[2] or 0, r[3] or 0, r[4] or 0, r[5] or 0, (r[6] or 0) / 60000,
                  r[7] or 0.0)
        for r in rows
    ]


def windows(now: datetime | None = None) -> tuple[str, str]:
    """(start of today, start of the last seven days), UTC ISO."""
    now = now or datetime.now(timezone.utc)
    today = now.replace(hour=0, minute=0, second=0, microsecond=0)
    return today.isoformat(), (now - timedelta(days=7)).isoformat()


def format_totals(title: str, rows: list[JobTotals]) -> str:
    if not rows:
        return f"{title}: no model calls recorded"
    head = f"{'job':<22}{'calls':>7}{'failed':>8}{'in tok':>11}{'out tok':>10}{'thinking':>10}{'min':>7}{'notional $':>12}"
    lines = [title, head]
    for r in rows:
        lines.append(
            f"{r.job[:22]:<22}{r.calls:>7}{r.failed:>8}{r.input_tokens:>11,}{r.output_tokens:>10,}"
            f"{r.thinking_tokens:>10,}{r.minutes:>7.1f}{r.cost_usd:>12.2f}"
        )
    t = JobTotals("total", sum(r.calls for r in rows), sum(r.failed for r in rows),
                  sum(r.input_tokens for r in rows), sum(r.output_tokens for r in rows),
                  sum(r.thinking_tokens for r in rows), sum(r.minutes for r in rows),
                  sum(r.cost_usd for r in rows))
    lines.append(
        f"{'total':<22}{t.calls:>7}{t.failed:>8}{t.input_tokens:>11,}{t.output_tokens:>10,}"
        f"{t.thinking_tokens:>10,}{t.minutes:>7.1f}{t.cost_usd:>12.2f}"
    )
    return "\n".join(lines)
