"""The gate log — the dead-threshold check.

Model-free. What is asserted is the two properties the log exists for: that a gate which admitted
NOTHING is called out by name (the `_MAX_DISTANCE` failure, which looked exactly like a subject
with nothing to say), and that recording can never break the pass it observes.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.observe import gates


@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "gates.db"
    migrate(db)
    c = get_connection(db)
    yield c
    c.close()


def test_counts_and_samples_accumulate_per_gate_and_day(conn):
    for i in range(4):
        gates.record(conn, "a.floor", rejected=True, value=f"score={i}")
    gates.record(conn, "a.floor", rejected=False)
    (row,) = gates.report(conn)
    assert (row["gate"], row["rejected"], row["passed"]) == ("a.floor", 4, 1)
    assert row["samples"] == ["score=0", "score=1", "score=2", "score=3"]


def test_samples_are_capped_and_distinct(conn):
    for i in range(50):
        gates.record(conn, "a.floor", rejected=True, value="the same value every time")
    for i in range(50):
        gates.record(conn, "a.floor", rejected=True, value=f"distinct {i}")
    (row,) = gates.report(conn)
    assert row["rejected"] == 100                      # every decision counted
    assert len(row["samples"]) == gates.MAX_SAMPLES    # but the samples stay readable
    assert len(set(row["samples"])) == len(row["samples"])


def test_a_gate_that_admitted_nothing_is_called_out(conn):
    """THE HEADLINE. `find_tensions` was inert for its whole life and every surface looked normal.

    A 100% reject rate is the only signal that separates a dead gate from an empty subject, so it
    is stated in words rather than left to be inferred from two numbers.
    """
    for i in range(6):
        gates.record(conn, "trajectory.max_distance", rejected=True, value=f"d=0.9{i}")
    gates.record(conn, "other.floor", rejected=True)
    gates.record(conn, "other.floor", rejected=False)

    text = gates.render(gates.report(conn))
    assert "ADMITTED NOTHING" in text
    dead = text.index("trajectory.max_distance")
    other = text.index("other.floor")
    assert dead < other                                # worst reject-rate first
    # ...and the callout belongs to the dead gate, not to the healthy one.
    assert text.index("ADMITTED NOTHING") < other


def test_recording_never_raises_when_the_table_is_missing(tmp_path):
    """Observability must not be able to break its subject — a pre-migration DB degrades to silence."""
    import sqlite3

    conn = sqlite3.connect(tmp_path / "bare.db")
    conn.row_factory = sqlite3.Row
    gates.record(conn, "a.floor", rejected=True, value="x")   # must not raise
    assert gates.report(conn) == []


def test_empty_report_says_so_rather_than_printing_a_bare_header(conn):
    assert "No gate decisions recorded" in gates.render(gates.report(conn), days=7)
