"""Connection prose — the guards that decide whether a connection is shown at all.

Model-free: the `claude -p` runner is injected, so no subprocess is spawned. What is asserted is
grounded-or-silent in all its forms, because on this surface a bad connection is worse than no
connection — the owner's verdict on the previous phrasing was "obscure and hard to read and
understand", and silence is the better failure.

The 2026-08-06 protocol: the model receives the shared-concept LIST, writes the prose, and names
its pick on a final `CONCEPT:` line. Verification is membership of that pick in the offered list
(the same shape as citation-existence checking), plus the refusal/leakage checks, plus a bounded
shorter-retry for format defects. NO_CONNECTION is a stored verdict, not a silent drop, so the
nightly writer never re-pays for the same answer.
"""

from __future__ import annotations

from pathlib import Path

from locus.agent.claude import ClaudeResult
from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.link.connect import (
    _MAX_PROSE_CHARS,
    pair_attempted,
    stored_note,
    stored_pair_note,
    write_note,
)


def _result(text: str) -> ClaudeResult:
    return ClaudeResult(text=text, cost_usd=0.0, usage={"input_tokens": 1, "output_tokens": 1})


def _conn(tmp_path: Path):
    db = tmp_path / "connect.db"
    migrate(db)
    c = get_connection(db)
    with c:
        for h, uri, title in (
            ("h1", "papers/dyadic.pdf", "Specification Testing"),
            ("h2", "coursework/stats.pdf", "Statistics and Probability"),
        ):
            c.execute(
                "INSERT INTO documents (content_hash, source_type, source_uri, raw_path, title, "
                "ingest_model, thesis, method, result) VALUES (?,'pdf',?,?,?,'t',?,?,?)",
                (h, uri, f"{h}.x", title, "a thesis", "a method", "a result"),
            )
    return c


_URIS = dict(src_uri="papers/dyadic.pdf", other_uri="coursework/stats.pdf")

_GOOD_BODY = (
    "You learned that testing a null hypothesis involves alpha and power trade-offs. What is "
    "the null hypothesis for linearity in your dyadic specification test?"
)
_GOOD = f"{_GOOD_BODY}\nCONCEPT: null hypothesis"


def test_prose_without_a_concept_line_is_dropped(tmp_path):
    """A reply that never says which offered concept it built on cannot be verified — dropped.

    Retried once (format defects usually fix on a re-ask), then silence.
    """
    conn = _conn(tmp_path)
    calls = []

    def runner(p, m):
        calls.append(p)
        return _result("These two documents are broadly about statistics.")

    out = write_note(conn, **_URIS, shared="null hypothesis", runner=runner)
    assert out is None
    assert len(calls) == 2                       # one bounded retry, not a loop
    assert stored_pair_note(conn, **_URIS) == ""


def test_a_pick_outside_the_offered_list_is_dropped(tmp_path):
    """Membership of the pick in the offered list is the citation-existence check of this pass."""
    conn = _conn(tmp_path)
    out = write_note(
        conn, **_URIS, shared="null hypothesis",
        shared_all=("null hypothesis", "confidence interval"),
        runner=lambda p, m: _result("Solid prose here.\nCONCEPT: Bayesian inference"),
    )
    assert out is None
    assert stored_pair_note(conn, **_URIS) == ""


def test_a_refusal_is_dropped_even_though_it_names_the_concept(tmp_path):
    """The live failure (2026-08-03): a refusal satisfies the naming check by quoting the concept.

    It reached `connection_notes` and would have printed on the page as the system asking HIM to
    clarify his own reading notes.
    """
    conn = _conn(tmp_path)
    refusal = (
        "I don't see \"null hypothesis\" mentioned explicitly in either the reading notes or "
        "the book material you've provided. Could you clarify where it appears?"
        "\nCONCEPT: null hypothesis"
    )
    assert write_note(conn, **_URIS, shared="null hypothesis",
                      runner=lambda p, m: _result(refusal)) is None
    assert stored_pair_note(conn, **_URIS) == ""


def test_a_grounded_prompt_is_stored_under_the_picked_concept(tmp_path):
    conn = _conn(tmp_path)
    note = write_note(
        conn, **_URIS, shared="confidence interval",
        shared_all=("confidence interval", "null hypothesis"),
        runner=lambda p, m: _result(_GOOD),
    )
    assert note is not None
    assert note.shared == "null hypothesis"      # the model's pick, not the candidate's first
    assert stored_note(conn, **_URIS, shared="null hypothesis") == _GOOD_BODY
    assert stored_pair_note(conn, **_URIS) == _GOOD_BODY


def test_no_connection_is_a_stored_verdict_not_a_silent_drop(tmp_path):
    """An honest "nothing here" must not be re-paid for every night.

    Measured 2026-08-06: Sonnet answered NO_CONNECTION on a junk pair where Haiku bluffed. The
    verdict is stored as an EMPTY note — the page shows nothing, `pair_attempted` turns True, and
    the nightly writer skips the pair from then on.
    """
    conn = _conn(tmp_path)
    out = write_note(conn, **_URIS, shared="null hypothesis",
                     runner=lambda p, m: _result("NO_CONNECTION"))
    assert out is None
    assert stored_pair_note(conn, **_URIS) == ""
    assert pair_attempted(conn, **_URIS)
    assert not pair_attempted(conn, src_uri="papers/dyadic.pdf", other_uri="nowhere.pdf")


def test_an_overrun_is_retried_shorter_then_dropped(tmp_path):
    """Sonnet overran a 420-char instruction ~40% of the time in the A/B run.

    The old code clipped stored prose at 400 chars, which printed sentences cut mid-word. Now:
    one retry with a terser instruction; a second overrun is dropped whole.
    """
    conn = _conn(tmp_path)
    long_body = "x" * (_MAX_PROSE_CHARS + 50)
    calls = []

    def runner(p, m):
        calls.append(p)
        return _result(f"{long_body}\nCONCEPT: null hypothesis")

    assert write_note(conn, **_URIS, shared="null hypothesis", runner=runner) is None
    assert len(calls) == 2
    assert "rejected" in calls[1]                # the retry says why
    assert stored_pair_note(conn, **_URIS) == ""


def test_side_a_side_b_leakage_is_not_printed(tmp_path):
    """The A/B run leaked template vocabulary ("Side A's team runs...") into otherwise-good prose.

    On the page "Side A" means nothing — the template forbids it and the check enforces it.
    """
    conn = _conn(tmp_path)
    leaky = "Side A's estimator matches Side B's derivation.\nCONCEPT: null hypothesis"
    assert write_note(conn, **_URIS, shared="null hypothesis",
                      runner=lambda p, m: _result(leaky)) is None
    assert stored_pair_note(conn, **_URIS) == ""


def test_the_bridge_framing_asks_him_to_apply_what_he_studied(tmp_path):
    """A coursework connection must not ask whether to ADOPT a second-year lecture.

    The default template frames the far side as "what he read" and asks whether he could use it;
    for material he was taught the useful question is the same-idea identification. Asserted on
    the prompt the runner receives, which is the only place the distinction is visible.
    """
    conn = _conn(tmp_path)
    seen: list[str] = []

    def runner(prompt, model):
        seen.append(prompt)
        return _result(
            "Your notes establish that a null hypothesis fixes alpha; apply it how?"
            "\nCONCEPT: null hypothesis"
        )

    write_note(conn, **_URIS, shared="null hypothesis", runner=runner, bridge=True)
    flat = " ".join(seen[0].split()).lower()          # the template hard-wraps
    assert "What he has already studied" in seen[0]
    assert "do not suggest he read the lecture notes" in flat

    seen.clear()
    write_note(conn, **_URIS, shared="null hypothesis", runner=runner, bridge=False)
    assert "What he read:" in seen[0]
    assert "What he has already studied" not in seen[0]


def test_the_project_framing_names_the_repo_side(tmp_path):
    """kind='project': the near side is a repo he wrote; the ask is an idea for THE PROJECT."""
    conn = _conn(tmp_path)
    with conn:
        conn.execute(
            "INSERT INTO documents (content_hash, source_type, source_uri, raw_path, title, "
            "ingest_model, thesis, method, result) VALUES "
            "('h3','code','/repos/alpha','h3.x','Alpha Fund','t','t','m','r')"
        )
    seen: list[str] = []

    def runner(prompt, model):
        seen.append(prompt)
        return _result("Add risk parity next to tangency weights?\nCONCEPT: portfolio optimization")

    note = write_note(
        conn, src_uri="/repos/alpha", other_uri="papers/dyadic.pdf",
        shared="portfolio optimization", kind="project", runner=runner,
    )
    assert note is not None
    assert "HIS PROJECT (code he wrote and maintains):" in seen[0]
    assert "Alpha Fund" in seen[0]


def test_the_concept_line_and_prompt_prefix_are_stripped_from_stored_prose(tmp_path):
    """What the page prints is the prose alone — protocol scaffolding never reaches ink."""
    conn = _conn(tmp_path)
    note = write_note(
        conn, **_URIS, shared="null hypothesis",
        runner=lambda p, m: _result(f"PROMPT: {_GOOD_BODY}\nCONCEPT: null hypothesis."),
    )
    assert note is not None
    assert note.prose == _GOOD_BODY
    assert "CONCEPT" not in stored_pair_note(conn, **_URIS)
