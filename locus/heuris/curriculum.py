"""A goal becomes a proposed curriculum (spec L, step L2; L-D2, L-D3).

1. **Evidence** (L-D3): what the goal points at, gathered from heuris: his notes and career files
   found by meaning (feedback, prep plans, the goal's own words), every pointer he names
   (`--evidence src:623`), the repo's docs for a project goal, and the facts about him (his
   education says Years 1 and 2 are known well; his goals say what the prep is for). Each piece
   keeps its pointer and full text, so a quote can be checked against it.
2. **One Opus call** turns the evidence into a tree of topics: a track (shared by name across
   goals), hours, a weight (weaker first) and, for each topic, why it is there with the exact
   line it rests on.
3. **Grounded or silent** (CLAUDE.md §2.1): a topic's evidence counts only if its pointer is one
   it was given and its quote is in that text (whitespace, case and dash differences forgiven).
   A topic left with no evidence is dropped, and every decision goes to `locus gates`
   (`learning.topic_quote`), so a gate that drops everything shows.
4. **A proposal, never a curriculum** (§2.2): topics are stored `proposed`; approving them is
   step L3. A new proposal replaces the old proposed topics, never approved ones.
"""

from __future__ import annotations

import json
import re
import sqlite3
from dataclasses import dataclass, field
from datetime import date, datetime, timedelta, timezone

from pydantic import BaseModel, Field

from locus.heuris import learning as L

MODEL = "opus"
MAX_TOKENS = 32_000
MAX_EVIDENCE_CHARS = 90_000
PER_ITEM_CHARS = 4_000
# A pointer he NAMES is read whole up to this (19 Nov): at 4,000 characters cpp-oxdaq's ROUNDS.md
# was cut off at its second round, and the spec documents its rounds point to are ~20,000.
NAMED_ITEM_CHARS = 24_000

CURRICULA_SCHEMA = """
CREATE TABLE IF NOT EXISTS curricula (
    id        INTEGER PRIMARY KEY,
    goal      TEXT NOT NULL REFERENCES goals(id),
    at        TEXT NOT NULL,
    summary   TEXT NOT NULL,
    evidence  TEXT NOT NULL,           -- JSON: the pointers it was given
    dropped   TEXT NOT NULL,           -- JSON: topics and quotes the check refused
    model     TEXT NOT NULL
);
"""

SYSTEM = """You design a curriculum for one learner, Alec Mitchell-Thomson: a third-year Oxford engineering student heading for quantitative trading, who learns best from practice testing and spaced practice, writes notes by hand, and knows the first two years of the Engineering Science degree very well.

You are given a goal (what he wants to master, by when, at how many hours a week) and EVIDENCE: numbered pieces of his own material, each with a pointer (feedback notes from interviews, his prep plans, facts about him, project documents, course syllabi, library sources). Build the curriculum from the evidence.

Rules:
- Return a tree of topics: 4 to 8 top-level topics, each with 1 to 6 subtopics. A subtopic names its parent by key.
- Every topic carries at least one piece of evidence: the pointer exactly as given and a quote of at most 25 words copied VERBATIM from that piece. A topic you cannot ground in the evidence is left out. Never invent a pointer or paraphrase a quote.
- "why" says in one sentence what in the evidence put the topic there (a weakness named in feedback, a format he will be tested in, a part of the project he must explain).
- "weight" is 0.5 to 3: higher for what the evidence says he is weaker at or tested on most; lower for what he is already strong at (strong points still get maintenance, at low weight).
- "hours" is honest study and practice time for that topic at his level, not reading time. Top-level hours are the sum of their subtopics.
- "track" is a short reusable name in lowercase words joined by hyphens (probability, linear-algebra, data-structures, cpp-memory-model): the same subject in another goal would use the same track.
- Order topics as he should meet them: prerequisites first, weakest early.
- "summary" says in two or three sentences what the curriculum emphasises and why, from the evidence.
- Plain British English, no em dashes."""


ROUNDS_RULES = """

This goal is built in ROUNDS. The evidence includes his rounds plan: a table of rounds in build order, each with the concepts it teaches, the part it builds, its hours and when it is done. That plan is the backbone and overrides the 4 to 8 rule above:
- One top-level topic per round that has hours, in the plan's order, titled "<id>: <the part it builds>" (for example "R5: price levels and the book side"). Leave out rounds with no hours (a later phase). Its hours equal the round's hours in the plan.
- Under each round, 2 to 6 subtopics in the order he meets them. First the concepts the round names, grouped into topics of 1 to 4 hours that one or a few sessions can teach and practise; each title says exactly what is learnt, and "why" names the part of the build that needs it. Then the build itself, in steps of 2 to 5 hours that follow the spec documents (what to write, in what order, ending at the round's "done when"). A round with no concepts in one language has no concept topics for it.
- Spread a round's hours honestly between concepts and build for what that round asks: a round heavy in new C++ ideas spends more on concepts, a round that applies known ideas spends more on the build.
- Ground concept topics in the plan's row (quote the concepts cell) and build steps in the spec documents where they describe that part, or the plan's row.
- Tracks: cpp-<subject> for C++ concepts (cpp-compilation-model, cpp-memory-ownership, cpp-containers, cpp-templates, cpp-performance), python-<subject> for Python (python-data-model, python-asyncio, python-testing, python-numpy, python-concurrency), oxdaq-build for build steps."""


class _Evidence(BaseModel):
    ref: str
    quote: str


class _Topic(BaseModel):
    key: str
    parent: str | None = None
    track: str
    title: str
    hours: float = Field(ge=0.25, le=200)
    weight: float = Field(ge=0.5, le=3)
    why: str
    evidence: list[_Evidence]


class _Curriculum(BaseModel):
    summary: str
    topics: list[_Topic]


@dataclass
class Piece:
    ref: str
    title: str
    text: str


@dataclass
class Proposal:
    goal: str
    summary: str
    kept: int = 0
    dropped: list[dict] = field(default_factory=list)
    total_hours: float = 0.0
    evidence: list[str] = field(default_factory=list)


def ensure(conn: sqlite3.Connection) -> None:
    L.ensure(conn)
    conn.executescript(CURRICULA_SCHEMA)


# --- 1. evidence --------------------------------------------------------------------------------

def _item(conn: sqlite3.Connection, ref: str, cap: int = PER_ITEM_CHARS) -> list[Piece]:
    """A pointer's text from the index: one item, or every section of a library document."""
    rows = conn.execute("SELECT ref, title, text FROM items WHERE ref=?", (ref,)).fetchall()
    if not rows and ref.startswith("src:") and "#" not in ref:
        rows = conn.execute("SELECT ref, title, text FROM items WHERE parent=? ORDER BY ref", (ref,)).fetchall()
    return [Piece(r[0], r[1] or r[0], (r[2] or "")[:cap]) for r in rows]


def _search(conn: sqlite3.Connection, query: str, *, kinds=("note", "file", "doc", "source"), k: int = 6,
            embed=None, rerank=None) -> list[str]:
    from locus.heuris import tools as ht
    from locus.heuris.index import QUERY_PREFIX

    if embed is None:
        from locus.ingest.embed import embed_texts as embed
    qvec = embed([QUERY_PREFIX + query])[0]
    hits = [h for kind in kinds for h in ht.heuris_candidates(conn, query, qvec, kind=kind)]
    hits = (rerank or ht._rerank)(query, hits)
    seen: list[str] = []
    for h in hits:
        if h.ref not in seen:
            seen.append(h.ref)
        if len(seen) >= k:
            break
    return seen


def gather(conn: sqlite3.Connection, goal: L.Goal, *, refs: list[str] = (), queries: list[str] = (),
           embed=None, rerank=None) -> list[Piece]:
    """The evidence for a goal, in order of weight: what he named, then what search finds, then
    the facts about him. Capped so the prompt stays a prompt."""
    from locus.heuris import facts as F

    out: list[Piece] = []
    have: set[str] = set()

    def add(pieces: list[Piece]) -> None:
        for p in pieces:
            # locus's own design documents describe his plans in Claude's words: a curriculum
            # citing its own spec is circular (the first real run did, 16 Nov), so they are not
            # evidence about him.
            if p.ref.startswith("doc:locus/"):
                continue
            if p.ref not in have and p.text.strip():
                have.add(p.ref)
                out.append(p)

    for r in refs:
        add(_item(conn, r, NAMED_ITEM_CHARS))
    qs = list(queries) or [goal.title, f"feedback and weaknesses for {goal.title}", f"prep plan for {goal.title}"]
    for q in qs:
        for r in _search(conn, q, embed=embed, rerank=rerank):
            add(_item(conn, r))
    if goal.thing:
        for r in conn.execute("SELECT ref FROM items WHERE thing=? AND kind='doc' AND ref LIKE ? LIMIT 3",
                              (goal.thing, f"doc:{goal.thing}/state.md%")):
            add(_item(conn, r[0]))
    for f in F.current(conn, "me"):
        if f.slot.split(":")[0] in ("goal", "education", "decision", "plan", "preference", "constraint", "experience"):
            add([Piece(f"fact:{f.id}", f"me · {f.slot}", f.value)])
    # Over the cap a piece is skipped, not the rest: the facts about him come last and are short,
    # and a long search hit must not push them out.
    total, kept = 0, []
    for p in out:
        if total + len(p.text) > MAX_EVIDENCE_CHARS:
            continue
        kept.append(p)
        total += len(p.text)
    return kept


# --- 2. the call ---------------------------------------------------------------------------------

def user_prompt(goal: L.Goal, pieces: list[Piece], today: date) -> str:
    by = goal.master_by or "no hard date"
    pace = f"{goal.pace:g} hours a week" if goal.pace else "no pace set yet"
    ev = "\n\n".join(f"[{i}] pointer: {p.ref}\n    title: {p.title}\n{p.text}" for i, p in enumerate(pieces, 1))
    return (f"Today is {today.isoformat()}.\n\nGOAL: {goal.title} (id {goal.id})\nMaster by: {by}\nPace: {pace}\n"
            f"Notes: {goal.notes or '-'}\n\nEVIDENCE ({len(pieces)} pieces):\n\n{ev}\n\n"
            "Return ONLY a JSON object matching this JSON Schema (no prose, no code fences):\n"
            f"{json.dumps(_Curriculum.model_json_schema())}")


def _norm(s: str) -> str:
    # A markdown escape is not text (19 Nov: ROUNDS.md's table has `py \| cpp`, the quote said
    # `py | cpp`, and R8 was dropped as unquoted).
    s = re.sub(r"\\([|*_`#\[\]()<>])", r"\1", s)
    s = s.lower().replace("’", "'").replace("‘", "'")
    s = re.sub(r"[‐-―\-]+", "-", s)
    return re.sub(r"\s+", " ", s).strip()


def _extract_json(text: str) -> str:
    start, end = text.find("{"), text.rfind("}")
    if start < 0 or end <= start:
        raise ValueError("no JSON object in the reply")
    return text[start:end + 1]


def propose(conn: sqlite3.Connection, gid: str, *, refs: list[str] = (), queries: list[str] = (),
            ask=None, embed=None, rerank=None, gate_conn=None, today: date | None = None) -> Proposal:
    """Build, check and store a proposed curriculum for goal `gid`."""
    from locus.agent import claude

    ensure(conn)
    goal = L.get_goal(conn, gid)
    today = today or date.today()
    pieces = gather(conn, goal, refs=refs, queries=queries, embed=embed, rerank=rerank)
    if not pieces:
        raise L.LearningError(f"no evidence found for {gid!r}: name some with --evidence")
    if ask is None:
        def ask(system: str, user: str) -> str:
            # A goal in rounds is ~13 sections of 2 to 6 topics, each with a quote: at the default
            # 16,000 (thinking included) the first cpp-oxdaq reply was cut mid-list (19 Nov).
            return claude.ask(user, system=system, model=MODEL, thinking=True, max_tokens=MAX_TOKENS).text
    raw = ask(SYSTEM + (ROUNDS_RULES if goal.shape == "rounds" else ""), user_prompt(goal, pieces, today))
    try:
        cur = _Curriculum.model_validate_json(_extract_json(raw))
    except Exception as e:  # noqa: BLE001
        raise L.LearningError(f"the curriculum came back unreadable: {e}") from e

    # 3. grounded or silent
    by_ref = {p.ref: _norm(p.text) for p in pieces}
    good: dict[str, list[_Evidence]] = {}
    prop = Proposal(goal=gid, summary=cur.summary, evidence=[p.ref for p in pieces])
    for t in cur.topics:
        ok = []
        for ev in t.evidence:
            q = _norm(ev.quote)
            passed = bool(q) and ev.ref in by_ref and q in by_ref[ev.ref]
            if gate_conn is not None:
                from locus.observe import gates
                gates.record(gate_conn, "learning.topic_quote", rejected=not passed,
                             value=f"{t.title} | {ev.ref} | {ev.quote[:80]}")
            if passed:
                ok.append(ev)
            else:
                prop.dropped.append({"topic": t.title, "ref": ev.ref, "quote": ev.quote})
        if t.parent and t.parent not in good:
            # Its section was dropped: kept, it would surface as a section of its own, out of
            # place (19 Nov, R8's four topics). Topics come parents first, so `good` knows.
            prop.dropped.append({"topic": t.title, "ref": None, "quote": "dropped with its section"})
        elif ok:
            good[t.key] = ok
        else:
            prop.dropped.append({"topic": t.title, "ref": None, "quote": "topic dropped: no evidence held"})

    # 4. stored as a proposal
    now = datetime.now(timezone.utc).isoformat(timespec="seconds")
    ids: dict[str, int] = {}
    with conn:
        old = [r[0] for r in conn.execute("SELECT id FROM topics WHERE goal=? AND state='proposed'", (gid,))]
        for tid in old:
            conn.execute("DELETE FROM topic_evidence WHERE topic=?", (tid,))
        conn.execute("DELETE FROM topics WHERE goal=? AND state='proposed'", (gid,))
        for ordn, t in enumerate(cur.topics):
            if t.key not in good:
                continue
            parent = ids.get(t.parent) if t.parent else None
            tid = conn.execute(
                "INSERT INTO topics(goal, parent, ord, track, title, hours, weight, state, created) "
                "VALUES (?,?,?,?,?,?,?,'proposed',?)",
                (gid, parent, ordn, t.track, t.title, t.hours, t.weight, now)).lastrowid
            ids[t.key] = tid
            for ev in good[t.key]:
                conn.execute("INSERT INTO topic_evidence(topic, ref, quote, why) VALUES (?,?,?,?)",
                             (tid, ev.ref, ev.quote, t.why))
            prop.kept += 1
        conn.execute("INSERT INTO curricula(goal, at, summary, evidence, dropped, model) VALUES (?,?,?,?,?,?)",
                     (gid, now, cur.summary, json.dumps(prop.evidence), json.dumps(prop.dropped), MODEL))
    prop.total_hours = leaf_hours(conn, gid, state="proposed")
    return prop


# --- reading it ----------------------------------------------------------------------------------

def leaf_hours(conn: sqlite3.Connection, gid: str, *, state: str = "proposed") -> float:
    """Hours of the topics that have no children (a parent's hours are its children's sum)."""
    r = conn.execute(
        "SELECT COALESCE(SUM(hours), 0) FROM topics t WHERE goal=? AND state=? AND NOT EXISTS "
        "(SELECT 1 FROM topics c WHERE c.parent=t.id)", (gid, state)).fetchone()
    return float(r[0])


def pace_line(goal: L.Goal, hours: float, today: date | None = None, *, conn: sqlite3.Connection | None = None) -> str:
    """Whether the curriculum fits the date at the pace (L-D7). With `conn`, the finish walks the
    real pace day by day (term and vacation, ramps); without, it assumes today's pace throughout."""
    today = today or date.today()
    if hours <= 0:
        return "no curriculum yet"
    if conn is not None:
        done = L.finish(hours, L.pace_fn(conn, goal.id), today)
        if done is None:
            return f"{hours:g} h in all; no pace set"
        line = f"{hours:g} h in all; at {L.pace_words(goal)}: done around {done.isoformat()}"
    else:
        if not goal.pace:
            return f"{hours:g} h in all; no pace set"
        weeks = hours / goal.pace
        done = today + timedelta(weeks=weeks)
        line = f"{hours:g} h in all; at {goal.pace:g} h a week: about {weeks:.0f} weeks, done around {done.isoformat()}"
    if goal.master_by:
        by = date.fromisoformat(goal.master_by)
        line += (" (in time for " if done <= by else f" ({(done - by).days // 7} weeks after ") + f"{goal.master_by})"
    return line


def render(conn: sqlite3.Connection, gid: str, *, state: str = "proposed") -> str:
    ensure(conn)
    goal = L.get_goal(conn, gid)
    rows = conn.execute("SELECT id, parent, track, title, hours, weight FROM topics WHERE goal=? AND state=? "
                        "ORDER BY ord", (gid, state)).fetchall()
    cur = conn.execute("SELECT summary, dropped, at FROM curricula WHERE goal=? ORDER BY id DESC LIMIT 1",
                       (gid,)).fetchone()
    out = [L.describe(goal)]
    if cur:
        out.append(f"\nProposed {cur[2][:16].replace('T', ' ')}: {cur[0]}")
    out.append(pace_line(goal, leaf_hours(conn, gid, state=state), conn=conn))
    children: dict[int | None, list] = {}
    for r in rows:
        children.setdefault(r[1], []).append(r)

    def show(parent: int | None, depth: int) -> None:
        for tid, _, track, title, hours, weight in children.get(parent, []):
            out.append(f"{'  ' * depth}- {title}  [{track}; {hours:g} h; weight {weight:g}]")
            for ref, quote, why in conn.execute("SELECT ref, quote, why FROM topic_evidence WHERE topic=?", (tid,)):
                out.append(f"{'  ' * depth}    why: {why}")
                out.append(f"{'  ' * depth}    \"{quote}\" [{ref}]")
                break
            show(tid, depth + 1)

    show(None, 0)
    if cur and json.loads(cur[1]):
        dropped = json.loads(cur[1])
        out.append(f"\nDropped by the evidence check: {len(dropped)} (`locus gates` has them)")
    return "\n".join(out)


def export(conn: sqlite3.Connection) -> dict:
    """The Learning screen's contract (`locus_contract::Learning`): goals, each with its
    curriculum depth first (the proposed one while it waits, else the current), and the tracks
    they name. Confidence is 0 until marked practice sets it (L-D5); there are no sheets before L4."""
    ensure(conn)
    goals, tracks = [], {}
    for g in L.goals(conn):
        # A proposal waiting (a first one, or a re-proposal of an active goal) is what shows, and
        # what Approve asks about (L3); otherwise the current curriculum.
        waiting = conn.execute("SELECT 1 FROM topics WHERE goal=? AND state='proposed' LIMIT 1", (g.id,)).fetchone()
        tstate = "proposed" if waiting or g.state == "proposed" else "current"
        state = "proposed" if tstate == "proposed" else {"done": "done"}.get(g.state, "active")
        rows = conn.execute("SELECT id, parent, track, title, hours, weight, confidence FROM topics "
                            "WHERE goal=? AND state=? ORDER BY ord", (g.id, tstate)).fetchall()
        kids: dict = {}
        for r in rows:
            kids.setdefault(r[1], []).append(r)
        items: list[dict] = []

        def walk(parent, depth):
            for tid, _, track, title, hours, weight, conf in kids.get(parent, []):
                ev = conn.execute("SELECT ref, quote, why FROM topic_evidence WHERE topic=? LIMIT 1", (tid,)).fetchone()
                items.append({"track": track, "what": title, "hours": float(hours), "from": ev[0] if ev else "",
                              "depth": min(depth, 255), "why": ev[2] if ev else None,
                              "quote": ev[1] if ev else None, "weight": float(weight)})
                tracks.setdefault(track, {"id": track, "name": track.replace("-", " "),
                                          "confidence": float(conf or 0.0), "history": [], "sessions": [],
                                          "open_misses": 0})
                walk(tid, depth + 1)

        walk(None, 0)
        cur = conn.execute("SELECT summary FROM curricula WHERE goal=? ORDER BY id DESC LIMIT 1", (g.id,)).fetchone()
        goal = {"id": g.id, "title": g.title, "state": state, "syllabus": items,
                "pace": pace_line(g, leaf_hours(conn, g.id, state=tstate), conn=conn)}
        if g.master_by:
            goal["by"] = g.master_by
        if cur:
            goal["summary"] = cur[0]
        goals.append(goal)
    return {"goals": goals, "tracks": list(tracks.values()), "sheets": []}


# --- adding to an approved curriculum ------------------------------------------------------------

def add_topics(conn: sqlite3.Connection, gid: str, topics: list[dict], *, parent: int | None = None,
               before: int | None = None) -> list[int]:
    """Topics he asked for, added to the CURRENT curriculum without re-proposing it (19 Nov:
    machine learning theory and Python iterators; a fresh proposal would rewrite every topic and
    its task). Each dict: key, title, track, hours, weight, why, evidence [{ref, quote}], and
    optionally `parent` (another new topic's key). New top-level topics hang under `parent` (an
    existing topic id) if given; all go in before the topic `before`, or at the end. Grounded
    like a proposal, all or nothing: a quote not found in its pointer refuses the lot. Tasks
    come from `learning_plan.approve`, which makes the missing ones."""
    ensure(conn)
    L.get_goal(conn, gid)
    parsed = [_Topic.model_validate(t) for t in topics]
    for t in parsed:
        if not t.evidence:
            raise L.LearningError(f"{t.title!r} has no evidence")
        for ev in t.evidence:
            texts = [_norm(p.text) for p in _item(conn, ev.ref, NAMED_ITEM_CHARS)]
            if not texts and ev.ref.startswith("fact:"):
                from locus.heuris import facts as F
                f = F.get(conn, int(ev.ref.split(":", 1)[1]))
                texts = [_norm(f.value)] if f else []
            if not any(_norm(ev.quote) in x for x in texts):
                raise L.LearningError(f"{t.title!r}: {ev.quote!r} is not in {ev.ref}")
    now = datetime.now(timezone.utc).isoformat(timespec="seconds")
    with conn:
        if before is not None:
            pos = conn.execute("SELECT ord FROM topics WHERE id=?", (before,)).fetchone()[0]
        else:
            pos = (conn.execute("SELECT MAX(ord) FROM topics WHERE goal=? AND state='current'", (gid,)).fetchone()[0] or 0) + 1
        conn.execute("UPDATE topics SET ord = ord + ? WHERE goal=? AND state='current' AND ord >= ?",
                     (len(parsed), gid, pos))
        ids: dict[str, int] = {}
        for k, t in enumerate(parsed):
            par = ids[t.parent] if t.parent else parent
            tid = conn.execute(
                "INSERT INTO topics(goal, parent, ord, track, title, hours, weight, state, created) "
                "VALUES (?,?,?,?,?,?,?,'current',?)",
                (gid, par, pos + k, t.track, t.title, t.hours, t.weight, now)).lastrowid
            ids[t.key] = tid
            for ev in t.evidence:
                conn.execute("INSERT INTO topic_evidence(topic, ref, quote, why) VALUES (?,?,?,?)",
                             (tid, ev.ref, ev.quote, t.why))
        # An existing parent's hours are its children's sum.
        if parent is not None:
            added = sum(t.hours for t in parsed if not t.parent)
            conn.execute("UPDATE topics SET hours = hours + ? WHERE id=?", (added, parent))
    return list(ids.values())
