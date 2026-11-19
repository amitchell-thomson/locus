"""A curriculum approved becomes time (spec L, step L3; L-D7).

**Approving** a goal's proposed curriculum makes its topics current (an earlier current curriculum
is kept as `replaced`), the goal active, and gives every leaf topic a kairos task: the estimate is
the topic's hours, and its window is where the topic falls when the curriculum is walked at the
goal's pace. The window is the task's `earliest_start` and `due`, so kairos's scheduler
places the hours inside it and the weekly load is the pace, not everything at once. Nothing is
hand-placed: one `plan` at the end puts the blocks in the calendar (the kairos rule).

**The sections run side by side** (16 Nov: "definitely interleave"): each top-level section is a
lane walked in its own order, at a share of the pace in proportion to its hours, so every week
holds some probability, some coding, some statistics, and every lane ends together. Walked in tree
order, all of probability came before any coding (seven weeks). Other goals are lanes of their
own at their own pace, so their tasks interleave in the calendar the same way.

**A goal in rounds is walked in order** (19 Nov, cpp-oxdaq): its rounds each need the one
before, so its leaves are one lane (`spread`); its concepts still come just before their build.

**The pace follows term and vacation** (16 Nov: "less time during term ... higher during
holidays"): the pace on each day is that day's season's (`learning.current_pace`), so a window
that crosses into the vacation shortens by itself.

**A window's end is soft** (17 Nov: blocks and ⚑ both showed, and the ⚑ read as 32 deadlines):
`soft_due` makes kairos plan to the date without drawing it. Nothing here is a real deadline;
the pace is the commitment.

**A window is at least a week** (`MIN_WINDOW_DAYS`): at 5 h a week a 2 h topic's exact share is
under three days, and a due date that tight turns one busy evening into a shortfall. Windows
of short topics therefore overlap a little; the scheduler spreads them, and the pace holds over
the weeks.

**A topic follows the one before it** (19 Nov): windows of a week overlap, and kairos placed
17 of cpp-oxdaq's topics before the one they follow (a build before its concepts). Each task
now names its predecessor (`after`: in rounds the topic before; in a tree the topic before in its
section) and kairos plans it only after that task's blocks.

**A pace change re-spreads** (L-D7: "easy to change if I want to ramp up or down"): the goal's
open tasks are walked again from today with what is LEFT of each (kairos's `remaining_min`), and
each gets its new window. Finished and dropped tasks are left alone.

The tasks are written as Alec (he approved them) and each is one low-risk changeset kairos applies
and can undo; `goal_tasks` remembers which task is which topic's, so approving twice never makes
a second set.
"""

from __future__ import annotations

import sqlite3
from collections.abc import Callable
from dataclasses import dataclass
from datetime import date, datetime, timedelta, timezone

from locus.heuris import curriculum as C
from locus.heuris import learning as L

SCHEMA = """
CREATE TABLE IF NOT EXISTS goal_tasks (
    topic    INTEGER PRIMARY KEY REFERENCES topics(id),
    goal     TEXT NOT NULL REFERENCES goals(id),
    task_id  TEXT NOT NULL,                -- kairos's task id
    created  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS goal_tasks_goal ON goal_tasks(goal);
"""
MIN_WINDOW_DAYS = 7
# A session is 45 to 90 minutes (17 Nov): kairos plans a 3 h task as coursework, in 2 to 3 h
# blocks, which is one topic for a whole morning; short sessions are what interleaving needs.
SESSION_MIN, SESSION_MAX = 45, 90
HORIZON_DAYS = 3 * 365          # a pace of 0 for longer than this is "never", said plainly

Kairos = Callable[..., object]  # locus.kairos_http.call's shape: (method, path, body=None, ...)


def ensure(conn: sqlite3.Connection) -> None:
    L.ensure(conn)
    C.ensure(conn)
    conn.executescript(SCHEMA)


@dataclass
class Window:
    topic: int
    title: str
    hours: float
    start: date
    due: date


def windows(items: list[tuple[int, str, float]], pace_on: Callable[[date], float], start: date) -> list[Window]:
    """Walk `items` (topic, title, hours) in order, a day at a time, each day giving a seventh
    of that day's weekly pace. A topic's window runs from the day its first hour falls to the day
    its last does, stretched to `MIN_WINDOW_DAYS`."""
    out: list[Window] = []
    day, left_today = start, pace_on(start) / 7

    def next_day() -> None:
        nonlocal day, left_today
        day += timedelta(days=1)
        if (day - start).days > HORIZON_DAYS:
            raise L.LearningError("at this pace the curriculum would not finish within three years: set a pace first")
        left_today = pace_on(day) / 7

    for tid, title, hours in items:
        while left_today <= 1e-9:
            next_day()
        first, need = day, hours
        while True:
            take = min(need, left_today)
            need -= take
            left_today -= take
            if need <= 1e-9:
                break
            next_day()
        out.append(Window(tid, title, hours, first, max(day, first + timedelta(days=MIN_WINDOW_DAYS - 1))))
    return out


def lanes(leaves: list[tuple[int, str, float]], root_of: dict[int, int], pace_on: Callable[[date], float],
          start: date) -> list[Window]:
    """Windows with the sections side by side: `leaves` in curriculum order, `root_of` each one's
    top-level section. A lane gets the pace times its share of the hours, so all lanes end
    together. Returned in start order."""
    by: dict[int, list[tuple[int, str, float]]] = {}
    for t in leaves:
        by.setdefault(root_of.get(t[0], t[0]), []).append(t)
    total = sum(h for _, _, h in leaves) or 1.0
    out: list[Window] = []
    for items in by.values():
        share = sum(h for _, _, h in items) / total
        out += windows(items, lambda d, s=share: pace_on(d) * s, start)
    order = {t[0]: i for i, t in enumerate(leaves)}
    return sorted(out, key=lambda w: (w.start, order[w.topic]))


def spread(conn: sqlite3.Connection, g: L.Goal, items: list[tuple[int, str, float]], start: date) -> list[Window]:
    """The goal's windows: a tree's sections side by side, a goal in rounds walked in order (its
    rounds depend on the ones before, so R2 never starts before R1's hours are spent)."""
    if g.shape == "rounds":
        return windows(items, L.pace_fn(conn, g.id), start)
    return lanes(items, _roots(conn, g.id), L.pace_fn(conn, g.id), start)


def leaders(conn: sqlite3.Connection, g: L.Goal, topics: list[int]) -> dict[int, int]:
    """Each topic's predecessor, which kairos plans it after (19 Nov: windows of a week overlap,
    so without it 17 of cpp-oxdaq's topics were placed before the one they follow, a build
    among them before its concepts). In rounds, the topic before it; in a tree, the topic before
    it in its own section, so sections still interleave."""
    roots = _roots(conn, g.id)
    last: dict[int, int] = {}
    out: dict[int, int] = {}
    for t in topics:
        lane = 0 if g.shape == "rounds" else roots.get(t, t)
        if lane in last:
            out[t] = last[lane]
        last[lane] = t
    return out


def _roots(conn: sqlite3.Connection, gid: str) -> dict[int, int]:
    parent = dict(conn.execute("SELECT id, parent FROM topics WHERE goal=?", (gid,)).fetchall())
    out = {}
    for t in parent:
        r = t
        while parent.get(r) is not None:
            r = parent[r]
        out[t] = r
    return out


def _leaves(conn: sqlite3.Connection, gid: str, state: str) -> list[tuple[int, str, float, str]]:
    """(id, title, hours, why) of the topics with no children, in the curriculum's order."""
    rows = conn.execute("SELECT id, parent, title, hours FROM topics WHERE goal=? AND state=? ORDER BY ord",
                        (gid, state)).fetchall()
    kids: dict[int | None, list] = {}
    for r in rows:
        kids.setdefault(r[1], []).append(r)
    out: list[tuple[int, str, float, str]] = []

    def walk(parent):
        for tid, _, title, hours in kids.get(parent, []):
            if tid in kids:
                walk(tid)
            else:
                ev = conn.execute("SELECT why FROM topic_evidence WHERE topic=? LIMIT 1", (tid,)).fetchone()
                out.append((tid, title, float(hours), ev[0] if ev else ""))
    walk(None)
    return out


def _area(conn: sqlite3.Connection, g: L.Goal) -> str:
    if g.area:
        return g.area
    if g.thing:
        from locus.heuris import registry

        if (a := registry.area_of(g.thing)) in L.AREAS:
            return a
    raise L.LearningError(f"{g.id} has no area: `locus learn area {g.id} Careers` (or Studies, Projects)")


@dataclass
class Approved:
    goal: L.Goal
    created: list[tuple[Window, str]]     # (window, kairos task id)
    kept: int                             # topics that already had a task
    plan: str                             # kairos's plan summary


def plan_preview(conn: sqlite3.Connection, gid: str, *, today: date | None = None) -> list[Window]:
    """The windows approving would give, from the proposed curriculum (or the current one)."""
    ensure(conn)
    g = L.get_goal(conn, gid)
    state = "proposed" if _leaves(conn, gid, "proposed") else "current"
    items = [(t, title, h) for t, title, h, _ in _leaves(conn, g.id, state)]
    return spread(conn, g, items, today or date.today())


def approve(conn: sqlite3.Connection, gid: str, *, kairos: Kairos, today: date | None = None) -> Approved:
    ensure(conn)
    g = L.get_goal(conn, gid)
    area = _area(conn, g)
    leaves = _leaves(conn, gid, "proposed")
    if not leaves:
        if not _leaves(conn, gid, "current"):
            raise L.LearningError(f"{gid} has no proposed curriculum: `locus learn propose {gid}` first")
        leaves = _leaves(conn, gid, "current")      # approved already: make any missing tasks
    else:
        with conn:
            conn.execute("UPDATE topics SET state='replaced' WHERE goal=? AND state='current'", (gid,))
            conn.execute("UPDATE topics SET state='current' WHERE goal=? AND state='proposed'", (gid,))
    if g.state != "active":
        g = L.set_state(conn, gid, "active")
    have = {r[0] for r in conn.execute("SELECT topic FROM goal_tasks WHERE goal=?", (gid,))}
    ws = spread(conn, g, [(t, title, h) for t, title, h, _ in leaves], today or date.today())
    why = {t: w for t, _, _, w in leaves}
    follows = leaders(conn, g, [t for t, _, _, _ in leaves])
    have_task = tasks_of(conn, gid)
    created: list[tuple[Window, str]] = []
    # Made in curriculum order, so the task a topic follows always exists first.
    order = {t: k for k, (t, _, _, _) in enumerate(leaves)}
    for w in sorted(ws, key=lambda w: order[w.topic]):
        if w.topic in have:
            continue
        body = {"title": w.title, "key": area, "estimate_min": max(15, round(w.hours * 60)),
                "earliest_start": w.start.isoformat(), "due": w.due.isoformat(),
                "soft_due": True, "create_deadline": False, "plan": False,
                "min_chunk_min": SESSION_MIN, "max_chunk_min": SESSION_MAX, "ideal_chunk_min": SESSION_MIN,
                "notes": f"Learning goal {gid} ({g.title}), topic {w.topic}. {why.get(w.topic, '')}\n"
                         f"Sheets and code: ~/files/Learning/{gid}/"}
        if g.thing:
            body["locus"] = g.thing
        if (lead := follows.get(w.topic)) is not None and (lt := have_task.get(lead)):
            body["after"] = lt
        resp = kairos("POST", "/api/tasks", body, author="alec")
        tid = resp["task"]["id"]
        with conn:
            conn.execute("INSERT INTO goal_tasks(topic, goal, task_id, created) VALUES (?,?,?,?)",
                         (w.topic, gid, tid, datetime.now(timezone.utc).isoformat(timespec="seconds")))
        have_task[w.topic] = tid
        created.append((w, tid))
    created.sort(key=lambda c: (c[0].start, order[c[0].topic]))
    p = kairos("POST", "/api/plan", {"dry_run": False}, author="alec") if created else None
    return Approved(g, created, len(ws) - len(created), (p or {}).get("summary", "nothing new to plan"))


def reject(conn: sqlite3.Connection, gid: str) -> int:
    """The proposed curriculum refused: its topics go to `rejected`, the current one stays."""
    ensure(conn)
    L.get_goal(conn, gid)
    with conn:
        n = conn.execute("UPDATE topics SET state='rejected' WHERE goal=? AND state='proposed'", (gid,)).rowcount
    return n


def respread(conn: sqlite3.Connection, gid: str, *, kairos: Kairos, today: date | None = None) -> tuple[int, str]:
    """After a pace change: every open task of the goal gets a new window for what is left of it.
    Returns (tasks moved, kairos's plan summary). A goal with no tasks yet moves nothing."""
    ensure(conn)
    rows = conn.execute("SELECT g.topic, g.task_id, t.title FROM goal_tasks g JOIN topics t ON t.id=g.topic "
                        "WHERE g.goal=? ORDER BY t.ord", (gid,)).fetchall()
    if not rows:
        return 0, "no tasks yet"
    items, ids = [], {}
    for topic, task_id, title in rows:
        t = kairos("GET", f"/api/tasks/{task_id}")
        if t.get("status", "open") != "open" or not t.get("remaining_min"):
            continue
        items.append((topic, title, t["remaining_min"] / 60))
        ids[topic] = task_id
    g = L.get_goal(conn, gid)
    ws = spread(conn, g, items, today or date.today())
    # A finished topic holds nothing back, so each open one follows the open one before it.
    follows = leaders(conn, g, [t for t, _, _ in items])
    for w in ws:
        lead = follows.get(w.topic)
        kairos("PATCH", f"/api/tasks/{ids[w.topic]}",
               {"earliest_start": w.start.isoformat(), "due": w.due.isoformat(), "plan": False,
                "after": ids[lead] if lead is not None else "",
                "min_chunk_min": SESSION_MIN, "max_chunk_min": SESSION_MAX, "ideal_chunk_min": SESSION_MIN},
               author="alec")
    p = kairos("POST", "/api/plan", {"dry_run": False}, author="alec") if ws else None
    return len(ws), (p or {}).get("summary", "")


def tasks_of(conn: sqlite3.Connection, gid: str) -> dict[int, str]:
    ensure(conn)
    return dict(conn.execute("SELECT topic, task_id FROM goal_tasks WHERE goal=?", (gid,)).fetchall())
