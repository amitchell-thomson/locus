"""Briefs (H2.7): Now and Next are exact and live; Open and Recent come from a batch whose
unsourced lines are dropped; a brief is rebuilt when a fact changes; newer records show."""

from __future__ import annotations

import json
from datetime import date, datetime, timezone

import pytest

from locus.agent import claude
from locus.heuris import briefs as B
from locus.heuris import db as hdb
from locus.heuris import facts as F
from locus.heuris import records as R
from locus.heuris.index import Item, index
from tests.test_heuris_index import fake_embed


@pytest.fixture
def conn(tmp_path):
    c = hdb.connect(tmp_path / "h.db")
    B.ensure(c)
    index(c, "repos", [Item(ref="doc:quast/state.md", kind="doc", at=datetime(2026, 11, 10, tzinfo=timezone.utc),
                            title="quast: state.md", text="# State\n\nv2.0 is frozen; the paper draft is next.",
                            thing="quast")], embed=fake_embed)
    F.write(c, "quast", "goal", "a paper on autonomous CZ tune-up", sources=["chat"], route="chat", as_of="2026-11-01")
    R.store(c, {"unit": "chat:laptop/s1", "kind": "chat", "thing": "quast", "started": "2026-11-10T09:00:00+00:00",
                "ended": "2026-11-10T10:00:00+00:00", "upto_ts": 1, "sources": []},
            "Docs restructured\nDecided:\n- merge the docs", "sonnet-5.5")
    yield c
    c.close()


def test_now_is_the_facts_and_the_repos_own_state(conn):
    now = B.now_lines(conn, "quast")
    assert now[0].startswith("goal = a paper on autonomous CZ tune-up (as of 2026-11-01) [fact:")
    assert "the repository's own state.md (2026-11-10): v2.0 is frozen" in now[1]


def test_a_built_brief_keeps_only_sourced_recent_lines_and_shows_newer_records(conn, monkeypatch):
    sent = []
    monkeypatch.setattr(claude, "credit_allows", lambda est, **k: (True, "ok"))
    monkeypatch.setattr(claude, "batch_submit", lambda reqs: sent.extend(reqs) or "b1")
    monkeypatch.setattr(claude, "batch_ended", lambda bid: False)
    rep = B.run_briefs(conn, only=["quast"])
    assert rep.submitted == 1 and "[record:chat:laptop/s1]" in sent[0].user
    reply = json.dumps({"open": ["the paper draft, on Alec"],
                        "recent": ["10 Nov: decided to merge the docs [record:chat:laptop/s1]",
                                   "an invented line [record:chat:laptop/zzz]", "no pointer at all"]})
    monkeypatch.setattr(claude, "batch_ended", lambda bid: True)
    monkeypatch.setattr(claude, "batch_collect", lambda bid, model: {"b0": claude.ClaudeResult(text=reply)})
    rep = B.run_briefs(conn, submit_new=False)
    assert rep.collected == 1 and rep.dropped == 2
    R.store(conn, {"unit": "commits:quast@2026-11-12", "kind": "commits", "thing": "quast",
                   "started": "2099-01-01T00:00:00+00:00", "ended": "2099-01-01T00:00:00+00:00", "upto_ts": 1,
                   "sources": []}, "Pulse shaping started\n- x", "haiku-5.5")
    out = B.render(conn, "quast", date(2026, 11, 11))
    assert "decided to merge the docs" in out and "invented" not in out
    assert "Since this brief was built:" in out and "Pulse shaping started" in out


def test_a_fact_change_rebuilds_the_brief(conn):
    conn.execute("INSERT INTO briefs VALUES ('quast','[]','[]',?,?)",
                 (datetime.now(timezone.utc).isoformat(), json.dumps(B.fingerprint(conn, "quast"))))
    assert not B.needs_build(conn, "quast")
    F.write(conn, "quast", "state", "writing the paper", sources=["chat"], route="chat")
    assert B.needs_build(conn, "quast")
