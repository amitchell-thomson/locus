"""Crash-safe journaling of agent runs into `agent_runs` (agent-layer plan §6.4, §10).

Every scheduled or on-demand agent run (capture-sync, enrich, structure) opens a row BEFORE it
does any work — committed immediately as `running` — and closes it with a terminal status when
done. If the process dies mid-run, the row survives as `running`: an orphan a later run or
`locus status` can see, rather than a silent gap. This is the same crash-safety `link`/`retitle`
get from per-verdict pass_cache commits, applied to whole runs.

`run()` is the ergonomic entry: a context manager that opens the row, hands back a mutable handle
(the caller fills `stats` and may set `status='degraded'`), and writes the terminal row on exit —
`error` (with the exception message) if the block raised, else the handle's status.
"""

from __future__ import annotations

import json
from contextlib import contextmanager
from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import Callable, Iterator


def _utcnow() -> str:
    return datetime.now(timezone.utc).isoformat()


@dataclass
class RunHandle:
    """Live handle for an open run. The caller fills `stats` (counts, cost, notes) and may set
    `status='degraded'` to record a partial success; `run()` writes it on exit."""

    id: int
    kind: str
    status: str = "ok"
    stats: dict = field(default_factory=dict)


def start_run(conn, kind: str, *, now: Callable[[], str] = _utcnow) -> int:
    """Open a run row as `running` and COMMIT it, so a crash leaves a visible orphan. Returns id."""
    with conn:
        cur = conn.execute(
            "INSERT INTO agent_runs (kind, started_at, status) VALUES (?,?,'running')",
            (kind, now()),
        )
    return cur.lastrowid


def finish_run(
    conn, run_id: int, status: str, *, stats: dict | None = None,
    now: Callable[[], str] = _utcnow,
) -> None:
    """Close a run row with a terminal status + JSON stats, committed."""
    with conn:
        conn.execute(
            "UPDATE agent_runs SET finished_at=?, status=?, stats=? WHERE id=?",
            (now(), status, json.dumps(stats or {}), run_id),
        )


@contextmanager
def run(conn, kind: str, *, now: Callable[[], str] = _utcnow) -> Iterator[RunHandle]:
    """Journal a run: open `running`, yield a handle, close `ok`/`degraded`/`error` on exit.

    A raised exception is recorded as `error` (with its message under stats['error']) and
    re-raised — journaling never swallows the failure, it just records it."""
    handle = RunHandle(id=start_run(conn, kind, now=now), kind=kind)
    try:
        yield handle
    except Exception as exc:
        finish_run(conn, handle.id, "error", stats={**handle.stats, "error": str(exc)}, now=now)
        raise
    finish_run(conn, handle.id, handle.status, stats=handle.stats, now=now)


def recent_runs(conn, kind: str | None = None, *, limit: int = 20) -> list[dict]:
    """Most-recent runs, optionally filtered by kind.

    NO PRODUCTION CALLER TODAY — `locus status` and `health.check` query `agent_runs` directly.
    The docstring used to claim `locus status` used it. Kept as the accessor the journal tests
    read through.
    """
    sql = "SELECT id, kind, started_at, finished_at, status, stats FROM agent_runs"
    params: list = []
    if kind is not None:
        sql += " WHERE kind=?"
        params.append(kind)
    sql += " ORDER BY started_at DESC, id DESC LIMIT ?"
    params.append(limit)
    out = []
    for r in conn.execute(sql, params):
        row = dict(r)
        row["stats"] = json.loads(row["stats"]) if row["stats"] else {}
        out.append(row)
    return out
