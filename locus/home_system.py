"""What Home's system section and the System and Budget pages show, as one JSON object (home-live
H7; the pages made live in cleanup C3, 13 Nov 2026: "it should properly show real data as that
real data exists").

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


def _timers() -> list[dict]:
    """Every locus timer: its next and last run, and how its service last ended."""
    try:
        out = subprocess.run(["systemctl", "--user", "list-timers", "locus-*", "--all", "-o", "json"],
                             capture_output=True, text=True, timeout=10).stdout
        timers = json.loads(out or "[]")
    except (OSError, subprocess.SubprocessError, ValueError):
        return []
    services = [t.get("activates", "") for t in timers if t.get("activates")]
    results: dict[str, str] = {}
    if services:
        try:
            shown = subprocess.run(["systemctl", "--user", "show", "-p", "Id,Result", *services],
                                   capture_output=True, text=True, timeout=10).stdout
        except (OSError, subprocess.SubprocessError):
            shown = ""
        unit = ""
        for line in shown.splitlines():
            if line.startswith("Id="):
                unit = line[3:]
            elif line.startswith("Result="):
                results[unit] = line[7:]

    def at(us) -> str | None:
        return datetime.fromtimestamp(us / 1e6, timezone.utc).isoformat() if us else None

    return [{"name": t["unit"].removeprefix("locus-").removesuffix(".timer"),
             "next": at(t.get("next")), "last": at(t.get("last")),
             "result": results.get(t.get("activates", ""), "")}
            for t in timers if t.get("unit")]


def _budget(conn: sqlite3.Connection, now: datetime, cycle_day: int) -> dict:
    """The credit this cycle, today's allowance (agent/claude.credit_allows's rule), the last
    seven days by job, and this cycle's jobs. Costs are ESTIMATES from [agent].api_prices; the
    console is exact."""
    from calendar import monthrange

    from locus.agent import usage

    cycle = usage.cycle_start(cycle_day, now)
    end = cycle + timedelta(days=monthrange(cycle.year, cycle.month)[1])
    used = usage.credit_used(conn, since=cycle.isoformat())
    day0, _ = usage.windows(now)
    today = usage.credit_used(conn, since=day0)
    left_days = max(1.0, (end - now).total_seconds() / 86400)
    daily = max(0.0, 100.0 - used) / left_days
    week0 = (now - timedelta(days=6)).replace(hour=0, minute=0, second=0, microsecond=0)
    days: dict[str, dict[str, float]] = {}
    for d in range(7):
        days[(week0 + timedelta(days=d)).date().isoformat()] = {}
    for day, job, usd in conn.execute(
        "SELECT substr(at, 1, 10), job, SUM(cost_usd) FROM model_calls "
        "WHERE backend='api' AND at >= ? GROUP BY 1, 2", (week0.isoformat(),)):
        if day in days and usd:
            days[day][job or "?"] = round(usd, 3)
    jobs = [{"job": t.job or "?", "backend": t.backend, "calls": t.calls, "failed": t.failed,
             "usd": round(t.cost_usd, 3)}
            for t in usage.totals(conn, since=cycle.isoformat())]
    jobs.sort(key=lambda j: (-j["usd"], -j["calls"]))
    return {
        "cap": 100.0,
        "used": round(used, 2),
        "since": cycle.date().isoformat(),
        "until": end.date().isoformat(),
        "today": round(today, 2),
        "daily": round(daily, 2),
        "bulk_daily": round(daily * 0.7, 2),
        "days": [{"date": k, "by_job": [{"job": j, "usd": v} for j, v in sorted(by.items(), key=lambda x: -x[1])]}
                 for k, by in days.items()],
        "jobs": jobs[:12],
    }


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
        if len(recent) < 12:
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
        "timers": _timers(),
        "budget": _budget(conn, now, cycle_day),
    }
