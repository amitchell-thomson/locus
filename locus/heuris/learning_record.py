"""The learning record (spec L, L-D13, step L10): what he showed and asked, and the cards born
from it.

His call (22 Nov): "recording concepts is a little static and rigid ... there would end up being
hundreds and hundreds of concepts"; "my questions and confusions which I will say in chat are
super useful for gauging my understanding". So the record is evidence, not a catalogue:

- **observations**: one moment from a session (or, from L11, any chat in `Learning/` or
  `oxdaq-core`): a track, a kind (cold, hinted, wrong, explained-back, explained-to-him,
  question, confusion, misconception), what it showed in a line, and HIS words quoted.
- **cards**: one idea each, born only from a real moment (L-D13), with its source pointer so the
  nightly pass never cards the same moment twice. Scheduling fields are here from the start;
  L12 (Review) moves them.
- **card_reviews**: every grade, 1 wrong / 2 hard / 3 right.

NOT derived: these sit beside the facts ledger and `locus backup` copies them
(`backup.LEDGER_TABLES`). What is derived is the profile a brief shows: computed from the
observations when read (fading from their dates), so it never goes stale.

**Grounded or silent** (CLAUDE.md §2.1). A session writes its observations at the close, before
its transcript is indexed (the laptop pushes transcripts every few minutes, the index runs every
15), so the quote cannot be checked then. `verify` checks it afterwards against HIS turns of that
session in the index, and pins the observation to the exact turn; one still unfound after
`VERIFY_GRACE_H` is marked unfound, counted in `locus gates` (`learning.observation_quote`), and
left out of every brief. A model asked to quote will quote, including words he never said.
"""

from __future__ import annotations

import difflib
import re
import sqlite3
from dataclasses import dataclass
from datetime import date, datetime, timedelta, timezone

from pydantic import BaseModel, Field

from locus.heuris import learning as L

SCHEMA = """
CREATE TABLE IF NOT EXISTS observations (
    id        INTEGER PRIMARY KEY,
    goal      TEXT NOT NULL REFERENCES goals(id),
    track     TEXT NOT NULL,
    topic     INTEGER REFERENCES topics(id),
    kind      TEXT NOT NULL,              -- KINDS
    text      TEXT NOT NULL,              -- what it showed, in a line
    quote     TEXT,                       -- his words; NULL only for a cold/timed result with no words
    ref       TEXT,                       -- chat:<machine>/<session>[#<turn>]: `open` follows it
    session   TEXT,                       -- the Claude session id it came from
    origin    TEXT NOT NULL,              -- session, build, nightly
    at        TEXT NOT NULL,              -- when it happened (UTC ISO)
    checked   INTEGER,                    -- NULL not yet, 1 the quote is his, 0 not found: hidden
    created   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS observations_track ON observations(track, at);
CREATE INDEX IF NOT EXISTS observations_session ON observations(session);
CREATE TABLE IF NOT EXISTS cards (
    id        INTEGER PRIMARY KEY,
    goal      TEXT NOT NULL REFERENCES goals(id),
    track     TEXT NOT NULL,
    topic     INTEGER REFERENCES topics(id),
    prompt    TEXT NOT NULL,
    answer    TEXT NOT NULL,
    moment    TEXT NOT NULL,              -- the observation kind it was born from (or first-meeting)
    source    TEXT,                       -- the pointer of the moment: no second card for it
    session   TEXT,
    observation INTEGER REFERENCES observations(id),
    origin    TEXT NOT NULL,              -- session, build, nightly
    state     TEXT NOT NULL DEFAULT 'new',-- CARD_STATES
    introduced TEXT,                      -- YYYY-MM-DD it entered the queue (L12: ~20 new a day)
    due       TEXT,                       -- YYYY-MM-DD, NULL until introduced
    interval_days REAL NOT NULL DEFAULT 0,
    ease      REAL NOT NULL DEFAULT 2.5,
    reps      INTEGER NOT NULL DEFAULT 0,
    lapses    INTEGER NOT NULL DEFAULT 0,
    note      TEXT,                       -- why it was flagged or sent back ("this answer is wrong")
    created   TEXT NOT NULL,
    changed   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS cards_track ON cards(track, state);
CREATE INDEX IF NOT EXISTS cards_due ON cards(state, due);
CREATE TABLE IF NOT EXISTS card_reviews (
    id        INTEGER PRIMARY KEY,
    card      INTEGER NOT NULL REFERENCES cards(id),
    at        TEXT NOT NULL,
    grade     INTEGER NOT NULL,           -- 1 wrong, 2 hard, 3 right
    interval_before REAL,
    interval_after  REAL
);
CREATE INDEX IF NOT EXISTS card_reviews_card ON card_reviews(card, at);
"""

TABLES = ("observations", "cards", "card_reviews")

# L-D13's kinds, in the order the weighting ranks them (explained back > cold > review right >
# hinted > explained to him); question, confusion and misconception are what he asked or got wrong.
KINDS = ("explained-back", "cold", "hinted", "explained-to-him", "wrong", "question", "confusion",
         "misconception")
SOLID = ("explained-back", "cold")
SHAKY = ("hinted", "explained-to-him", "wrong", "confusion")
CARD_MOMENTS = ("wrong", "hinted", "question", "confusion", "misconception", "first-meeting")
CARD_STATES = ("new", "active", "suspended", "deleted", "wrong", "flagged")
ORIGINS = ("session", "build", "nightly")
# Coding practice has a profile per language (L-D13), beside the curricula's own tracks.
PRACTICE_TRACKS = ("coding-python", "coding-cpp")
FADING_DAYS = 21          # solid evidence older than this, with no review since, is fading
VERIFY_GRACE_H = 48       # a quote not in the index after this is unfound (the push can lag a day)
FLAG_LAPSES = 3           # a card failed this often goes back to a session (L-D13)


class RecordError(L.LearningError):
    pass


def ensure(conn: sqlite3.Connection) -> None:
    L.ensure(conn)
    conn.executescript(SCHEMA)


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def _norm(s: str) -> str:
    s = re.sub(r"\\([|*_`#\[\]()<>])", r"\1", s or "")
    s = s.lower().replace("’", "'").replace("‘", "'").replace("“", '"').replace("”", '"')
    s = re.sub(r"[‐-―\-]+", "-", s)
    return re.sub(r"\s+", " ", s).strip()


def tracks(conn: sqlite3.Connection) -> list[str]:
    """Every track a curriculum names (current or proposed, any goal), and the practice tracks."""
    have = {r[0] for r in conn.execute("SELECT DISTINCT track FROM topics WHERE state IN ('current', 'proposed')")}
    return sorted(have | set(PRACTICE_TRACKS))


def _track(conn: sqlite3.Connection, track: str) -> str:
    t = (track or "").strip().lower().replace(" ", "-").replace("_", "-")
    known = tracks(conn)
    if t in known:
        return t
    near = difflib.get_close_matches(t, known, n=3, cutoff=0.5)
    raise RecordError(f"unknown track {track!r}" + (f"; nearest: {', '.join(near)}" if near else "")
                      + f". Tracks: {', '.join(known)}.")


def _kind(kind: str, allowed: tuple[str, ...]) -> str:
    k = (kind or "").strip().lower().replace(" ", "-").replace("_", "-")
    if k not in allowed:
        raise RecordError(f"unknown kind {kind!r}; one of {', '.join(allowed)}")
    return k


def _topic(conn: sqlite3.Connection, goal: str, topic: int | str | None) -> int | None:
    if topic in (None, ""):
        return None
    try:
        tid = int(str(topic).removeprefix("topic:").strip())
    except ValueError:
        raise RecordError(f"topic {topic!r} is not a topic id (the number in the task's notes)") from None
    row = conn.execute("SELECT goal FROM topics WHERE id=?", (tid,)).fetchone()
    if row is None:
        raise RecordError(f"no topic {tid}")
    if row[0] != goal:
        raise RecordError(f"topic {tid} belongs to {row[0]}, not {goal}")
    return tid


def _session_ref(session: str | None, ref: str | None, machine: str) -> str | None:
    if ref:
        return ref
    return f"chat:{machine}/{session}" if session else None


# --- writing ---------------------------------------------------------------------------------------


class ObservationIn(BaseModel):
    """One moment of a session, written at its close."""
    track: str = Field(description="the curriculum track it is evidence for (python-data-model, probability, "
                                   "coding-python, ...); an unknown one is refused with the nearest")
    kind: str = Field(description="explained-back, cold, hinted, explained-to-him, wrong, question, confusion, "
                                  "misconception")
    text: str = Field(description="what it showed, one line, plain: 'set up the reflection but did not finish'")
    quote: str | None = Field(default=None, description="HIS words from this chat, verbatim (checked later "
                                                        "against the transcript; one not found is hidden)")
    topic: int | None = Field(default=None, description="the topic id from the kairos task's notes, if any")


class CardIn(BaseModel):
    """One card, from one real moment."""
    track: str
    prompt: str = Field(description="one idea; recall, not recognition; why/when over what; his own confusion "
                                    "as the prompt where there was one; code: predict the output or what is "
                                    "wrong; maths: the set-up rather than the number")
    answer: str = Field(description="short and right: a wrong answer drilled is worse than none")
    moment: str = Field(description="what it came from: wrong, hinted, question, confusion, misconception, "
                                    "first-meeting")
    quote: str | None = Field(default=None, description="his words at that moment, if any: ties the card to it")
    topic: int | None = None


@dataclass
class Written:
    ids: list[int]
    refused: list[str]

    def line(self, noun: str) -> str:
        s = f"{len(self.ids)} {noun}(s) recorded" + (f" (ids {', '.join(map(str, self.ids))})" if self.ids else "")
        if self.refused:
            s += f"; {len(self.refused)} refused: " + "; ".join(self.refused)
        return s + "."


def observe(conn: sqlite3.Connection, goal: str, items: list[ObservationIn | dict], *,
            session: str | None = None, ref: str | None = None, origin: str = "session",
            machine: str = "laptop", at: str | None = None) -> Written:
    """Record a session's observations. Each is refused alone (an unknown track, a bad kind), so
    one mistake never loses the rest."""
    ensure(conn)
    L.get_goal(conn, goal)
    origin = _kind(origin, ORIGINS)
    when = at or _now()
    out = Written([], [])
    for raw in items:
        o = raw if isinstance(raw, ObservationIn) else ObservationIn.model_validate(raw)
        try:
            tr, k, tid = _track(conn, o.track), _kind(o.kind, KINDS), _topic(conn, goal, o.topic)
            if not o.text.strip():
                raise RecordError("no text")
        except RecordError as e:
            out.refused.append(f"{o.text[:40]!r}: {e}")
            continue
        with conn:
            cur = conn.execute(
                "INSERT INTO observations(goal, track, topic, kind, text, quote, ref, session, origin, at, checked, created) "
                "VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
                (goal, tr, tid, k, o.text.strip(), (o.quote or "").strip() or None,
                 _session_ref(session, ref, machine), session, origin, when,
                 None if (o.quote or "").strip() else 1, _now()))
        out.ids.append(cur.lastrowid)
    return out


def add_cards(conn: sqlite3.Connection, goal: str, items: list[CardIn | dict], *,
              session: str | None = None, ref: str | None = None, origin: str = "session",
              machine: str = "laptop") -> Written:
    """Write the cards he kept at the close (he strikes any first, L-D13). A card whose prompt is
    already on its track, or whose moment (his quote, in that session) already has a card, is
    refused: the session and the nightly pass see the same moments."""
    ensure(conn)
    L.get_goal(conn, goal)
    origin = _kind(origin, ORIGINS)
    out = Written([], [])
    for raw in items:
        c = raw if isinstance(raw, CardIn) else CardIn.model_validate(raw)
        try:
            tr, m, tid = _track(conn, c.track), _kind(c.moment, CARD_MOMENTS), _topic(conn, goal, c.topic)
            if not c.prompt.strip() or not c.answer.strip():
                raise RecordError("a card needs a prompt and an answer")
            p = _norm(c.prompt)
            for (other,) in conn.execute("SELECT prompt FROM cards WHERE track=? AND state != 'deleted'", (tr,)):
                if _norm(other) == p:
                    raise RecordError("the same card is already on this track")
            obs = None
            if c.quote and session:
                q = _norm(c.quote)
                obs = next((oid for oid, oq in conn.execute(
                    "SELECT id, quote FROM observations WHERE session=? AND quote IS NOT NULL", (session,))
                    if _norm(oq) == q), None)
                if obs and conn.execute("SELECT 1 FROM cards WHERE observation=? AND state != 'deleted'", (obs,)).fetchone():
                    raise RecordError("that moment already has a card")
        except RecordError as e:
            out.refused.append(f"{c.prompt[:40]!r}: {e}")
            continue
        now = _now()
        with conn:
            cur = conn.execute(
                "INSERT INTO cards(goal, track, topic, prompt, answer, moment, source, session, observation, origin, "
                "state, created, changed) VALUES (?,?,?,?,?,?,?,?,?,?, 'new', ?, ?)",
                (goal, tr, tid, c.prompt.strip(), c.answer.strip(), m, _session_ref(session, ref, machine),
                 session, obs, origin, now, now))
        out.ids.append(cur.lastrowid)
    return out


# --- grounding -------------------------------------------------------------------------------------


def verify(conn: sqlite3.Connection, *, gate_conn=None, now: datetime | None = None) -> dict[str, int]:
    """Check every unchecked quote against HIS turns of its session in the index, and pin the
    observation (and any card from it) to the exact turn. Deterministic and cheap: run after each
    index tick. Returns counts: found, waiting, unfound."""
    ensure(conn)
    now = now or datetime.now(timezone.utc)
    out = {"found": 0, "waiting": 0, "unfound": 0}
    rows = conn.execute("SELECT id, quote, session, created FROM observations WHERE checked IS NULL").fetchall()
    for oid, quote, session, created in rows:
        turns = conn.execute(
            "SELECT ref, text FROM items WHERE source='chats' AND parent LIKE ? AND json_extract(meta, '$.role')='you'",
            (f"chat:%/{session}",)).fetchall() if session else []
        q = _norm(quote)
        hit = next((r for r, t in turns if q and q in _norm(t)), None)
        old = now - datetime.fromisoformat(created) > timedelta(hours=VERIFY_GRACE_H)
        if hit is None and not old:
            out["waiting"] += 1
            continue
        with conn:
            if hit:
                conn.execute("UPDATE observations SET checked=1, ref=? WHERE id=?", (hit, oid))
                conn.execute("UPDATE cards SET source=? WHERE observation=?", (hit, oid))
            else:
                conn.execute("UPDATE observations SET checked=0 WHERE id=?", (oid,))
        out["found" if hit else "unfound"] += 1
        if gate_conn is not None:
            from locus.observe import gates
            gates.record(gate_conn, "learning.observation_quote", rejected=hit is None,
                         value=f"obs {oid} | {session} | {quote[:80]}")
    return out


# --- reading: the brief ----------------------------------------------------------------------------


@dataclass
class Obs:
    id: int
    goal: str
    track: str
    topic: int | None
    kind: str
    text: str
    quote: str | None
    ref: str | None
    at: str

    def line(self, today: date) -> str:
        d = date.fromisoformat(self.at[:10])
        ago = (today - d).days
        when = "today" if ago == 0 else f"{ago} d ago"
        s = f"  {self.at[:10]} ({when}) [{self.kind}] {self.text}"
        if self.quote:
            s += f'  he said: "{self.quote}"'
        return s + (f"  [{self.ref}]" if self.ref else "")


def _observations(conn: sqlite3.Connection, track: str) -> list[Obs]:
    return [Obs(*r) for r in conn.execute(
        "SELECT id, goal, track, topic, kind, text, quote, ref, at FROM observations "
        "WHERE track=? AND (checked IS NULL OR checked=1) ORDER BY at DESC", (track,))]


def profile(conn: sqlite3.Connection, track: str, *, today: date | None = None, limit: int = 6) -> list[str]:
    """The track as the evidence stands, computed when read: solid, shaky, open questions,
    misconceptions, and what is fading (solid evidence older than FADING_DAYS with no review of
    a card on the track since). No evidence says so: unknown, never assumed."""
    today = today or date.today()
    obs = _observations(conn, track)
    if not obs:
        return [f"No evidence on {track} yet: unknown. Probe it in the starters; assume nothing."]
    last_review = conn.execute(
        "SELECT MAX(r.at) FROM card_reviews r JOIN cards c ON c.id=r.card WHERE c.track=? AND r.grade>=2",
        (track,)).fetchone()[0]
    out: list[str] = []
    groups = (("Solid (explained back, or done cold)", SOLID), ("Shaky (hinted, wrong, told, confused)", SHAKY),
              ("Open questions he asked", ("question",)), ("Misconceptions", ("misconception",)))
    for head, kinds in groups:
        got = [o for o in obs if o.kind in kinds][:limit]
        if got:
            out.append(f"{head}:")
            out.extend(o.line(today) for o in got)
    solid = [o for o in obs if o.kind in SOLID]
    if solid:
        newest = max(date.fromisoformat(o.at[:10]) for o in solid)
        reviewed = date.fromisoformat(last_review[:10]) if last_review else None
        age = (today - max(newest, reviewed) if reviewed else today - newest).days
        if age > FADING_DAYS:
            out.append(f"Fading: the last solid evidence is {age} days old, with no review since. Recall it first.")
    return out


def _cards_line(conn: sqlite3.Connection, track: str, today: date) -> list[str]:
    t = today.isoformat()
    due = conn.execute("SELECT COUNT(*) FROM cards WHERE track=? AND state='active' AND due<=?", (track, t)).fetchone()[0]
    new = conn.execute("SELECT COUNT(*) FROM cards WHERE track=? AND state='new'", (track,)).fetchone()[0]
    back = conn.execute(
        "SELECT id, prompt, answer, state, lapses, note FROM cards WHERE track=? AND "
        "(state IN ('flagged','wrong') OR (state='active' AND lapses>=?)) ORDER BY changed", (track, FLAG_LAPSES)).fetchall()
    out = [f"Cards on {track}: {due} due, {new} new waiting."]
    for cid, prompt, answer, state, lapses, note in back:
        why = ("its answer was marked wrong in Review: fix the card with him" if state == "wrong"
               else f"failed {lapses} times in Review: re-teach it this session")
        out.append(f"  card:{cid} ({why}) Q: {prompt}  A: {answer}" + (f"  note: {note}" if note else ""))
    return out


def _topic_row(conn: sqlite3.Connection, tid: int):
    return conn.execute("SELECT id, goal, track, title, hours, parent, ord FROM topics WHERE id=?", (tid,)).fetchone()


def brief(conn: sqlite3.Connection, goal: str, topic: int | str | None = None, *, today: date | None = None,
          history=None) -> str:
    """What a session reads before teaching (L-D13: "no evidence means unknown"): the topic, the
    profile of its track (and the coding track for a coding goal), what on the track has no
    evidence at all, the cards to re-teach, and his related history. `history(query)` returns
    lines from the index (the MCP tool passes heuris search; tests pass a fake)."""
    ensure(conn)
    today = today or date.today()
    g = L.get_goal(conn, goal)
    lines: list[str] = []
    tid = _topic(conn, goal, topic)
    if tid is None:
        trs = [r[0] for r in conn.execute(
            "SELECT track FROM topics WHERE goal=? AND state='current' GROUP BY track ORDER BY MIN(id)", (goal,))]
        lines.append(f"# {g.title} ({goal}): every track, no topic given")
        for tr in trs:
            n = conn.execute("SELECT COUNT(*) FROM observations WHERE track=? AND (checked IS NULL OR checked=1)",
                             (tr,)).fetchone()[0]
            lines.append(f"  {tr}: {n} observation(s)" if n else f"  {tr}: no evidence yet")
        lines.append("Give a topic id (from the kairos task's notes) for its full brief.")
        return "\n".join(lines)

    _, _, track, title, hours, parent, _ = _topic_row(conn, tid)
    lines.append(f"# {title}  (topic {tid}, track {track}, {hours:g} h planned, goal {goal})")
    sib = conn.execute(
        "SELECT id, title FROM topics WHERE goal=? AND state='current' AND parent IS ? ORDER BY ord, id",
        (goal, parent)).fetchall()
    ids = [s[0] for s in sib]
    if tid in ids:
        i = ids.index(tid)
        if i > 0:
            lines.append(f"Before it: {sib[i - 1][1]} (topic {sib[i - 1][0]})")
        if i + 1 < len(sib):
            lines.append(f"After it: {sib[i + 1][1]} (topic {sib[i + 1][0]})")

    lines.append(f"\n## Track {track}: what the evidence shows")
    lines.extend(profile(conn, track, today=today))
    unseen = [r for r in conn.execute(
        "SELECT t.id, t.title, t.goal FROM topics t WHERE t.track=? AND t.state='current' AND t.parent IS NOT NULL "
        "AND NOT EXISTS (SELECT 1 FROM observations o WHERE o.topic=t.id AND (o.checked IS NULL OR o.checked=1)) "
        "ORDER BY t.goal, t.ord, t.id", (track,))]
    if unseen:
        lines.append("No evidence yet for these topics on the track (probe, do not assume):")
        lines.extend(f"  topic {i} ({gl}): {t}" for i, t, gl in unseen[:10])
        if len(unseen) > 10:
            lines.append(f"  ... and {len(unseen) - 10} more")

    coding = [t for t in PRACTICE_TRACKS if t.split("-")[1] in track] or (
        list(PRACTICE_TRACKS) if goal == "cpp-oxdaq" else [])
    for ct in coding:
        lines.append(f"\n## Coding practice: {ct} (a review expects only what is seen here; introduce the rest)")
        lines.extend(profile(conn, ct, today=today, limit=8))

    lines.append("\n## Cards")
    for tr in [track, *coding]:
        lines.extend(_cards_line(conn, tr, today))
    total = conn.execute("SELECT COUNT(*) FROM cards WHERE state='active' AND due<=?", (today.isoformat(),)).fetchone()[0]
    lines.append(f"All goals: {total} card(s) due.")

    if history is not None:
        try:
            h = history(title)
        except Exception as e:  # noqa: BLE001  (the brief stands without it)
            h = f"(history search failed: {type(e).__name__}: {e})"
        if h:
            lines.append(f"\n## His history on it (search: {title!r})")
            lines.append(h)
    return "\n".join(lines)


def counts(conn: sqlite3.Connection, today: date | None = None) -> dict[str, int]:
    """For Home and the Learning screen: cards due, new waiting, flagged."""
    ensure(conn)
    t = (today or date.today()).isoformat()
    q = lambda sql, *a: conn.execute(sql, a).fetchone()[0]  # noqa: E731
    return {"due": q("SELECT COUNT(*) FROM cards WHERE state='active' AND due<=?", t),
            "new": q("SELECT COUNT(*) FROM cards WHERE state='new'"),
            "flagged": q("SELECT COUNT(*) FROM cards WHERE state IN ('flagged','wrong') OR (state='active' AND lapses>=?)",
                         FLAG_LAPSES),
            "observations": q("SELECT COUNT(*) FROM observations")}


def recent(conn: sqlite3.Connection, goal: str | None = None, limit: int = 30) -> list[str]:
    """The newest observations and cards, for `locus learn record`."""
    ensure(conn)
    today = date.today()
    w, a = ("WHERE goal=?", (goal,)) if goal else ("", ())
    out = ["Observations:"]
    for r in conn.execute(f"SELECT id, goal, track, topic, kind, text, quote, ref, at, checked FROM observations {w} "
                          f"ORDER BY at DESC LIMIT ?", (*a, limit)):
        o = Obs(*r[:9])
        mark = {None: "unchecked", 1: "", 0: "QUOTE NOT FOUND, hidden"}[r[9]]
        out.append(f"obs:{o.id} {o.goal}/{o.track}" + (f" {mark}" if mark else "") + "\n" + o.line(today))
    out.append("Cards:")
    for cid, gl, tr, st, p, ans in conn.execute(
            f"SELECT id, goal, track, state, prompt, answer FROM cards {w} ORDER BY id DESC LIMIT ?", (*a, limit)):
        out.append(f"card:{cid} {gl}/{tr} [{st}] Q: {p}\n    A: {ans}")
    return out

