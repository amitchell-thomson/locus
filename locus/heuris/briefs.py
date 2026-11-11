"""Briefs (H2.7, D4): a short current page per live or active registry item, and one about him.
Four headings, the same each time: Now, Open, Recent, Next.

**What is written by a model and what is not.** Now is the current facts and Next is the
calendar and tasks coming up: both are rendered from the ledger and kairos's index when the brief
is READ, so they are exact and never stale (heuris B: "briefs are never stale where they are
read"). Open and Recent are the prose a model adds: written by Sonnet in a batch from the facts
and the records, and stored. Every pointer in Recent must be a record the model was given; a
line citing anything else is dropped (CLAUDE.md §2.1, verified after the call). Any record
written after the brief was built is listed under it when read.

**Briefs take state only from facts** (heuris B). Records supply Recent and are said to be
history; a hypothetical in a chat's record can appear as "discussed", never in Now.

A repository's own state file (`state.md`), where it has one, is shown under Now as what the
repository says, dated, beside the facts: it is the project's written account of where it
stands, kept in the repo, not a model's summary. [proposal, 11 Nov: raised with him.]

**When a brief is rebuilt.** When a fact it rests on changes, and otherwise at most once a day
when its records or plans have moved. The fingerprint of its inputs is stored with it.
"""

from __future__ import annotations

import hashlib
import json
import re
import sqlite3
from dataclasses import dataclass
from datetime import date, datetime, timedelta, timezone

from locus.heuris import derive as D
from locus.heuris import facts as F
from locus.heuris import records as R
from locus.heuris import registry

BRIEF_MODEL = "sonnet-5.5"
DAILY = timedelta(hours=20)

BRIEFS_SCHEMA = """
CREATE TABLE IF NOT EXISTS briefs (
    subject     TEXT PRIMARY KEY,
    open        TEXT NOT NULL,          -- JSON list of lines
    recent      TEXT NOT NULL,          -- JSON list of lines, each with its [record:...] pointer
    built_at    TEXT NOT NULL,
    built_from  TEXT NOT NULL           -- JSON: {"facts": hash, "inputs": hash}
);
"""

SYSTEM = """You write two parts of a short brief about one thing in Alec Mitchell-Thomson's life (a project, a course, a job application, a role, or Alec himself), for any Claude that later needs to know where it stands.

You are given its current facts (true now, each dated), records of recent work (each with a pointer), and what is coming up. Write JSON with:
- "open": up to 5 short lines: what is waiting, on Alec or on someone else, and for what. Only from the facts and records.
- "recent": up to 6 short lines, newest first, each saying what happened and ending with the pointer of the record it comes from, exactly as given, in square brackets, e.g. [record:chat:laptop/abc]. A record says what was discussed or decided at the time; report it as that ("decided to...", "discussed..."), never as what is true now.
Nothing that the inputs do not say. No em dashes. Lines under 25 words."""

SCHEMA = {
    "type": "object",
    "properties": {"open": {"type": "array", "items": {"type": "string"}},
                   "recent": {"type": "array", "items": {"type": "string"}}},
    "required": ["open", "recent"],
    "additionalProperties": False,
}


def ensure(conn: sqlite3.Connection) -> None:
    conn.executescript(BRIEFS_SCHEMA)
    R.ensure(conn)


def subjects() -> list[str]:
    """`me`, and every registry item that is live or active (D4)."""
    out = ["me"]
    for pid, e in registry.entries().items():
        if e.get("state") == "live" or e.get("active"):
            out.append(pid)
    return out


# ---------------------------------------------------------------------------------------------
# the parts rendered when read


def now_lines(conn: sqlite3.Connection, subject: str) -> list[str]:
    fs = F.current(conn, subject)
    if subject == "me":  # his brief carries where each application stands
        apps = [i for i, e in registry.entries().items() if e.get("kind") == "application"]
        fs += [f for f in F.current(conn) if f.subject in apps and f.slot in ("stage", "turn")]
    lines = [f"{'' if f.subject == subject else f.subject + ' · '}{f.slot} = {f.value} (as of {f.as_of}) [fact:{f.id}]"
             for f in sorted(fs, key=lambda f: (f.subject != subject, f.subject, ":" in f.slot, f.slot))]
    st = conn.execute("SELECT ref, at, text FROM items WHERE kind='doc' AND ref=?", (f"doc:{subject}/state.md",)).fetchone()
    if st:
        text = re.sub(r"^#.*\n", "", st[2] or "").strip()
        lines.append(f"the repository's own state.md ({st[1][:10]}): {text} [{st[0]}]")
    return lines


def next_lines(conn: sqlite3.Connection, subject: str, today: date | None = None, days: int = 21) -> list[str]:
    today = today or date.today()
    t0 = int(datetime(today.year, today.month, today.day, tzinfo=timezone.utc).timestamp())
    where = "thing = ?" if subject != "me" else "COALESCE(area,'') != 'Family'"
    params = [subject] if subject != "me" else []
    rows = conn.execute(f"SELECT at, kind, title, meta FROM items WHERE kind IN ('event','task') AND at_ts >= ? "
                        f"AND at_ts < ? AND {where} ORDER BY at_ts LIMIT 12",
                        [t0, t0 + days * 86400, *params]).fetchall()
    out = []
    for at, kind, title, meta in rows:
        m = json.loads(meta or "{}")
        if kind == "task" and m.get("status") not in ("open", None):
            continue
        out.append(f"{at[:16].replace('T', ' ')} {'due: ' if kind == 'task' else ''}{title}")
    return out


# ---------------------------------------------------------------------------------------------
# the model's part


def _records_for(conn, subject: str, days: int = 28):
    return R.recent(conn, None if subject == "me" else subject, days=days, limit=12 if subject != "me" else 20)


def fingerprint(conn: sqlite3.Connection, subject: str) -> dict:
    facts = "\n".join(now_lines(conn, subject))
    inputs = json.dumps([[r[3], r[0]] for r in _records_for(conn, subject)] + next_lines(conn, subject))
    h = lambda s: hashlib.sha256(s.encode()).hexdigest()[:16]  # noqa: E731
    return {"facts": h(facts), "inputs": h(inputs)}


def needs_build(conn: sqlite3.Connection, subject: str, now: datetime | None = None) -> bool:
    now = now or datetime.now(timezone.utc)
    row = conn.execute("SELECT built_at, built_from FROM briefs WHERE subject=?", (subject,)).fetchone()
    fp = fingerprint(conn, subject)
    if not row:
        return bool(now_lines(conn, subject) or _records_for(conn, subject))
    old = json.loads(row[1])
    if old.get("facts") != fp["facts"]:
        return True
    return old.get("inputs") != fp["inputs"] and now - datetime.fromisoformat(row[0]) >= DAILY


def user_prompt(conn: sqlite3.Connection, subject: str, today: date) -> str:
    name = "Alec himself" if subject == "me" else f"{registry.entries()[subject].get('name', subject)} ({registry.entries()[subject].get('kind')})"
    facts = "\n".join(f"- {line}" for line in now_lines(conn, subject)) or "- none recorded"
    recs = "\n\n".join(f"[record:{u}] {ended[:10]}: {title}\n{text[:1200]}" for ended, title, text, u in _records_for(conn, subject)) or "none"
    nxt = "\n".join(f"- {line}" for line in next_lines(conn, subject, today)) or "- nothing booked"
    return (f"Today is {today.isoformat()}. The brief is about: {name}.\n\nCurrent facts:\n{facts}\n\n"
            f"Coming up:\n{nxt}\n\nRecords of the last four weeks, newest first:\n<records>\n{recs}\n</records>")


def apply(conn: sqlite3.Connection, subject: str, raw: str, fp: dict, given: set[str]) -> int:
    """Store a built brief; a Recent line whose pointer was not given is dropped. Returns drops."""
    try:
        out = json.loads(raw)
    except ValueError:
        return 0
    recent, dropped = [], 0
    for line in out.get("recent", [])[:6]:
        ptrs = re.findall(r"\[(record:[^\]]+)\]", line)
        if ptrs and all(p in given for p in ptrs):
            recent.append(line.strip())
        else:
            dropped += 1
    open_ = [str(x).strip() for x in out.get("open", [])[:5] if str(x).strip()]
    with conn:
        conn.execute("INSERT OR REPLACE INTO briefs(subject, open, recent, built_at, built_from) VALUES (?,?,?,?,?)",
                     (subject, json.dumps(open_), json.dumps(recent), D._now(), json.dumps(fp)))
    return dropped


@dataclass
class BriefsReport:
    collected: int = 0
    submitted: int = 0
    dropped: int = 0
    note: str = ""

    def describe(self) -> str:
        return (f"briefs: collected {self.collected} ({self.dropped} unsourced lines dropped); submitted "
                f"{self.submitted}" + (f"; {self.note}" if self.note else ""))


def run_briefs(conn: sqlite3.Connection, *, force: bool = False, submit_new: bool = True,
               only: list[str] | None = None) -> BriefsReport:
    from locus.agent.claude import BatchRequest

    ensure(conn)
    rep = BriefsReport()

    def done(cid, meta, text):
        rep.dropped += apply(conn, meta["subject"], text, meta["fp"], set(meta["given"]))

    rep.collected = D.collect(conn, "briefs", done)
    if not submit_new:
        return rep
    waiting = {v["subject"] for (items,) in conn.execute("SELECT items FROM batches WHERE job='briefs' AND applied=0")
               for v in json.loads(items).values()}
    today = date.today()
    reqs, meta = [], {}
    for i, s in enumerate(only or subjects()):
        if s in waiting or not (only or needs_build(conn, s)):
            continue
        cid = f"b{i}"
        meta[cid] = {"subject": s, "fp": fingerprint(conn, s),
                     "given": [f"record:{r[3]}" for r in _records_for(conn, s)]}
        reqs.append(BatchRequest(custom_id=cid, model=BRIEF_MODEL, system=SYSTEM, user=user_prompt(conn, s, today),
                                 max_tokens=2000, schema=SCHEMA))
    estimate = sum(len(r.user) for r in reqs) / 4 * 2.0 / 1e6 * 0.5 + len(reqs) * 400 * 10 / 1e6 * 0.5
    rep.submitted, rep.note = D.submit(conn, "briefs", BRIEF_MODEL, reqs, meta, estimate=estimate, force=force)
    return rep


# ---------------------------------------------------------------------------------------------
# reading one


def render(conn: sqlite3.Connection, subject: str, today: date | None = None, *, with_now: bool = True) -> str:
    """The brief as `context(about=...)` shows it: Now and Next exact and live, Open and Recent as
    last built, and every record written since."""
    ensure(conn)
    name = "you (Alec)" if subject == "me" else registry.entries().get(subject, {}).get("name", subject)
    row = conn.execute("SELECT open, recent, built_at FROM briefs WHERE subject=?", (subject,)).fetchone()
    out = [f"Brief: {name}"]
    if with_now:
        now = now_lines(conn, subject)
        out.append("Now (facts, exact; each with its date and pointer):")
        out += [f"  - {x}" for x in now] or ["  - no facts recorded"]
    if row:
        open_, recent, built = json.loads(row[0]), json.loads(row[1]), row[2]
        out.append(f"Open (as of {built[:10]}):")
        out += [f"  - {x}" for x in open_] or ["  - nothing noted"]
        out.append(f"Recent (records, newest first; built {built[:16].replace('T', ' ')}):")
        out += [f"  - {x}" for x in recent] or ["  - nothing recorded"]
        newer = R.recent(conn, None if subject == "me" else subject, after=built, limit=8)
        if newer:
            out.append("Since this brief was built:")
            out += [f"  - {e[:10]} {t} [record:{u}]" for e, t, _x, u in newer]
    else:
        recs = R.recent(conn, None if subject == "me" else subject, limit=6)
        out.append("Recent (records, newest first; no brief built yet):")
        out += [f"  - {e[:10]} {t} [record:{u}]" for e, t, _x, u in recs] or ["  - nothing recorded"]
    nxt = next_lines(conn, subject, today)
    out.append("Next:")
    out += [f"  - {x}" for x in nxt] or ["  - nothing booked in the next three weeks"]
    return "\n".join(out)
