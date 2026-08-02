"""Mark intent: what he meant by a mark, and the fate that follows (step 4).

Model-free — the `claude -p` runner is injected. What matters here is the confidence floor (his
"infer, then let me correct" only means something if a low guess is NOT acted on), the durability
of a correction he makes, and that an `idea` becomes an object linked to the passage that caused it.
"""

from __future__ import annotations

from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest

from locus.agent import state
from locus.capture import intent as I
from locus.db.connection import get_connection
from locus.db.migrate import migrate


@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "intent.db"
    migrate(db)
    c = get_connection(db)
    yield c
    c.close()


def _now():
    return datetime(2026, 8, 2, tzinfo=timezone.utc)


def _mark(conn, *, text="", note="", kind="underline", captured=None, page=0):
    with conn:
        cur = conn.execute(
            "INSERT INTO pdf_annotations (source_uri, pdf_page, kind, bbox_key, covered_text, "
            "note, in_margin, captured_at) VALUES ('books/apm.pdf',?,?,?,?,?,0,?)",
            (page, kind, f"k{page}{len(text)}{len(note)}", text, note,
             captured or (_now() - timedelta(days=1)).isoformat()),
        )
    return cur.lastrowid


def _runner(intent, confidence, why="because"):
    from locus.agent.claude import ClaudeResult

    def run(prompt, model):
        return ClaudeResult(text=f"INTENT: {intent}\nCONFIDENCE: {confidence}\nWHY: {why}")

    return run


def _row(conn, mid):
    return conn.execute("SELECT * FROM pdf_annotations WHERE id=?", (mid,)).fetchone()


# --- classification -------------------------------------------------------------------------


def test_the_three_intents_are_parsed_and_stored(conn):
    mid = _mark(conn, text="a passage about factor models")
    results = I.classify_pending(conn, runner=_runner("idea", 0.9), now=_now())
    assert results[0].intent == I.IDEA and results[0].confidence == 0.9
    assert _row(conn, mid)["intent"] == "idea"
    assert _row(conn, mid)["intent_by"] == I.BY_MODEL


@pytest.mark.parametrize("raw,expected", [
    ("not understood", I.NOT_UNDERSTOOD),
    ("not-understood", I.NOT_UNDERSTOOD),
    ("Important.", I.IMPORTANT),
])
def test_label_spelling_is_tolerated(conn, raw, expected):
    _mark(conn, text="a passage")
    assert I.classify_pending(conn, runner=_runner(raw, 0.8), now=_now())[0].intent == expected


def test_an_unrecognised_label_is_an_error_not_a_guess(conn):
    mid = _mark(conn, text="a passage")
    out = I.classify_pending(conn, runner=_runner("interesting", 0.9), now=_now())
    assert not out[0].ok and "unrecognised" in out[0].error
    assert _row(conn, mid)["intent"] is None


def test_a_dry_run_writes_nothing(conn):
    """This is the first pass creating durable structure from inference rather than from a tick,
    so being able to see the whole split before it acts is the difference between trusting it
    and hoping."""
    mid = _mark(conn, text="a passage")
    out = I.classify_pending(conn, runner=_runner("idea", 0.9), dry_run=True, now=_now())
    assert out[0].ok
    assert _row(conn, mid)["intent"] is None


def test_ink_that_has_not_settled_is_left_alone(conn):
    """His 12 hours: never react to ink he is still in the middle of writing."""
    _mark(conn, text="a passage", captured=(_now() - timedelta(hours=2)).isoformat())
    assert I.classify_pending(conn, runner=_runner("idea", 0.9), now=_now()) == []


def test_an_intent_he_set_is_never_re_guessed(conn):
    mid = _mark(conn, text="a passage")
    I.set_owner_intent(conn, mid, I.IDEA)
    assert I.classify_pending(conn, runner=_runner("important", 0.99), force=True, now=_now()) == []
    assert _row(conn, mid)["intent"] == "idea"


# --- the fates ------------------------------------------------------------------------------


def test_an_idea_becomes_an_object_linked_to_the_passage(conn):
    mid = _mark(conn, note="can we plot this behaviour?", text="a passage about reversal")
    I.classify_pending(conn, runner=_runner("idea", 0.9), now=_now())
    acted = I.act_on(conn, mid)

    assert acted.outcome == "became an idea"
    object_id = _row(conn, mid)["object_id"]
    assert object_id, "the mark records what it became — that is `build_marked`'s done flag"
    obj = state.get_object(conn, object_id)
    assert obj.type == "idea" and obj.status == "active"
    assert any(link.target_key == "books/apm.pdf" for link in obj.links), "grounded in the passage"


def test_a_low_confidence_guess_is_recorded_but_not_acted_on(conn):
    """"Infer, then let me correct" — acting on a low guess silently removes the correction."""
    mid = _mark(conn, text="take the")
    I.classify_pending(conn, runner=_runner("idea", 0.3), now=_now())
    acted = I.act_on(conn, mid, floor=0.6)

    assert acted.outcome == "held"
    assert _row(conn, mid)["object_id"] is None
    assert [m["id"] for m in I.uncertain_marks(conn)] == [mid], "it becomes a `locus decide` item"


def test_important_pushes_nothing_at_him(conn):
    """The mark already said it matters; re-offering it is the unread count §9 forbids."""
    mid = _mark(conn, text="a definition worth keeping")
    I.classify_pending(conn, runner=_runner("important", 0.9), now=_now())
    assert I.act_on(conn, mid).outcome == "left in place"
    assert _row(conn, mid)["object_id"] is None


def test_a_re_read_needs_a_searchable_question(conn):
    """Live, `take the` and `NMVSPY` were classified not-understood. Fed to retrieval they return
    noise, and a re-read justified by noise is the useless suggestion this section was fixed to
    stop making."""
    thin = _mark(conn, text="take the")
    real = _mark(conn, note="what is a factor covariance estimator and how is it built?")
    for mid in (thin, real):
        I.set_owner_intent(conn, mid, I.NOT_UNDERSTOOD)

    assert I.reread_seed(_row(conn, thin)) == ""
    assert I.reread_seed(_row(conn, real)).startswith("what is a factor")
    assert [m["id"] for m in I.not_understood_marks(conn)] == [real]


def test_a_mark_already_acted_on_is_not_acted_on_twice(conn):
    mid = _mark(conn, note="an idea worth building into the regime project")
    I.classify_pending(conn, runner=_runner("idea", 0.9), now=_now())
    I.act_on(conn, mid)
    assert I.act_on(conn, mid).outcome == "already acted on"
