"""The facts ledger (H2.1): slots, newer-replaces-older by as-of, history, forget, proposals,
the applications import, and that the ledger survives an index rebuild and is backed up."""

from __future__ import annotations

import sqlite3

import pytest

from locus.heuris import db as hdb
from locus.heuris import facts as F
from locus.heuris import tools as ht

SRC = ["mail:personal/1/2"]


@pytest.fixture
def conn(tmp_path):
    c = hdb.connect(tmp_path / "h.db")
    yield c
    c.close()


def test_a_slot_not_on_the_list_is_refused_with_the_nearest(conn):
    with pytest.raises(F.FactError, match="Nearest: stage"):
        F.write(conn, "millennium", "stag", "offer accepted", sources=SRC, route="chat")
    with pytest.raises(F.FactError, match="note:<name>"):
        F.write(conn, "millennium", "salary", "x", sources=SRC, route="chat")
    with pytest.raises(F.FactError, match="offer:pay"):
        F.write(conn, "millennium", "offer", "£x", sources=SRC, route="chat")
    with pytest.raises(F.FactError, match="unknown subject"):
        F.write(conn, "millenium", "stage", "x", sources=SRC, route="chat")
    with pytest.raises(F.FactError, match="source"):
        F.write(conn, "millennium", "stage", "x", sources=[], route="chat")


def test_newer_replaces_and_older_is_history(conn):
    a = F.write(conn, "jump", "stage", "onsite", sources=SRC, route="mail", as_of="2026-09-20")
    b = F.write(conn, "jump", "stage", "not offered", sources=SRC, route="chat", as_of="2026-10-22")
    assert b.action == "write" and b.replaced.id == a.fact.id
    # A backfill reading August's mail after November's chat must not undo it.
    c = F.write(conn, "jump", "stage", "applied", sources=SRC, route="mail", as_of="2026-08-01")
    assert c.action == "history"
    cur = F.current(conn, "jump", "stage")
    assert [f.value for f in cur] == ["not offered"]
    assert [f.value for f in F.history(conn, "jump", "stage")] == ["not offered", "onsite", "applied"]


def test_the_same_value_confirms_and_adds_its_source(conn):
    F.write(conn, "bh-2027", "stage", "contract signed", sources=["a"], route="import")
    w = F.write(conn, "bh-2027", "stage", "Contract  signed.", sources=["b"], route="mail")
    assert w.action == "confirm" and w.fact.sources == ["a", "b"]
    assert len(F.history(conn, "bh-2027", "stage")) == 1


def test_named_slots_replace_only_under_the_same_name(conn):
    F.write(conn, "millennium", "offer:pay", "£x a week", sources=SRC, route="mail")
    F.write(conn, "millennium", "offer:dates", "21 Jun to 27 Aug", sources=SRC, route="mail")
    F.write(conn, "millennium", "offer:pay", "£y a week", sources=SRC, route="mail")
    assert {f.slot: f.value for f in F.current(conn, "millennium", "offer")} == {
        "offer:pay": "£y a week", "offer:dates": "21 Jun to 27 Aug"}


def test_forget_restores_what_it_replaced(conn):
    a = F.write(conn, "jump", "stage", "onsite", sources=SRC, route="mail", as_of="2026-09-20")
    b = F.write(conn, "jump", "stage", "offer", sources=SRC, route="chat", as_of="2026-10-22")
    gone, back = F.forget(conn, b.fact.id)
    assert gone.status == "retired" and back.id == a.fact.id
    assert [f.value for f in F.current(conn, "jump", "stage")] == ["onsite"]
    with pytest.raises(F.FactError, match="not current"):
        F.forget(conn, b.fact.id)


def test_proposals_wait_and_a_rejected_one_is_never_offered_again(conn):
    F.write(conn, "jump", "stage", "not offered", sources=SRC, route="chat")
    p = F.propose(conn, "jump", "stage", "offer", sources=SRC, route="mail", why="conflict")
    assert p and p.fact.status == "proposed"
    assert F.propose(conn, "jump", "stage", "offer", sources=SRC, route="mail", why="conflict") is None
    assert F.propose(conn, "jump", "stage", "not offered", sources=SRC, route="mail", why="x") is None
    F.reject(conn, p.fact.id)
    assert F.propose(conn, "jump", "stage", "offer", sources=SRC, route="mail", why="conflict") is None
    q = F.propose(conn, "jump", "feedback:onsite", "strong on probability", sources=SRC, route="mail", why="not authority")
    w = F.approve(conn, q.fact.id)
    assert w.action == "write" and F.get(conn, q.fact.id).status == "approved"
    assert [f.value for f in F.current(conn, "jump", "feedback")] == ["strong on probability"]


def test_automatic_log_lists_mail_writes_and_undo_restores(conn):
    F.write(conn, "b18", "tutor", "Isobel", sources=SRC, route="import")
    w = F.write(conn, "b18", "tutor", "Isobel Smith", sources=SRC, route="mail")
    log = F.automatic_log(conn)
    assert [r["fact"] for r in log] == [w.fact.id]
    F.forget(conn, w.fact.id, route="home", action="undo")
    assert F.automatic_log(conn)[0]["undone"]
    assert F.current(conn, "b18", "tutor")[0].value == "Isobel"


def test_import_applications_is_idempotent(conn, tmp_path):
    p = tmp_path / "applications.toml"
    p.write_text('[millennium]\nstage = "offer accepted"\nsince = 2026-11-10\nnote = "Brian: move close to risk"\n'
                 '[bh-2027]\nstage = "contract signed · reneging"\nsince = 2026-11-10\nturn = "you"\n')
    first = F.import_applications(conn, p)
    assert [w.action for w in first] == ["write"] * 4
    assert {w.action for w in F.import_applications(conn, p)} == {"confirm"}
    out = ht.context(conn)
    assert "millennium" in out and "stage = offer accepted  (as of 2026-11-10" in out
    assert "turn = you" in out and "note:from-file = Brian" in out


def test_facts_tool_history_and_errors(conn):
    F.write(conn, "millennium", "stage", "final round", sources=SRC, route="mail", as_of="2026-10-20")
    F.write(conn, "millennium", "stage", "offer accepted", sources=SRC, route="import", as_of="2026-11-02")
    h = ht.facts(conn, "millennium", "stage", history=True)
    assert h.index("CURRENT") < h.index("replaced") and "final round" in h
    assert "unknown subject" in ht.facts(conn, "nope", "stage", history=True)


def test_the_ledger_survives_an_index_rebuild_and_is_backed_up(conn, tmp_path):
    from locus.backup import _backup_ledger

    F.write(conn, "me", "goal:career", "systematic trading", sources=SRC, route="chat")
    hdb.drop_index(conn)
    conn.close()
    c = hdb.connect(tmp_path / "h.db")
    assert F.current(c, "me")[0].value == "systematic trading"
    c.close()
    n = _backup_ledger(tmp_path / "h.db", tmp_path / "ledger.db")
    assert n >= 2
    assert sqlite3.connect(tmp_path / "ledger.db").execute("SELECT value FROM facts").fetchone()[0] == "systematic trading"


def test_an_earlier_source_for_the_same_value_moves_its_date_back(conn):
    F.write(conn, "millennium", "stage", "offer accepted", sources=["file"], route="import", as_of="2026-11-10")
    w = F.write(conn, "millennium", "stage", "offer accepted", sources=["mail"], route="mail", as_of="2026-11-02")
    assert w.action == "confirm" and w.fact.as_of == "2026-11-02"
    # Not across a different value in between.
    F.write(conn, "jump", "stage", "applied", sources=SRC, route="mail", as_of="2026-08-01")
    F.write(conn, "jump", "stage", "onsite", sources=SRC, route="mail", as_of="2026-09-20")
    F.write(conn, "jump", "stage", "applied", sources=SRC, route="mail", as_of="2026-10-01")
    w = F.write(conn, "jump", "stage", "applied", sources=["x"], route="mail", as_of="2026-07-01")
    assert w.fact.as_of == "2026-10-01"


def test_import_maps_a_repo_stage_to_its_state(conn, tmp_path):
    p = tmp_path / "a.toml"
    p.write_text('[cv]\nstage = "released the G-Research version"\nsince = 2026-10-17\n')
    F.import_applications(conn, p)
    assert F.current(conn, "cv", "state")[0].value == "released the G-Research version"


def test_history_refuses_an_unknown_slot(conn):
    with pytest.raises(F.FactError, match="Nearest: stage"):
        F.history(conn, "millennium", "stag")


def test_remember_and_forget_tools(monkeypatch, tmp_path):
    monkeypatch.setenv("LOCUS_HEURIS_DB", str(tmp_path / "h.db"))
    from locus import mcp_server

    tools = {}

    def tool():
        def deco(fn):
            tools[fn.__name__] = fn
            return fn
        return deco

    mcp_server._register_heuris(tool)
    out = tools["remember"]("jump", "stage", "no offer; reapply next year")
    assert out.startswith("Remembered: jump · stage = no offer; reapply next year") and "he said so in a chat" in out
    assert tools["remember"]("jump", "stages", "x").startswith("Not recorded: `stages` is not a slot")
    out = tools["remember"]("jump", "stage", "offer")
    assert "replaces 'no offer; reapply next year'" in out
    fid = out.split("[fact:")[1].split("]")[0]
    out = tools["forget"](f"fact:{fid}")
    assert "Restored: jump · stage = no offer" in out
    assert tools["forget"]("fact:999").startswith("Nothing forgotten")
