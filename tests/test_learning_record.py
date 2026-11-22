"""Spec L, step L10: the learning record (observations, cards) and the brief a session reads."""

from __future__ import annotations

import json
from datetime import date, datetime, timedelta, timezone

import pytest

from locus import backup as bk
from locus.heuris import db as hdb
from locus.heuris import learning as L
from locus.heuris import learning_record as R


@pytest.fixture(autouse=True)
def no_terms(monkeypatch):
    monkeypatch.setattr(L, "_terms_cache", [])
    monkeypatch.setattr(L, "_away_cache", [])


def _topic(c, tid, goal, track, title, parent=None, ord_=0):
    c.execute("INSERT INTO topics(id, goal, parent, ord, track, title, hours, state, created) "
              "VALUES (?,?,?,?,?,?,2,'current','2026-11-19')", (tid, goal, parent, ord_, track, title))


@pytest.fixture
def conn(tmp_path):
    c = hdb.connect(tmp_path / "h.db")
    R.ensure(c)
    L.add_goal(c, "cpp-oxdaq", "C++ alongside OXDAQ")
    L.add_goal(c, "interview-prep", "Quant interviews")
    with c:
        _topic(c, 1, "cpp-oxdaq", "python-data-model", "R1 concepts")
        _topic(c, 41, "cpp-oxdaq", "python-data-model", "Data model and dataclasses", parent=1, ord_=1)
        _topic(c, 42, "cpp-oxdaq", "python-data-model", "SortedList key semantics", parent=1, ord_=2)
        _topic(c, 2, "interview-prep", "probability", "Probability")
        _topic(c, 3, "interview-prep", "probability", "Counting", parent=2)
    yield c
    c.close()


SESSION = "5f0c-session-01"


def _his_turn(c, text, uuid="u1", session=SESSION):
    """One of his prompts as the chats indexer stores it."""
    with c:
        c.execute("INSERT INTO items(ref, source, kind, at, at_ts, parent, title, text, meta, hash, indexed_at) "
                  "VALUES (?, 'chats', 'chat', '2026-11-21T15:00:00+00:00', 0, ?, '', ?, ?, 'h', 'now')",
                  (f"chat:laptop/{session}#{uuid}", f"chat:laptop/{session}", text,
                   json.dumps({"role": "you", "turn": 3})))


def test_a_session_close_records_what_he_showed_and_the_next_brief_shows_it(conn):
    w = R.observe(conn, "cpp-oxdaq", [
        {"track": "python-data-model", "kind": "question", "topic": 41,
         "text": "asked why a dataclass with eq=True is unhashable",
         "quote": "why does eq make it unhashable?"},
        {"track": "python-data-model", "kind": "explained-back", "topic": 41,
         "text": "explained __hash__ and __eq__ agreeing, unprompted",
         "quote": "so equal things must hash the same"},
        {"track": "Python Data Model", "kind": "Hinted", "text": "needed a hint on frozen", "quote": "frozen?"},
    ], session=SESSION)
    assert len(w.ids) == 3 and not w.refused                     # names are normalised, not refused
    out = R.brief(conn, "cpp-oxdaq", 41, today=date.today())
    assert "Open questions he asked" in out and "why does eq make it unhashable?" in out
    assert "Solid" in out and "so equal things must hash the same" in out
    assert "SortedList key semantics" in out                     # next topic, and it has no evidence yet
    assert "No evidence yet for these topics" in out and "topic 42" in out
    assert "coding-python" in out and "No evidence on coding-python yet: unknown" in out


def test_a_bad_observation_is_refused_alone_with_the_nearest_track(conn):
    w = R.observe(conn, "cpp-oxdaq", [
        {"track": "python-datamodel", "kind": "cold", "text": "x"},
        {"track": "python-data-model", "kind": "guessed", "text": "y"},
        {"track": "python-data-model", "kind": "cold", "text": "did the ordering cold"},
    ], session=SESSION)
    assert len(w.ids) == 1 and len(w.refused) == 2
    assert "nearest: python-data-model" in w.refused[0] and "unknown kind" in w.refused[1]
    with pytest.raises(L.LearningError):
        R.observe(conn, "no-such-goal", [{"track": "probability", "kind": "cold", "text": "x"}])
    with pytest.raises(L.LearningError, match="belongs to interview-prep"):
        R._topic(conn, "cpp-oxdaq", 3)


def test_quotes_are_checked_against_his_turns_and_an_invented_one_is_hidden(conn):
    R.observe(conn, "cpp-oxdaq", [
        {"track": "python-data-model", "kind": "question", "text": "asked about eq",
         "quote": "why does eq make it   UNHASHABLE?"},
        {"track": "python-data-model", "kind": "confusion", "text": "said something he never said",
         "quote": "I love metaclasses"},
    ], session=SESSION)
    _his_turn(conn, "ok but why does eq make it unhashable? that seems odd", uuid="u7")
    v = R.verify(conn)
    assert v == {"found": 1, "waiting": 1, "unfound": 0}         # the other may not be indexed yet
    row = conn.execute("SELECT ref, checked FROM observations WHERE quote LIKE 'why%'").fetchone()
    assert tuple(row) == (f"chat:laptop/{SESSION}#u7", 1)        # pinned to the exact turn
    later = datetime.now(timezone.utc) + timedelta(hours=R.VERIFY_GRACE_H + 1)
    assert R.verify(conn, now=later)["unfound"] == 1
    out = R.brief(conn, "cpp-oxdaq", 41)
    assert "metaclasses" not in out                              # grounded or silent


def test_claude_turns_do_not_count_as_his_words(conn):
    R.observe(conn, "cpp-oxdaq", [{"track": "python-data-model", "kind": "explained-back", "text": "x",
                                   "quote": "equal objects must hash equal"}], session=SESSION)
    with conn:
        conn.execute("INSERT INTO items(ref, source, kind, at, at_ts, parent, title, text, meta, hash, indexed_at) "
                     "VALUES ('chat:laptop/s#c1', 'chats', 'chat', 'x', 0, ?, '', 'equal objects must hash equal', "
                     "'{\"role\": \"claude\"}', 'h', 'now')", (f"chat:laptop/{SESSION}",))
    assert R.verify(conn)["found"] == 0


def test_cards_are_one_per_moment_and_never_duplicated(conn):
    R.observe(conn, "cpp-oxdaq", [{"track": "python-data-model", "kind": "question", "text": "eq",
                                   "quote": "why does eq make it unhashable?"}], session=SESSION)
    card = {"track": "python-data-model", "moment": "question", "quote": "why does eq make it unhashable?",
            "prompt": "Why does @dataclass(eq=True) set __hash__ to None?",
            "answer": "Equal objects must hash equal; a mutable eq'd object could change hash, so Python disables it."}
    w = R.add_cards(conn, "cpp-oxdaq", [card], session=SESSION)
    assert len(w.ids) == 1
    again = R.add_cards(conn, "cpp-oxdaq", [dict(card, prompt=card["prompt"].upper())], session=SESSION)
    assert again.refused and "already on this track" in again.refused[0]
    other = R.add_cards(conn, "cpp-oxdaq", [dict(card, prompt="When is a dataclass hashable?")], session=SESSION)
    assert other.refused and "already has a card" in other.refused[0]
    bad = R.add_cards(conn, "cpp-oxdaq", [dict(card, moment="vibes", prompt="p2")])
    assert bad.refused
    c = conn.execute("SELECT state, due, source FROM cards WHERE id=?", (w.ids[0],)).fetchone()
    assert tuple(c) == ("new", None, f"chat:laptop/{SESSION}")   # waits to be introduced (L12)


def test_a_card_failed_three_times_comes_back_to_the_session(conn):
    w = R.add_cards(conn, "interview-prep", [{"track": "probability", "moment": "wrong",
                                              "prompt": "Ways to split n identical balls into k boxes: the set-up?",
                                              "answer": "Stars and bars: C(n+k-1, k-1)."}])
    with conn:
        conn.execute("UPDATE cards SET state='active', lapses=3, due=? WHERE id=?", (date.today().isoformat(), w.ids[0]))
    out = R.brief(conn, "interview-prep", 3)
    assert "failed 3 times in Review: re-teach it" in out and "Stars and bars" in out
    assert "1 due" in out and R.counts(conn)["flagged"] == 1


def test_solid_evidence_fades_when_read_never_when_written(conn):
    R.observe(conn, "interview-prep", [{"track": "probability", "kind": "cold", "text": "did a Bayes problem cold"}],
              at="2026-10-01T10:00:00+00:00")
    assert any("Fading" in x for x in R.profile(conn, "probability", today=date(2026, 11, 22)))
    assert not any("Fading" in x for x in R.profile(conn, "probability", today=date(2026, 10, 10)))


def test_the_record_survives_a_backup(tmp_path, conn):
    R.observe(conn, "cpp-oxdaq", [{"track": "python-data-model", "kind": "cold", "text": "x"}])
    R.add_cards(conn, "cpp-oxdaq", [{"track": "python-data-model", "moment": "wrong", "prompt": "p", "answer": "a"}])
    db = tmp_path / "h.db"
    snap = tmp_path / "snap"
    snap.mkdir()
    bk._backup_ledger(db, snap / "heuris-ledger.db")
    added = bk.restore_ledger(snap, tmp_path / "fresh.db")
    assert added["observations"] == 1 and added["cards"] == 1
