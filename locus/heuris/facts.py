"""The ledger of what is true now (heuris H2.1, D1): `subject · slot = value`, as of a date, with
its sources. A newer fact replaces an older one under the same slot and keeps it as history.

**Why slots and not free text.** Two phrasings of one fact both look current to a similarity
match, which is how a chat came to think he was still waiting on Jump after he had said otherwise
elsewhere (heuris A). A slot from a fixed list (`taxonomy/fact-slots.toml`) means "Millennium's
stage" is always the same key, so a newer value FINDS the one it replaces. A slot not on the list
is refused with the nearest ones (his call, 11 Nov: "every fact worth remembering must have a
slot"), so a new kind of fact gets a slot on purpose.

**This is a source of truth, not derived data.** Everything else in heuris.db is rebuilt by
re-indexing (invariant 3); a fact he approved in a chat cannot be. So the ledger's tables are
never dropped by a schema rebuild (`db.connect`), and `locus backup` copies them.

**Newer means newer AS OF, not newer written.** The backfill reads August's mail after a chat in
November has stated the current stage; writing the August fact as current would undo the
November one. A fact older than the current one is kept as history and does not replace it.

**States.** current · replaced (history: a newer one took its place) · retired (`forget`: it was
wrong) · proposed (waits in Approve: a conflict, or not from a sender of authority, D2) ·
rejected (he said no; the same value is never proposed again for that slot) · approved (a
proposal he said yes to: kept for the record, its value written as a new current fact).
"""

from __future__ import annotations

import difflib
import json
import re
import sqlite3
import tomllib
from dataclasses import dataclass
from datetime import date, datetime, timezone
from functools import lru_cache
from pathlib import Path

from locus.heuris import registry

SLOTS_PATH = Path(__file__).resolve().parents[2] / "taxonomy" / "fact-slots.toml"

LEDGER_SCHEMA = """
CREATE TABLE IF NOT EXISTS facts (
    id        INTEGER PRIMARY KEY,
    subject   TEXT NOT NULL,            -- me, a registry id, or person:<name>
    slot      TEXT NOT NULL,            -- stage, or a named slot with its name: offer:pay
    value     TEXT NOT NULL,
    as_of     TEXT NOT NULL,            -- YYYY-MM-DD: when it became true, or was stated
    sources   TEXT NOT NULL,            -- JSON list: pointers, or "you said so in chat ..."
    quote     TEXT,                     -- the sentence it rests on, verified in the source (D2)
    route     TEXT NOT NULL,            -- chat, home, mail, calendar, import
    status    TEXT NOT NULL,            -- current, replaced, retired, proposed, rejected
    replaces  INTEGER,                  -- the fact that was current when this one was written
    why       TEXT,                     -- why it waits in Approve
    created   TEXT NOT NULL,
    changed   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS facts_key ON facts(subject, slot, status);
CREATE INDEX IF NOT EXISTS facts_status ON facts(status);

-- Every change to the ledger, so an automatic one can be undone and none is silent.
CREATE TABLE IF NOT EXISTS fact_log (
    id       INTEGER PRIMARY KEY,
    at       TEXT NOT NULL,
    action   TEXT NOT NULL,             -- write, history, confirm, propose, approve, reject, forget, undo
    fact_id  INTEGER NOT NULL,
    route    TEXT NOT NULL,
    detail   TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS fact_log_at ON fact_log(at);
"""

NAME_RE = re.compile(r"^[a-z0-9][a-z0-9_\-]{0,47}$")
ROUTES = ("chat", "home", "mail", "calendar", "import")


class FactError(ValueError):
    """A fact that cannot be stored as given; the message says what would be accepted."""


@dataclass
class Fact:
    id: int
    subject: str
    slot: str
    value: str
    as_of: str
    sources: list[str]
    quote: str | None
    route: str
    status: str
    replaces: int | None
    why: str | None
    created: str
    changed: str

    @classmethod
    def of(cls, row) -> "Fact":
        r = tuple(row)
        return cls(r[0], r[1], r[2], r[3], r[4], json.loads(r[5] or "[]"), r[6], r[7], r[8], r[9], r[10], r[11], r[12])

    def line(self) -> str:
        src = "; ".join(self.sources[:3]) + (f"; +{len(self.sources) - 3} more" if len(self.sources) > 3 else "")
        return f"{self.subject} · {self.slot} = {self.value}  (as of {self.as_of}; {src}) [fact:{self.id}]"


_COLS = "id, subject, slot, value, as_of, sources, quote, route, status, replaces, why, created, changed"


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


# ---------------------------------------------------------------------------------------------
# slots and subjects


@lru_cache(maxsize=1)
def slot_table(path: Path = SLOTS_PATH) -> dict[str, dict[str, list[str]]]:
    data = tomllib.loads(path.read_text())
    return {k: {"single": list(v.get("single", [])), "named": list(v.get("named", []))} for k, v in data.items()}


def subject_kind(subject: str) -> str:
    """The slot table a subject uses, or FactError naming what a subject can be."""
    if subject == "me":
        return "me"
    if subject.startswith("person:") and NAME_RE.match(subject[7:]):
        return "person"
    e = registry.entries().get(subject)
    if e:
        kind = e.get("kind", "repo")
        return kind if kind in slot_table() else "repo"
    raise FactError(f"unknown subject {subject!r}: use `me`, `person:<name>` (lowercase, hyphens), or a "
                    f"registry id: {', '.join(sorted(registry.entries()))}.")


def normalise_subject(subject: str) -> str:
    s = subject.strip().lower()
    if s.startswith("person:"):
        s = "person:" + re.sub(r"[^a-z0-9]+", "-", s[7:]).strip("-")
    return s


def check_slot(subject: str, slot: str) -> tuple[str, str]:
    """(subject, slot) as stored, or FactError with the nearest slots this subject has."""
    subject = normalise_subject(subject)
    kind = subject_kind(subject)
    table = slot_table()[kind]
    slot = slot.strip().lower()
    base, _, name = slot.partition(":")
    if base in table["single"] and not name:
        return subject, base
    if base in table["named"]:
        if not name:
            raise FactError(f"`{base}` holds many values, each under a short name: `{base}:<name>`, e.g. "
                            f"`{base}:{'pay' if base == 'offer' else 'brian-advice' if base == 'note' else 'main'}`.")
        name = re.sub(r"\s+", "-", name.strip())
        if not NAME_RE.match(name):
            raise FactError(f"the name in `{base}:{name}` must be short, lowercase letters, digits and hyphens.")
        return subject, f"{base}:{name}"
    every = table["single"] + [f"{n}:<name>" for n in table["named"]]
    near = difflib.get_close_matches(base, table["single"] + table["named"], n=3, cutoff=0.4)
    raise FactError(f"`{slot}` is not a slot for a {kind}. Nearest: "
                    f"{', '.join(near) if near else 'none close'}. All: {', '.join(every)}. "
                    "A fact that fits none of them goes under `note:<name>`; a new kind of fact needs a "
                    "slot added to taxonomy/fact-slots.toml.")


def _norm_value(v: str) -> str:
    return re.sub(r"\s+", " ", v).strip().casefold().rstrip(".")


def _as_of(v: str | date | None) -> str:
    if v is None or v == "":
        return date.today().isoformat()
    if isinstance(v, date):
        return v.isoformat()
    try:
        return date.fromisoformat(str(v)[:10]).isoformat()
    except ValueError:
        raise FactError(f"as_of {v!r} is not a date: YYYY-MM-DD.") from None


# ---------------------------------------------------------------------------------------------
# reading


def get(conn: sqlite3.Connection, fact_id: int) -> Fact | None:
    r = conn.execute(f"SELECT {_COLS} FROM facts WHERE id=?", (fact_id,)).fetchone()
    return Fact.of(r) if r else None


def current(conn: sqlite3.Connection, subject: str | None = None, slot: str | None = None) -> list[Fact]:
    where, params = ["status='current'"], []
    if subject:
        subject = normalise_subject(subject)
        subject_kind(subject)
        where.append("subject=?")
        params.append(subject)
    if slot:
        s = slot.strip().lower()
        if ":" in s:
            where.append("slot=?")
            params.append(s)
        else:  # a named slot's base finds every name under it
            where.append("(slot=? OR slot LIKE ?)")
            params += [s, f"{s}:%"]
    return [Fact.of(r) for r in conn.execute(
        f"SELECT {_COLS} FROM facts WHERE {' AND '.join(where)} ORDER BY subject, slot", params)]


def history(conn: sqlite3.Connection, subject: str, slot: str) -> list[Fact]:
    """Every value the slot has held or been offered, newest as-of first."""
    subject = normalise_subject(subject)
    kind = subject_kind(subject)  # an unknown subject is named, never answered with "nothing recorded"
    s = slot.strip().lower()
    if s.split(":")[0] not in slot_table()[kind]["single"] + slot_table()[kind]["named"]:
        check_slot(subject, s)  # raises with the nearest slots
    clause, params = ("slot=?", [s]) if ":" in s else ("(slot=? OR slot LIKE ?)", [s, f"{s}:%"])
    return [Fact.of(r) for r in conn.execute(
        f"SELECT {_COLS} FROM facts WHERE subject=? AND {clause} ORDER BY slot, as_of DESC, id DESC",
        [subject, *params])]


def proposed(conn: sqlite3.Connection) -> list[Fact]:
    return [Fact.of(r) for r in conn.execute(f"SELECT {_COLS} FROM facts WHERE status='proposed' ORDER BY id")]


def _current_one(conn, subject: str, slot: str) -> Fact | None:
    r = conn.execute(f"SELECT {_COLS} FROM facts WHERE subject=? AND slot=? AND status='current' "
                     "ORDER BY id DESC LIMIT 1", (subject, slot)).fetchone()
    return Fact.of(r) if r else None


def _log(conn, action: str, fact_id: int, route: str, detail: str = "") -> int:
    return conn.execute("INSERT INTO fact_log(at, action, fact_id, route, detail) VALUES (?,?,?,?,?)",
                        (_now(), action, fact_id, route, detail)).lastrowid


# ---------------------------------------------------------------------------------------------
# writing


@dataclass
class Written:
    fact: Fact
    action: str            # write (now current), history (older than current), confirm (same value), propose
    replaced: Fact | None = None
    added_source: bool = False

    def describe(self) -> str:
        f = self.fact
        if self.action == "confirm":
            return f"Already true: {f.line()}." + (" Source added." if self.added_source else "")
        if self.action == "history":
            cur = self.replaced
            return (f"Kept as history: {f.line()}. It is older than the current value"
                    + (f" ({cur.value}, as of {cur.as_of})" if cur else "") + ", which stays.")
        if self.action == "propose":
            return f"Proposed in Approve ({f.why}): {f.line()}"
        was = f"; it replaces {self.replaced.value!r} (as of {self.replaced.as_of})" if self.replaced else ""
        return f"Remembered: {f.line()}{was}."


def write(conn: sqlite3.Connection, subject: str, slot: str, value: str, *, sources: list[str], route: str,
          as_of: str | date | None = None, quote: str | None = None) -> Written:
    """Store a fact as current, or as history when an as-newer value already holds the slot."""
    subject, slot = check_slot(subject, slot)
    value = (value or "").strip()
    if not value:
        raise FactError("a fact needs a value.")
    sources = [s.strip() for s in sources if s and s.strip()]
    if not sources:
        raise FactError("a fact is stored only with a source: a pointer, or where he said it.")
    if route not in ROUTES:
        raise FactError(f"route is one of {', '.join(ROUTES)}.")
    as_of = _as_of(as_of)
    now = _now()
    with conn:
        cur = _current_one(conn, subject, slot)
        if cur and _norm_value(cur.value) == _norm_value(value):
            merged = cur.sources + [s for s in sources if s not in cur.sources]
            # The same value from an earlier-dated source was already true then (Millennium:
            # imported as of the day he told locus, accepted by mail eight days before), unless
            # another value sits between the two dates.
            between = conn.execute("SELECT 1 FROM facts WHERE subject=? AND slot=? AND status='replaced' "
                                   "AND as_of >= ? AND as_of < ?", (subject, slot, as_of, cur.as_of)).fetchone()
            new_as_of = as_of if as_of < cur.as_of and not between else cur.as_of
            conn.execute("UPDATE facts SET sources=?, quote=COALESCE(quote, ?), as_of=?, changed=? WHERE id=?",
                         (json.dumps(merged), quote, new_as_of, now, cur.id))
            _log(conn, "confirm", cur.id, route, json.dumps(sources))
            return Written(get(conn, cur.id), "confirm", added_source=len(merged) > len(cur.sources))
        older = cur is not None and as_of < cur.as_of
        fid = conn.execute(
            f"INSERT INTO facts(subject, slot, value, as_of, sources, quote, route, status, replaces, created, changed) "
            f"VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            (subject, slot, value, as_of, json.dumps(sources), quote, route,
             "replaced" if older else "current", None if older else (cur.id if cur else None), now, now)).lastrowid
        if older:
            _log(conn, "history", fid, route)
            return Written(get(conn, fid), "history", cur)
        if cur:
            conn.execute("UPDATE facts SET status='replaced', changed=? WHERE id=?", (now, cur.id))
        _log(conn, "write", fid, route, json.dumps({"replaced": cur.id if cur else None}))
        return Written(get(conn, fid), "write", cur)


def propose(conn: sqlite3.Connection, subject: str, slot: str, value: str, *, sources: list[str], route: str,
            why: str, as_of: str | date | None = None, quote: str | None = None) -> Written | None:
    """A fact for Approve. None when it is already true, already waiting, or he said no to it."""
    subject, slot = check_slot(subject, slot)
    as_of = _as_of(as_of)
    cur = _current_one(conn, subject, slot)
    if cur and _norm_value(cur.value) == _norm_value(value):
        return None
    for r in conn.execute("SELECT value, status FROM facts WHERE subject=? AND slot=? AND status IN "
                          "('proposed','rejected')", (subject, slot)):
        if _norm_value(r[0]) == _norm_value(value):
            return None
    now = _now()
    with conn:
        fid = conn.execute(
            "INSERT INTO facts(subject, slot, value, as_of, sources, quote, route, status, why, created, changed) "
            "VALUES (?,?,?,?,?,?,?,'proposed',?,?,?)",
            (subject, slot, value.strip(), as_of, json.dumps(sources), quote, route, why, now, now)).lastrowid
        _log(conn, "propose", fid, route, why)
    return Written(get(conn, fid), "propose", cur)


def approve(conn: sqlite3.Connection, fact_id: int) -> Written:
    f = get(conn, fact_id)
    if not f or f.status != "proposed":
        raise FactError(f"fact:{fact_id} is not waiting for approval.")
    w = write(conn, f.subject, f.slot, f.value, sources=f.sources, route=f.route, as_of=f.as_of, quote=f.quote)
    with conn:
        conn.execute("UPDATE facts SET status='approved', changed=? WHERE id=?", (_now(), fact_id))
        _log(conn, "approve", w.fact.id, "home", json.dumps({"proposal": fact_id}))
    return w


def reject(conn: sqlite3.Connection, fact_id: int) -> Fact:
    f = get(conn, fact_id)
    if not f or f.status != "proposed":
        raise FactError(f"fact:{fact_id} is not waiting for approval.")
    with conn:
        conn.execute("UPDATE facts SET status='rejected', changed=? WHERE id=?", (_now(), fact_id))
        _log(conn, "reject", fact_id, "home")
    return get(conn, fact_id)


def forget(conn: sqlite3.Connection, fact_id: int, *, route: str = "chat", action: str = "forget") -> tuple[Fact, Fact | None]:
    """Retire a wrong current fact and restore the one it replaced, if any (heuris B)."""
    f = get(conn, fact_id)
    if not f:
        raise FactError(f"no fact:{fact_id}.")
    if f.status != "current":
        raise FactError(f"fact:{fact_id} is {f.status}, not current: only a current fact can be forgotten.")
    now = _now()
    restored = None
    with conn:
        conn.execute("UPDATE facts SET status='retired', changed=? WHERE id=?", (now, fact_id))
        prev = get(conn, f.replaces) if f.replaces else None
        if prev is None or prev.status != "replaced":
            r = conn.execute(f"SELECT {_COLS} FROM facts WHERE subject=? AND slot=? AND status='replaced' "
                             "ORDER BY as_of DESC, id DESC LIMIT 1", (f.subject, f.slot)).fetchone()
            prev = Fact.of(r) if r else None
        if prev:
            conn.execute("UPDATE facts SET status='current', changed=? WHERE id=?", (now, prev.id))
            restored = get(conn, prev.id)
        _log(conn, action, fact_id, route, json.dumps({"restored": prev.id if prev else None}))
    return get(conn, fact_id), restored


def automatic_log(conn: sqlite3.Connection, days: int = 14) -> list[dict]:
    """Facts written without asking (mail, calendar), newest first, for Approve's `l` with undo."""
    rows = conn.execute(
        "SELECT l.id, l.at, l.action, l.fact_id, l.route FROM fact_log l WHERE l.route IN ('mail','calendar') "
        "AND l.action='write' AND l.at >= datetime('now', ?) ORDER BY l.id DESC", (f"-{days} days",)).fetchall()
    out = []
    for lid, at, action, fid, route in rows:
        f = get(conn, fid)
        if f:
            out.append({"log": lid, "at": at, "route": route, "fact": f.id, "line": f.line(), "status": f.status,
                        "undone": f.status == "retired"})
    return out


# ---------------------------------------------------------------------------------------------
# rendering


def render(facts: list[Fact], *, empty: str = "No current facts.") -> str:
    if not facts:
        return empty
    return "\n".join(f.line() for f in facts)


def render_history(facts: list[Fact]) -> str:
    if not facts:
        return "Nothing recorded for that slot."
    out = []
    for f in facts:
        tag = {"current": "CURRENT", "replaced": "replaced", "retired": "retired (wrong)",
               "proposed": "proposed, waiting", "rejected": "rejected"}.get(f.status, f.status)
        out.append(f"{tag:17} {f.line()}" + (f"\n                  replaced fact:{f.replaces}" if f.replaces else ""))
    return "\n".join(out)


# ---------------------------------------------------------------------------------------------
# the applications file (D3): imported once, then retired


APPLICATIONS_SOURCE = "his applications file (~/files/Careers/applications.toml)"


def import_applications(conn: sqlite3.Connection, path: Path) -> list[Written]:
    """Each entry's stage, turn and note as facts, dated by its `since`. Idempotent: a second run
    finds every value already true and only confirms it."""
    data = tomllib.loads(path.read_text())
    out = []
    for app, e in data.items():
        if not isinstance(e, dict):
            continue
        as_of = e.get("since")
        # `cv` is a repo kept in the same file: its stage is its `state`.
        kind = subject_kind(normalise_subject(app))
        stage = "stage" if "stage" in slot_table()[kind]["single"] else "state"
        for slot, key in ((stage, "stage"), ("turn", "turn"), ("note:from-file", "note")):
            if e.get(key):
                out.append(write(conn, app, slot, str(e[key]), sources=[APPLICATIONS_SOURCE], route="import",
                                 as_of=as_of))
    return out
