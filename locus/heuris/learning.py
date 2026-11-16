"""The learning system's state (spec L, L-D8): goals, their topics, their pace.

NOT derived: a goal he approved, the marks he accepted and the pace he set exist nowhere else, so
these tables sit beside the facts ledger in heuris.db, outside `INDEX_TABLES` (a rebuilt index
never touches them), and `locus backup` copies them (`backup.LEDGER_TABLES`).

- **goals**: one row per goal, its id readable (`interview-prep`, L-D12: lowercase words joined
  by hyphens, the folder `~/files/Learning/<id>/` takes it), a title, a master-by date, a state
  (proposed, active, paused, done, archived) and a priority (1 highest).
- **topics**: the curriculum's tree (L-D2), filled from L2; a topic belongs to a goal and names
  its track, so a track shared by two goals (linear algebra) is one track.
- **topic_evidence**: why a topic is there, as pointers (L-D2's grounding).
- **pace**: hours a week per goal, from a date, optionally until one (L-D7: "easy to change if I
  want to ramp up or down the practice at any point in time"). The current pace is the latest row
  whose window holds today; a change is a new row, so its history is kept.
"""

from __future__ import annotations

import re
import sqlite3
from dataclasses import dataclass
from datetime import date, datetime, timezone

SCHEMA = """
CREATE TABLE IF NOT EXISTS goals (
    id         TEXT PRIMARY KEY,
    title      TEXT NOT NULL,
    master_by  TEXT,                       -- YYYY-MM-DD, or NULL for no hard date
    state      TEXT NOT NULL DEFAULT 'proposed',
    priority   INTEGER NOT NULL DEFAULT 2,
    thing      TEXT,                       -- a registry id it serves (oxdaq's oqts, quast), if any
    notes      TEXT,
    created    TEXT NOT NULL,
    changed    TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS topics (
    id          INTEGER PRIMARY KEY,
    goal        TEXT NOT NULL REFERENCES goals(id),
    parent      INTEGER REFERENCES topics(id),
    ord         INTEGER NOT NULL DEFAULT 0,
    track       TEXT NOT NULL,             -- shared across goals by name: `linear-algebra`
    title       TEXT NOT NULL,
    hours       REAL NOT NULL DEFAULT 1,
    weight      REAL NOT NULL DEFAULT 1,   -- weaker first
    confidence  REAL,                      -- NULL until a marked answer sets it
    state       TEXT NOT NULL DEFAULT 'current',
    created     TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS topic_evidence (
    id      INTEGER PRIMARY KEY,
    topic   INTEGER NOT NULL REFERENCES topics(id),
    ref     TEXT NOT NULL,                 -- a pointer `open` follows
    quote   TEXT,                          -- the line it rests on, checked to be there
    why     TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS pace (
    id       INTEGER PRIMARY KEY,
    goal     TEXT NOT NULL REFERENCES goals(id),
    hours    REAL NOT NULL,                -- per week
    since    TEXT NOT NULL,                -- YYYY-MM-DD
    until    TEXT,                         -- YYYY-MM-DD inclusive, or NULL: from then on
    set_at   TEXT NOT NULL,
    note     TEXT
);
CREATE INDEX IF NOT EXISTS topics_goal ON topics(goal);
CREATE INDEX IF NOT EXISTS pace_goal ON pace(goal, since);
"""

TABLES = ("goals", "topics", "topic_evidence", "pace")
STATES = ("proposed", "active", "paused", "done", "archived")
_ID = re.compile(r"^[a-z0-9]+(-[a-z0-9]+)*$")


class LearningError(ValueError):
    pass


def ensure(conn: sqlite3.Connection) -> None:
    conn.executescript(SCHEMA)


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def _day(v: str | date | None) -> str | None:
    if v is None or v == "":
        return None
    d = v if isinstance(v, date) else date.fromisoformat(str(v))
    return d.isoformat()


@dataclass
class Goal:
    id: str
    title: str
    master_by: str | None
    state: str
    priority: int
    thing: str | None
    notes: str | None
    pace: float | None = None        # hours a week now
    pace_until: str | None = None    # when the current pace ends, if it does


def add_goal(conn: sqlite3.Connection, gid: str, title: str, *, master_by: str | date | None = None,
             priority: int = 2, thing: str | None = None, notes: str | None = None,
             state: str = "proposed") -> Goal:
    ensure(conn)
    if not _ID.match(gid):
        raise LearningError(f"goal id {gid!r}: lowercase words joined by hyphens (interview-prep)")
    if state not in STATES:
        raise LearningError(f"state is one of {', '.join(STATES)}")
    now = _now()
    with conn:
        if conn.execute("SELECT 1 FROM goals WHERE id=?", (gid,)).fetchone():
            raise LearningError(f"there is already a goal {gid!r}")
        conn.execute("INSERT INTO goals(id, title, master_by, state, priority, thing, notes, created, changed) "
                     "VALUES (?,?,?,?,?,?,?,?,?)",
                     (gid, title.strip(), _day(master_by), state, priority, thing, notes, now, now))
    return get_goal(conn, gid)


def set_notes(conn: sqlite3.Connection, gid: str, notes: str) -> Goal:
    """What steers the goal's curriculum, in his words (16 Nov: "get rid of weekly mocks"); the
    next proposal reads it."""
    get_goal(conn, gid)
    with conn:
        conn.execute("UPDATE goals SET notes=?, changed=? WHERE id=?", (notes.strip(), _now(), gid))
    return get_goal(conn, gid)


def set_state(conn: sqlite3.Connection, gid: str, state: str) -> Goal:
    if state not in STATES:
        raise LearningError(f"state is one of {', '.join(STATES)}")
    get_goal(conn, gid)
    with conn:
        conn.execute("UPDATE goals SET state=?, changed=? WHERE id=?", (state, _now(), gid))
    return get_goal(conn, gid)


def current_pace(conn: sqlite3.Connection, gid: str, on: date | None = None) -> tuple[float, str | None] | None:
    """(hours a week, until) in force on `on` (today): the latest pace set whose window holds it."""
    ensure(conn)
    d = (on or date.today()).isoformat()
    r = conn.execute("SELECT hours, until FROM pace WHERE goal=? AND since<=? AND (until IS NULL OR until>=?) "
                     "ORDER BY since DESC, id DESC LIMIT 1", (gid, d, d)).fetchone()
    return (float(r[0]), r[1]) if r else None


def set_pace(conn: sqlite3.Connection, gid: str, hours: float, *, since: str | date | None = None,
             until: str | date | None = None, note: str | None = None) -> Goal:
    """Hours a week from `since` (today), until `until` (inclusive) or from then on. A window
    that ends returns the goal to the pace that held around it: "8 h until 10 Jan, then 5" is
    `set_pace(g, 5)` once and `set_pace(g, 8, until="2027-01-10")` for the ramp."""
    get_goal(conn, gid)
    if not 0 <= hours <= 60:
        raise LearningError("hours a week between 0 and 60")
    s, u = _day(since) or date.today().isoformat(), _day(until)
    if u and u < s:
        raise LearningError("`until` is before `since`")
    with conn:
        conn.execute("INSERT INTO pace(goal, hours, since, until, set_at, note) VALUES (?,?,?,?,?,?)",
                     (gid, float(hours), s, u, _now(), note))
    return get_goal(conn, gid)


def pace_history(conn: sqlite3.Connection, gid: str) -> list[tuple]:
    ensure(conn)
    return [tuple(r) for r in conn.execute(
        "SELECT hours, since, until, set_at, note FROM pace WHERE goal=? ORDER BY id", (gid,))]


def get_goal(conn: sqlite3.Connection, gid: str) -> Goal:
    ensure(conn)
    r = conn.execute("SELECT id, title, master_by, state, priority, thing, notes FROM goals WHERE id=?",
                     (gid,)).fetchone()
    if not r:
        known = [x[0] for x in conn.execute("SELECT id FROM goals ORDER BY priority, id")]
        raise LearningError(f"no goal {gid!r}" + (f"; goals: {', '.join(known)}" if known else ""))
    g = Goal(*r)
    p = current_pace(conn, gid)
    if p:
        g.pace, g.pace_until = p
    return g


def goals(conn: sqlite3.Connection, *, include_archived: bool = False) -> list[Goal]:
    ensure(conn)
    ids = [r[0] for r in conn.execute(
        "SELECT id FROM goals" + ("" if include_archived else " WHERE state != 'archived'") +
        " ORDER BY priority, id")]
    return [get_goal(conn, i) for i in ids]


def describe(g: Goal) -> str:
    pace = "no pace set" if g.pace is None else f"{g.pace:g} h a week" + (f" until {g.pace_until}" if g.pace_until else "")
    by = f"by {g.master_by}" if g.master_by else "no hard date"
    return f"{g.id}  [{g.state}, priority {g.priority}]  {g.title}  ({by}; {pace})"
