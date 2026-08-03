"""Connection prose — the two guards that decide whether a connection is shown at all.

Model-free: the `claude -p` runner is injected, so no subprocess is spawned. What is asserted is
grounded-or-silent in both its forms, because on this surface a bad connection is worse than no
connection — the owner's verdict on the previous phrasing was "obscure and hard to read and
understand", and silence is the better failure.
"""

from __future__ import annotations

from pathlib import Path

from locus.agent.claude import ClaudeResult
from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.link.connect import _BRIDGE_TEMPLATE, stored_note, write_note


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


def test_prose_that_cannot_name_the_shared_concept_is_dropped(tmp_path):
    conn = _conn(tmp_path)
    out = write_note(
        conn, **_URIS, shared="null hypothesis",
        runner=lambda p, m: _result("These two documents are broadly about statistics."),
    )
    assert out is None
    assert stored_note(conn, **_URIS, shared="null hypothesis") == ""


def test_a_refusal_is_dropped_even_though_it_names_the_concept(tmp_path):
    """The live failure (2026-08-03): a refusal satisfies the naming check by quoting the concept.

    It reached `connection_notes` and would have printed on the page as the system asking HIM to
    clarify his own reading notes.
    """
    conn = _conn(tmp_path)
    refusal = (
        "I don't see \"null hypothesis\" mentioned explicitly in either the reading notes or "
        "the book material you've provided. Could you clarify where it appears?"
    )
    assert write_note(conn, **_URIS, shared="null hypothesis",
                      runner=lambda p, m: _result(refusal)) is None
    assert stored_note(conn, **_URIS, shared="null hypothesis") == ""


def test_a_grounded_prompt_is_stored(tmp_path):
    conn = _conn(tmp_path)
    good = (
        "You learned that testing a null hypothesis involves alpha and power trade-offs. What is "
        "the null hypothesis for linearity in your dyadic specification test?"
    )
    note = write_note(conn, **_URIS, shared="null hypothesis",
                      runner=lambda p, m: _result(good))
    assert note is not None
    assert stored_note(conn, **_URIS, shared="null hypothesis") == good


def test_the_bridge_framing_asks_him_to_apply_what_he_studied(tmp_path):
    """A coursework connection must not ask whether to ADOPT a second-year lecture.

    The default template frames the far side as "what he read" and asks whether he could use it;
    for material he was taught the useful question runs the other way. Asserted on the prompt the
    runner receives, which is the only place the distinction is visible.
    """
    conn = _conn(tmp_path)
    seen: list[str] = []

    def runner(prompt, model):
        seen.append(prompt)
        return _result("Your notes establish that a null hypothesis fixes alpha; apply it how?")

    write_note(conn, **_URIS, shared="null hypothesis", runner=runner, bridge=True)
    flat = " ".join(seen[0].split()).lower()          # the template hard-wraps
    assert "What he has already studied" in seen[0]
    assert "do not suggest he read the lecture notes" in flat

    seen.clear()
    write_note(conn, **_URIS, shared="null hypothesis", runner=runner, bridge=False)
    assert "What he read" in seen[0]
    assert "What he has already studied" not in seen[0]
    assert "{shared}" not in _BRIDGE_TEMPLATE.format(
        his_title="a", his_text="b", other_title="c", other_text="d", shared="e"
    )
