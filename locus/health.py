"""Does the system still work, and what did it cost — answered without being asked.

THE FAILURE THIS EXISTS FOR. `locus-maintain` failed six consecutive nights and nothing said so;
it was found by accident. The fix is not "log harder" — the logs were there. It is that nothing
ever ASKED whether the nightly work happened, so silence and success looked identical.

THREE WAYS A RUN CAN FAIL, and they need three different detectors, which is why one check is not
enough:

  1. **It ran and broke.** `agent_runs` closes with `error`/`degraded`. Visible already.
  2. **It started and vanished** — killed, OOM, machine rebooted mid-run. The row is left open at
     `running`, forever. Silence here is what made the six nights invisible.
  3. **It never ran at all.** The timer did not fire, the unit failed to start, the import blew
     up. THERE IS NO ROW, and no amount of reading `agent_runs` will produce one. This is the
     case that needs a clock: a run kind that has not appeared within its own cadence is overdue,
     whether or not anything anywhere errored. `timer_failures` (written by systemd's `OnFailure`)
     catches the same case with an exact cause, when systemd is the one that noticed.

OVERDUE IS DERIVED FROM CADENCE, NOT STORED. A stored "expected next run" has to be updated by
something, and the something is the code that is failing. Comparing the newest run of each kind
against how often it is supposed to happen needs no bookkeeping and cannot itself go stale.

SPEND is reported from the same rows, because the ledger already exists: `claude -p` reports
`total_cost_usd` in its envelope and `journal` stores it in `agent_runs.stats`. What was missing
was not a store but a reader — and the fact that almost nothing was journaled, so the numbers had
nothing to sum.
"""

from __future__ import annotations

import json
import sqlite3
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone

# Run kind -> how often it is expected, in hours. Derived from `deploy/systemd/*.timer`; a kind
# absent here is on-demand and is never reported as overdue. The grace multiplier below is what
# stops a timer that fires a few minutes late from reading as a failure.
EXPECTED_CADENCE_HOURS: dict[str, float] = {
    "capture": 1,
    "daily": 24,
    "daily-pull": 1,
    "discover-pull": 1,
    "discover-harvest": 24 * 7,
    "maintain": 24,
    "backup": 24,
}

# A run may be this many times its cadence late before it is called overdue. Generous on purpose:
# a false alarm every morning trains him to ignore the line, which is the failure mode this whole
# module exists to prevent.
OVERDUE_GRACE = 2.5

SEVERITY_ORDER = {"broken": 0, "overdue": 1, "stalled": 2}


@dataclass
class Problem:
    kind: str            # the run kind, or the systemd unit for a hard failure
    severity: str        # 'broken' | 'stalled' | 'overdue'
    detail: str
    since: str = ""

    def render(self) -> str:
        when = f" since {self.since[:16]}" if self.since else ""
        return f"{self.kind} {self.detail}{when}"


@dataclass
class Health:
    problems: list[Problem] = field(default_factory=list)
    ran: dict[str, int] = field(default_factory=dict)      # kind -> successful runs in the window
    # Failures a LATER run of the same kind has since superseded. Kept out of `problems` and off
    # the page, because the status line answers "what is broken now" in the present tense — but
    # still listed by `locus status`, since a job that fails and recovers repeatedly is worth
    # seeing, and silently discarding a failure is how the six invisible maintain nights happened.
    recovered: list[Problem] = field(default_factory=list)
    cost_usd: float = 0.0
    calls: int = 0

    @property
    def ok(self) -> bool:
        return not self.problems

    def summary(self) -> str:
        """The one line the daily page prints at the foot of page 1."""
        made = " · ".join(f"{k} x{n}" if n > 1 else k for k, n in sorted(self.ran.items()))
        spend = f" · ${self.cost_usd:.2f}" if self.cost_usd else ""
        if not self.problems:
            return f"overnight: {made or 'nothing new'}{spend} · all runs healthy"
        loud = " · ".join(p.render().upper() for p in self.problems[:3])
        return f"overnight: {made or 'nothing new'}{spend} · {loud}"


def _parse(stamp: str | None) -> datetime | None:
    if not stamp:
        return None
    try:
        parsed = datetime.fromisoformat(stamp)
    except ValueError:
        return None
    return parsed if parsed.tzinfo else parsed.replace(tzinfo=timezone.utc)


def _last_run_by_kind(conn: sqlite3.Connection) -> dict[str, tuple[str, str]]:
    """kind -> (newest started_at, its status), over all time."""
    out: dict[str, tuple[str, str]] = {}
    for row in conn.execute(
        "SELECT kind, started_at, status FROM agent_runs ORDER BY started_at DESC, id DESC"
    ):
        out.setdefault(row["kind"], (row["started_at"], row["status"]))
    return out


def check(
    conn: sqlite3.Connection,
    *,
    now: datetime | None = None,
    window_hours: int = 24,
    cadence: dict[str, float] | None = None,
) -> Health:
    """Everything wrong, plus what ran and what it cost, over the last `window_hours`."""
    now = now or datetime.now(timezone.utc)
    cadence = EXPECTED_CADENCE_HOURS if cadence is None else cadence
    since = (now - timedelta(hours=window_hours)).isoformat()
    health = Health()

    try:
        recent = conn.execute(
            "SELECT kind, status, started_at, finished_at, stats FROM agent_runs "
            "WHERE started_at >= ? ORDER BY started_at",
            (since,),
        ).fetchall()
    except sqlite3.OperationalError:
        return health

    # A failure the same kind has since RECOVERED from is not a present-tense problem. `recent` is
    # ordered by `started_at`, so the newest success per kind is the last one seen.
    latest_ok: dict[str, str] = {
        r["kind"]: (r["started_at"] or "") for r in recent if r["status"] == "ok"
    }

    for row in recent:
        if row["status"] == "ok":
            health.ran[row["kind"]] = health.ran.get(row["kind"], 0) + 1
        elif row["status"] in ("error", "degraded"):
            problem = Problem(
                row["kind"], "broken", row["status"], since=row["started_at"] or ""
            )
            # A TRANSIENT FAILURE MUST NOT SHOUT ALL DAY. `review` broke once at 03:37 on
            # 2026-08-06 (a syntax error in a file being edited while the timer fired), ran
            # successfully eight times afterwards, and was still printed in capitals on the daily
            # page that afternoon — alongside the `maintain` run it took down, which had also
            # since succeeded. A line that reports settled history as a live fault is one he
            # learns to skip, and then it cannot do the job it exists for.
            if latest_ok.get(row["kind"], "") > (row["started_at"] or ""):
                health.recovered.append(problem)
            else:
                health.problems.append(problem)
        elif row["status"] == "running" and _is_stale(row["started_at"], now):
            # Opened and never closed: the process died. Silence here is what made the six
            # maintain failures invisible.
            health.problems.append(Problem(
                row["kind"], "stalled", "started and never finished", since=row["started_at"] or ""
            ))
        try:
            stats = json.loads(row["stats"] or "{}")
        except (TypeError, ValueError):
            stats = {}
        health.cost_usd += float(stats.get("cost_usd") or 0.0)
        health.calls += int(stats.get("calls") or 0)

    # THE CASE WITH NO ROW. Everything above reads rows that exist; a unit that never started
    # leaves none, and is exactly the failure that hid for six nights.
    last = _last_run_by_kind(conn)
    watching_since = _journalling_since(conn)
    for kind, hours in cadence.items():
        stamp = last.get(kind, (None, None))[0]
        ran_at = _parse(stamp)
        overdue_after = timedelta(hours=hours * OVERDUE_GRACE)
        if ran_at is None:
            # "No row" only means something once we have been WATCHING for longer than the
            # cadence. Journalling at dispatch began 2026-08-02; without this, a healthy weekly
            # job that had run seventeen hours earlier was reported as HAS NEVER RUN, and would
            # have been — in capitals, on the daily page — every morning for the next seven days.
            # `OVERDUE_GRACE` already encodes that a false alarm every morning is the failure
            # this module exists to prevent; the no-row branch simply never applied it.
            if watching_since is None or now - watching_since < overdue_after:
                continue
            health.problems.append(Problem(kind, "overdue", "has never run"))
        elif now - ran_at > overdue_after:
            late = now - ran_at
            health.problems.append(Problem(
                kind, "overdue", f"last ran {_ago(late)} ago (expected every {_hours(hours)})",
                since=stamp or "",
            ))

    hard, hard_recovered = _hard_failures(conn, since=since, latest_ok=latest_ok)
    health.problems += hard
    health.recovered += hard_recovered
    health.problems.sort(key=lambda p: (SEVERITY_ORDER.get(p.severity, 9), p.kind))
    health.recovered.sort(key=lambda p: p.since)
    return health


def _journalling_since(conn: sqlite3.Connection) -> datetime | None:
    """When this system first recorded ANY run — how long "no row" has been meaningful evidence.

    Derived from the data rather than stored, for the same reason `EXPECTED_CADENCE_HOURS` is:
    a stored "watching since" has to be written by something, and nothing can write it for a
    history that predates it.
    """
    row = conn.execute("SELECT MIN(started_at) AS first FROM agent_runs").fetchone()
    return _parse(row["first"] if row else None)


def _is_stale(started_at: str | None, now: datetime) -> bool:
    """A `running` row is only a problem once it is older than any plausible run.

    Two hours: `locus-maintain` sets TimeoutStartSec=7200, so anything still open past that is
    not slow, it is gone.
    """
    started = _parse(started_at)
    return started is not None and (now - started) > timedelta(hours=2)


def _unit_kind(unit: str) -> str:
    """`locus-maintain.service` -> `maintain`, the kind that unit journals its runs under.

    The naming is the contract between the unit files and `locus record <kind> --ok`; without it
    a systemd-observed failure and the Python-observed recovery of the same job cannot be matched.
    """
    return unit.removeprefix("locus-").removesuffix(".service")


def _hard_failures(
    conn: sqlite3.Connection, *, since: str, latest_ok: dict[str, str] | None = None
) -> tuple[list[Problem], list[Problem]]:
    """What systemd saw, split into (live, recovered).

    The only detector that works when the process cannot reach Python — and the only one that can
    see a unit that never started. A failure the same unit has since completed successfully is
    reported as recovered: `locus-maintain.service` died at 03:37 on 2026-08-06 and ran clean at
    04:36, and the daily page was still shouting about the 03:37 death that afternoon.

    Grouped by unit, so `since` is the FIRST failure in the window and the count carries the rest;
    that means a unit is only recovered once its latest success is newer than its latest failure.
    """
    latest_ok = latest_ok or {}
    try:
        rows = conn.execute(
            "SELECT unit, MIN(failed_at) AS failed_at, MAX(failed_at) AS last_failed_at, "
            "detail, COUNT(*) AS n FROM timer_failures "
            "WHERE failed_at >= ? GROUP BY unit ORDER BY unit",
            (since,),
        ).fetchall()
    except sqlite3.OperationalError:
        return [], []
    live: list[Problem] = []
    recovered: list[Problem] = []
    for row in rows:
        times = "" if row["n"] == 1 else f" x{row['n']}"
        cause = f": {row['detail']}" if row["detail"] else ""
        problem = Problem(
            row["unit"], "broken", f"failed to start{times}{cause}", since=row["failed_at"] or "",
        )
        if latest_ok.get(_unit_kind(row["unit"]), "") > (row["last_failed_at"] or ""):
            recovered.append(problem)
        else:
            live.append(problem)
    return live, recovered


def _hours(hours: float) -> str:
    if hours >= 24 * 7:
        return f"{hours / (24 * 7):.0f}w"
    if hours >= 24:
        return f"{hours / 24:.0f}d"
    return f"{hours:.0f}h"


def _ago(delta: timedelta) -> str:
    total = int(delta.total_seconds())
    if total >= 86400:
        return f"{total // 86400}d"
    if total >= 3600:
        return f"{total // 3600}h"
    return f"{max(1, total // 60)}m"


def record_failure(
    conn: sqlite3.Connection, unit: str, detail: str = "", *, now: datetime | None = None
) -> None:
    """Record what systemd saw. Called by `locus record-failure` from `OnFailure=`."""
    stamp = (now or datetime.now(timezone.utc)).isoformat()
    with conn:
        conn.execute(
            "INSERT INTO timer_failures (unit, failed_at, detail) VALUES (?,?,?)",
            (unit, stamp, detail or None),
        )


def spend_by_kind(
    conn: sqlite3.Connection, *, now: datetime | None = None, window_hours: int = 24
) -> list[tuple[str, float, int]]:
    """(kind, cost_usd, calls) over the window, dearest first — what the night actually cost."""
    now = now or datetime.now(timezone.utc)
    since = (now - timedelta(hours=window_hours)).isoformat()
    totals: dict[str, list[float]] = {}
    try:
        rows = conn.execute(
            "SELECT kind, stats FROM agent_runs WHERE started_at >= ?", (since,)
        ).fetchall()
    except sqlite3.OperationalError:
        return []
    for row in rows:
        try:
            stats = json.loads(row["stats"] or "{}")
        except (TypeError, ValueError):
            continue
        entry = totals.setdefault(row["kind"], [0.0, 0])
        entry[0] += float(stats.get("cost_usd") or 0.0)
        entry[1] += int(stats.get("calls") or 0)
    return sorted(
        ((k, v[0], int(v[1])) for k, v in totals.items() if v[0] or v[1]),
        key=lambda t: -t[1],
    )
