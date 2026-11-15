"""Syllabi as structure (spec P, step P8 second half; P-D9).

Each submodule's `syllabus.md` (written verbatim by `locus canvas syllabus`, P8 first half) lives
in `~/files/Studies/<course>/<module>/` on the laptop; `locus heuris-push` sends them to
`data/heuris-in/syllabi/` (`syllabi_push.py`). Here, on the server:

- **Indexed** as heuris items, kind `syllabus`, ref `syllabus:<course>/<module>`, the course as
  their thing, so search finds what a course covers (Q31: "what did I study in years 1 and 2
  that is relevant to stochastic calculus?").
- **Read into topics** (`syllabus` table): one row per topic, each with a QUOTE of the page. A
  topic whose quote is not in the page (whitespace and case aside) is dropped: grounded or
  silent (invariant 1), counted in `locus gates` as `heuris.syllabus_quote`. A file is read again
  only when its text changes; the old rows are kept as history (`status='old'`), so a topic
  Canvas dropped can still be shown as changed.
- **Linked to the material** (`syllabus_links`): each topic searched within its course over the
  library's sections (lectures, sheets, his notes), the best section per document reranked by
  the cross-encoder, kept only above `LINK_FLOOR`. A topic with nothing above it is UNCOVERED,
  never a weak match. The department's own syllabus documents are never material.
  Links are recomputed for a course when its library items change, or when its topics do.
- **Facts are only the summary**: one `education:<course>` fact per course (which submodules,
  which year), PROPOSED, since it is about him; the topics are the course's, not facts.

Not derived in the invariant-3 sense: the topics cost model calls, and their history is the point.
So `syllabus` and `syllabus_links` are not index tables and `drop_index` keeps them.
"""

from __future__ import annotations

import hashlib
import json
import re
import sqlite3
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Callable, Iterator

from pydantic import BaseModel

from locus.heuris import registry
from locus.heuris.index import Item

SCHEMA = """
CREATE TABLE IF NOT EXISTS syllabus (
    id        INTEGER PRIMARY KEY,
    course    TEXT NOT NULL,          -- registry id
    module    TEXT NOT NULL,          -- the submodule's folder
    ord       INTEGER NOT NULL,
    topic     TEXT NOT NULL,          -- as the page puts it
    quote     TEXT NOT NULL,          -- the line of the page it rests on
    source    TEXT NOT NULL,          -- syllabus:<course>/<module>
    file_hash TEXT NOT NULL,
    status    TEXT NOT NULL,          -- current | old
    extracted TEXT NOT NULL,
    linked    TEXT,                   -- the course library's state when last linked
    best      REAL                    -- its best link score, kept or not: the floor's evidence
);
CREATE INDEX IF NOT EXISTS syllabus_mod ON syllabus(course, module, status);
CREATE TABLE IF NOT EXISTS syllabus_links (
    topic_id INTEGER NOT NULL REFERENCES syllabus(id) ON DELETE CASCADE,
    ref      TEXT NOT NULL,           -- src:<doc>#s<n>, the best section of that document
    doc      INTEGER,
    title    TEXT NOT NULL,
    score    REAL NOT NULL,
    PRIMARY KEY (topic_id, ref)
);
"""

# Cross-encoder logit a link must clear. The library's own floor (config `[retrieve]
# min_rerank_score`, 0.22, calibrated 5 Jun on library queries); checked against this task's own
# distribution on the first run (P8 Progress in the spec).
LINK_FLOOR = 0.22
LINKS_PER_TOPIC = 3
# The department's syllabus and curriculum PDFs are in the library too, and match every topic.
_NOT_MATERIAL = re.compile(r"syllabus|curriculum", re.I)
YEAR = {"1st": "Year 1 (Prelims)", "2nd": "Year 2 (Part A)", "3rd": "Year 3 (Part B)", "4th": "Year 4 (Part C)"}


@dataclass
class Syllabus:
    course: str
    module: str
    head: dict
    body: str
    rel: str = ""

    @property
    def ref(self) -> str:
        return f"syllabus:{self.course}/{self.module}"

    @property
    def hash(self) -> str:
        return hashlib.sha256(self.body.encode()).hexdigest()

    @property
    def name(self) -> str:
        return self.head.get("canvas_module") or self.module.replace("-", " ")


def parse(path: Path, root: Path) -> Syllabus | None:
    text = path.read_text(errors="replace")
    head: dict = {}
    body = text
    if text.startswith("---"):
        end = text.find("\n---", 3)
        if end > 0:
            for line in text[3:end].splitlines():
                k, _, v = line.partition(":")
                if k.strip():
                    head[k.strip()] = v.strip()
            body = text[end + 4:]
    # The note above the verbatim text is ours, not the department's.
    body = re.sub(r"^\s*The department's syllabus for this submodule, word for word\.[^\n]*\n", "", body).strip()
    course = head.get("course") or path.parent.parent.name.split("-")[0]
    if not body:
        return None
    return Syllabus(course=course, module=path.parent.name, head=head, body=body,
                    rel=str(path.relative_to(root)))


def read_all(root: Path) -> list[Syllabus]:
    out = []
    for p in sorted(root.rglob("syllabus.md")):
        s = parse(p, root)
        if s:
            out.append(s)
    return out


def items(root: Path) -> Iterator[Item]:
    """Each syllabus as a heuris item, so search finds what a course covers."""
    for s in read_all(root):
        e = registry.entries().get(s.course, {})
        edition = s.head.get("edition", "")
        yield Item(
            ref=s.ref, kind="syllabus",
            at=_edition_start(edition),
            title=f"{e.get('name', s.course.upper())} · {s.name} · syllabus ({edition or 'undated'})",
            text=s.body, area="Studies", thing=s.course if e else None,
            meta={"course": s.course, "module": s.module, "edition": edition, "source": s.head.get("source"),
                  "file": f"~/files/Studies/{s.rel}"},
        )


def _edition_start(edition: str) -> datetime:
    """'2nd Year Syllabus 2025-26' -> 1 Oct 2025: the year he studied it, for date filters."""
    m = re.search(r"(20\d\d)-\d\d", edition)
    return datetime(int(m.group(1)) if m else 1970, 10, 1, tzinfo=timezone.utc)


# ── topics ─────────────────────────────────────────────────────────────────────────────────────

class _Topic(BaseModel):
    topic: str
    quote: str


class _Topics(BaseModel):
    topics: list[_Topic]


PROMPT = """Below is one submodule's syllabus from an engineering degree, copied word for word from the department.

List the topics it says the course covers, in the order it gives them: every subject named in its
description and every learning outcome. Split a list of several subjects into one topic each where they
are distinct things to learn ("Butterworth filter" and "the z-transform" are two topics). Skip the
lecture and sheet counts, accreditation codes like [AHEP 4.0 - M12], and general aims ("demonstrate
what can be modelled").

For each topic give:
- "topic": a short name for it, in the syllabus's own words where possible (at most 12 words);
- "quote": the words of the syllabus that state it, copied EXACTLY, character for character, from a single
  place in the text below (a phrase is enough; do not join pieces from different places, do not fix typos).

Reply with JSON only: {"topics": [{"topic": "...", "quote": "..."}]}

Syllabus:
<<<
%s
>>>"""


def _norm(s: str) -> str:
    s = s.replace("–", "-").replace("—", "-").replace("’", "'").replace("‘", "'")
    s = s.replace("“", '"').replace("”", '"').replace("•", " ")
    return re.sub(r"\s+", " ", s).strip().lower()


def grounded(topics: list[_Topic], body: str, gate_conn=None) -> list[tuple[str, str]]:
    """Topics whose quote is in the page, whitespace, case and typographic dashes aside."""
    from locus.observe import gates

    page = _norm(body)
    out, seen = [], set()
    for t in topics:
        q, name = t.quote.strip(), t.topic.strip()
        ok = len(_norm(q)) >= 4 and _norm(q) in page and bool(name)
        if gate_conn is not None:
            gates.record(gate_conn, "heuris.syllabus_quote", rejected=not ok, value=q[:160])
        if ok and _norm(name) not in seen:
            seen.add(_norm(name))
            out.append((name, q))
    return out


Extract = Callable[[str], _Topics]


def _extract(body: str) -> _Topics:
    from locus.agent import claude

    return claude.run_structured(PROMPT % body, schema=_Topics, model="sonnet")


def store(conn: sqlite3.Connection, s: Syllabus, topics: list[tuple[str, str]]) -> None:
    now = datetime.now(timezone.utc).isoformat(timespec="seconds")
    with conn:
        conn.execute("UPDATE syllabus SET status='old' WHERE course=? AND module=? AND status='current'",
                     (s.course, s.module))
        conn.executemany(
            "INSERT INTO syllabus(course, module, ord, topic, quote, source, file_hash, status, extracted) "
            "VALUES (?,?,?,?,?,?,?,'current',?)",
            [(s.course, s.module, i, t, q, s.ref, s.hash, now) for i, (t, q) in enumerate(topics, 1)])


@dataclass
class Report:
    read: int = 0
    extracted: list[str] = field(default_factory=list)
    dropped: int = 0
    failed: list[str] = field(default_factory=list)
    linked: int = 0
    covered: int = 0
    proposed: list[str] = field(default_factory=list)
    scores: list[float] = field(default_factory=list)   # every topic's best score, for the floor


def ensure(conn: sqlite3.Connection) -> None:
    conn.executescript(SCHEMA)
    cols = {r[1] for r in conn.execute("PRAGMA table_info(syllabus)")}
    if "best" not in cols:      # the table as first deployed, 15 Nov
        conn.execute("ALTER TABLE syllabus ADD COLUMN best REAL")


def extract_changed(conn: sqlite3.Connection, sylls: list[Syllabus], *, extract: Extract = _extract,
                    gate_conn=None, rep: Report | None = None) -> Report:
    """Read topics from each syllabus whose text changed since it was last read."""
    rep = rep or Report()
    ensure(conn)
    for s in sylls:
        rep.read += 1
        have = conn.execute("SELECT file_hash FROM syllabus WHERE course=? AND module=? AND status='current' "
                            "LIMIT 1", (s.course, s.module)).fetchone()
        if have and have[0] == s.hash:
            continue
        try:
            got = extract(s.body)
        except Exception as exc:  # one file failing never stops the rest; it is tried next run
            rep.failed.append(f"{s.ref}: {exc}"[:200])
            continue
        kept = grounded(got.topics, s.body, gate_conn)
        rep.dropped += len(got.topics) - len(kept)
        if not kept:
            rep.failed.append(f"{s.ref}: no grounded topics")
            continue
        store(conn, s, kept)
        rep.extracted.append(s.ref)
    return rep


# ── links ──────────────────────────────────────────────────────────────────────────────────────

Score = Callable[[str, list[str]], list[float]]


def _score(query: str, texts: list[str]) -> list[float]:
    from locus.retrieve.rerank import score_pairs

    return score_pairs(query, texts)


def _course_changed(conn: sqlite3.Connection, course: str) -> str:
    """The state of a course's library: newest write and count, so a section added in the same
    second as the last linking, or one removed, still counts as a change."""
    t, n = conn.execute("SELECT COALESCE(MAX(indexed_at), ''), COUNT(*) FROM items WHERE source='library' "
                        "AND thing=?", (course,)).fetchone()
    return f"{t}|{n}"


def link(conn: sqlite3.Connection, *, embed=None, score: Score = _score, everything: bool = False,
         rep: Report | None = None) -> Report:
    """Link each current topic to the best sections of its course's library documents."""
    from locus.heuris.index import QUERY_PREFIX
    from locus.heuris.tools import heuris_candidates

    rep = rep or Report()
    if embed is None:
        from locus.ingest.embed import embed_texts as embed
    ensure(conn)
    rows = conn.execute("SELECT id, course, topic, quote, linked FROM syllabus WHERE status='current' "
                        "ORDER BY course, module, ord").fetchall()
    changed: dict[str, str] = {}
    for tid, course, topic, quote, linked in rows:
        if course not in changed:
            changed[course] = _course_changed(conn, course)
        if not everything and linked == changed[course]:
            continue
        query = topic if _norm(quote) in _norm(topic) else f"{topic}: {quote}"
        qvec = embed([QUERY_PREFIX + query])[0]
        hits = [h for h in heuris_candidates(conn, query, qvec, thing=course, k=60)
                if h.kind in ("source", "note") and h.ref.startswith("src:")
                and not _NOT_MATERIAL.search(h.meta.get("doc_title") or h.title)]
        # The nearest sections first, at most 30 to the cross-encoder.
        hits = sorted(hits, key=lambda h: h.score, reverse=True)[:30]
        scored = list(zip(score(query, [f"{h.title}\n{h.text[:1500]}" for h in hits]), hits)) if hits else []
        best: dict[int, tuple[float, object]] = {}
        for sc, h in scored:
            doc = h.meta.get("doc")
            if doc not in best or sc > best[doc][0]:
                best[doc] = (sc, h)
        top = sorted(best.values(), key=lambda x: x[0], reverse=True)
        rep.scores.append(top[0][0] if top else float("-inf"))
        keep = [(sc, h) for sc, h in top if sc >= LINK_FLOOR][:LINKS_PER_TOPIC]
        with conn:
            conn.execute("DELETE FROM syllabus_links WHERE topic_id=?", (tid,))
            conn.executemany("INSERT OR REPLACE INTO syllabus_links(topic_id, ref, doc, title, score) VALUES (?,?,?,?,?)",
                             [(tid, h.ref, h.meta.get("doc"), h.title, float(sc)) for sc, h in keep])
            conn.execute("UPDATE syllabus SET linked=?, best=? WHERE id=?",
                         (changed[course], top[0][0] if top else None, tid))
        rep.linked += 1
        rep.covered += bool(keep)
    return rep


# ── education facts ────────────────────────────────────────────────────────────────────────────

def propose_education(conn: sqlite3.Connection, sylls: list[Syllabus], rep: Report | None = None) -> Report:
    """One `education:<course>` fact per course, proposed for Approve (it is about him)."""
    from locus.heuris import facts as F

    rep = rep or Report()
    by: dict[str, list[Syllabus]] = {}
    for s in sylls:
        by.setdefault(s.course, []).append(s)
    for course, ss in sorted(by.items()):
        e = registry.entries().get(course)
        if not e:
            continue
        edition = ss[0].head.get("edition", "")
        m = re.match(r"(\dst|\dnd|\drd|\dth) Year Syllabus (20\d\d-\d\d)", edition)
        when = f"{YEAR.get(m.group(1), m.group(1) + ' year')}, {m.group(2)}" if m else edition
        mods = ", ".join(sorted(s.name for s in ss))
        value = f"{e.get('name', course)} in {when}: {mods}"
        w = F.propose(conn, "me", f"education:{course}", value, sources=[s.ref for s in ss], route="syllabus",
                      why="from the department's syllabi of his courses (P8)", as_of=_edition_start(edition).date())
        if w:
            rep.proposed.append(course)
    return rep


# ── reading it back ────────────────────────────────────────────────────────────────────────────

def render(conn: sqlite3.Connection, course: str, module: str | None = None) -> str:
    ensure(conn)
    q = "SELECT id, module, ord, topic, quote, linked FROM syllabus WHERE status='current' AND course=?"
    args: list = [course]
    if module:
        q += " AND module=?"
        args.append(module)
    rows = conn.execute(q + " ORDER BY module, ord", args).fetchall()
    if not rows:
        return f"No syllabus topics for {course}{'/' + module if module else ''}."
    out, cur = [], None
    for tid, mod, _, topic, quote, linked in rows:
        if mod != cur:
            cur = mod
            out.append(f"\n{course} · {mod}  [syllabus:{course}/{mod}]")
        links = conn.execute("SELECT ref, title, score FROM syllabus_links WHERE topic_id=? ORDER BY score DESC",
                             (tid,)).fetchall()
        mark = "·" if links else ("✗" if linked else "?")
        out.append(f"  {mark} {topic}" + ("" if links else "   UNCOVERED" if linked else "   not linked yet"))
        out.append(f"      \"{' '.join(quote.split())}\"")
        for ref, title, sc in links:
            out.append(f"      {sc:5.1f}  {title[:90]}  [{ref}]")
    return "\n".join(out).lstrip("\n")


def summary(conn: sqlite3.Connection) -> list[dict]:
    """Per submodule: topics, linked so far, covered."""
    ensure(conn)
    return [{"course": c, "module": m, "topics": n, "linked": li, "covered": k} for c, m, n, li, k in conn.execute(
        "SELECT s.course, s.module, COUNT(*), SUM(s.linked IS NOT NULL), "
        "SUM(EXISTS(SELECT 1 FROM syllabus_links l WHERE l.topic_id=s.id)) "
        "FROM syllabus s WHERE s.status='current' GROUP BY 1, 2 ORDER BY 1, 2")]


class Busy(RuntimeError):
    pass


def _lock(path: Path):
    """One syllabus run at a time: the hourly unit and a long first linking by hand would
    otherwise both link the same topics. Non-blocking: a second run says so and stops."""
    import fcntl

    path.parent.mkdir(parents=True, exist_ok=True)
    f = open(path, "w")
    try:
        fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        f.close()
        raise Busy("another syllabus run holds the lock")
    return f


def run(conn: sqlite3.Connection, root: Path, *, extract: Extract = _extract, embed=None, score: Score = _score,
        relink: bool = False, propose: bool = True, gate_conn=None, lock: Path | None = None) -> Report:
    held = _lock(lock) if lock else None
    try:
        return _run(conn, root, extract=extract, embed=embed, score=score, relink=relink, propose=propose,
                    gate_conn=gate_conn)
    finally:
        if held:
            held.close()


def _run(conn, root, *, extract, embed, score, relink, propose, gate_conn) -> Report:
    sylls = read_all(root)
    rep = extract_changed(conn, sylls, extract=extract, gate_conn=gate_conn)
    link(conn, embed=embed, score=score, everything=relink, rep=rep)
    if propose:
        propose_education(conn, sylls, rep)
    return rep


def describe(rep: Report) -> str:
    s = (f"{rep.read} syllabi; topics read from {len(rep.extracted)} ({rep.dropped} topics dropped, quote not in "
         f"the page); {rep.linked} topics linked, {rep.covered} of them to material")
    if rep.proposed:
        s += f"; education proposed for {', '.join(rep.proposed)}"
    if rep.failed:
        s += "\n  failed: " + "\n  failed: ".join(rep.failed)
    return s


def to_json(rep: Report) -> str:
    return json.dumps(rep.__dict__, default=str)
