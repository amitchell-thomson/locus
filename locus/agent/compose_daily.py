"""The daily reMarkable page: four pages, one section each (daily-use refinement plan §2).

**Aggregates; does not compute.** Every item already exists somewhere in agent state — due review
rows, `reading_proposals`, marked passages, his own open threads, `related_documents` joins.
Nothing here calls a model. That is a boundary, not an economy: the page must render identically
whether or not last night's runs succeeded, and a composer that computed its own content could not
offer that. The written reasons it prints were composed and STORED earlier (at proposal time), so
prose and aggregate-only are not in tension.

WHAT THE REQUIREMENTS INTERVIEW CHANGED (2026-08-01/02), because each of these was a considered
design that the owner overruled:

**One section per physical page.** The old page was one scrolling document with six sections and
hard caps of 3/5/3/3/2/2. He asked instead for "one full page for each, each page should be well
laid out and look nice". The cap stops being an arbitrary number and becomes what fits — best
items fit, the rest wait (`_FIT`).

**Marks, threads and connections are ONE page.** "I do not want all these different sections that
are all very similar and confusingly labelled." They are all *something of yours, react to it*, so
they share a page, an anchor series and one action vocabulary. What they do NOT share is where
they came from, which was the actual missing information — hence three subsections named for
provenance (`From your reading` / `Still open` / `Connections found`), and only the
agent-originated one carries a tick box, so a tick means exactly one thing on the whole page.

**"Never linked" is a fact about the database, not a reason to care.** The old connection line
read "shares 3 concepts with a document you have not linked it to". He asked what that was
supposed to tell him. A connection now has to name what both documents actually develop — and if
it cannot name one, it does not appear.

**Blessings left the page entirely.** He wants every pending approval at once, cleared fast, in a
terminal TUI — never split across two surfaces. `build_blessings` is gone; `locus objects --bless`
carries it until the TUI lands (step 3). No decision may appear both here and there.

**Nothing is shown twice.** A page is built every morning regardless of whether the last was read,
so the no-repeat ledger (`daily_shown`, migration 0023) is what makes a skipped day lose nothing
and repeat nothing. `item_key` carries the item's VERSION, so a thread he developed returns and an
untouched one does not — see `_shown_keys`.

Longevity guardrails (§9, which still outrank any feature here): glanceable; empty is a valid calm
state (an empty subsection is omitted, an empty page is dropped); no guilt metrics — with ONE
deliberate exception he asked for, the reading queue's true state ("6 waiting, oldest 3 weeks"),
because there the count is the brake.
"""

from __future__ import annotations

import sqlite3
from dataclasses import dataclass, field
from datetime import date, datetime, timedelta, timezone

from locus.agent import state
from locus.learn import review as learn_review
from locus.link.related import related_documents

# What fits one reMarkable page at 11pt with room to write. Not a judgement about how much he
# should do — a typographic fact about the page, which is why it is expressed per section and in
# items rather than as one global cap.
# Derived from the geometry, not chosen: see `_lines_for`. A page holds ~9 ruled lines, and an
# item needs 2-3 to be answerable, so a WRITING page holds three or four items — not the six the
# first cut assumed, which silently pushed Recall onto a second page and broke the whole
# one-section-per-page layout. `read` is larger because a reading item carries one rule, not a
# writing region: it is a decision aid, not something he answers.
_FIT = {"read": 3, "think": 3, "recall": 4}

# The object types that represent an OPEN THREAD of his own — something he asked or proposed and
# has not finished with. Concepts and projects are not threads: they are things that exist.
_THREAD_TYPES = ("question", "idea")

# Categories whose documents are the owner's OWN recent capture — the "why now" that makes a
# connection worth surfacing today rather than any other day.
_CAPTURE_CATEGORIES = ("note",)

# A marked passage needs enough words to mean something on its own. Below this it is a stray
# stroke or a single word, which tells him nothing when it comes back a week later.
_MIN_PASSAGE_CHARS = 40

# Subsection headings on the Think page, in render order. Named for PROVENANCE — where the item
# came from — because that is the distinction the old labels failed to make.
SECTION_MARKED = "From your reading"
SECTION_OPEN = "Still open"
SECTION_CONNECTION = "Connections found"


@dataclass
class Anchor:
    """A numbered, writable region on the page and what it refers to.

    `anchor` is what is PRINTED (`D1`, `T2`, `R3`) and `kind` is how the pull-back routes it, and
    they are deliberately not the same thing: the three Think subsections share one printed series
    so he reads one list, while a mark, an open thread and a connection still route to three
    different places.
    """

    anchor: str          # 'D1' | 'T2' | 'R3' | 'Q1' — stable within a page date
    kind: str            # 'reading' | 'mark' | 'open' | 'connection' | 'recall' | 'question'
    target_kind: str     # 'review_item' | 'doc' | 'object' | 'proposal' | 'none'
    target_key: str      # stable string key, never a row id that a re-ingest changes
    label: str = ""


@dataclass
class ReadItem:
    """One thing to read, and why — page 1."""

    anchor: str
    title: str
    why: str                  # the written reason if one was composed, else the deterministic one
    grounding: str            # what it matched, so the prose is checkable
    shelf: str                # 'Proposed' | 're-read'
    target_kind: str
    target_key: str
    item_key: str = ""


@dataclass
class InProgress:
    """Something he is actually reading, and what it links to."""

    title: str
    links_to: str = ""      # 'regime-ml' — the project it sits closest to
    why: str = ""           # the written reason, when one has been composed
    marks: int = 0
    owner_added: bool = False


@dataclass
class ReadingState:
    """The shelf as it actually stands. The one place counts are printed, by request."""

    in_progress: tuple[InProgress, ...] = ()
    proposed: int = 0
    oldest_days: int | None = None

    @property
    def is_empty(self) -> bool:
        return not self.in_progress and not self.proposed


@dataclass
class ThreadItem:
    """Something of his to react to — page 2. One type for all three subsections."""

    anchor: str
    section: str              # SECTION_MARKED | SECTION_OPEN | SECTION_CONNECTION
    kind: str                 # routing kind: 'mark' | 'open' | 'connection'
    headline: str
    context: str              # provenance: where it came from and when
    body: tuple[str, ...] = ()  # his own prior words, oldest first
    tick: bool = False        # renders `[ ] keep` — connections only
    target_kind: str = "doc"
    target_key: str = ""
    item_key: str = ""


@dataclass
class Recall:
    anchor: str
    prompt: str
    source: str
    item_id: int
    answer: str = ""          # printed on the answers page, never beside the question
    item_key: str = ""


@dataclass
class Status:
    """One line at the foot of page 1: what the system produced, and loudly what is broken."""

    produced: tuple[str, ...] = ()
    failures: tuple[str, ...] = ()

    def render(self) -> str:
        made = " · ".join(self.produced) if self.produced else "nothing new"
        if not self.failures:
            return f"overnight: {made} · all runs healthy"
        return f"overnight: {made} · " + " · ".join(f.upper() for f in self.failures)


@dataclass
class DailyPage:
    page_date: str
    readings: list[ReadItem] = field(default_factory=list)
    reading_state: ReadingState = field(default_factory=ReadingState)
    threads: list[ThreadItem] = field(default_factory=list)
    recalls: list[Recall] = field(default_factory=list)
    status: Status = field(default_factory=Status)
    anchors: list[Anchor] = field(default_factory=list)

    @property
    def is_empty(self) -> bool:
        return not (
            self.readings or self.threads or self.recalls or not self.reading_state.is_empty
        )

    def items_shown(self) -> list[tuple[str, str]]:
        """(item_key, kind) for everything rendered — what `record_shown` writes to the ledger."""
        out = [(r.item_key, "reading") for r in self.readings]
        out += [(t.item_key, t.kind) for t in self.threads]
        out += [(r.item_key, "recall") for r in self.recalls]
        return [(k, kind) for k, kind in out if k]


def _utcnow() -> str:
    return datetime.now(timezone.utc).isoformat()


# --- the no-repeat ledger --------------------------------------------------------------------


def _shown_keys(conn: sqlite3.Connection) -> set[str]:
    """Every item_key already printed on some page.

    THE KEY CARRIES THE ITEM'S VERSION, not just its identity (`object:41:<updated_at>`, not
    `object:41`). That single decision is what makes one rule mean the right thing everywhere:

      - a thread he DEVELOPED has a new `updated_at`, so it comes back with his new words on it;
        an untouched one does not, which is the literal repetition he objected to;
      - a recall item re-scheduled by SM-2 has a new `due`, so spaced repetition still works —
        blocking it would have quietly disabled the whole review loop;
      - a proposal whose `why` was rewritten at the 7-day mark returns carrying NEW TEXT, which is
        the only circumstance in which a repeat is worth his attention.

    Degrades to "nothing shown yet" if the table is absent, so an un-migrated DB still renders.
    """
    try:
        return {r["item_key"] for r in conn.execute("SELECT item_key FROM daily_shown")}
    except sqlite3.OperationalError:
        return set()


def record_shown(conn: sqlite3.Connection, page: DailyPage) -> None:
    """Record what this page offered. Separate from `compose` so composing stays a pure read."""
    rows = [(k, kind, page.page_date, _utcnow()) for k, kind in page.items_shown()]
    if not rows:
        return
    with conn:
        conn.executemany(
            "INSERT INTO daily_shown (item_key, kind, page_date, shown_at) VALUES (?,?,?,?) "
            "ON CONFLICT(item_key) DO NOTHING",
            rows,
        )


# --- page 1: Read ------------------------------------------------------------------------------


def _days_since(stamp: str | None, *, now: datetime) -> int | None:
    if not stamp:
        return None
    try:
        parsed = datetime.fromisoformat(stamp)
    except ValueError:
        return None
    if not parsed.tzinfo:
        parsed = parsed.replace(tzinfo=timezone.utc)
    return max(0, (now - parsed).days)


def _proposal_grounding(row: sqlite3.Row) -> str:
    """The checkable facts under the prose: what it matched, and how it was found."""
    bits = []
    if row["why_kind"]:
        bits.append(str(row["why_kind"]).replace("_", " "))
    if row["evidence_key"]:
        bits.append(str(row["evidence_key"]).rsplit("/", 1)[-1])
    if row["year"]:
        bits.append(str(row["year"]))
    return " · ".join(bits)


def build_readings(
    conn: sqlite3.Connection, *, limit: int = _FIT["read"], seen: set[str] | None = None
) -> list[ReadItem]:
    """What to read next, from the discovery shelf — NOT from the corpus.

    THE SECTION THIS REPLACES WAS WORSE THAN EMPTY. It ranked corpus documents by how many open
    gaps a re-read would close, and offered him the Optibook Python reference — documentation for
    a trading competition he did a year ago. Meanwhile ten real papers sat in `Reading/Proposed`
    and `compose_daily` contained zero references to `reading_proposals`. The two halves of the
    system did not know about each other.

    Corpus re-reads are not merely deprioritised here, they are ABSENT: a re-read earns a slot
    only when it answers a passage he marked as not understanding, and that signal (the mark
    intent pass) is step 4. Offering a generic gap-closer in the meantime is exactly what
    discredited the section in the first place.

    Degrades silently if the discovery tables are absent — the reading loop is optional.
    """
    seen = seen if seen is not None else set()
    try:
        rows = conn.execute(
            "SELECT * FROM reading_proposals WHERE status='proposed' "
            "ORDER BY COALESCE(score, 0) DESC, id ASC LIMIT ?",
            (limit * 4,),
        ).fetchall()
    except sqlite3.OperationalError:
        return []

    out: list[ReadItem] = []
    for row in rows:
        keys = row.keys()
        why_long = (row["why_long"] if "why_long" in keys else None) or ""
        written = (row["why_written_at"] if "why_written_at" in keys else None) or ""
        # The version in the key is the reason's timestamp: a proposal returns to the page only
        # when its `why` has been rewritten, so a repeat always carries new text (plan §2).
        item_key = f"proposal:{row['id']}:{written}"
        if item_key in seen:
            continue
        out.append(
            ReadItem(
                anchor="",
                title=row["title"],
                why=" ".join(why_long.split()) or row["why"],
                grounding=_proposal_grounding(row),
                shelf="Proposed",
                target_kind="proposal",
                target_key=row["dedupe_key"],
                item_key=item_key,
            )
        )
        if len(out) >= limit:
            break
    return out


def build_rereads(
    conn: sqlite3.Connection, *, limit: int = 1, seen: set[str] | None = None
) -> list[ReadItem]:
    """A corpus document that answers a passage he said he did not follow.

    THE ONLY THING THAT EARNS A CORPUS RE-READ A SLOT. The old read-next ranked corpus documents
    by how many open gaps a re-read would close and offered him a year-old competition manual;
    a generic gap-closer is precisely what discredited the section. A re-read appears here only
    when he has marked a specific passage as not understood, which is a request rather than an
    inference — and it disappears the moment that mark is resolved, because the intent is read
    live rather than copied into a queue.

    Joins only: the passage is matched against section summaries through the ordinary retrieval
    arm, and if nothing in the corpus covers it the slot stays empty (the discovery channel is
    where "the corpus does not contain the answer" gets handled).
    """
    from locus.capture import intent as I

    seen = seen if seen is not None else set()
    out: list[ReadItem] = []
    for mark in I.not_understood_marks(conn, limit=limit * 6):
        question = I.reread_seed(mark)
        if not question:
            continue
        item_key = f"reread:{mark['id']}"
        if item_key in seen:
            continue
        target = _explains(conn, question, exclude_uri=mark["source_uri"])
        if target is None:
            continue
        title, uri = target
        out.append(
            ReadItem(
                anchor="",
                title=title,
                why=(
                    f"you marked p.{mark['pdf_page'] + 1} of "
                    f"{mark['doc_title'] or 'your reading'} as not following it"
                    + (f" — \u201c{question[:90]}\u201d" if question else "")
                ),
                grounding="re-read · from a passage you marked",
                shelf="re-read",
                target_kind="doc",
                target_key=uri,
                item_key=item_key,
            )
        )
        if len(out) >= limit:
            break
    return out


def _explains(
    conn: sqlite3.Connection, question: str, *, exclude_uri: str
) -> tuple[str, str] | None:
    """The corpus document that best covers `question`, or None. Degrades silently."""
    try:
        from locus.retrieve.search import search
    except ImportError:
        return None
    try:
        candidates = search(conn, question)
    except Exception:                                  # ollama down, no vectors yet
        return None

    # Best-scoring DOCUMENT, not unit: the slot offers something to read, and several strong
    # chunks from one document say the same thing about it.
    for cand in sorted(candidates or [], key=lambda c: -(c.rerank_score or c.score or 0.0)):
        row = conn.execute(
            "SELECT title, source_uri FROM documents WHERE id=?", (cand.doc_id,)
        ).fetchone()
        if row and row["source_uri"] != exclude_uri:
            return row["title"] or row["source_uri"], row["source_uri"]
    return None


def build_reading_state(
    conn: sqlite3.Connection, *, now: datetime | None = None
) -> ReadingState:
    """What is on the shelf right now — the one place the page prints counts.

    §9 forbids guilt metrics, and this is the deliberate exception the owner asked for: "full
    queue, and tell me the truth". The count is the brake. If `oldest 6 weeks` keeps growing while
    nothing moves, that is the signal to lower the cap — which only works if the number is printed
    rather than hidden behind the principle.
    """
    from locus.reading import relevance as R

    now = now or datetime.now(timezone.utc)
    try:
        rows = conn.execute(
            "SELECT status, proposed_at FROM reading_proposals WHERE status='proposed'"
        ).fetchall()
    except sqlite3.OperationalError:
        rows = []

    proposed = len(rows)
    oldest: int | None = None
    for row in rows:
        age = _days_since(row["proposed_at"], now=now)
        if age is not None:
            oldest = age if oldest is None else max(oldest, age)

    # IN-PROGRESS COMES FROM `reading_targets`, NOT FROM PROPOSALS. The first cut read proposals,
    # so the one document he was genuinely reading and annotating — a book he added himself, with
    # 26 marks on it — was the single thing this section left out.
    reading = [
        InProgress(
            title=t["title"] or R.title_for(t["device_path"], t["source_uri"]),
            links_to=t["subject_label"] or "",
            why=" ".join((t["why_long"] or "").split()),
            marks=t["marks"] or 0,
            owner_added=t["proposal_id"] is None,
        )
        for t in R.in_progress(conn)
    ]
    return ReadingState(
        in_progress=tuple(reading), proposed=proposed, oldest_days=oldest
    )


# --- page 2: Think -----------------------------------------------------------------------------


def _recent_capture(conn: sqlite3.Connection, *, limit: int) -> list[sqlite3.Row]:
    """The owner's most recently captured own-writing, newest first.

    Ordered by `source_date` (when it was WRITTEN) rather than ingest time — the 2026-07-29
    capture fix established that as the meaningful date for handwriting, and "why now" is a claim
    about when he was thinking about something, not when a timer ran.
    """
    marks = ",".join("?" for _ in _CAPTURE_CATEGORIES)
    return list(
        conn.execute(
            f"SELECT id, title, source_uri, source_date FROM documents "
            f"WHERE category IN ({marks}) AND source_date IS NOT NULL "
            f"ORDER BY source_date DESC, id DESC LIMIT ?",
            (*_CAPTURE_CATEGORIES, limit),
        )
    )


def _source_uri(conn: sqlite3.Connection, doc_id: int) -> str:
    row = conn.execute("SELECT source_uri FROM documents WHERE id=?", (doc_id,)).fetchone()
    return (row["source_uri"] if row else "") or str(doc_id)


def build_marked(
    conn: sqlite3.Connection, *, limit: int = 2, seen: set[str] | None = None
) -> list[ThreadItem]:
    """Passages he marked while reading, not yet turned into anything (Loop B's consumer).

    A mark is finished with when it has become an OBJECT (`object_id`), not when it has a `note`:
    `note` is the transcription of what he wrote beside it, and the first cut used that as the
    dealt-with flag — so transcribing the ink HID the mark, exactly backwards, since a passage he
    wrote a paragraph about is the one most worth bringing back.

    Marks carrying his own words rank first for the same reason, then newest, because what he
    marked last night is what he is thinking about now.
    """
    seen = seen if seen is not None else set()
    try:
        rows = conn.execute(
            "SELECT a.*, d.title AS doc_title FROM pdf_annotations a "
            "LEFT JOIN documents d ON d.source_uri = a.source_uri "
            "WHERE a.object_id IS NULL "
            "AND (LENGTH(TRIM(COALESCE(a.covered_text,''))) >= ? "
            "     OR TRIM(COALESCE(a.note,'')) != '') "
            "ORDER BY (TRIM(COALESCE(a.note,'')) != '') DESC, a.captured_at DESC, a.id DESC "
            "LIMIT ?",
            (_MIN_PASSAGE_CHARS, limit * 8),
        ).fetchall()
    except sqlite3.OperationalError:
        return []

    out: list[ThreadItem] = []
    seen_docs: set[str] = set()
    for row in rows:
        item_key = f"mark:{row['id']}"
        if item_key in seen:
            continue
        uri = row["source_uri"]
        # One passage per document: two quotes from the same chapter are one thought, and the
        # section is small enough that a single book would otherwise own it.
        if uri in seen_docs:
            continue
        seen_docs.add(uri)
        passage = " ".join((row["covered_text"] or "").split())
        note = " ".join((row["note"] or "").split())
        title = row["doc_title"] or uri.rsplit("/", 1)[-1]
        # A MARGIN NOTE COVERS NO TEXT, so there is no passage to lead with and the item rendered
        # as a bare "T1." with his words demoted to a quote beneath it. When the passage is empty
        # his handwriting IS the item, and repeating it as "you wrote:" underneath says it twice.
        headline = passage or note
        out.append(
            ThreadItem(
                anchor="",
                section=SECTION_MARKED,
                kind="mark",
                headline=(headline if len(headline) <= 300 else headline[:297] + "..."),
                context=f"{title} — p.{row['pdf_page'] + 1}",
                # His own words, printed back with the passage they belong to. This pairing is
                # the whole point of Loop B: the objection and the claim it objects to.
                body=(f"you wrote: {note}",) if note and passage else (),
                target_kind="doc",
                target_key=uri,
                item_key=item_key,
            )
        )
        if len(out) >= limit:
            break
    return out


def _format_grounding(link: state.ObjectLink) -> str:
    """Human-readable grounding for an object link.

    An entity `target_key` is `name\\x1ftype` — a unit separator, chosen because it cannot occur
    in a real entity name. It is also INVISIBLE when printed, so the raw key renders as
    "order processing event loopconcept". Split it back out.

    No longer used by this page (it went with the blessing section, which moved to `locus decide`)
    but still used by `agent/promote.py` when it writes a thread's grounding into a note — and it
    will be needed again by the decisions TUI. Deleting it as "blessing-only" broke `locus
    daily-pull` on the first thread that carried a link, a path no test covered.
    """
    if link.target_kind == "entity":
        name, type_ = state.parse_entity_key(link.target_key)
        return f"{name} ({type_})"
    if link.target_kind == "doc":
        return link.target_key.rsplit("/", 1)[-1]
    return link.target_key


DEVELOPMENT_KEY = "development"


def development_entries(body: dict) -> list[str]:
    """The owner's successive notes on a thread, oldest first.

    Stored as `[{"at": iso, "text": ...}]` rather than one blob so the page can show what he last
    said without losing the earlier passes, and so promotion to the corpus can date them.
    """
    out: list[str] = []
    for entry in (body or {}).get(DEVELOPMENT_KEY) or []:
        if isinstance(entry, dict) and str(entry.get("text", "")).strip():
            out.append(str(entry["text"]).strip())
        elif isinstance(entry, str) and entry.strip():
            out.append(entry.strip())
    return out


def build_open(
    conn: sqlite3.Connection, *, limit: int = 2, seen: set[str] | None = None
) -> list[ThreadItem]:
    """His own open questions and ideas, least-recently-touched first.

    Least-recently-touched is the fair queue: it surfaces the thread going stale rather than the
    one he just wrote. Combined with the version-keyed ledger, a thread he develops comes straight
    back with his new words on it and an untouched one waits its turn.
    """
    seen = seen if seen is not None else set()
    rows = conn.execute(
        f"SELECT id, updated_at FROM objects WHERE status='active' AND type IN "
        f"({','.join('?' * len(_THREAD_TYPES))}) ORDER BY updated_at, id LIMIT ?",
        (*_THREAD_TYPES, limit * 6),
    ).fetchall()

    out: list[ThreadItem] = []
    for row in rows:
        item_key = f"object:{row['id']}:{row['updated_at'] or ''}"
        if item_key in seen:
            continue
        obj = state.get_object(conn, row["id"])
        if obj is None:
            continue
        development = development_entries(obj.body)
        out.append(
            ThreadItem(
                anchor="",
                section=SECTION_OPEN,
                kind="open",
                headline=obj.title,
                context=f"{obj.type} — raised {(obj.created_at or '')[:10]}",
                # The last couple of passes only: the point is to continue the thought, not to
                # reprint a transcript he has already read.
                body=tuple(development[-2:]),
                target_kind="object",
                target_key=str(obj.id),
                item_key=item_key,
            )
        )
        if len(out) >= limit:
            break
    return out


def build_connections(
    conn: sqlite3.Connection, *, limit: int = 2, seen: set[str] | None = None
) -> list[ThreadItem]:
    """Cross-corpus links out of recent capture — a pure `related_documents` join.

    Only links that LEAVE the note cluster are surfaced. The captured notes formed their own
    mutual cluster (shared=10 among themselves, measured 2026-07-29), so a nearest-neighbour query
    over recent capture returns sibling notes almost every time — true, but not news to the person
    who wrote both.

    THE `why now` LINE IS THE PART THAT HAD TO CHANGE. It used to read "shares 3 concepts with a
    document you have not linked it to", and the owner's response was the right one: that is a
    fact about the database, and being unlinked is not a reason to care. It now names what the two
    documents both develop, which is the most a joins-only builder can honestly say; the written
    "and here is what you could do with it" needs a model pass and is step 4. A connection that
    cannot name a single shared concept is DROPPED rather than softened — grounded or silent.
    """
    seen = seen if seen is not None else set()
    out: list[ThreadItem] = []
    seen_pairs: set[tuple[int, int]] = set()
    for src in _recent_capture(conn, limit=12):
        if len(out) >= limit:
            break
        for rel in related_documents(conn, src["id"], top_n=5):
            cat = conn.execute(
                "SELECT category FROM documents WHERE id=?", (rel.doc_id,)
            ).fetchone()
            if cat is None or cat["category"] in _CAPTURE_CATEGORIES:
                continue  # sibling note — see docstring
            if not rel.shared_names:
                continue  # nothing nameable to say about it
            pair = (min(src["id"], rel.doc_id), max(src["id"], rel.doc_id))
            if pair in seen_pairs:
                continue
            seen_pairs.add(pair)
            other_uri = _source_uri(conn, rel.doc_id)
            item_key = f"conn:{src['source_uri']}|{other_uri}"
            if item_key in seen:
                continue
            shared = ", ".join(rel.shared_names[:3])
            out.append(
                ThreadItem(
                    anchor="",
                    section=SECTION_CONNECTION,
                    kind="connection",
                    headline=f"{src['title'] or src['source_uri']} → {rel.title}",
                    context=f"both develop {shared} — you wrote yours on {src['source_date']}",
                    tick=True,
                    target_kind="doc",
                    target_key=other_uri,
                    item_key=item_key,
                )
            )
            break  # one connection per note, so several notes are represented, not one
    return out[:limit]


def build_threads(
    conn: sqlite3.Connection, *, limit: int = _FIT["think"], seen: set[str] | None = None
) -> list[ThreadItem]:
    """The Think page: three provenance subsections sharing one page and one anchor series.

    Marks come first because they are the freshest thing of his and the one the old page never
    consumed at all; connections come last because they are the only agent-originated items and
    the only ones he can decline.
    """
    seen = seen if seen is not None else set()
    per = max(1, limit // 3)
    items = build_marked(conn, limit=per, seen=seen)
    items += build_open(conn, limit=per, seen=seen)
    items += build_connections(conn, limit=max(0, limit - len(items)), seen=seen)
    return items[:limit]


# --- page 3: Recall ----------------------------------------------------------------------------


def _ask_and_answer(conn: sqlite3.Connection, item) -> tuple[str, str, str]:
    """(prompt, answer, source) for one scheduled item.

    With a stored question the page can finally ASK something: the question goes on page 3 and the
    proposition — his own corpus's words, never a generated answer — goes on page 4. Without one
    the prompt falls back to the proposition and NO answer is printed, because an "answer"
    identical to the prompt is worse than none.
    """
    prompt, source = learn_review.resolve_prompt(conn, item)
    question = (getattr(item, "question", None) or "").strip()
    if not question:
        return prompt, "", source
    return question, " ".join(prompt.split()), source


def build_recalls(
    conn: sqlite3.Connection,
    *,
    today: date | None = None,
    limit: int = _FIT["recall"],
    seen: set[str] | None = None,
) -> list[Recall]:
    """Due SM-2 items, soonest first, with the answer held back for page 4.

    He asked for "the answer, but also tick/cross if I knew it and space for me to try myself" —
    so the attempt, the self-grade and the answer are three separate things, and the answer is
    physically on another page. Printing it under the question would mean he sees it before he has
    genuinely tried, which is the one thing that makes recall practice worthless.
    """
    seen = seen if seen is not None else set()
    out: list[Recall] = []
    for item in learn_review.due_items(conn, today=today, limit=limit * 3):
        # The `due` date is the version: a re-scheduled item is legitimately a new offering, which
        # is what spaced repetition IS. Without it the ledger would silently disable review.
        item_key = f"review:{item.id}:{getattr(item, 'due', '')}"
        if item_key in seen:
            continue
        prompt, answer, source = _ask_and_answer(conn, item)
        out.append(
            Recall(
                anchor="", prompt=prompt, source=source, item_id=item.id,
                answer=answer, item_key=item_key,
            )
        )
        if len(out) >= limit:
            break
    return out


# --- the status line ---------------------------------------------------------------------------

# Interesting run kinds, in the order they are reported. Anything else is still checked for
# failure — a kind we forgot to name here must never be able to fail silently.
_REPORTED_KINDS = ("discover-harvest", "discover-pull", "capture", "daily-pull", "maintain")


def build_status(conn: sqlite3.Connection, *, now: datetime | None = None) -> Status:
    """What the system did since yesterday, and what is broken.

    The reporting gap this closes: `locus-maintain` failed six consecutive nights (2026-08-01) and
    nothing said so — it was found by accident. Systemd-level detection (a unit that never started
    at all, so has no `agent_runs` row to fail) is step 5; this reports what DID run and any run
    that ended badly, which covers everything that starts and then breaks.
    """
    now = now or datetime.now(timezone.utc)
    cutoff = (now - timedelta(days=1)).isoformat()
    try:
        rows = conn.execute(
            "SELECT kind, status FROM agent_runs WHERE started_at >= ? ORDER BY started_at",
            (cutoff,),
        ).fetchall()
    except sqlite3.OperationalError:
        return Status()

    failures: list[str] = []
    for row in rows:
        if row["status"] in ("error", "degraded"):
            failures.append(f"{row['kind']} {row['status']}")
        elif row["status"] == "running":
            # An orphan: the process died without closing its row. Silence here is what made the
            # maintain failure invisible for six nights.
            failures.append(f"{row['kind']} did not finish")

    produced: list[str] = []
    for kind in _REPORTED_KINDS:
        n = sum(1 for r in rows if r["kind"] == kind and r["status"] == "ok")
        if n:
            produced.append(f"{kind} x{n}" if n > 1 else kind)
    return Status(produced=tuple(produced), failures=tuple(dict.fromkeys(failures)))


# --- composition -------------------------------------------------------------------------------


def compose(conn: sqlite3.Connection, *, today: date | None = None) -> DailyPage:
    """Build the day's page. Pure read — persisting it is `persist()`, a separate step."""
    today = today or date.today()
    seen = _shown_keys(conn)
    page = DailyPage(page_date=today.isoformat())
    page.readings = build_readings(conn, seen=seen)
    # New material leads; a re-read takes a slot only if one is spare, and only ever because he
    # marked something as not understood.
    if len(page.readings) < _FIT["read"]:
        page.readings += build_rereads(
            conn, limit=_FIT["read"] - len(page.readings), seen=seen
        )
    page.reading_state = build_reading_state(conn)
    page.threads = build_threads(conn, seen=seen)
    page.recalls = build_recalls(conn, today=today, seen=seen)
    page.status = build_status(conn)

    for i, r in enumerate(page.readings, 1):
        r.anchor = f"D{i}"
        page.anchors.append(
            Anchor(r.anchor, "reading", r.target_kind, r.target_key, label=r.title)
        )
    # ONE printed series across all three Think subsections — he reads one list — while `kind`
    # keeps a mark, a thread and a connection routing to three different places on the way back.
    for i, t in enumerate(page.threads, 1):
        t.anchor = f"T{i}"
        page.anchors.append(
            Anchor(t.anchor, t.kind, t.target_kind, t.target_key, label=t.headline[:120])
        )
    for i, r in enumerate(page.recalls, 1):
        r.anchor = f"R{i}"
        page.anchors.append(
            Anchor(r.anchor, "recall", "review_item", str(r.item_id), label=r.prompt[:120])
        )
    # One always-present open region. An invitation, not a prompt with a right answer, so it
    # carries no count and no backlog — anything written here becomes a thread he already owns.
    page.anchors.append(Anchor("Q1", "question", "none", "open", label="open question"))
    return page


def persist(
    conn: sqlite3.Connection,
    page: DailyPage,
    *,
    md_path: str | None = None,
    pdf_path: str | None = None,
    source_run: int | None = None,
) -> None:
    """Record the page, its anchors, and what it offered, so the annotated version maps back.

    Rebuilding a date REPLACES its anchors: the physical page he will write on is the latest one,
    and a stale anchor row would silently route an answer to whatever used to sit at that number.
    Annotations are keyed by (date, anchor) independently and are NOT touched here — evidence he
    produced outlives a rebuild of the page that prompted it.
    """
    stamp = _utcnow()
    with conn:
        conn.execute(
            "INSERT INTO daily_pages (page_date, built_at, source_run, md_path, pdf_path) "
            "VALUES (?,?,?,?,?) ON CONFLICT(page_date) DO UPDATE SET "
            "built_at=excluded.built_at, source_run=excluded.source_run, "
            "md_path=excluded.md_path, pdf_path=excluded.pdf_path",
            (page.page_date, stamp, source_run, md_path, pdf_path),
        )
        conn.execute("DELETE FROM daily_anchors WHERE page_date=?", (page.page_date,))
        conn.executemany(
            "INSERT INTO daily_anchors (page_date, anchor, kind, target_kind, target_key, label) "
            "VALUES (?,?,?,?,?,?)",
            [
                (page.page_date, a.anchor, a.kind, a.target_kind, a.target_key, a.label)
                for a in page.anchors
            ],
        )
    record_shown(conn, page)


def anchors_for(conn: sqlite3.Connection, page_date: str) -> dict[str, Anchor]:
    """anchor -> Anchor for a date. The pull-back's only source of truth about the page."""
    return {
        r["anchor"]: Anchor(
            r["anchor"], r["kind"], r["target_kind"], r["target_key"], r["label"] or ""
        )
        for r in conn.execute(
            "SELECT * FROM daily_anchors WHERE page_date=? ORDER BY id", (page_date,)
        )
    }


def mark_read(conn: sqlite3.Connection, page_date: str, *, at: str | None = None) -> bool:
    """Stamp `read_at` the FIRST time ink is seen on a page. Returns True if this set it.

    The /Daily inbox is built on this: a page stays loose at the root until it has been written
    on, then archives to /Daily/YYYY-MM. Set once and never overwritten, so re-pulling a page he
    wrote on last week does not make it look like today's reading.
    """
    with conn:
        cur = conn.execute(
            "UPDATE daily_pages SET read_at=? WHERE page_date=? AND read_at IS NULL",
            (at or _utcnow(), page_date),
        )
    return cur.rowcount > 0


# --- rendering ---------------------------------------------------------------------------------

# Ruled lines he writes on. `***` on its own line, with a PLAIN blank line before it. The first
# draft used a non-breaking space as the spacer and `---` as the rule, and neither survived
# pandoc: U+00A0 is not markdown whitespace, so the spacer counted as text and the `---` under it
# parsed as a SETEXT HEADING UNDERLINE rather than a thematic break. (A pair of `---` fences
# around a blank line has a second bad reading too — pandoc's YAML metadata block.) Verified
# 2026-07-30: zero `#horizontalrule` reached the Typst body, so the page offered blank space with
# no lines on it. `***` has neither ambiguity.
_RULE = "\n***\n"

# A real page break. Pandoc's typst writer passes a raw `{=typst}` block through untouched, which
# is what turns this from one scrolling document into four laid-out pages (verified 2026-08-02).
_PAGEBREAK = "\n```{=typst}\n#pagebreak()\n```\n"

# Typst's fractional spacer: it absorbs all remaining vertical space, pushing whatever follows to
# the foot of the page. Used so the status line sits at the BOTTOM of page 1 rather than halfway
# up under the shelf state, which is where it landed when the page was one flowing column.
_PUSH_TO_FOOT = "\n```{=typst}\n#v(1fr)\n```\n"


def _rules(n: int) -> str:
    return _RULE * n


# Ruled lines a page can hold once its items' printed text is allowed for. Approximate by
# construction — the exact number depends on how long each item runs — and deliberately erring
# short, since a section that overflows onto a second page breaks the one-section-per-page rule
# that is the whole point of the layout.
# Derivation, so this is not a guess that drifts: the page is 9.43in tall with 0.5in margins =>
# ~607pt of body. A ruled line at [daily].rule_gap_em = 2.6 consumes its gap above AND below,
# ~5.2em ~= 57pt at 11pt text, so ~10 lines fit a page with no text on it at all. Each item also
# prints a headline, sometimes his prior words, and a provenance line — call it 3 printed lines,
# ~45pt. Eight ruled lines per page across all items leaves real margin for that, and overflowing
# is the one failure that matters here because it breaks one-section-per-page.
_LINE_BUDGET = 9
_MAX_LINES = 5

# Floor per section, because the two writing pages differ in what a region is FOR. Developing an
# idea needs room to think on paper; recalling a stored claim is a sentence or two, so two lines
# there buys a fourth question rather than white space.
_MIN_LINES = {"think": 3, "recall": 2}


def _lines_for(n_items: int, section: str = "think") -> int:
    """How many ruled lines each item gets, so a section FILLS its page and never exceeds it.

    Both failure directions are real and both were hit while building this. A fixed three lines
    per item left a two-item page 60% white — on a surface meant to be written on, empty space is
    wasted paper, not restraint. Then a budget of 15 pushed Recall onto a second page, which
    breaks one-section-per-page, the constraint the entire layout rests on.

    THE BUDGET IS DERIVED, so it does not drift: the page is 9.43in tall with 0.5in margins =>
    ~607pt of body. A ruled line at `[daily].rule_gap_em` = 2.6 consumes its gap above AND below,
    ~5.2em ~= 57pt at 11pt text, so ~10 fit a page carrying no text at all. Each item also prints
    a headline, often his prior words, and a provenance line, so the usable budget is 9. Change
    `rule_gap_em` and this must be recomputed — `scripts/analysis/render_daily_sample.py` and the
    overflow test are how you check.
    """
    floor = _MIN_LINES.get(section, 3)
    if n_items <= 0:
        return floor
    return max(floor, min(_MAX_LINES, _LINE_BUDGET // n_items))


def _status_block(page: DailyPage) -> str:
    return f"*{page.status.render()}*"


def _open_region() -> list[str]:
    return ["## Anything on your mind", "", "**Q1.**", "", _rules(4)]


def _render_read(page: DailyPage) -> str:
    lines = ["## Read", ""]
    for d in page.readings:
        tag = "  *re-read*" if d.shelf == "re-read" else ""
        lines += [f"**{d.anchor}. {d.title}**{tag}", "", d.why]
        if d.grounding:
            lines += ["", f"`{d.grounding}`"]
        lines += ["", _rules(1)]

    st = page.reading_state
    if st.in_progress:
        lines += ["", "**In progress**", ""]
        for item in st.in_progress:
            link = f" — {item.links_to}" if item.links_to else ""
            # `yours` marks material he chose himself. It is the interesting case: nothing in the
            # pipeline decided it was relevant, so the link is a finding rather than a restatement.
            tag = "  *yours*" if item.owner_added else ""
            lines += [f"- **{item.title[:58]}**{link}{tag}"]
            # THE LINK IS THE POINT HERE, not the argument. This is a status list; printing the
            # full written reason for each of five in-progress items pushed the whole section onto
            # a second page. Only material he chose himself gets a line of it, because that is the
            # case he asked to see, and only a line.
            if item.why and item.owner_added:
                lines += [f"  {item.why[:110]}"]
    if st.proposed:
        oldest = f", oldest {st.oldest_days} days" if st.oldest_days is not None else ""
        lines += ["", f"**On the shelf:** {st.proposed} waiting{oldest}"]

    lines += ["", _PUSH_TO_FOOT, "", _status_block(page)]
    return "\n".join(lines)


def _render_think(page: DailyPage) -> str:
    lines = ["## Think", ""]
    per_item = _lines_for(len(page.threads), "think")
    current = ""
    for t in page.threads:
        if t.section != current:
            current = t.section
            lines += [f"### {current}", ""]
        # ASCII brackets, not U+2610 BALLOT BOX: the Typst body font has no glyph for it and it
        # rendered as a tofu box (verified 2026-07-30 — the character was absent from the
        # extracted text while a stray rectangle was drawn in its place). A tick box he must
        # recognise cannot depend on font coverage.
        box = "`[   ]` keep   " if t.tick else ""
        lines += [f"**{t.anchor}.** {box}{t.headline}", ""]
        for prior in t.body:
            lines += [f"> {prior}", ""]
        lines += [f"*{t.context}*", "", _rules(per_item)]
    return "\n".join(lines)


def _render_recall(page: DailyPage) -> str:
    lines = ["## Recall", "", "Answer, then tick if you knew it. Answers overleaf.", ""]
    per_item = _lines_for(len(page.recalls), "recall")
    for r in page.recalls:
        lines += [f"**{r.anchor}.** {r.prompt}", ""]
        if r.source:
            lines += [f"*{r.source}*", ""]
        lines += [_rules(per_item), "`[   ]` knew it", ""]
    return "\n".join(lines)


def _render_back(page: DailyPage) -> str:
    """Page 4: the open region first, answers small at the foot.

    His order, and it is the right one — the blank space is for a thought he wants to keep, so it
    should not sit under a block of answers he has already checked.
    """
    lines = _open_region()
    answered = [r for r in page.recalls if r.answer]
    if answered:
        lines += ["", "***", "", "*Answers*", ""]
        for r in answered:
            lines += [f"*{r.anchor}. {r.answer}*", ""]
    return "\n".join(lines)


def render(page: DailyPage) -> str:
    """Markdown for the page. Anchors are printed verbatim so the pull-back can find them.

    One section per PHYSICAL page, by request. An empty section is omitted entirely rather than
    rendered as a bare heading — and omitting it takes its page break with it, so a quiet day is a
    short document rather than four pages of white space.
    """
    pages: list[str] = []
    if page.readings or not page.reading_state.is_empty:
        pages.append(_render_read(page))
    if page.threads:
        pages.append(_render_think(page))
    if page.recalls:
        pages.append(_render_recall(page))

    if not pages:
        # A page with nothing on it is a valid, calm state — and it is still the only place a
        # failure would be announced, so the status line survives an empty day.
        head = [f"# {page.page_date}", "", "Nothing to surface today.", ""]
        return "\n".join(head + _open_region() + ["", _status_block(page)])

    body = _PAGEBREAK.join(pages + [_render_back(page)])
    return f"# {page.page_date}\n\n{body}"
