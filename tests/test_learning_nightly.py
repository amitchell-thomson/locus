"""Spec L, step L11: the nightly pass over his learning chats."""

from __future__ import annotations

import json
from datetime import datetime, timezone

import pytest

from locus.heuris import db as hdb
from locus.heuris import learning as L
from locus.heuris import learning_nightly as N
from locus.heuris import learning_record as R

T0 = int(datetime(2026, 11, 23, 15, 0, tzinfo=timezone.utc).timestamp())
NIGHT = datetime(2026, 11, 24, 3, 0, tzinfo=timezone.utc)
DAY = datetime(2026, 11, 24, 14, 0, tzinfo=timezone.utc)


@pytest.fixture(autouse=True)
def no_terms(monkeypatch):
    monkeypatch.setattr(L, "_terms_cache", [])
    monkeypatch.setattr(L, "_away_cache", [])


@pytest.fixture
def conn(tmp_path):
    c = hdb.connect(tmp_path / "h.db")
    N.ensure(c)
    for gid in ("cpp-oxdaq", "interview-prep"):
        L.add_goal(c, gid, gid)
        L.set_state(c, gid, "active")
    with c:
        c.execute("INSERT INTO topics(id, goal, track, title, state, created) VALUES "
                  "(41,'cpp-oxdaq','python-data-model','DM','current','x'), (3,'interview-prep','probability','Counting','current','x')")
    yield c
    c.close()


def _turn(c, session, uuid, role, text, ts, cwd):
    with c:
        c.execute("INSERT INTO items(ref, source, kind, at, at_ts, parent, title, text, meta, hash, indexed_at) "
                  "VALUES (?, 'chats', 'chat', 'x', ?, ?, '', ?, ?, 'h', 'now')",
                  (f"chat:laptop/{session}#{uuid}", ts, f"chat:laptop/{session}", text,
                   json.dumps({"role": role, "session": session, "cwd": cwd})))


LEARN = "/Users/a/files/Learning"
CORE = "/Users/a/local-projects/oqts/oxdaq-core"
SITE = "/Users/a/local-projects/oqts/oqts-site"


def test_only_quiet_learning_and_oxdaq_chats_are_read_and_only_once(conn):
    _turn(conn, "learn", "u1", "you", "why does eq not use the hash?", T0, LEARN)
    _turn(conn, "core", "u1", "you", "why did remove take the wrong order?", T0, CORE + "/packages")
    _turn(conn, "site", "u1", "you", "make the button navy", T0, SITE)        # his call: not the website
    _turn(conn, "live", "u1", "you", "what is a martingale?", int(NIGHT.timestamp()) - 600, LEARN)  # still going
    got = [s for s, _, _ in N.sessions_to_read(conn, now=NIGHT)]
    assert sorted(got) == ["core", "learn"]
    with conn:
        conn.execute("INSERT INTO learning_reads VALUES ('learn', ?, 'x')", (T0,))
    assert [s for s, _, _ in N.sessions_to_read(conn, now=NIGHT)] == ["core"]
    _turn(conn, "learn", "u2", "you", "and what about frozen?", T0 + 60, LEARN)   # a new turn of his
    assert sorted(s for s, _, _ in N.sessions_to_read(conn, now=NIGHT)) == ["core", "learn"]


def test_the_prompt_marks_his_new_turns_and_lists_what_is_recorded(conn):
    _turn(conn, "s", "u1", "you", "old question", T0, LEARN)
    _turn(conn, "s", "c1", "claude", "x" * 3000, T0 + 1, LEARN)
    _turn(conn, "s", "u2", "you", "new question", T0 + 2, LEARN)
    R.observe(conn, "cpp-oxdaq", [{"track": "python-data-model", "kind": "question", "text": "asked x",
                                   "quote": "old question"}], session="s")
    p = N.user_prompt(conn, "s", LEARN, N._turns(conn, "s"), T0)
    assert "[Alec, already read]\nold question" in p and "[Alec, NEW]\nnew question" in p
    assert '("old question")' in p and "[...]" in p and "x" * 1300 not in p


def _reply(obs=(), cards=()):
    return json.dumps({"observations": list(obs), "cards": list(cards)})


def test_quotes_must_be_his_new_words_and_cards_need_a_moment(conn):
    _turn(conn, "s", "u1", "you", "I think frozen stops append, right?", T0, LEARN)
    _turn(conn, "s", "c1", "claude", "No: frozen is shallow.", T0 + 1, LEARN)
    _turn(conn, "s", "u2", "you", "ok so why does slots make it faster", T0 + 2, LEARN)
    raw = _reply(
        obs=[{"goal": "cpp-oxdaq", "track": "python-data-model", "kind": "misconception", "text": "thought frozen is deep",
              "quote": "I think frozen stops append"},
             {"goal": "cpp-oxdaq", "track": "python-data-model", "kind": "wrong", "text": "invented",
              "quote": "frozen is shallow"},                               # Claude's words, not his
             {"goal": "cpp-oxdaq", "track": "python-data-model", "kind": "question", "text": "asked about slots",
              "quote": "why does slots make it faster"}],
        cards=[{"goal": "cpp-oxdaq", "track": "python-data-model", "moment": "misconception", "quote": "I think frozen stops append",
                "prompt": "Does frozen stop f.tags.append?", "answer": "No: frozen is shallow."},
               {"goal": "cpp-oxdaq", "track": "python-data-model", "moment": "wrong", "quote": "something he never said",
                "prompt": "Q?", "answer": "A."}])
    a = N.apply(conn, "s", 0, raw, default_goal=None)
    assert (a.observations, a.dropped_quote, a.cards, a.dropped_card) == (2, 1, 1, 1)
    refs = {r[0] for r in conn.execute("SELECT ref FROM observations WHERE checked=1")}
    assert refs == {"chat:laptop/s#u1", "chat:laptop/s#u2"}               # pinned to his turns
    assert conn.execute("SELECT origin, state FROM cards").fetchone()[:] == ("nightly", "new")


def test_a_moment_the_session_close_recorded_is_not_recorded_or_carded_again(conn):
    _turn(conn, "s", "u1", "you", "so equality methods use that instead of the hash", T0, LEARN)
    R.observe(conn, "cpp-oxdaq", [{"track": "python-data-model", "kind": "misconception", "text": "eq vs hash",
                                   "quote": "equality methods use that instead of the hash"}], session="s")
    R.add_cards(conn, "cpp-oxdaq", [{"track": "python-data-model", "moment": "misconception", "prompt": "Does == use the hash?",
                                    "answer": "No.", "quote": "equality methods use that instead of the hash"}], session="s")
    raw = _reply(obs=[{"goal": "cpp-oxdaq", "track": "python-data-model", "kind": "misconception", "text": "same",
                       "quote": "so equality methods use that instead of the hash"}],
                 cards=[{"goal": "cpp-oxdaq", "track": "python-data-model", "moment": "misconception",
                         "quote": "equality methods use that instead of the hash",
                         "prompt": "When does a == b consult __hash__?", "answer": "Never."}])
    a = N.apply(conn, "s", 0, raw, default_goal=None)
    assert a.observations == 0 and a.dropped_dupe == 1 and a.cards == 0
    assert any("already has a card" in r for r in a.refused)
    assert conn.execute("SELECT COUNT(*) FROM cards").fetchone()[0] == 1


def test_an_oxdaq_chat_defaults_to_cpp_oxdaq(conn):
    _turn(conn, "b", "u1", "you", "why did remove take the wrong order?", T0, CORE)
    raw = _reply(obs=[{"goal": "", "track": "coding-python", "kind": "question", "text": "asked about remove",
                       "quote": "why did remove take the wrong order?"}])
    a = N.apply(conn, "b", 0, raw, default_goal=N.default_goal(CORE))
    assert a.observations == 1
    assert conn.execute("SELECT goal, origin FROM observations").fetchone()[:] == ("cpp-oxdaq", "nightly")


def test_a_run_submits_only_at_night_and_marks_the_chat_read_when_applied(conn, monkeypatch):
    _turn(conn, "s", "u1", "you", "why does eq make it unhashable?", T0, LEARN)
    sent = {}

    def fake_submit(c, job, model, reqs, meta, **k):
        if not reqs:
            return 0, ""
        sent.update(meta)
        with c:
            c.execute("INSERT INTO batches(id, job, model, created, items) VALUES ('b1', ?, ?, 'x', ?)",
                      (job, model, json.dumps(meta)))
        return len(reqs), "batch b1"

    monkeypatch.setattr(N.D, "submit", fake_submit)
    from locus.agent import claude
    monkeypatch.setattr(claude, "batch_ended", lambda bid: False)
    assert N.run(conn, now=DAY).submitted == 0                             # not in the day
    assert N.run(conn, now=NIGHT).submitted == 1
    assert N.run(conn, now=NIGHT).submitted == 0                           # waiting on its batch: not twice

    monkeypatch.setattr(claude, "batch_ended", lambda bid: True)
    reply = _reply(obs=[{"goal": "cpp-oxdaq", "track": "python-data-model", "kind": "question", "text": "asked",
                         "quote": "why does eq make it unhashable?"}])
    monkeypatch.setattr(claude, "batch_collect",
                        lambda bid, model: {cid: claude.ClaudeResult(text=reply) for cid in sent})
    rep = N.run(conn, now=NIGHT, submit_new=False)
    assert rep.collected == 1 and rep.applied.observations == 1
    assert N.sessions_to_read(conn, now=NIGHT) == []


def test_a_card_with_a_stray_moment_takes_the_kind_of_the_moment_it_quotes(conn):
    """24 Nov, the first real run: the model wrote "timed problem (d)" as the moment."""
    _turn(conn, "s", "u1", "you", "these two objects are the same", T0, LEARN)
    raw = _reply(obs=[{"goal": "cpp-oxdaq", "track": "python-data-model", "kind": "wrong", "text": "is vs ==",
                       "quote": "these two objects are the same"}],
                 cards=[{"goal": "cpp-oxdaq", "track": "python-data-model", "moment": "timed problem (d)",
                         "quote": "these two objects are the same", "prompt": "Is k1 is k2 True?", "answer": "No."}])
    a = N.apply(conn, "s", 0, raw, default_goal=None)
    assert a.cards == 1 and not a.refused
    assert conn.execute("SELECT moment FROM cards").fetchone()[0] == "wrong"
