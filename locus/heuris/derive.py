"""What heuris derives on the credit, hourly (H2.4 onwards): facts from mail and the calendar,
then records (H2.6) and briefs (H2.7). `locus heuris derive` runs on `locus-heuris-derive.timer`.

**One run, two halves.** Batches finish within the hour, not within a run. So each run first
COLLECTS the batches earlier runs submitted (heuris.db `batches`) and applies their results, then
SUBMITS new work. Nothing waits: a run takes seconds, and a result lands one run later.

**Facts from mail (D2).** Sonnet reads each changed thread from a person or a firm and proposes
facts in the ledger's shape, each with the sentence it rests on. Then code, not the model, decides:

- the QUOTE must be in the thread's text (whitespace and quote marks aside). A model told to cite
  will cite, including sentences it wrote itself (CLAUDE.md §2.1), so it is checked after the
  call; a fact whose quote is not there is dropped and counted (`locus gates`,
  `heuris.fact_quote`);
- the SLOT must be on the list (facts.check_slot); a fact with an unknown one is dropped and
  counted (`heuris.fact_slot`);
- AUTHORITY is read from who sent the message the quote is in, never from the model's say-so
  (§14: model self-assessment is not a signal). An application's stage, offer terms, dates and
  interviews from the firm or its recruiting system, and a course's tutorials, sheets and tutor
  from Oxford, are written automatically, logged with undo. Everything else (his own sent mail, a
  friend, a society, any fact about `me` or a person) waits in Approve;
- a CONFLICT waits in Approve: a value that would replace one he stated himself (chat, the Home
  chat, the applications file) is never overwritten by mail. A newer value from mail replacing an
  older one from mail is the ordinary course of an application and is written.

**Interviews from the calendar** need no model: an event filed under an application whose title
names an interview, round, onsite or call is `interview:<date>` on that application.
"""

from __future__ import annotations

import json
import re
import sqlite3
from dataclasses import dataclass
from datetime import date, datetime, timezone

from locus.heuris import facts as F
from locus.heuris import registry

FACT_MODEL = "sonnet-5.5"
THREAD_CHARS = 14000
SINCE_DEFAULT = "2026-06-01"   # D9: the backfill reaches back to this recruiting season

DERIVE_SCHEMA = """
CREATE TABLE IF NOT EXISTS batches (
    id       TEXT PRIMARY KEY,          -- the API's batch id
    job      TEXT NOT NULL,             -- facts, records, briefs
    model    TEXT NOT NULL,
    created  TEXT NOT NULL,
    items    TEXT NOT NULL,             -- JSON: custom_id -> what it was about
    ended    TEXT,
    applied  INTEGER NOT NULL DEFAULT 0,
    note     TEXT
);
-- How far each thread has been read for facts: re-read only when a newer message arrives.
CREATE TABLE IF NOT EXISTS fact_reads (
    thread   TEXT PRIMARY KEY,
    upto_ts  INTEGER NOT NULL,
    at       TEXT NOT NULL
);
"""

# Why H1 kept a mail (its `why`): people, firms, his own; not societies' newsletters or bookings.
READ_WHY = {"a person (named sender)", "you sent it", "a thread you replied to", "a firm or recruiting system",
            "someone you have written to", "an Oxford list or office"}
_BULK = re.compile(r"unsubscribe|mailing list|view (?:it )?in (?:your )?browser|manage (?:your )?preferences|"
                   r"you are receiving this", re.I)
HIS = re.compile(r"alec\.mitchellthomson@gmail\.com|newc7370@ox\.ac\.uk|alec\.mitchell-thomson@", re.I)

# Slots written without asking, from a sender of authority (D2).
AUTO_APPLICATION = ("stage", "start", "end", "deadline", "outcome", "offer", "interview")
AUTO_COURSE = ("tutor", "exam", "tutorials", "sheet_due")


def ensure(conn: sqlite3.Connection) -> None:
    conn.executescript(DERIVE_SCHEMA)


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


# ---------------------------------------------------------------------------------------------
# threads


@dataclass
class Message:
    ref: str
    at: str
    sender: str
    name: str
    why: str
    title: str
    text: str


def threads_to_read(conn: sqlite3.Connection, *, since: str, limit: int | None = None) -> list[str]:
    """Threads with a message newer than what was last read for facts, since `since`."""
    ensure(conn)
    t0 = int(datetime.fromisoformat(since).replace(tzinfo=timezone.utc).timestamp())
    rows = conn.execute(
        "SELECT i.parent, MAX(i.at_ts) FROM items i WHERE i.kind='mail' AND i.at_ts >= ? GROUP BY i.parent "
        "ORDER BY MAX(i.at_ts) DESC", (t0,)).fetchall()
    read = dict(conn.execute("SELECT thread, upto_ts FROM fact_reads"))
    out = [p for p, ts in rows if read.get(p, -1) < ts]
    return out[:limit] if limit else out


def thread_messages(conn: sqlite3.Connection, parent: str) -> list[Message]:
    return [Message(r[0], r[1], (r[2] or "").lower(), r[3] or "", r[4] or "", r[5] or "", r[6] or "")
            for r in conn.execute(
                "SELECT ref, at, json_extract(meta,'$.from'), json_extract(meta,'$.from_name'), "
                "json_extract(meta,'$.why'), title, text FROM items WHERE parent=? AND kind='mail' ORDER BY at_ts",
                (parent,))]


def worth_reading(msgs: list[Message]) -> bool:
    """From a person or a firm (or his own), and not bulk: lists and newsletters state no facts
    about him, and reading them is most of the cost."""
    if not msgs:
        return False
    if not any(m.why in READ_WHY for m in msgs):
        return False
    return not all(_BULK.search(m.text) for m in msgs)


def render_thread(msgs: list[Message]) -> str:
    parts = []
    for i, m in enumerate(msgs, 1):
        who = "Alec (him)" if HIS.search(m.sender) else f"{m.name or m.sender} <{m.sender}>"
        parts.append(f"[message {i}] {m.at[:16].replace('T', ' ')} from {who}\nSubject: {m.title}\n{m.text}")
    text = "\n\n".join(parts)
    # The newest messages carry what is true now: keep the end when a thread is long.
    return text if len(text) <= THREAD_CHARS else "[earlier messages cut]\n" + text[-THREAD_CHARS:]


# ---------------------------------------------------------------------------------------------
# the prompt


def _subjects_block() -> str:
    lines = []
    for pid, e in registry.entries().items():
        extra = f", company {e['company']}" if e.get("company") else ""
        lines.append(f"- {pid}: {e.get('name', pid)} ({e.get('kind')}{extra})")
    return "\n".join(lines)


def _slots_block() -> str:
    out = []
    for kind, t in F.slot_table().items():
        out.append(f"- {kind}: single {', '.join(t['single'])}; named {', '.join(n + ':<name>' for n in t['named'])}")
    return "\n".join(out)


def system_prompt() -> str:
    return f"""You read one email thread from Alec Mitchell-Thomson's mail (an Oxford engineering student applying to quant firms) and list the facts it STATES that belong in his ledger of what is true now.

A fact is subject · slot = value.
Subjects: `me` (Alec), `person:<first-last>`, or one of these registry ids:
{_subjects_block()}

Slots, by the subject's kind (repo covers projects; a registry kind not listed uses repo):
{_slots_block()}
Named slots take a short lowercase hyphenated name: offer:pay, offer:dates, interview:2026-10-01, tutorials:2026-11-25, sheet_due:sheet-1, contact:sarah-scadding, note:<name>. Reuse the name of an existing fact when it is the same thing.

Rules:
- Only what the thread states as true or decided. Never a guess, a hypothetical, a question, a request, or something only proposed.
- Only facts about Alec, his applications, his courses, his projects and roles, or people as they relate to him. Nothing from newsletters, adverts or event listings that are not about him.
- value: short and exact, in plain words; dates as YYYY-MM-DD (with a time, YYYY-MM-DD HH:MM).
- as_of: the date (YYYY-MM-DD) of the message that states it.
- quote: ONE sentence or line copied character for character from the thread, which states the fact. It is checked against the thread; a quote that is not there loses the fact.
- At most 8 facts; the ones most worth knowing later. An empty list is the right answer for most threads."""


SCHEMA = {
    "type": "object",
    "properties": {
        "facts": {
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "subject": {"type": "string"},
                    "slot": {"type": "string"},
                    "value": {"type": "string"},
                    "as_of": {"type": "string"},
                    "quote": {"type": "string"},
                },
                "required": ["subject", "slot", "value", "as_of", "quote"],
                "additionalProperties": False,
            },
        }
    },
    "required": ["facts"],
    "additionalProperties": False,
}


def user_prompt(conn: sqlite3.Connection, msgs: list[Message], today: date) -> str:
    known = [f for f in F.current(conn) if F.subject_kind(f.subject) in ("application", "course", "role", "me")]
    facts = "\n".join(f"- {f.subject} · {f.slot} = {f.value} (as of {f.as_of})" for f in known[:120]) or "- none yet"
    return (f"Today is {today.isoformat()}.\n\nFacts already recorded (reuse their slot names; restate one only "
            f"if the thread changes or confirms it):\n{facts}\n\nThe thread:\n<thread>\n{render_thread(msgs)}\n</thread>")


# ---------------------------------------------------------------------------------------------
# verifying and applying


def _norm(s: str) -> str:
    s = s.replace("’", "'").replace("‘", "'").replace("“", '"').replace("”", '"').replace("–", "-").replace("—", "-")
    return re.sub(r"\s+", " ", s).strip().casefold()


def find_quote(msgs: list[Message], quote: str) -> Message | None:
    """The message whose text (or subject) contains the quote; None when no message does."""
    q = _norm(quote).strip(" .\"'")
    if len(q) < 8:
        return None
    for m in msgs:
        if q in _norm(m.text) or q in _norm(m.title):
            return m
    return None


def _firm_domains(app: str) -> list[str]:
    from locus.heuris.tools import FIRM_DOMAINS

    company = (registry.entries().get(app, {}).get("company") or "").lower()
    return FIRM_DOMAINS.get(company, [re.sub(r"[^a-z0-9]", "", company)] if company else [])


def has_authority(subject: str, slot: str, m: Message) -> bool:
    """D2, from the sender's address: the firm about its application; Oxford about a course."""
    if HIS.search(m.sender) or "@" not in m.sender:
        return False
    kind = F.subject_kind(subject)
    base = slot.split(":")[0]
    domain = m.sender.split("@", 1)[1]
    if kind == "application" and base in AUTO_APPLICATION:
        return any(d in domain for d in _firm_domains(subject)) or m.why == "a firm or recruiting system"
    if kind == "course" and base in AUTO_COURSE:
        return domain == "ox.ac.uk" or domain.endswith(".ox.ac.uk")
    return False


HIS_ROUTES = ("chat", "home", "import")


@dataclass
class Applied:
    written: int = 0
    proposed: int = 0
    history: int = 0
    confirmed: int = 0
    dropped_quote: int = 0
    dropped_slot: int = 0


def apply_facts(conn: sqlite3.Connection, msgs: list[Message], raw: str, *, gate_conn=None) -> Applied:
    """Verify each extracted fact and write it, propose it, or drop it (module docstring)."""
    from locus.observe import gates

    st = Applied()
    try:
        items = json.loads(raw).get("facts", [])
    except (ValueError, AttributeError):
        return st
    for x in items[:8]:
        try:
            subject, slot = F.check_slot(str(x.get("subject", "")), str(x.get("slot", "")))
        except F.FactError:
            st.dropped_slot += 1
            if gate_conn is not None:
                gates.record(gate_conn, "heuris.fact_slot", rejected=True, value=f"{x.get('subject')} · {x.get('slot')}")
            continue
        m = find_quote(msgs, str(x.get("quote", "")))
        if gate_conn is not None:
            gates.record(gate_conn, "heuris.fact_quote", rejected=m is None, value=str(x.get("quote", ""))[:160])
        if m is None:
            st.dropped_quote += 1
            continue
        value = str(x.get("value", "")).strip()
        try:
            as_of = date.fromisoformat(str(x.get("as_of", ""))[:10]).isoformat()
        except ValueError:
            as_of = m.at[:10]
        as_of = min(as_of, m.at[:10])  # a fact is not known before the message that states it
        src = [m.ref]
        quote = str(x.get("quote", "")).strip()
        cur = [f for f in F.current(conn, subject, slot) if f.slot == slot]
        cur_f = cur[0] if cur else None
        same = cur_f is not None and F._norm_value(cur_f.value) == F._norm_value(value)
        older = cur_f is not None and as_of < cur_f.as_of
        his_word = cur_f is not None and cur_f.route in HIS_ROUTES
        auto = has_authority(subject, slot, m)
        try:
            if same or older:
                # Confirms what is true, or is history behind it: neither changes the current value.
                w = F.write(conn, subject, slot, value, sources=src, route="mail", as_of=as_of, quote=quote)
                st.confirmed += w.action == "confirm"
                st.history += w.action == "history"
            elif auto and not his_word:
                F.write(conn, subject, slot, value, sources=src, route="mail", as_of=as_of, quote=quote)
                st.written += 1
            else:
                why = ("conflicts with what he said" if his_word else
                       "his own mail" if HIS.search(m.sender) else "not from a sender of authority")
                if F.propose(conn, subject, slot, value, sources=src, route="mail", why=why, as_of=as_of, quote=quote):
                    st.proposed += 1
        except F.FactError:
            st.dropped_slot += 1
    return st


# ---------------------------------------------------------------------------------------------
# the calendar: interviews, with no model


_INTERVIEW = re.compile(r"\b(interview|onsite|on-site|superday|super day|assessment centre|call with)\b", re.I)
_NOT_A_BOOKING = re.compile(r"^[✓☐]|\bprep\b|\bpractice\b", re.I)


def app_for_event(title: str, at: str, thing: str | None) -> str | None:
    """The application an interview event is for. Events are mostly filed with no `thing`, so the
    firm is read from the title; a firm with several applications is narrowed by the other words
    of an application's name ("AI/ML"), then by the year it is for (an October 2026 interview is
    for an internship in 2027). Still ambiguous: None, never a guess."""
    apps = {i: e for i, e in registry.entries().items() if e.get("kind") == "application"}
    if thing in apps:
        return thing
    t = (title or "").lower()
    cands = [i for i, e in apps.items() if e.get("company") and e["company"].lower() in t]
    if len(cands) > 1:
        def extra(i):
            words = set(re.findall(r"[a-z0-9/]+", apps[i]["name"].lower())) - set(re.findall(r"[a-z0-9/]+", apps[i]["company"].lower()))
            return {w for w in words if not w.isdigit() and len(w) > 2 and w not in {"internship", "the"}}
        named = [i for i in cands if extra(i) and any(w in t for w in extra(i))]
        if len(named) == 1:
            return named[0]
        unnamed = [i for i in cands if not (extra(i) & set(re.findall(r"[a-z0-9/]+", t)))]
        year = int(at[:4]) + (1 if int(at[5:7]) >= 7 else 0)
        dated = [i for i in (unnamed or cands) if str(year) in apps[i]["name"]]
        cands = dated or unnamed
    return cands[0] if len(cands) == 1 else None


def calendar_facts(conn: sqlite3.Connection, *, since: str) -> int:
    """Interview bookings: his events (not the family's) whose title names an interview or a call
    with a firm he applied to."""
    t0 = int(datetime.fromisoformat(since).replace(tzinfo=timezone.utc).timestamp())
    n = 0
    for ref, at, title, thing, area in conn.execute(
            "SELECT ref, at, title, thing, area FROM items WHERE kind='event' AND at_ts >= ?", (t0,)):
        if area == "Family" or not _INTERVIEW.search(title or "") or _NOT_A_BOOKING.search(title or ""):
            continue
        app = app_for_event(title, at, thing)
        if not app:
            continue
        day = at[:10]
        value = f"{title} ({at[:16].replace('T', ' ')})"
        as_of = min(day, date.today().isoformat())
        try:
            w = F.write(conn, app, f"interview:{day}", value, sources=[ref], route="calendar", as_of=as_of)
            n += w.action == "write"
        except F.FactError:
            continue
    return n


# ---------------------------------------------------------------------------------------------
# the hourly run


@dataclass
class RunReport:
    collected: int = 0
    applied: Applied | None = None
    submitted: int = 0
    skipped_threads: int = 0
    calendar: int = 0
    note: str = ""

    def describe(self) -> str:
        a = self.applied or Applied()
        return (f"facts: collected {self.collected} results (written {a.written}, proposed {a.proposed}, "
                f"confirmed {a.confirmed}, history {a.history}; dropped {a.dropped_quote} unquoted, "
                f"{a.dropped_slot} bad slot); submitted {self.submitted} threads "
                f"({self.skipped_threads} not worth reading); calendar {self.calendar} interviews"
                + (f"; {self.note}" if self.note else ""))


def collect(conn: sqlite3.Connection, job: str, apply) -> int:
    """Apply every ended batch of `job` not yet applied. Returns results applied."""
    from locus.agent import claude

    ensure(conn)
    n = 0
    for bid, model, items in conn.execute("SELECT id, model, items FROM batches WHERE job=? AND applied=0", (job,)).fetchall():
        res = claude.batch_collect(bid, model)
        if res is None:
            continue
        meta = json.loads(items)
        for cid, r in res.items():
            if isinstance(r, claude.ClaudeResult):
                apply(cid, meta.get(cid), r.text)
                n += 1
        with conn:
            conn.execute("UPDATE batches SET ended=?, applied=1 WHERE id=?", (_now(), bid))
    return n


def submit(conn: sqlite3.Connection, job: str, model: str, requests, meta: dict, *, estimate: float,
           force: bool = False) -> tuple[int, str]:
    from locus.agent import claude

    if not requests:
        return 0, ""
    ok, why = claude.credit_allows(estimate)
    if not ok and not force:
        return 0, f"not submitted: {why}"
    bid = claude.batch_submit(requests)
    with conn:
        conn.execute("INSERT INTO batches(id, job, model, created, items) VALUES (?,?,?,?,?)",
                     (bid, job, model, _now(), json.dumps(meta)))
    return len(requests), f"batch {bid}"


def run_facts(conn: sqlite3.Connection, *, since: str = SINCE_DEFAULT, limit: int | None = None,
              force: bool = False, gate_conn=None, submit_new: bool = True) -> RunReport:
    from locus.agent.claude import BatchRequest

    ensure(conn)
    rep = RunReport(applied=Applied())

    def apply(cid, thread, text):
        msgs = thread_messages(conn, thread)
        a = apply_facts(conn, msgs, text, gate_conn=gate_conn)
        for k in vars(a):
            setattr(rep.applied, k, getattr(rep.applied, k) + getattr(a, k))

    rep.collected = collect(conn, "facts", apply)
    rep.calendar = calendar_facts(conn, since=since)
    if not submit_new:
        return rep
    system = system_prompt()
    reqs, meta, reads = [], {}, []
    for i, thread in enumerate(threads_to_read(conn, since=since)):
        if limit and len(reqs) >= limit:
            break
        msgs = thread_messages(conn, thread)
        upto = conn.execute("SELECT MAX(at_ts) FROM items WHERE parent=?", (thread,)).fetchone()[0]
        reads.append((thread, upto))
        if not worth_reading(msgs):
            rep.skipped_threads += 1
            continue
        cid = f"t{i}"
        meta[cid] = thread
        reqs.append(BatchRequest(custom_id=cid, model=FACT_MODEL, system=system,
                                 user=user_prompt(conn, msgs, date.today()), max_tokens=3000, schema=SCHEMA))
    # ~4k tokens in and ~300 out a thread on Sonnet 5.5, at the batch price.
    estimate = len(reqs) * (4000 * 2.0 + 300 * 10.0) / 1e6 * 0.5
    rep.submitted, rep.note = submit(conn, "facts", FACT_MODEL, reqs, meta, estimate=estimate, force=force)
    # A thread is marked read once it went in a batch, or needed none. When the credit refused
    # the batch, the threads that needed reading stay unread and the next run tries again.
    sent = set(meta.values())
    mark = [(t, u) for t, u in reads if rep.submitted or t not in sent]
    with conn:
        conn.executemany("INSERT OR REPLACE INTO fact_reads(thread, upto_ts, at) VALUES (?,?,?)",
                         [(t, u, _now()) for t, u in mark])
    return rep
