"""The nightly pass over his learning chats (spec L, L-D13, step L11).

Why (22 Nov): a session's close records only what happened inside the session. Session 01 showed
the gap: a third of his turns in that chat came after the close (the codec, the `_Value` /
`_Message` split, `Annotated[]` and `Literal[]`), exactly the "assumed knowledge" signal, and
nothing would have recorded them. Build chats, sessions that never close, and moments a close
missed are the same gap.

Which chats (24 Nov, his call): those whose working directory is `~/files/Learning`,
`oqts/oxdaq-core` or `oqts/oxdaq-ops`, "not the whole of oqts because it is not worth going over
throwaway website tweaking chats". `CHAT_DIRS`.

How:
- A session is read once it has been quiet for `QUIET_H` (so a live chat is not read half way),
  and again only when it has new turns of his since (`learning_reads`). The model sees the whole
  session (Claude's turns shortened) but is asked only about his NEW turns, and is shown what is
  already recorded for the session so it does not repeat a session's close.
- **Grounded or silent.** Every observation's quote must be in one of his new turns of that
  session, checked here and pinned to the turn; one that is not is dropped and counted in
  `locus gates` (`learning.nightly_quote`). A card must quote a moment this pass (or the session)
  recorded: a card with no moment behind it is dropped (L-D13: cards are born only from real
  moments).
- **No duplicates.** An observation whose quote overlaps one already recorded in the session is
  dropped; a card whose moment already has a card, or whose prompt is already on the track, is
  refused by `learning_record.add_cards`.
- Cards add themselves [you, 22 Nov: "add themselves, I will delete any cards that are bad"]; at
  most about 20 new a day enter the review queue (L12).
"""

from __future__ import annotations

import json
import re
import sqlite3
from dataclasses import dataclass, field
from datetime import datetime, timezone

from pydantic import BaseModel

from locus.heuris import derive as D
from locus.heuris import learning as L
from locus.heuris import learning_record as R

MODEL = "sonnet-5.5"
JOB = "learning"
SINCE = "2026-11-21"          # the first session; nothing before it is a learning chat
QUIET_H = 2.0
CHAT_DIRS = re.compile(r"/files/Learning(/|$)|/oqts/oxdaq-core(/|$)|/oqts/oxdaq-ops(/|$)")
OXDAQ_DIRS = re.compile(r"/oxdaq-(core|ops)(/|$)")
CLAUDE_TURN_CHARS = 1200      # Claude's turns, shortened: context for his, not evidence
SESSION_CHARS = 80_000
NIGHT = (2, 6)                # London hours a run may submit in: once a night, not every hour
MAX_SESSIONS = 30             # a run's batch; the rest wait for the next run

READS_SCHEMA = """
CREATE TABLE IF NOT EXISTS learning_reads (
    session  TEXT PRIMARY KEY,
    upto_ts  INTEGER NOT NULL,          -- his turns up to here have been read
    at       TEXT NOT NULL
);
"""


def ensure(conn: sqlite3.Connection) -> None:
    R.ensure(conn)
    D.ensure(conn)
    conn.executescript(READS_SCHEMA)


@dataclass
class Turn:
    ref: str
    role: str        # you, claude
    at_ts: int
    text: str


def _turns(conn: sqlite3.Connection, session: str) -> list[Turn]:
    return [Turn(r, (json.loads(m or "{}").get("role") or "you"), ts, t) for r, m, ts, t in conn.execute(
        "SELECT ref, meta, at_ts, text FROM items WHERE source='chats' AND parent LIKE ? ORDER BY at_ts, id",
        (f"chat:%/{session}",))]


def sessions_to_read(conn: sqlite3.Connection, *, since: str = SINCE, now: datetime | None = None) -> list[tuple[str, str, int]]:
    """(session, cwd, newest turn ts) for learning chats with turns of his not yet read, quiet for
    QUIET_H. Oldest first."""
    ensure(conn)
    now_ts = int((now or datetime.now(timezone.utc)).timestamp())
    since_ts = int(datetime.fromisoformat(since).replace(tzinfo=timezone.utc).timestamp())
    rows = conn.execute(
        "SELECT json_extract(meta,'$.session') s, json_extract(meta,'$.cwd') cwd, MAX(at_ts) last, "
        "MAX(CASE WHEN json_extract(meta,'$.role')='you' THEN at_ts END) his "
        "FROM items WHERE source='chats' AND at_ts >= ? GROUP BY s", (since_ts,)).fetchall()
    out = []
    for s, cwd, last, his in rows:
        if not s or not cwd or not CHAT_DIRS.search(cwd) or his is None:
            continue
        if now_ts - last < QUIET_H * 3600:
            continue
        done = conn.execute("SELECT upto_ts FROM learning_reads WHERE session=?", (s,)).fetchone()
        if done and done[0] >= his:
            continue
        out.append((s, cwd, last))
    return sorted(out, key=lambda x: x[2])


# --- the prompt ------------------------------------------------------------------------------------


def _goals_block(conn: sqlite3.Connection) -> str:
    lines = []
    for g in L.goals(conn):
        if g.state != "active":
            continue
        trs = [r[0] for r in conn.execute(
            "SELECT track FROM topics WHERE goal=? AND state='current' GROUP BY track ORDER BY MIN(id)", (g.id,))]
        lines.append(f"- {g.id} ({g.title}): tracks {', '.join(trs)}")
    lines.append(f"- coding practice, any goal: tracks {', '.join(R.PRACTICE_TRACKS)} (how he writes code: idioms, "
                 "testing, design patterns, tooling; one per language)")
    return "\n".join(lines)


def system_prompt(conn: sqlite3.Connection) -> str:
    return f"""You read one Claude Code chat of Alec's, a student learning for quant interviews and building a C++/Python exchange (OXDAQ), and record the moments that show what he can and cannot yet do. They feed his learning record: before teaching, a session reads them and probes anything with no evidence, so they must be accurate and specific.

Goals and their tracks:
{_goals_block(conn)}

Record observations, one per real moment in HIS NEW turns (marked NEW):
- kind: explained-back (put an idea correctly in his own words), cold (solved something unaided), hinted (got there after a hint), explained-to-him (was told something he did not know), wrong (an answer or claim that was wrong), question (a question that shows what he does not yet know), confusion (said he did not follow), misconception (believed something false).
- track: one of the tracks above; goal: the goal it belongs to (an OXDAQ build chat is cpp-oxdaq).
- text: what happened, one plain line ("asked why == does not use the hash"), never a verdict ("weak at hashing").
- quote: HIS words, ONE sentence or phrase copied character for character from one of his NEW turns. It is checked; a quote that is not there loses the observation. Never quote Claude.
- Skip logistics (scheduling, "push the branch", thanks), questions about tools or the setup, and anything already recorded (listed in the prompt).

Then cards, only for moments worth drilling (wrong, hinted, question, confusion, misconception, or something met for the first time): one idea each; recall, not recognition; why/when over what; his own confusion as the prompt where there was one; for code "predict the output" or "what is wrong here"; for maths the set-up rather than the number. quote: the quote of the observation it comes from. The answer must be correct and short: a wrong answer drilled is worse than none; leave out a card you are not sure of.

Most chats give 0 to 8 observations and 0 to 5 cards. An empty list is the right answer for a chat with no learning in it."""


SCHEMA = {
    "type": "object",
    "properties": {
        "observations": {"type": "array", "items": {
            "type": "object",
            "properties": {k: {"type": "string"} for k in ("goal", "track", "kind", "text", "quote")},
            "required": ["goal", "track", "kind", "text", "quote"], "additionalProperties": False}},
        "cards": {"type": "array", "items": {
            "type": "object",
            "properties": {k: {"type": "string"} for k in ("goal", "track", "moment", "prompt", "answer", "quote")},
            "required": ["goal", "track", "moment", "prompt", "answer", "quote"], "additionalProperties": False}},
    },
    "required": ["observations", "cards"],
    "additionalProperties": False,
}


def render(turns: list[Turn], upto_ts: int) -> str:
    parts = []
    for t in turns:
        if t.role == "you":
            parts.append(("[Alec, NEW]" if t.at_ts > upto_ts else "[Alec, already read]") + "\n" + t.text)
        else:
            txt = t.text if len(t.text) <= CLAUDE_TURN_CHARS else t.text[:CLAUDE_TURN_CHARS] + " [...]"
            parts.append("[Claude]\n" + txt)
    s = "\n\n".join(parts)
    if len(s) > SESSION_CHARS:      # keep the end: the new turns are there
        s = "[earlier turns cut]\n" + s[-SESSION_CHARS:]
    return s


def user_prompt(conn: sqlite3.Connection, session: str, cwd: str, turns: list[Turn], upto_ts: int) -> str:
    have = conn.execute("SELECT kind, track, text, quote FROM observations WHERE session=?", (session,)).fetchall()
    cards = conn.execute("SELECT track, prompt FROM cards WHERE session=? AND state != 'deleted'", (session,)).fetchall()
    rec = "\n".join(f"- [{k}] {tr}: {t}" + (f' ("{q}")' if q else "") for k, tr, t, q in have) or "- nothing"
    crd = "\n".join(f"- {tr}: {p}" for tr, p in cards) or "- none"
    return (f"Working directory: {cwd}\n\nAlready recorded for this chat (do not repeat):\n{rec}\n"
            f"Cards already made from it:\n{crd}\n\nThe chat:\n<chat>\n{render(turns, upto_ts)}\n</chat>")


# --- applying --------------------------------------------------------------------------------------


class _Out(BaseModel):
    observations: list[dict] = []
    cards: list[dict] = []


@dataclass
class Applied:
    observations: int = 0
    cards: int = 0
    dropped_quote: int = 0
    dropped_dupe: int = 0
    dropped_card: int = 0
    refused: list[str] = field(default_factory=list)


def _overlap(a: str, b: str) -> bool:
    a, b = R._norm(a), R._norm(b)
    return bool(a and b) and (a in b or b in a)


def apply(conn: sqlite3.Connection, session: str, upto_ts: int, raw: str, *, default_goal: str | None,
          gate_conn=None) -> Applied:
    ensure(conn)
    out = Applied()
    got = _Out.model_validate(json.loads(raw[raw.find("{"): raw.rfind("}") + 1]))
    his_new = [t for t in _turns(conn, session) if t.role == "you" and t.at_ts > upto_ts]
    goals = {g.id for g in L.goals(conn)}
    existing = [q for (q,) in conn.execute("SELECT quote FROM observations WHERE session=? AND quote IS NOT NULL", (session,))]
    kept_quotes: list[str] = []
    for o in got.observations:
        q = (o.get("quote") or "").strip()
        turn = next((t for t in his_new if len(R._norm(q)) >= 6 and R._norm(q) in R._norm(t.text)), None)
        if gate_conn is not None:
            from locus.observe import gates
            gates.record(gate_conn, "learning.nightly_quote", rejected=turn is None, value=f"{session} | {q[:100]}")
        if turn is None:
            out.dropped_quote += 1
            continue
        if any(_overlap(q, e) for e in existing):
            out.dropped_dupe += 1
            continue
        goal = o.get("goal") if o.get("goal") in goals else default_goal
        if goal is None:
            out.refused.append(f"{q[:40]!r}: no goal")
            continue
        w = R.observe(conn, goal, [{k: o.get(k) for k in ("track", "kind", "text", "quote")}],
                      session=session, ref=turn.ref, origin="nightly",
                      at=datetime.fromtimestamp(turn.at_ts, timezone.utc).isoformat(timespec="seconds"))
        if w.ids:
            with conn:
                conn.execute("UPDATE observations SET checked=1 WHERE id=?", (w.ids[0],))
            out.observations += 1
            existing.append(q)
            kept_quotes.append(q)
        out.refused += w.refused
    for c in got.cards:
        q = (c.get("quote") or "").strip()
        # A card is born from a moment: its quote must be an observation of this session.
        match = next((e for e in existing if _overlap(q, e)), None)
        if match is None:
            out.dropped_card += 1
            continue
        goal = c.get("goal") if c.get("goal") in goals else default_goal
        if goal is None:
            continue
        w = R.add_cards(conn, goal, [{"track": c.get("track"), "moment": c.get("moment"), "prompt": c.get("prompt"),
                                      "answer": c.get("answer"), "quote": match}],
                        session=session, origin="nightly")
        out.cards += len(w.ids)
        out.refused += w.refused
    if gate_conn is not None and out.refused:
        from locus.observe import gates
        for r in out.refused:
            gates.record(gate_conn, "learning.nightly_refused", rejected=True, value=r[:120])
    return out


def default_goal(cwd: str) -> str | None:
    return "cpp-oxdaq" if OXDAQ_DIRS.search(cwd or "") else None


# --- the run ---------------------------------------------------------------------------------------


@dataclass
class Report:
    collected: int = 0
    applied: Applied = field(default_factory=Applied)
    submitted: int = 0
    note: str = ""

    def describe(self) -> str:
        a = self.applied
        return (f"learning: collected {self.collected} chats ({a.observations} observations, {a.cards} cards; dropped "
                f"{a.dropped_quote} unquoted, {a.dropped_dupe} already recorded, {a.dropped_card} cards with no moment"
                + (f", {len(a.refused)} refused" if a.refused else "") + f"); submitted {self.submitted} chats"
                + (f"; {self.note}" if self.note else ""))


def run(conn: sqlite3.Connection, *, since: str = SINCE, force: bool = False, gate_conn=None,
        submit_new: bool = True, now: datetime | None = None) -> Report:
    from locus.agent.claude import BatchRequest

    ensure(conn)
    rep = Report()

    def on_result(cid, meta, text):
        a = apply(conn, meta["session"], meta["upto"], text, default_goal=default_goal(meta["cwd"]), gate_conn=gate_conn)
        for k in ("observations", "cards", "dropped_quote", "dropped_dupe", "dropped_card"):
            setattr(rep.applied, k, getattr(rep.applied, k) + getattr(a, k))
        rep.applied.refused += a.refused
        with conn:
            conn.execute("INSERT INTO learning_reads(session, upto_ts, at) VALUES (?,?,?) ON CONFLICT(session) DO UPDATE "
                         "SET upto_ts=MAX(upto_ts, excluded.upto_ts), at=excluded.at",
                         (meta["session"], meta["his_upto"], D._now()))

    rep.collected = D.collect(conn, JOB, on_result)
    if not submit_new:
        return rep
    hour = (now or datetime.now(timezone.utc)).astimezone(D.LONDON).hour
    if not force and not NIGHT[0] <= hour < NIGHT[1]:
        rep.note = f"reads chats only between {NIGHT[0]:02d}:00 and {NIGHT[1]:02d}:00"
        return rep
    waiting = {m["session"] for (items,) in conn.execute("SELECT items FROM batches WHERE job=? AND applied!=1", (JOB,))
               for m in json.loads(items).values()}
    system = system_prompt(conn)
    reqs, meta = [], {}
    for i, (s, cwd, _) in enumerate(sessions_to_read(conn, since=since, now=now)):
        if s in waiting:
            continue
        if len(reqs) >= MAX_SESSIONS:
            break
        turns = _turns(conn, s)
        r = conn.execute("SELECT upto_ts FROM learning_reads WHERE session=?", (s,)).fetchone()
        upto = r[0] if r else 0
        his_upto = max(t.at_ts for t in turns if t.role == "you")
        cid = f"s{i}"
        meta[cid] = {"session": s, "cwd": cwd, "upto": upto, "his_upto": his_upto}
        reqs.append(BatchRequest(custom_id=cid, model=MODEL, system=system,
                                 user=user_prompt(conn, s, cwd, turns, upto), max_tokens=4000, schema=SCHEMA))
    # ~25k tokens in and ~1k out a chat on Sonnet 5.5, at the batch price.
    estimate = len(reqs) * (25_000 * 2.0 + 1_000 * 10.0) / 1e6 * 0.5
    rep.submitted, rep.note = D.submit(conn, JOB, MODEL, reqs, meta, estimate=estimate, force=force)
    return rep
