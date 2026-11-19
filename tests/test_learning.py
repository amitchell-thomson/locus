"""Spec L, step L1: learning state, its pace, its backup; the Learning/ folder in the sync."""

from __future__ import annotations

from datetime import date, datetime, timezone

import pytest

from locus import backup as bk
from locus.heuris import db as hdb
from locus.heuris import learning as L


@pytest.fixture(autouse=True)
def no_terms(monkeypatch):
    """Never kairos's live constraints file: no terms, every day vacation (CLAUDE.md §9)."""
    monkeypatch.setattr(L, "_terms_cache", [])
    monkeypatch.setattr(L, "_away_cache", [])


@pytest.fixture
def conn(tmp_path):
    c = hdb.connect(tmp_path / "h.db")
    yield c
    c.close()


def test_a_goal_has_a_readable_id_and_a_pace_that_changes_any_time(conn):
    with pytest.raises(L.LearningError, match="lowercase words"):
        L.add_goal(conn, "Interview Prep", "x")
    g = L.add_goal(conn, "interview-prep", "Quant interview mastery", priority=1)
    assert g.pace is None and g.state == "proposed"
    L.set_pace(conn, "interview-prep", 5, since="2026-11-16")
    # a vacation ramp that ends by itself (16 Nov: "easy to ramp up or down at any point in time")
    L.set_pace(conn, "interview-prep", 8, since="2026-12-06", until="2027-01-10", note="Christmas")
    assert L.current_pace(conn, "interview-prep", date(2026, 11, 20)) == (5.0, None)
    assert L.current_pace(conn, "interview-prep", date(2026, 12, 20)) == (8.0, "2027-01-10")
    assert L.current_pace(conn, "interview-prep", date(2027, 1, 11)) == (5.0, None)
    assert len(L.pace_history(conn, "interview-prep")) == 2        # every change kept
    with pytest.raises(L.LearningError):
        L.set_pace(conn, "interview-prep", 5, since="2027-01-10", until="2027-01-01")


def test_learning_state_survives_a_backup_and_restores_without_overwriting(tmp_path):
    db = tmp_path / "heuris.db"
    c = hdb.connect(db)
    L.add_goal(c, "quast", "QUAST start to finish", priority=3)
    L.set_pace(c, "quast", 1, since="2026-11-16")
    c.close()
    snap = tmp_path / "snap"
    snap.mkdir()
    assert bk._backup_ledger(db, snap / "heuris-ledger.db") >= 2
    fresh = tmp_path / "fresh.db"
    added = bk.restore_ledger(snap, fresh)
    assert added["goals"] == 1 and added["pace"] == 1
    assert bk.restore_ledger(snap, fresh)["goals"] == 0             # nothing twice
    c = hdb.connect(fresh)
    assert L.get_goal(c, "quast").pace == 1.0
    c.close()


def test_learning_folder_syncs_its_pdfs_but_never_its_code(tmp_path):
    from locus import files_content, files_sync

    root = tmp_path / "files"
    (root / "Learning" / ".git").mkdir(parents=True)                # the folder is a git repo
    sheets = root / "Learning" / "interview-prep" / "sheets"
    code = root / "Learning" / "cpp-oxdaq" / "code" / "ex01"
    sheets.mkdir(parents=True)
    code.mkdir(parents=True)
    (sheets / "baseline.pdf").write_bytes(b"%PDF-1.4 sheet")
    (code / "solution.pdf").write_bytes(b"%PDF-1.4 not for the tablet")
    (code / "main.cpp").write_text("int main() {}")
    folders = files_sync.laptop_folders(root)
    assert "Learning/interview-prep/sheets" in folders
    assert not any("/code" in f for f in folders)                   # code folders stay on the laptop
    pdfs = files_content.laptop_pdfs(root, {})
    assert "Learning/interview-prep/sheets/baseline.pdf" in pdfs
    assert not any("/code/" in p for p in pdfs)
    # a goal's folder is not a registry entry, and is never asked about
    plan = files_sync.plan(folders, {"Learning"}, {a: set() for a in files_sync.AREAS})
    assert "Learning/interview-prep" in plan.to_tablet and not plan.questions


# --- L2: a goal becomes a proposed curriculum ----------------------------------------------------

def _seed(conn):
    from locus.heuris.index import Item, index
    from tests.test_heuris_index import fake_embed
    from datetime import datetime, timezone

    at = datetime(2026, 10, 23, tzinfo=timezone.utc)
    index(conn, "library", [
        Item(ref="src:623#s0", kind="note", at=at, title="Feedback", thing="jump", parent="src:623",
             text="Manual coding in projects - digging deeper. Maths/stats - just way more practice. "
                  "chessboard question; probability/stats/regression stuff",
             meta={"unit": "section", "doc": 623, "lead": 1}),
        Item(ref="src:614#s0", kind="note", at=at, title="Jump call", thing="jump", parent="src:614",
             text="Areas for growth - programming. Do data structures and algorithms. trader interview - "
                  "brainteaser/probability", meta={"unit": "section", "doc": 614, "lead": 1}),
    ], embed=fake_embed)
    index(conn, "repos", [Item(ref="doc:locus/docs/redesign/slices/l-learning.md", kind="doc", at=at, title="spec L",
                               text="DSA problems written by hand, timed; feedback and weaknesses", thing="locus")],
          embed=fake_embed)
    return fake_embed


def test_a_curriculum_keeps_only_what_its_evidence_holds(conn):
    from locus.heuris import curriculum as C

    embed = _seed(conn)
    L.add_goal(conn, "interview-prep", "Quant interview mastery", priority=1)
    L.set_pace(conn, "interview-prep", 5, since="2026-11-16")
    reply = {
        "summary": "Programming and probability first, from the Jump feedback.",
        "topics": [
            {"key": "dsa", "track": "data-structures", "title": "Data structures and algorithms", "hours": 30,
             "weight": 3, "why": "named as the area for growth",
             "evidence": [{"ref": "src:614#s0", "quote": "Do data   structures and algorithms"}]},
            {"key": "dsa-trees", "parent": "dsa", "track": "data-structures", "title": "Trees", "hours": 10,
             "weight": 2, "why": "part of DSA",
             "evidence": [{"ref": "src:614#s0", "quote": "Areas for growth - programming"}]},
            {"key": "prob", "track": "probability", "title": "Probability puzzles", "hours": 20, "weight": 2.5,
             "why": "more practice", "evidence": [{"ref": "src:623#s0", "quote": "just way more practice"}]},
            # invented: the quote is not in the note, so the topic goes
            {"key": "ml", "track": "ml", "title": "Deep learning", "hours": 40, "weight": 1,
             "why": "made up", "evidence": [{"ref": "src:623#s0", "quote": "you must master transformers"}]},
            # a pointer it was never given
            {"key": "x", "track": "x", "title": "Elsewhere", "hours": 5, "weight": 1,
             "why": "made up", "evidence": [{"ref": "src:999#s0", "quote": "anything"}]},
        ],
    }
    seen = {}

    def ask(system, user):
        seen["user"] = user
        return "Here you go:\n" + __import__("json").dumps(reply)

    p = C.propose(conn, "interview-prep", refs=["src:623", "src:614"], queries=["feedback"], ask=ask, embed=embed,
                  rerank=lambda q, hits: hits, today=date(2026, 11, 16))
    assert "src:623#s0" in seen["user"] and "src:614#s0" in seen["user"]
    assert "doc:locus/" not in seen["user"]                    # its own spec is not evidence about him
    assert p.kept == 3 and {d["topic"] for d in p.dropped} >= {"Deep learning", "Elsewhere"}
    out = C.render(conn, "interview-prep")
    assert "Data structures and algorithms" in out and "Deep learning" not in out
    assert "  - Trees" in out                                  # a subtopic under its parent
    assert "30 h in all" in out and "at 5 h a week: done around" in out           # leaf hours (trees 10 + prob 20) at 5 h a week
    # a second proposal replaces the first proposed topics, never approved ones
    C.propose(conn, "interview-prep", refs=["src:614"], queries=["feedback"], ask=ask, embed=embed,
              rerank=lambda q, hits: hits, today=date(2026, 11, 16))
    assert conn.execute("SELECT COUNT(*) FROM topics WHERE goal='interview-prep'").fetchone()[0] == 3


def test_the_learning_screen_reads_the_curriculum_depth_first(conn):
    import json

    from locus.heuris import curriculum as C

    embed = _seed(conn)
    L.add_goal(conn, "interview-prep", "Quant interview mastery", priority=1)
    L.set_pace(conn, "interview-prep", 5, since="2026-11-16")
    reply = {"summary": "DSA first.", "topics": [
        {"key": "dsa", "track": "data-structures", "title": "DSA", "hours": 30, "weight": 3, "why": "named",
         "evidence": [{"ref": "src:614#s0", "quote": "Do data structures and algorithms"}]},
        {"key": "trees", "parent": "dsa", "track": "data-structures", "title": "Trees", "hours": 30, "weight": 2,
         "why": "part", "evidence": [{"ref": "src:614#s0", "quote": "Areas for growth - programming"}]}]}
    C.propose(conn, "interview-prep", refs=["src:614"], queries=["x"], ask=lambda s, u: json.dumps(reply),
              embed=embed, rerank=lambda q, h: h)
    out = C.export(conn)
    [g] = out["goals"]
    assert g["state"] == "proposed" and g["summary"] == "DSA first." and "at 5 h a week: done around" in g["pace"]
    assert [(i["what"], i["depth"]) for i in g["syllabus"]] == [("DSA", 0), ("Trees", 1)]
    assert g["syllabus"][1]["quote"] == "Areas for growth - programming" and out["tracks"][0]["id"] == "data-structures"
    # the shape the Rust contract accepts (deny_unknown_fields): checked by the TUI's own test
    assert set(g) <= {"id", "title", "by", "state", "syllabus", "pace", "summary"}


def test_a_goal_in_rounds_reads_the_whole_plan_and_is_told_to_keep_it(conn):
    """19 Nov: cpp-oxdaq's ROUNDS.md was cut at its second round by the 4,000 character cap, and
    the generic prompt rebuilt it as 4 to 8 sections of its own."""
    from locus.heuris import curriculum as C
    from locus.heuris.index import Item, index

    embed = _seed(conn)
    rounds = ("| R1 | reference book |" + " filler" * 1200 + " | R8 | the `engine = py \\| cpp` switch |"
              " | R13 | engine v2, cache hierarchy |")
    index(conn, "repos", [Item(ref="doc:learning/cpp-oxdaq/ROUNDS.md", kind="doc", at=datetime(2026, 11, 19, tzinfo=timezone.utc),
                               title="OXDAQ build rounds", text=rounds, thing="learning")], embed=embed)
    L.add_goal(conn, "cpp-oxdaq", "C++ alongside OXDAQ", priority=1)
    with pytest.raises(L.LearningError, match="shape is one of"):
        L.set_shape(conn, "cpp-oxdaq", "spiral")
    assert "in rounds" in L.describe(L.set_shape(conn, "cpp-oxdaq", "rounds"))
    seen = {}

    def ask(system, user):
        seen.update(system=system, user=user)
        def t(key, quote, parent=None):
            return {"key": key, "parent": parent, "track": "x", "title": key, "hours": 2, "weight": 2, "why": "w",
                    "evidence": [{"ref": "doc:learning/cpp-oxdaq/ROUNDS.md", "quote": quote}]}
        return __import__("json").dumps({"summary": "s", "topics": [
            t("R8", "the `engine = py | cpp` switch"), t("R8 build", "reference book", "R8"),   # escape unread
            t("R9", "an invented quote"), t("R9 build", "reference book", "R9"),               # its section went
            t("R13", "engine v2, cache hierarchy")]})

    p = C.propose(conn, "cpp-oxdaq", refs=["doc:learning/cpp-oxdaq/ROUNDS.md"], queries=["c++"], ask=ask,
                  embed=embed, rerank=lambda q, hits: hits, today=date(2026, 11, 19))
    assert "cache hierarchy" in seen["user"]                   # the last round reached the prompt
    assert "ROUNDS" in seen["system"]
    kept = [r[0] for r in conn.execute("SELECT title FROM topics WHERE goal='cpp-oxdaq' ORDER BY ord")]
    assert kept == ["R8", "R8 build", "R13"] and p.kept == 3
    assert {"topic": "R9 build", "ref": None, "quote": "dropped with its section"} in p.dropped
