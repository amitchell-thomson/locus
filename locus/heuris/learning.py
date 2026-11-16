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
from datetime import date, datetime, timedelta, timezone

SCHEMA = """
CREATE TABLE IF NOT EXISTS goals (
    id         TEXT PRIMARY KEY,
    title      TEXT NOT NULL,
    master_by  TEXT,                       -- YYYY-MM-DD, or NULL for no hard date
    state      TEXT NOT NULL DEFAULT 'proposed',
    priority   INTEGER NOT NULL DEFAULT 2,
    thing      TEXT,                       -- a registry id it serves (oxdaq's oqts, quast), if any
    area       TEXT,                       -- Studies, Projects or Careers: its kairos tasks' key
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
    note     TEXT,
    season   TEXT                          -- term, vacation, or NULL: every day
);
CREATE INDEX IF NOT EXISTS topics_goal ON topics(goal);
CREATE INDEX IF NOT EXISTS pace_goal ON pace(goal, since);
"""

TABLES = ("goals", "topics", "topic_evidence", "pace")
STATES = ("proposed", "active", "paused", "done", "archived")
AREAS = ("Studies", "Projects", "Careers")
_ID = re.compile(r"^[a-z0-9]+(-[a-z0-9]+)*$")


class LearningError(ValueError):
    pass


def ensure(conn: sqlite3.Connection) -> None:
    conn.executescript(SCHEMA)
    # `area` came with L3 (16 Nov): a goal's tasks need a kairos key.
    if "area" not in {r[1] for r in conn.execute("PRAGMA table_info(goals)")}:
        conn.execute("ALTER TABLE goals ADD COLUMN area TEXT")
    # `season` likewise (16 Nov: "less time during term ... higher during holidays").
    if "season" not in {r[1] for r in conn.execute("PRAGMA table_info(pace)")}:
        conn.execute("ALTER TABLE pace ADD COLUMN season TEXT")


# --- term and vacation ---------------------------------------------------------------------------
# The term dates are kairos's (`[[profiles]] name = "term"` in ~/.config/kairos/constraints.toml,
# full term plus 0th week), so the scheduler and the pace agree on what term is. A day outside
# them is vacation. No file (the laptop, a test) means no terms: every day is vacation, which
# `describe` says rather than hides.
SEASONS = ("term", "vacation")
CONSTRAINTS = "~/.config/kairos/constraints.toml"
_terms_cache: list[tuple[date, date]] | None = None


def term_ranges() -> list[tuple[date, date]]:
    global _terms_cache
    if _terms_cache is None:
        import tomllib
        from pathlib import Path

        out: list[tuple[date, date]] = []
        try:
            cfg = tomllib.loads(Path(CONSTRAINTS).expanduser().read_text())
            for prof in cfg.get("profiles", []):
                if prof.get("name") == "term":
                    for r in prof.get("dates", []):
                        out.append((date.fromisoformat(str(r["from"])), date.fromisoformat(str(r["to"]))))
        except (OSError, ValueError, KeyError):
            out = []
        _terms_cache = out
    return _terms_cache


def season_of(d: date, terms: list[tuple[date, date]] | None = None) -> str:
    terms = term_ranges() if terms is None else terms
    return "term" if any(a <= d <= b for a, b in terms) else "vacation"


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
    area: str | None = None          # Studies, Projects or Careers (its kairos tasks' key)
    pace: float | None = None        # hours a week now
    pace_until: str | None = None    # when the current pace ends, if it does
    seasons: dict | None = None      # {"term": 3.0, "vacation": 7.0} from today on


def add_goal(conn: sqlite3.Connection, gid: str, title: str, *, master_by: str | date | None = None,
             priority: int = 2, thing: str | None = None, notes: str | None = None,
             state: str = "proposed", area: str | None = None) -> Goal:
    ensure(conn)
    if area is not None and area not in AREAS:
        raise LearningError(f"area is one of {', '.join(AREAS)}")
    if not _ID.match(gid):
        raise LearningError(f"goal id {gid!r}: lowercase words joined by hyphens (interview-prep)")
    if state not in STATES:
        raise LearningError(f"state is one of {', '.join(STATES)}")
    now = _now()
    with conn:
        if conn.execute("SELECT 1 FROM goals WHERE id=?", (gid,)).fetchone():
            raise LearningError(f"there is already a goal {gid!r}")
        conn.execute("INSERT INTO goals(id, title, master_by, state, priority, thing, notes, area, created, changed) "
                     "VALUES (?,?,?,?,?,?,?,?,?,?)",
                     (gid, title.strip(), _day(master_by), state, priority, thing, notes, area, now, now))
    return get_goal(conn, gid)


def set_notes(conn: sqlite3.Connection, gid: str, notes: str) -> Goal:
    """What steers the goal's curriculum, in his words (16 Nov: "get rid of weekly mocks"); the
    next proposal reads it."""
    get_goal(conn, gid)
    with conn:
        conn.execute("UPDATE goals SET notes=?, changed=? WHERE id=?", (notes.strip(), _now(), gid))
    return get_goal(conn, gid)


def set_area(conn: sqlite3.Connection, gid: str, area: str) -> Goal:
    if area not in AREAS:
        raise LearningError(f"area is one of {', '.join(AREAS)}")
    get_goal(conn, gid)
    with conn:
        conn.execute("UPDATE goals SET area=?, changed=? WHERE id=?", (area, _now(), gid))
    return get_goal(conn, gid)


def set_state(conn: sqlite3.Connection, gid: str, state: str) -> Goal:
    if state not in STATES:
        raise LearningError(f"state is one of {', '.join(STATES)}")
    get_goal(conn, gid)
    with conn:
        conn.execute("UPDATE goals SET state=?, changed=? WHERE id=?", (state, _now(), gid))
    return get_goal(conn, gid)


def current_pace(conn: sqlite3.Connection, gid: str, on: date | None = None,
                 terms: list[tuple[date, date]] | None = None) -> tuple[float, str | None] | None:
    """(hours a week, until) in force on `on` (today): the latest pace set whose window holds it
    and whose season, if it has one, is the season of that day."""
    ensure(conn)
    day = on or date.today()
    d = day.isoformat()
    r = conn.execute("SELECT hours, until FROM pace WHERE goal=? AND since<=? AND (until IS NULL OR until>=?) "
                     "AND (season IS NULL OR season=?) ORDER BY since DESC, id DESC LIMIT 1",
                     (gid, d, d, season_of(day, terms))).fetchone()
    return (float(r[0]), r[1]) if r else None


def pace_fn(conn: sqlite3.Connection, gid: str, terms: list[tuple[date, date]] | None = None):
    """The weekly pace on any day, cached: what the schedule and the finish date walk."""
    cache: dict[date, float] = {}

    def at(d: date) -> float:
        if d not in cache:
            p = current_pace(conn, gid, on=d, terms=terms)
            cache[d] = p[0] if p else 0.0
        return cache[d]
    return at


def finish(hours: float, pace_on, start: date, horizon_days: int = 3 * 365) -> date | None:
    """The day `hours` are done, a day giving a seventh of that day's weekly pace; None if never."""
    left, day = hours, start
    while left > 1e-9:
        left -= pace_on(day) / 7
        if left <= 1e-9:
            return day
        day += timedelta(days=1)
        if (day - start).days > horizon_days:
            return None
    return start


def season_paces(conn: sqlite3.Connection, gid: str, on: date | None = None) -> dict[str, float]:
    """The pace a term day and a vacation day get from `on` on, as now set (for the screen)."""
    d = (on or date.today()).isoformat()
    out = {}
    for season in SEASONS:
        r = conn.execute("SELECT hours FROM pace WHERE goal=? AND since<=? AND (until IS NULL OR until>=?) "
                         "AND (season IS NULL OR season=?) ORDER BY since DESC, id DESC LIMIT 1",
                         (gid, d, d, season)).fetchone()
        if r:
            out[season] = float(r[0])
    return out


def set_pace(conn: sqlite3.Connection, gid: str, hours: float, *, since: str | date | None = None,
             until: str | date | None = None, note: str | None = None, season: str | None = None) -> Goal:
    """Hours a week from `since` (today), until `until` (inclusive) or from then on. A window
    that ends returns the goal to the pace that held around it: "8 h until 10 Jan, then 5" is
    `set_pace(g, 5)` once and `set_pace(g, 8, until="2027-01-10")` for the ramp."""
    get_goal(conn, gid)
    if not 0 <= hours <= 60:
        raise LearningError("hours a week between 0 and 60")
    s, u = _day(since) or date.today().isoformat(), _day(until)
    if u and u < s:
        raise LearningError("`until` is before `since`")
    if season is not None and season not in SEASONS:
        raise LearningError("season is term or vacation")
    with conn:
        conn.execute("INSERT INTO pace(goal, hours, since, until, set_at, note, season) VALUES (?,?,?,?,?,?,?)",
                     (gid, float(hours), s, u, _now(), note, season))
    return get_goal(conn, gid)


def pace_history(conn: sqlite3.Connection, gid: str) -> list[tuple]:
    ensure(conn)
    return [tuple(r) for r in conn.execute(
        "SELECT hours, since, until, set_at, note, season FROM pace WHERE goal=? ORDER BY id", (gid,))]


def get_goal(conn: sqlite3.Connection, gid: str) -> Goal:
    ensure(conn)
    r = conn.execute("SELECT id, title, master_by, state, priority, thing, notes, area FROM goals WHERE id=?",
                     (gid,)).fetchone()
    if not r:
        known = [x[0] for x in conn.execute("SELECT id FROM goals ORDER BY priority, id")]
        raise LearningError(f"no goal {gid!r}" + (f"; goals: {', '.join(known)}" if known else ""))
    g = Goal(*r)
    p = current_pace(conn, gid)
    if p:
        g.pace, g.pace_until = p
    g.seasons = season_paces(conn, gid) or None
    return g


def goals(conn: sqlite3.Connection, *, include_archived: bool = False) -> list[Goal]:
    ensure(conn)
    ids = [r[0] for r in conn.execute(
        "SELECT id FROM goals" + ("" if include_archived else " WHERE state != 'archived'") +
        " ORDER BY priority, id")]
    return [get_goal(conn, i) for i in ids]


def pace_words(g: Goal) -> str:
    s = g.seasons or {}
    if s.get("term") is not None and s.get("vacation") is not None and s["term"] != s["vacation"]:
        words = f"{s['term']:g} h a week in term, {s['vacation']:g} in vacations"
        return words + ("" if term_ranges() else " (no term dates found: every day counts as vacation)")
    if g.pace is None:
        return "no pace set"
    return f"{g.pace:g} h a week" + (f" until {g.pace_until}" if g.pace_until else "")


def describe(g: Goal) -> str:
    pace = pace_words(g)
    by = f"by {g.master_by}" if g.master_by else "no hard date"
    return f"{g.id}  [{g.state}, priority {g.priority}]  {g.title}  ({by}; {pace})"
