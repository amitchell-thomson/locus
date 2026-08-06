"""Health and spend (`locus/health.py`).

The module exists because `locus-maintain` failed six consecutive nights and nothing said so. So
what is asserted is the three DIFFERENT ways a run can fail, because one detector catches only one
of them — and the case that hid for six nights is the one that leaves no row at all.
"""

from __future__ import annotations

import json
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest

from locus import health as H
from locus.agent import journal
from locus.db.connection import get_connection
from locus.db.migrate import migrate

NOW = datetime(2026, 8, 2, 12, 0, tzinfo=timezone.utc)
CADENCE = {"maintain": 24, "capture": 1}


@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "health.db"
    migrate(db)
    c = get_connection(db)
    yield c
    c.close()


def _run(conn, kind, status="ok", *, at=None, stats=None):
    at = (at or NOW).isoformat()
    with conn:
        cur = conn.execute(
            "INSERT INTO agent_runs (kind, started_at, status, stats) VALUES (?,?,?,?)",
            (kind, at, status, json.dumps(stats or {})),
        )
    return cur.lastrowid


# --- the three failure modes ------------------------------------------------------------------


def test_a_run_that_broke_is_reported(conn):
    _run(conn, "maintain", "error", at=NOW - timedelta(hours=1))
    _run(conn, "capture", at=NOW - timedelta(minutes=5))
    problems = H.check(conn, now=NOW, cadence=CADENCE).problems
    assert any(p.kind == "maintain" and p.severity == "broken" for p in problems)


def test_a_failure_a_later_run_recovered_from_is_not_a_live_problem(conn):
    """The status line is present tense: it says what is broken NOW.

    Live on 2026-08-06: `review` broke once at 03:37 (a syntax error in a file being edited while
    the timer fired) and ran clean eight times afterwards, yet the daily page was still shouting
    REVIEW ERROR SINCE 03:37 that afternoon. A line that reports settled history as a live fault
    is one he learns to skip — and then it cannot do the job it exists for. Still counted, though:
    a job that fails and recovers repeatedly is only visible if something says so.
    """
    _run(conn, "review", "error", at=NOW - timedelta(hours=8))
    _run(conn, "review", at=NOW - timedelta(hours=2))
    checked = H.check(conn, now=NOW, cadence={})
    assert checked.problems == []
    assert [p.kind for p in checked.recovered] == ["review"]
    assert checked.ran["review"] == 1


def test_a_failure_with_no_later_success_still_shouts(conn):
    """The other direction, which is the one that matters: recovery must be EARNED."""
    _run(conn, "structure", at=NOW - timedelta(hours=8))
    _run(conn, "structure", "degraded", at=NOW - timedelta(hours=2))
    checked = H.check(conn, now=NOW, cadence={})
    assert [p.kind for p in checked.problems] == ["structure"]
    assert checked.recovered == []


def test_a_unit_failure_is_recovered_by_a_later_run_of_its_kind(conn):
    """systemd sees the unit; Python journals the kind. `locus-maintain.service` -> `maintain`.

    Without that mapping the two halves of one job cannot be matched, and the hard failure at
    03:37 outlives the clean run at 04:36 that fixed it.
    """
    H.record_failure(conn, "locus-maintain.service", "exit-code", now=NOW - timedelta(hours=6))
    _run(conn, "maintain", at=NOW - timedelta(hours=5))
    checked = H.check(conn, now=NOW, cadence={})
    assert checked.problems == []
    assert [p.kind for p in checked.recovered] == ["locus-maintain.service"]

    # ...and a failure AFTER the last success is still live.
    H.record_failure(conn, "locus-maintain.service", "exit-code", now=NOW - timedelta(hours=1))
    assert [p.kind for p in H.check(conn, now=NOW, cadence={}).problems] == [
        "locus-maintain.service"
    ]


def test_a_run_that_started_and_vanished_is_reported(conn):
    """Killed, OOM, rebooted mid-run: the row stays open at `running` forever."""
    _run(conn, "maintain", "running", at=NOW - timedelta(hours=6))
    _run(conn, "capture", at=NOW - timedelta(minutes=5))
    problems = H.check(conn, now=NOW, cadence=CADENCE).problems
    assert any(p.severity == "stalled" and "never finished" in p.detail for p in problems)


def test_a_run_in_progress_is_not_a_fault(conn):
    """A false alarm every morning trains him to skip the line — the exact failure this prevents."""
    _run(conn, "maintain", "running", at=NOW - timedelta(minutes=10))
    _run(conn, "capture", at=NOW - timedelta(minutes=5))
    assert not [p for p in H.check(conn, now=NOW, cadence=CADENCE).problems if p.severity == "stalled"]


def test_a_run_THAT_NEVER_HAPPENED_is_reported(conn):
    """THE CASE THAT HID FOR SIX NIGHTS. It leaves no row, so no amount of reading `agent_runs`
    finds it — only comparing against the cadence does.

    The fixture carries WEEKS of journalling history because that is the real shape of the
    failure: `capture` had been recording itself all along while `maintain` never appeared once.
    Without that history "no row" is not yet evidence — see the test below.
    """
    _run(conn, "capture", at=NOW - timedelta(days=14))
    _run(conn, "capture", at=NOW - timedelta(minutes=5))
    problems = H.check(conn, now=NOW, cadence=CADENCE).problems
    assert [p.kind for p in problems if p.severity == "overdue"] == ["maintain"]
    assert "never run" in next(p for p in problems if p.kind == "maintain").detail


def test_no_row_is_not_yet_evidence_on_a_freshly_journalling_system(conn):
    """A weekly job that ran seventeen hours ago is HEALTHY, and must not be shouted about.

    Live 2026-08-03: journalling at dispatch had just been introduced, so `discover-harvest` —
    which had run that morning — had no row yet and was reported as HAS NEVER RUN. In capitals,
    on the daily page, and it would have been every morning for the next seven days.
    `OVERDUE_GRACE` already encodes that a daily false alarm is the failure this module exists to
    prevent; the no-row branch simply never applied it.
    """
    _run(conn, "capture", at=NOW - timedelta(minutes=5))
    assert H.check(conn, now=NOW, cadence=CADENCE).ok, "5 minutes of history proves nothing"


def test_a_kind_that_ran_recently_enough_is_not_overdue(conn):
    _run(conn, "maintain", at=NOW - timedelta(hours=20))
    _run(conn, "capture", at=NOW - timedelta(minutes=30))
    assert H.check(conn, now=NOW, cadence=CADENCE).ok


@pytest.mark.parametrize("hours_ago,expect_overdue", [
    (2, False),     # cadence 1h, grace 2.5 => forgiven
    (4, True),      # past the grace window
])
def test_lateness_is_forgiven_up_to_the_grace_multiplier(conn, hours_ago, expect_overdue):
    """A timer firing a few minutes late must not read as a failure — a false alarm every
    morning is how a status line stops being read."""
    _run(conn, "capture", at=NOW - timedelta(hours=hours_ago))
    problems = H.check(conn, now=NOW, cadence={"capture": 1}).problems
    assert bool(problems) is expect_overdue


def test_a_kind_with_no_cadence_is_never_overdue(conn):
    """On-demand commands are not timers, and nagging about them is noise."""
    assert H.check(conn, now=NOW, cadence={}).ok


# --- what systemd saw --------------------------------------------------------------------------


def test_a_unit_that_could_not_start_is_recorded_and_reported(conn):
    """`agent_runs` cannot see an import error or an OOM kill: neither reaches Python."""
    H.record_failure(conn, "locus-maintain.service", "exit-code", now=NOW - timedelta(hours=1))
    problems = H.check(conn, now=NOW, cadence={}).problems
    assert len(problems) == 1
    assert "failed to start" in problems[0].detail and "exit-code" in problems[0].detail


def test_repeated_hard_failures_are_counted_not_repeated(conn):
    for i in range(6):
        H.record_failure(conn, "locus-maintain.service", "exit-code",
                         now=NOW - timedelta(hours=i + 1))
    problems = H.check(conn, now=NOW, cadence={}).problems
    assert len(problems) == 1, "six nights is one problem, not six lines"
    assert "x6" in problems[0].detail


# --- spend --------------------------------------------------------------------------------------


def test_spend_is_summed_and_attributed(conn):
    _run(conn, "intent", stats={"cost_usd": 0.12, "calls": 26}, at=NOW - timedelta(hours=1))
    _run(conn, "discover-why", stats={"cost_usd": 0.03, "calls": 6}, at=NOW - timedelta(hours=1))
    checked = H.check(conn, now=NOW, cadence={})
    assert round(checked.cost_usd, 4) == 0.15
    assert checked.calls == 32
    assert H.spend_by_kind(conn, now=NOW)[0][0] == "intent", "dearest in calls first"


def test_spend_outside_the_window_is_not_counted(conn):
    _run(conn, "intent", stats={"cost_usd": 9.99}, at=NOW - timedelta(days=3))
    assert H.check(conn, now=NOW, cadence={}).cost_usd == 0.0


def test_the_summary_line_shouts_and_stays_short(conn):
    for kind in ("a", "b", "c", "d"):
        _run(conn, kind, "error", at=NOW - timedelta(hours=1))
    line = H.check(conn, now=NOW, cadence={}).summary()
    assert line.count("·") <= 8, "a wall of problems is a line he learns to skip"
    assert "ERROR" in line


def test_a_healthy_system_says_so(conn):
    _run(conn, "maintain", at=NOW - timedelta(hours=2))
    _run(conn, "capture", at=NOW - timedelta(minutes=5))
    assert "all runs healthy" in H.check(conn, now=NOW, cadence=CADENCE).summary()


def test_it_degrades_when_the_tables_are_absent(conn):
    conn.execute("DROP TABLE timer_failures")
    conn.execute("DROP TABLE agent_runs")
    conn.commit()
    assert H.check(conn, now=NOW, cadence=CADENCE).ok
