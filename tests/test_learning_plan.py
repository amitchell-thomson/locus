"""Spec L, step L3: an approved curriculum becomes kairos tasks at the goal's pace."""

from __future__ import annotations

from datetime import date, datetime, timezone

import pytest

from locus.heuris import db as hdb
from locus.heuris import learning as L
from locus.heuris import learning_plan as LP

D = date(2026, 11, 16)


@pytest.fixture(autouse=True)
def no_terms(monkeypatch):
    """Never kairos's live constraints file: no terms, every day vacation (CLAUDE.md §9)."""
    monkeypatch.setattr(L, "_terms_cache", [])
    monkeypatch.setattr(L, "_away_cache", [])


@pytest.fixture
def conn(tmp_path):
    c = hdb.connect(tmp_path / "h.db")
    LP.ensure(c)
    yield c
    c.close()


class FakeKairos:
    def __init__(self):
        self.tasks: dict[str, dict] = {}
        self.calls: list[tuple] = []

    def __call__(self, method, path, body=None, headers=None, *, author="claude"):
        self.calls.append((method, path, body, author))
        if method == "POST" and path == "/api/tasks":
            tid = f"t{len(self.tasks) + 1}"
            self.tasks[tid] = {"id": tid, "status": "open", "remaining_min": body["estimate_min"], **body}
            return {"task": self.tasks[tid], "changesets": [{"status": "applied"}]}
        if method == "POST" and path == "/api/plan":
            return {"summary": "planned"}
        tid = path.rsplit("/", 1)[-1]
        if method == "GET":
            return self.tasks[tid]
        if method == "PATCH":
            self.tasks[tid].update(body)
            return {"task": self.tasks[tid]}
        raise AssertionError(path)


def _goal(conn, *, state="proposed"):
    L.add_goal(conn, "interview-prep", "Quant interviews", area="Careers")
    L.set_pace(conn, "interview-prep", 7, since=D)          # an hour a day: easy arithmetic
    now = datetime.now(timezone.utc).isoformat()
    ids = {}
    for key, parent, ord_, title, hours in [("p", None, 0, "Probability", 10), ("a", "p", 1, "EV", 3),
                                            ("b", "p", 2, "Markov", 7), ("c", None, 3, "r^2 = R^2", 2)]:
        ids[key] = conn.execute("INSERT INTO topics(goal, parent, ord, track, title, hours, state, created) "
                                "VALUES (?,?,?,?,?,?,?,?)",
                                ("interview-prep", ids.get(parent), ord_, "x", title, hours, state, now)).lastrowid
        conn.execute("INSERT INTO topic_evidence(topic, ref, quote, why) VALUES (?,?,?,?)",
                     (ids[key], "fact:1", "q", f"why {title}"))
    conn.commit()
    return ids


def test_windows_follow_the_pace_and_last_at_least_a_week():
    pace = lambda d: 7.0 if d < date(2026, 12, 1) else 14.0      # noqa: E731  (a ramp)
    ws = LP.windows([(1, "a", 3), (2, "b", 7), (3, "c", 20)], pace, D)
    assert ws[0].start == D and ws[0].due == date(2026, 11, 22)    # 3 days of work, stretched to a week
    assert ws[1].start == date(2026, 11, 19) and ws[1].due == date(2026, 11, 25)
    # from 1 Dec two hours a day: 15 Nov 26..30 give 5, then 15 more over 8 days
    assert ws[2].start == date(2026, 11, 26) and ws[2].due == date(2026, 12, 8)
    with pytest.raises(L.LearningError, match="set a pace"):
        LP.windows([(1, "a", 1)], lambda d: 0.0, D)


def test_approving_makes_one_task_per_leaf_topic_in_order_and_never_twice(conn):
    ids = _goal(conn)
    k = FakeKairos()
    r = LP.approve(conn, "interview-prep", kairos=k, today=D)
    # leaves only, the two sections side by side: r^2 starts with EV, not after Markov
    assert [w.title for w, _ in r.created] == ["EV", "r^2 = R^2", "Markov"]
    posted = {c[2]["title"]: c[2] for c in k.calls if c[1] == "/api/tasks"}
    assert {t: p["estimate_min"] for t, p in posted.items()} == {"EV": 180, "Markov": 420, "r^2 = R^2": 120}
    assert all(p["key"] == "Careers" and p["soft_due"] is True and p["create_deadline"] is False and p["max_chunk_min"] == 90
               and p["plan"] is False for p in posted.values())
    assert posted["EV"]["earliest_start"] == posted["r^2 = R^2"]["earliest_start"] == "2026-11-16"
    assert posted["Markov"]["earliest_start"] == "2026-11-19"       # 3 h at 10/12 of an hour a day
    assert {c[3] for c in k.calls} == {"alec"}                        # he approved it
    assert [c[1] for c in k.calls].count("/api/plan") == 1            # one plan at the end
    assert L.get_goal(conn, "interview-prep").state == "active"
    states = {r[0] for r in conn.execute("SELECT state FROM topics WHERE goal='interview-prep'")}
    assert states == {"current"}
    again = LP.approve(conn, "interview-prep", kairos=k, today=D)
    assert again.created == [] and again.kept == 3 and len(k.tasks) == 3
    assert LP.tasks_of(conn, "interview-prep")[ids["c"]] == "t3"           # made in curriculum order


def test_a_pace_change_re_spreads_what_is_left_of_the_open_tasks(conn):
    _goal(conn)
    k = FakeKairos()
    LP.approve(conn, "interview-prep", kairos=k, today=D)
    by = {t["title"]: t for t in k.tasks.values()}
    by["EV"]["status"] = "done"
    by["Markov"]["remaining_min"] = 210                                 # half of Markov left
    by["r^2 = R^2"]["remaining_min"] = 60
    L.set_pace(conn, "interview-prep", 9, since=D)                     # 4.5 h left: 3.5 + 1
    n, _ = LP.respread(conn, "interview-prep", kairos=k, today=D)
    assert n == 2
    # Markov's lane gets 3.5/4.5 of 9 h a week = 1 h a day: 3.5 days, stretched to a week
    assert by["Markov"]["earliest_start"] == "2026-11-16" and by["Markov"]["due"] == "2026-11-22"
    assert by["EV"]["due"] == "2026-11-22" and by["EV"]["status"] == "done"   # untouched


def test_rejecting_keeps_the_current_curriculum_and_approving_replaces_it(conn):
    _goal(conn, state="current")
    now = datetime.now(timezone.utc).isoformat()
    conn.execute("INSERT INTO topics(goal, ord, track, title, hours, state, created) VALUES "
                 "('interview-prep', 0, 'x', 'New', 4, 'proposed', ?)", (now,))
    assert LP.reject(conn, "interview-prep") == 1
    assert conn.execute("SELECT COUNT(*) FROM topics WHERE state='current'").fetchone()[0] == 4
    conn.execute("INSERT INTO topics(goal, ord, track, title, hours, state, created) VALUES "
                 "('interview-prep', 0, 'x', 'Newer', 4, 'proposed', ?)", (now,))
    conn.commit()
    r = LP.approve(conn, "interview-prep", kairos=FakeKairos(), today=D)
    assert [w.title for w, _ in r.created] == ["Newer"]
    assert conn.execute("SELECT COUNT(*) FROM topics WHERE state='replaced'").fetchone()[0] == 4


def test_a_goal_needs_an_area_for_its_tasks(conn):
    L.add_goal(conn, "loose", "No area")
    with pytest.raises(L.LearningError, match="no area"):
        LP._area(conn, L.get_goal(conn, "loose"))
    assert L.set_area(conn, "loose", "Studies").area == "Studies"


def test_term_and_vacation_paces_and_the_finish_they_give(conn):
    terms = [(date(2026, 10, 4), date(2026, 12, 5)), (date(2027, 1, 10), date(2027, 3, 13))]
    L.add_goal(conn, "g", "G", area="Careers")
    L.set_pace(conn, "g", 5, since=D)                                   # the old flat pace
    L.set_pace(conn, "g", 3, since=D, season="term")
    L.set_pace(conn, "g", 7, since=D, season="vacation")
    assert L.current_pace(conn, "g", date(2026, 11, 20), terms) == (3.0, None)
    assert L.current_pace(conn, "g", date(2026, 12, 20), terms) == (7.0, None)
    assert L.current_pace(conn, "g", date(2027, 2, 1), terms) == (3.0, None)
    # a later flat pace overrides both seasons
    L.set_pace(conn, "g", 4, since=date(2027, 4, 1))
    assert L.current_pace(conn, "g", date(2027, 4, 2), terms) == (4.0, None)
    on = L.pace_fn(conn, "g", terms)
    # term at 3/7 h a day until 5 Dec (20 days, 8.6 h), then vacation at 1 h a day
    assert L.finish(3, on, D) == date(2026, 11, 22)
    assert L.finish(6, on, D) == date(2026, 11, 29)
    assert L.finish(10, on, D) == date(2026, 12, 7)


def test_sections_interleave_from_the_first_week():
    leaves = [(1, "EV", 6), (2, "Markov", 6), (3, "Arrays", 6), (4, "Trees", 6)]
    roots = {1: 10, 2: 10, 3: 20, 4: 20}
    ws = LP.lanes(leaves, roots, lambda d: 7.0, D)
    first = {w.title: w.start for w in ws}
    assert first["EV"] == first["Arrays"] == D                        # both sections start at once
    assert first["Markov"] == first["Trees"] == date(2026, 11, 28)    # each lane at half an hour a day


def test_a_snapshot_from_before_a_column_restores_by_name(tmp_path):
    import sqlite3

    from locus import backup as bk

    snap = tmp_path / "snap"
    snap.mkdir()
    old = sqlite3.connect(snap / "heuris-ledger.db")
    old.executescript("CREATE TABLE goals (id TEXT PRIMARY KEY, title TEXT NOT NULL, master_by TEXT, "
                      "state TEXT NOT NULL, priority INTEGER NOT NULL, thing TEXT, notes TEXT, "
                      "created TEXT NOT NULL, changed TEXT NOT NULL);"
                      "INSERT INTO goals VALUES ('quast', 'QUAST', NULL, 'proposed', 2, 'quast', 'little and often', "
                      "'2026-11-16', '2026-11-16');")
    old.commit()
    old.close()
    added = bk.restore_ledger(snap, tmp_path / "h.db")
    assert added["goals"] == 1
    c = hdb.connect(tmp_path / "h.db")
    g = L.get_goal(c, "quast")
    assert (g.thing, g.notes, g.area) == ("quast", "little and often", None)


def test_a_goal_in_rounds_is_walked_in_order_not_side_by_side(conn):
    """19 Nov, cpp-oxdaq: each round needs the one before, so its second round starts only
    when the first's hours are spent; a tree goal's sections start together."""
    ids = _goal(conn)
    side = LP.plan_preview(conn, "interview-prep", today=D)
    assert {w.title: w.start for w in side}["r^2 = R^2"] == D      # its own lane, from day one
    L.set_shape(conn, "interview-prep", "rounds")
    ws = {w.title: w for w in LP.plan_preview(conn, "interview-prep", today=D)}
    assert ws["EV"].start == D and ws["Markov"].start == date(2026, 11, 19)
    assert ws["r^2 = R^2"].start == date(2026, 11, 26)              # after all ten hours of the first
    k = FakeKairos()
    LP.approve(conn, "interview-prep", kairos=k, today=D)
    made = [b for _, p, b, _ in k.calls if p == "/api/tasks"]
    assert [b["earliest_start"] for b in made] == ["2026-11-16", "2026-11-19", "2026-11-26"]
    # each follows the one before it, whatever its section; sessions prefer 45 to 90 min
    assert [b.get("after") for b in made] == [None, "t1", "t2"]
    assert all(b["ideal_chunk_min"] == 45 and b["max_chunk_min"] == 90 for b in made)
    assert all(b["asap"] and b["term_weekend_bias"] for b in made)
    assert ids


def test_a_tree_goal_chains_topics_within_each_section_only(conn):
    _goal(conn)
    k = FakeKairos()
    LP.approve(conn, "interview-prep", kairos=k, today=D)
    made = {b["title"]: b for _, p, b, _ in k.calls if p == "/api/tasks"}
    tid = {b["title"]: t for t, b in k.tasks.items()}
    assert made["Markov"]["after"] == tid["EV"]
    assert "after" not in made["EV"] and "after" not in made["r^2 = R^2"]     # sections start together
    # a re-spread keeps the chain and the session sizes on what is still open
    k.tasks[tid["EV"]]["status"] = "done"
    LP.respread(conn, "interview-prep", kairos=k, today=D)
    patches = {path.rsplit("/", 1)[-1]: b for m, path, b, _ in k.calls if m == "PATCH"}
    assert patches[tid["Markov"]]["after"] == "" and patches[tid["Markov"]]["ideal_chunk_min"] == 45


def test_days_away_carry_no_pace(conn, monkeypatch):
    """19 Nov: Christmas and 28 Dec to 3 Jan are kairos's `away` days; the walk skips them."""
    _goal(conn)
    monkeypatch.setattr(L, "_away_cache", [(date(2026, 11, 17), date(2026, 11, 18))])
    ws = {w.title: w for w in LP.plan_preview(conn, "interview-prep", today=D)}
    # EV's 3 h at 10/12 of an hour a day (its lane's share) ends on the 19th without the days
    # away (test above); skipping the 17th and 18th, on the 21st
    assert ws["Markov"].start == date(2026, 11, 21)
