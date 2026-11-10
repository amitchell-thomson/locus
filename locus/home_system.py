"""What Home's system section shows, as one JSON object (home-live H7).

Home reads it over ssh every five minutes (`locus home-system`). Everything here is read from
the run journal (`agent_runs`), systemd and `model_calls`; nothing is estimated except the
credit, which says so.

A run still marked `running` after six hours is not running: its process died before it could
write an end (the same rule `health._is_stale` applies). Shown as running it would sit on Home
forever, so it is counted as failed with that cause instead.
"""

from __future__ import annotations

import json
import sqlite3
import subprocess
from datetime import datetime, timedelta, timezone

STALE = timedelta(hours=6)


def _parse(s: str | None) -> datetime | None:
    if not s:
        return None
    try:
        d = datetime.fromisoformat(s)
    except ValueError:
        return None
    return d if d.tzinfo else d.replace(tzinfo=timezone.utc)


def _cause(stats: str | None) -> str:
    try:
        d = json.loads(stats or "{}")
    except ValueError:
        return ""
    if not isinstance(d, dict):
        return ""
    for k in ("error", "cause", "notes"):
        v = d.get(k)
        if isinstance(v, str) and v:
            return v.splitlines()[0][:120]
    return ""


def _active_units() -> list[str]:
    """locus services running now, daemons excluded (kairosd is always running)."""
    try:
        out = subprocess.run(
            ["systemctl", "--user", "list-units", "locus-*.service", "--state=active",
             "--no-legend", "--plain"],
            capture_output=True, text=True, timeout=10,
        ).stdout
    except (OSError, subprocess.SubprocessError):
        return []
    units = [line.split()[0] for line in out.splitlines() if line.strip()]
    return [u.removeprefix("locus-").removesuffix(".service") for u in units
            if u not in ("locus-kairosd.service", "locus-http.service", "locus-mcp.service")]


def collect(conn: sqlite3.Connection, *, now: datetime | None = None,
            cycle_day: int = 1, backend: str = "api") -> dict:
    from locus.agent import usage

    now = now or datetime.now(timezone.utc)
    today = now.astimezone().replace(hour=0, minute=0, second=0, microsecond=0)
    running, recent = [], []
    done = failed = 0
    rows = conn.execute(
        "SELECT kind, started_at, finished_at, status, stats FROM agent_runs "
        "WHERE started_at >= ? ORDER BY started_at DESC, id DESC",
        ((now - timedelta(days=2)).isoformat(),),
    ).fetchall()
    for kind, started, finished, status, stats in rows:
        s = _parse(started)
        if s is None:
            continue
        cause = _cause(stats)
        if status == "running":
            if now - s < STALE:
                running.append({"kind": kind, "started": started})
                continue
            status, cause = "error", "never finished (its process died)"
        job = {"kind": kind, "started": started, "ended": finished, "ok": status in ("ok", "degraded"),
               "cause": cause}
        if len(recent) < 6:
            recent.append(job)
        if s >= today:
            if job["ok"]:
                done += 1
            else:
                failed += 1
    seen = {r["kind"] for r in running}
    for u in _active_units():
        if u not in seen:
            running.append({"kind": u, "started": None})
    cycle = usage.cycle_start(cycle_day, now)
    day0, _ = usage.windows(now)
    calls = conn.execute("SELECT COUNT(*) FROM model_calls WHERE at >= ?", (day0,)).fetchone()[0]
    return {
        "at": now.isoformat(),
        "running": running,
        "recent": recent,
        "done_today": done,
        "failed_today": failed,
        "credit_used": round(usage.credit_used(conn, since=cycle.isoformat()), 2),
        "credit_since": cycle.date().isoformat(),
        "calls_today": calls,
        "backend": backend,
    }
