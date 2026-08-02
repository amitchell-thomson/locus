"""The written reason a paper was proposed (`discover/why.py`).

Model-free: the `claude -p` runner is injected, so what is asserted here is the grounding
discipline rather than the prose — which is the part that can silently go wrong. The deterministic
`why` is always still true, so every failure path here must degrade to it rather than block.
"""

from __future__ import annotations

from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest

from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.discover import why as W


@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "why.db"
    migrate(db)
    c = get_connection(db)
    yield c
    c.close()


def _profile(conn, label="Alpha Fund", kind="project", facet="synthesis", text="..."):
    with conn:
        conn.execute(
            "INSERT INTO discovery_profiles (subject_kind, subject_key, facet, label, text, "
            "built_at) VALUES (?,?,?,?,?,'2026-07-31')",
            (kind, "1", facet, label, text),
        )


def _proposal(conn, *, evidence="project:Alpha Fund", why_long=None, written=None,
              status="proposed", title="Tail-Risk Analytics of Actively Managed ETFs") -> int:
    with conn:
        cur = conn.execute(
            "INSERT INTO reading_proposals (kind, dedupe_key, title, why, why_kind, evidence_key, "
            "status, score, created_at, abstract, why_long, why_written_at) VALUES "
            "('paper',?,?,'fit 0.76','discovery',?,?,1.0,'2026-07-31','an abstract',?,?)",
            (f"k-{title}", title, evidence, status, why_long, written),
        )
    return cur.lastrowid


def _runner(reason: str):
    """Stand in for `claude -p`: `CliRunner` is (prompt, model) -> ClaudeResult.

    Plain prose, not JSON — the reason is free text. Wrapping it in a schema was the original
    design and it discarded 6 good answers out of 6 on the first live run."""
    from locus.agent.claude import ClaudeResult

    def run(prompt: str, model: str | None) -> ClaudeResult:
        return ClaudeResult(text=reason)

    return run


# --- grounding ---------------------------------------------------------------------------------


def test_a_reason_is_written_from_his_project_and_the_abstract(conn):
    _profile(conn, text="Alpha Fund estimates a 55x55 covariance from 250 days of returns.")
    pid = _proposal(conn)

    out = W.write_reason(
        conn, pid,
        runner=_runner("Alpha Fund estimates covariance from 250 days; shrinkage is the fix."),
    )
    assert out.ok
    row = conn.execute("SELECT why_long, why_written_at FROM reading_proposals WHERE id=?",
                       (pid,)).fetchone()
    assert "shrinkage is the fix" in row["why_long"]
    assert row["why_written_at"], "the timestamp is what drives the rewrite AND the page repeat"


def test_a_generic_paper_summary_is_dropped(conn):
    """Grounded or silent: it connects nothing, and the deterministic `why` is still true — so
    storing the prose would be strictly worse than storing nothing."""
    _profile(conn, text="Alpha Fund estimates a 55x55 covariance from 250 days of returns.")
    pid = _proposal(conn)

    out = W.write_reason(conn, pid, runner=_runner("This paper studies portfolio optimisation."))
    assert not out.ok and "never named the subject" in out.detail
    assert conn.execute(
        "SELECT why_long FROM reading_proposals WHERE id=?", (pid,)
    ).fetchone()["why_long"] is None


def test_a_reason_engaging_his_material_survives_without_naming_the_project(conn):
    """The check that rejected 6 good reasons out of 6 on the first live run: the model had done
    exactly what it was asked and written "Your HMM regime training..." rather than the literal
    project label. Sharing his distinctive vocabulary is grounding too."""
    _profile(conn, label="regime-ml",
             text="regime-ml trains a hidden Markov model over volatility regimes, "
                  "re-estimating transition probabilities on every new dataset.")
    pid = _proposal(conn, evidence="project:regime-ml")

    out = W.write_reason(conn, pid, runner=_runner(
        "Your hidden Markov transition probabilities are re-estimated on every new dataset; "
        "amortised inference learns that mapping once instead."
    ))
    assert out.ok, "engaging his material IS grounding, even without repeating the label"
    assert conn.execute(
        "SELECT why_long FROM reading_proposals WHERE id=?", (pid,)
    ).fetchone()["why_long"].startswith("Your hidden Markov")


def test_a_subject_with_no_stored_profile_writes_nothing(conn):
    """There is nothing honest to say, so it keeps the deterministic why rather than guessing."""
    pid = _proposal(conn, evidence="project:Never Profiled")
    out = W.write_reason(conn, pid, runner=_runner("Never Profiled is relevant here."))
    assert not out.ok and "no stored profile" in out.detail


def test_a_model_failure_degrades_instead_of_raising(conn):
    from locus.agent.claude import ClaudeError

    _profile(conn)
    pid = _proposal(conn)

    def boom(prompt: str, model: str | None):
        raise ClaudeError("rate limited")

    out = W.write_reason(conn, pid, runner=boom)
    assert not out.ok
    assert conn.execute(
        "SELECT why_long FROM reading_proposals WHERE id=?", (pid,)
    ).fetchone()["why_long"] is None


def test_the_synthesis_facet_leads_the_prompt(conn):
    """Ordering by id alone put a test module ahead of the project's thesis."""
    _profile(conn, facet="section:0", text="tests/test_x.py contains unit tests.")
    _profile(conn, facet="synthesis", text="Alpha Fund is a long-short equity book.")
    label, facets = W._subject_facets(conn, "project:Alpha Fund")
    assert label == "Alpha Fund"
    assert facets.index("long-short") < facets.index("unit tests")


def test_a_long_reason_is_trimmed_to_stay_glanceable(conn):
    _profile(conn)
    pid = _proposal(conn)
    out = W.write_reason(conn, pid, runner=_runner("Alpha Fund " + "word " * 400))
    assert out.ok and len(out.reason) <= W._MAX_REASON_CHARS + 3


# --- what needs writing ------------------------------------------------------------------------


def test_missing_and_stale_reasons_are_selected_but_fresh_ones_are_not(conn):
    now = datetime(2026, 8, 2, tzinfo=timezone.utc)
    missing = _proposal(conn, title="missing")
    stale = _proposal(conn, title="stale", why_long="old",
                      written=(now - timedelta(days=9)).isoformat())
    fresh = _proposal(conn, title="fresh", why_long="new",
                      written=(now - timedelta(days=1)).isoformat())

    pending = W.needs_a_reason(conn, now=now)
    assert missing in pending and stale in pending
    assert fresh not in pending, "rewriting a reason he has not seen yet is spend for nothing"


def test_only_papers_still_on_the_shelf_get_an_argument(conn):
    """One he already accepted needs no argument for reading it; one he rejected needs none."""
    accepted = _proposal(conn, title="accepted", status="accepted")
    rejected = _proposal(conn, title="rejected", status="rejected")
    assert W.needs_a_reason(conn) == []
    assert accepted not in W.needs_a_reason(conn)
    assert rejected not in W.needs_a_reason(conn)


def test_write_missing_respects_its_limit(conn):
    _profile(conn)
    for i in range(5):
        _proposal(conn, title=f"paper {i}")
    results = W.write_missing(conn, limit=2, runner=_runner("Alpha Fund: try shrinkage."))
    assert len(results) == 2


def test_markdown_emphasis_is_stripped(conn):
    """A live reply came back with `**frequency response analysis**`; asterisks are noise on an
    e-ink page rendered through typst."""
    _profile(conn)
    pid = _proposal(conn)
    out = W.write_reason(conn, pid, runner=_runner("**Alpha Fund** holds a *long* book."))
    assert out.ok and "*" not in out.reason


def test_a_long_reason_is_cut_at_a_sentence_not_mid_clause(conn):
    """Live, every reply hit the cap and was cut mid-sentence — losing the 'what you could do
    with it' half, which is the valuable one. A dangling fragment reads as the system running out
    of room rather than as a finished thought."""
    first = "Alpha Fund holds a long-short book with a concentrated risk profile. "
    trimmed = W._trim(first + "x" * 900, limit=len(first) + 50)
    assert trimmed == first.rstrip(), "cut back to the last complete sentence"
    assert not trimmed.endswith("...")


def test_a_reason_with_no_sentence_break_still_degrades_gracefully(conn):
    out = W._trim("Alpha " + "word " * 400, limit=60)
    assert out.endswith("...") and len(out) <= 63
