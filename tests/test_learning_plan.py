"""Spec L, step L3: an approved curriculum becomes kairos tasks at the goal's pace."""

from __future__ import annotations

from datetime import date, datetime, timezone

import pytest

from locus.heuris import db as hdb
from locus.heuris import learning as L
from locus.heuris import learning_plan as LP

D = date(2026, 11, 16)


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
    assert [w.title for w, _ in r.created] == ["EV", "Markov", "r^2 = R^2"]        # leaves only
    posted = [c[2] for c in k.calls if c[1] == "/api/tasks"]
    assert [p["estimate_min"] for p in posted] == [180, 420, 120]
    assert all(p["key"] == "Careers" and p["create_deadline"] is False and p["plan"] is False for p in posted)
    assert posted[0]["earliest_start"] == "2026-11-16" and posted[2]["earliest_start"] == "2026-11-26"
    assert {c[3] for c in k.calls} == {"alec"}                        # he approved it
    assert [c[1] for c in k.calls].count("/api/plan") == 1            # one plan at the end
    assert L.get_goal(conn, "interview-prep").state == "active"
    states = {r[0] for r in conn.execute("SELECT state FROM topics WHERE goal='interview-prep'")}
    assert states == {"current"}
    again = LP.approve(conn, "interview-prep", kairos=k, today=D)
    assert again.created == [] and again.kept == 3 and len(k.tasks) == 3
    assert LP.tasks_of(conn, "interview-prep")[ids["c"]] == "t3"


def test_a_pace_change_re_spreads_what_is_left_of_the_open_tasks(conn):
    _goal(conn)
    k = FakeKairos()
    LP.approve(conn, "interview-prep", kairos=k, today=D)
    k.tasks["t1"]["status"] = "done"
    k.tasks["t2"]["remaining_min"] = 210                                # half of Markov left
    L.set_pace(conn, "interview-prep", 14, since=D)
    n, _ = LP.respread(conn, "interview-prep", kairos=k, today=D)
    assert n == 2
    assert k.tasks["t2"]["earliest_start"] == "2026-11-16" and k.tasks["t2"]["due"] == "2026-11-22"
    assert k.tasks["t3"]["earliest_start"] == "2026-11-17"            # 3.5 h then 2 h at 2 h a day
    assert "earliest_start" in k.tasks["t1"] and k.tasks["t1"]["earliest_start"] == "2026-11-16"   # untouched


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
