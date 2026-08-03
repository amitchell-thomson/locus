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
terminal TUI — never split across two surfaces. `build_blessings` is gone and `locus decide` is
now the ONLY place an object's status changes (`locus objects --bless` was the interim carrier and
was removed once the TUI landed). No decision may appear both here and there.

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
# item needs 2-3 to be answerable. RE-CALIBRATED 2026-08-03 when `[daily].rule_gap_em` went
# 2.6 -> 1.8 (10.1mm -> 7.0mm between rules, measured): tighter rules buy a whole extra item, so
# Think went 3 -> 4 and Recall's writing regions went 2 lines -> 3. Verified by rendering, not
# derived: 5 Think items or 4 recall lines push a fifth page.
_FIT = {"read": 3, "think": 4, "recall": 4}

# The Read page is the one section with no writing regions, so `_lines_for` does not bound it and
# nothing else did either: three proposals plus an unbounded in-progress list overflowed onto a
# second page, taking the `#v(1fr)` status line with it and leaving p2 ~90% white. Three is what
# fits underneath a full `_FIT["read"]` set; anything held back is counted on the page.
_MAX_IN_PROGRESS = 3

# Seats on the Read page held for a passage he said he did not follow, so new material cannot
# take all three. One, not two: the shelf is the main channel and a re-read is the exception that
# he asked for by marking something.
_REREAD_SLOTS = 1

# How many raw search hits are handed to the cross-encoder in `_explains`. Small: this runs on
# CPU during page composition, and the answer is either near the top of the dense arm or absent.
_REREAD_POOL = 20

# A re-read has to clear a HIGHER bar than `[retrieve].min_rerank_score` (0.22). That floor asks
# "is this worth showing as context alongside other material"; this one asks "is this worth one
# of three seats on the page, on its own". Measured over his ten not-understood marks
# (2026-08-03): documents that genuinely answer the mark score 4.07 and 4.82, while the best
# WRONG match — *Sensing, Signals and Communications* offered for a note about trading "signals"
# — scores 1.917. Most marks correctly produce nothing at this bar, because the corpus does not
# contain an answer, and printing nothing is the honest result: the discovery channel is where
# "the corpus cannot answer this" is meant to be handled.
_REREAD_MIN_RERANK = 2.5

# The object types that represent an OPEN THREAD of his own — something he asked or proposed and
# has not finished with. Concepts and projects are not threads: they are things that exist.
_THREAD_TYPES = ("question", "idea")

# A marked passage needs enough words to mean something on its own. Below this it is a stray
# stroke or a single word, which tells him nothing when it comes back a week later.
_MIN_PASSAGE_CHARS = 40

# Subsection headings on the Think page, in render order. Named for PROVENANCE — where the item
# came from — because that is the distinction the old labels failed to make.
SECTION_MARKED = "From your reading"
SECTION_OPEN = "Still open"
SECTION_CONNECTION = "Connections found"

# A DRAWN box, not a character. §18 found that U+2610 BALLOT BOX has no glyph in the body font
# and rendered as tofu, so the fallback was ASCII `[   ]` — which reads as three spaces in
# brackets rather than something to tick. Inline raw typst draws a real square, so it cannot
# depend on font coverage at all.
def _anchor(label: str) -> str:
    """The printed anchor (`D1`, `T2`) as its own styled element.

    Raw inline typst rather than `**bold**`: bold is also how "In progress" and "On the shelf"
    are marked, and the item title shares the anchor's `**...**` span — so styling `strong`
    turned all of them into accent-coloured sans.
    """
    return f'`#anc[{label}]`{{=typst}}'


_TICKBOX = "`#tickbox`{=typst}"


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
    cost_usd: float = 0.0

    def render(self) -> str:
        made = " · ".join(self.produced) if self.produced else "nothing new"
        # What a night cost, on the surface he actually reads. Four billed passes now run
        # nightly and nothing reported against `[agent].daily_cost_cap_usd`; spend that is never
        # shown is spend nobody is deciding about.
        spend = f" · ${self.cost_usd:.2f}" if self.cost_usd else ""
        if not self.failures:
            return f"overnight: {made}{spend} · all runs healthy"
        # Only the first few, and loud: a status line listing eight problems is a wall he learns
        # to skip, which is the failure this whole line exists to prevent.
        loud = " · ".join(f.upper() for f in self.failures[:3])
        more = f" · +{len(self.failures) - 3} more" if len(self.failures) > 3 else ""
        return f"overnight: {made}{spend} · {loud}{more}"


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
    # A re-read he already declined must not be offered again. `pull_daily` records exactly this
    # on the `reading` surface (the document's source_uri, verdict `rejected`), and until now
    # nothing read it — eight refusals sitting in a table while the same documents stayed
    # eligible. A rejection is data, or it should not be written.
    declined = _declined_rereads(conn)
    out: list[ReadItem] = []
    for mark in I.not_understood_marks(conn, limit=limit * 6):
        question = I.reread_seed(mark)
        if not question:
            continue
        item_key = f"reread:{mark['id']}"
        if item_key in seen:
            continue
        target = _explains(conn, question, exclude_uri=mark["source_uri"], declined=declined)
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


def _declined_rereads(conn: sqlite3.Connection) -> set[str]:
    """`source_uri`s he has already turned down as a re-read, from the `reading` surface."""
    try:
        rows = conn.execute(
            "SELECT candidate_key, verdict FROM acceptance_log WHERE surface='reading' "
            "ORDER BY at, id"
        ).fetchall()
    except sqlite3.OperationalError:
        return set()
    latest: dict[str, str] = {r["candidate_key"]: r["verdict"] for r in rows}
    return {key for key, verdict in latest.items() if verdict == "rejected"}


def _explains(
    conn: sqlite3.Connection, question: str, *, exclude_uri: str, declined: set[str] | None = None
) -> tuple[str, str] | None:
    """The corpus document that best covers `question`, or None. Degrades silently.

    IT MUST BE RERANKED, and it must clear the floor. `search()` returns raw dense/lexical
    similarity, where everything scores 0.84-0.98 and the number is meaningless in absolute
    terms — taking its argmax offered a document that merely shared a WORD. Measured 2026-08-03:
    "what are the 'signals'" returned *Sampling, aliasing, modulation and spectral density*, and
    "Momentum is a robust anomaly" returned *Dynamics — impulse-momentum*. Both are puns, and a
    useless suggestion is exactly what discredited the old read-next section.

    The cross-encoder is what makes a score comparable to a threshold, so this reuses the same
    same cross-encoder every corpus query uses, against `_REREAD_MIN_RERANK`. Below it there is
    no answer in the corpus, and the honest output is None — the slot goes back to new reading.
    """
    try:
        from locus.retrieve.search import search
    except ImportError:
        return None
    try:
        candidates = search(conn, question)
    except Exception:                                  # ollama down, no vectors yet
        return None
    candidates = [c for c in (candidates or []) if c.text]
    if not candidates:
        return None

    pool = sorted(candidates, key=lambda c: -(c.score or 0.0))[:_REREAD_POOL]
    try:
        from locus.retrieve.rerank import score_pairs

        scores = score_pairs(question, [c.text for c in pool])
    except Exception:                                  # the `rerank` extra is optional
        return None

    ranked = sorted(zip(pool, scores), key=lambda pair: -pair[1])
    # Best-scoring DOCUMENT, not unit: the slot offers something to read, and several strong
    # chunks from one document say the same thing about it.
    for cand, score in ranked:
        if score < _REREAD_MIN_RERANK:
            break
        row = conn.execute(
            "SELECT title, source_uri FROM documents WHERE id=?", (cand.doc_id,)
        ).fetchone()
        if row and row["source_uri"] != exclude_uri and row["source_uri"] not in (declined or ()):
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
    # HIS writing, by the one shared definition — not `category='note'`. Keying on category
    # meant his own handwriting filed `coursework` or `project` by the device folder produced no
    # connections at all, which is the same defect the proposer's gate had.
    clause, params = state.owner_authored_sql("d")
    return list(
        conn.execute(
            f"SELECT d.id, d.title, d.source_uri, d.source_date FROM documents d "
            f"WHERE {clause} AND d.source_date IS NOT NULL "
            f"ORDER BY d.source_date DESC, d.id DESC LIMIT ?",
            (*params, limit),
        )
    )


def _source_uri(conn: sqlite3.Connection, doc_id: int) -> str:
    row = conn.execute("SELECT source_uri FROM documents WHERE id=?", (doc_id,)).fetchone()
    return (row["source_uri"] if row else "") or str(doc_id)


def _distinct_documents_first(rows: list) -> list:
    """Re-order marks so one per document comes first, then the rest — variety without a cap.

    Stable within each pass, so the SQL ordering (his own words first, then newest) still decides
    which mark of a document leads.
    """
    first, rest, seen_docs = [], [], set()
    for row in rows:
        uri = row["source_uri"]
        if uri in seen_docs:
            rest.append(row)
        else:
            seen_docs.add(uri)
            first.append(row)
    return first + rest


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

    # VARIETY IS A PREFERENCE, NOT A CAP. One passage per document was a hard rule, and since
    # every mark he has is on the same book it meant the section could never offer more than ONE
    # mark a day however much space there was — a 26-mark backlog moving at one a day. The worry
    # it encoded (a single book owning the page) is now handled where it belongs, by the equal
    # share `build_threads` allocates, so this becomes soft: distinct documents first, then fill
    # the remaining seats from whatever is left rather than printing white space.
    ordered = _distinct_documents_first(rows)

    out: list[ThreadItem] = []
    for row in ordered:
        item_key = f"mark:{row['id']}"
        if item_key in seen:
            continue
        uri = row["source_uri"]
        passage = " ".join((row["covered_text"] or "").split())
        note = " ".join((row["note"] or "").split())
        title = row["doc_title"] or uri.rsplit("/", 1)[-1]
        # A MARGIN NOTE COVERS NO TEXT, so there is no passage to lead with and the item rendered
        # as a bare "T1." with his words demoted to a quote beneath it. When the passage is empty
        # his handwriting IS the item, and repeating it as "you wrote:" underneath says it twice.
        #
        # The same applies to a passage too SHORT to stand alone, which the page only started
        # showing once marks stopped being capped at one per document: an underline of "market is"
        # led the item with a nine-character fragment while the words he actually wrote sat below
        # it. A stub is not a headline — lead with his note and keep the fragment as the context.
        leads_with_note = len(passage) < _MIN_PASSAGE_CHARS and bool(note)
        headline = note if leads_with_note else (passage or note)
        out.append(
            ThreadItem(
                anchor="",
                section=SECTION_MARKED,
                kind="mark",
                headline=_clip(headline, 300),
                context=(
                    f"{title} — p.{row['pdf_page'] + 1}"
                    + (f" · beside “{passage}”" if leads_with_note and passage else "")
                ),
                # His own words, printed back with the passage they belong to. This pairing is
                # the whole point of Loop B: the objection and the claim it objects to.
                body=(f"you wrote: {note}",) if note and passage and not leads_with_note else (),
                target_kind="doc",
                target_key=uri,
                item_key=item_key,
            )
        )
        if len(out) >= limit:
            break
    return out


def _format_grounding(link: state.ObjectLink, conn: sqlite3.Connection | None = None) -> str:
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
    if link.target_kind == "object" and conn is not None and link.target_key.isdigit():
        # An object link's `target_key` is `str(object_id)`, so without resolving it a promoted
        # thread reached the corpus reading "Raised against: Advanced Portfolio Management.pdf,
        # 15, 95, 80" — three bare integers, embedded and searchable as such.
        row = conn.execute(
            "SELECT type, title FROM objects WHERE id=?", (int(link.target_key),)
        ).fetchone()
        if row is not None:
            return f"{row['type']}: {row['title']}"
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
                context=_thread_context(conn, obj),
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


def _thread_context(conn: sqlite3.Connection, obj) -> str:
    """`idea — regime-ml · also touches 2 of your threads · raised 2026-07-30`.

    An idea belongs BESIDE the project it is about and the other things he has thought that touch
    it. Both facts existed in `object_links` and neither was ever printed, so a thread came back
    looking free-floating even when it was connected.
    """
    bits = [obj.type]
    linked = [
        link for link in obj.links
        if link.target_kind == "object" and link.target_key.isdigit()
    ]
    threads = 0
    for link in linked:
        other = state.get_object(conn, int(link.target_key))
        if other is None:
            continue
        if other.type == "project":
            bits.append(other.title)
        elif other.type in ("idea", "question"):
            threads += 1
    if threads:
        bits.append(f"also touches {threads} of your threads")
    bits.append(f"raised {(obj.created_at or '')[:10]}")
    return " · ".join(bits)


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
            sib_clause, sib_params = state.owner_authored_sql("d")
            cat = conn.execute(
                f"SELECT ({sib_clause}) AS own FROM documents d WHERE d.id=?",
                (*sib_params, rel.doc_id),
            ).fetchone()
            if cat is None or cat["own"]:
                continue  # sibling note of his own — see docstring
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
    # EQUAL SHARE FIRST, THEN REFILL. Each subsection gets the same number of seats, which is
    # what "equal weighting" means; but a subsection with nothing to offer must not leave a third
    # of the page blank, because the page is the scarce thing and he has a 26-mark backlog behind
    # it. The old split took a fixed `per` from marks and open and let connections absorb the
    # slack in only ONE direction, so a day with no connections rendered a two-thirds page.
    pools = [
        build_marked(conn, limit=limit, seen=seen),
        build_open(conn, limit=limit, seen=seen),
        build_connections(conn, limit=limit, seen=seen),
    ]
    per = max(1, limit // len(pools))
    take = [min(per, len(pool)) for pool in pools]
    # Round-robin the leftover seats so the refill stays as even as the remainder allows.
    while sum(take) < limit and any(take[i] < len(pools[i]) for i in range(len(pools))):
        for i, pool in enumerate(pools):
            if sum(take) >= limit:
                break
            if take[i] < len(pool):
                take[i] += 1
    # Emitted in POOL order, not selection order: `_render_think` prints a heading whenever the
    # section changes, so an interleaved list would repeat all three headings down the page.
    return [item for i, pool in enumerate(pools) for item in pool[:take[i]]]


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
    """What the system did since yesterday, what it cost, and what is broken.

    Delegates to `locus.health`, which is also what `locus status` reads — one definition of
    "healthy", so the page and the terminal can never disagree about whether the system is fine.
    The page previously did its own reckoning over `agent_runs` and could only see runs that
    started; a unit that never started leaves no row, which is exactly how `locus-maintain` failed
    six consecutive nights (2026-08-01) without saying so.
    """
    from locus import health as H

    checked = H.check(conn, now=now)
    return Status(
        produced=tuple(
            f"{k} x{n}" if n > 1 else k for k, n in sorted(checked.ran.items())
        ),
        failures=tuple(p.render() for p in checked.problems),
        cost_usd=checked.cost_usd,
    )


def _legacy_build_status(conn: sqlite3.Connection, *, now: datetime | None = None) -> Status:
    """Superseded by `locus.health`; kept only until the next tidy pass."""
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
    # A re-read gets a RESERVED slot, not the leftovers. "Only if one is spare" sounded modest
    # and meant never: the shelf caps at 10 proposals and the page shows 3, so `build_rereads`
    # was unreachable in every state the system is normally in — 12 passages marked "I did not
    # follow this", which §20 makes the ONLY thing that earns a corpus re-read, reached nothing.
    # New material still leads; it just cannot take every seat.
    rereads = build_rereads(conn, limit=_REREAD_SLOTS, seen=seen)
    page.readings = build_readings(conn, limit=_FIT["read"] - len(rereads), seen=seen)
    page.readings += rereads
    # If the shelf could not fill its share, let a second re-read use the gap rather than
    # printing white space.
    if len(page.readings) < _FIT["read"]:
        page.readings += build_rereads(
            conn, limit=_FIT["read"] - len(page.readings),
            seen=seen | {r.item_key for r in rereads},
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
# MEASURE, DO NOT DERIVE. The previous derivation here was wrong by a factor of two: it assumed a
# rule "consumes its gap above AND below", but Typst COLLAPSES a block's `above` against the
# previous block's `below`, so consecutive rules sit ONE gap apart. The delivered page measured
# 28.6pt between rules at rule_gap_em 2.6, not the ~57pt claimed. The budget below survived only
# because printed text absorbed the difference. It is now checked by rendering a real PDF and
# counting pages (`test_a_full_page_does_not_overflow_into_a_fifth`), which is the only method
# that has ever caught this, and overflowing is the failure that matters because it breaks
# one-section-per-page.
_LINE_BUDGET = 9
_MAX_LINES = 5

# Floor per section, because the two writing pages differ in what a region is FOR. Developing an
# idea needs room to think on paper; recalling a stored claim is a sentence or two, so two lines
# there buys a fourth question rather than white space.
_MIN_LINES = {"think": 3, "recall": 3}


def _lines_for(n_items: int, section: str = "think") -> int:
    """How many ruled lines each item gets, so a section FILLS its page and never exceeds it.

    Both failure directions are real and both were hit while building this. A fixed three lines
    per item left a two-item page 60% white — on a surface meant to be written on, empty space is
    wasted paper, not restraint. Then a budget of 15 pushed Recall onto a second page, which
    breaks one-section-per-page, the constraint the entire layout rests on.

    THE BUDGET IS MEASURED, not derived — see the note on `_LINE_BUDGET` for why the arithmetic
    that used to live here was wrong by 2x. Change `rule_gap_em` and re-run the overflow test:
    it renders a real PDF and counts pages, which is the only check that has ever caught this.
    """
    floor = _MIN_LINES.get(section, 3)
    if n_items <= 0:
        return floor
    return max(floor, min(_MAX_LINES, _LINE_BUDGET // n_items))


def _clip(text: str, limit: int) -> str:
    """Truncate on a word boundary and SAY SO with an ellipsis.

    A bare slice produced "AlphaZeroBeta Deep Reinforcement Learning for Market-Neutr" and
    "...during adverse events—the " on the live page: mid-word cuts that read as corruption
    rather than as brevity. One helper so the three call sites cannot drift apart again.
    """
    text = " ".join((text or "").split())
    if len(text) <= limit:
        return text
    cut = text[: limit - 1]
    spaced = cut.rsplit(" ", 1)[0]
    # Only prefer the word boundary when it does not throw away most of the budget.
    return (spaced if len(spaced) >= limit * 0.6 else cut).rstrip(" ,;:—-") + "…"


# Typst content-mode metacharacters. The status line carries "$0.55", and `$` OPENS MATH in
# typst — an unescaped one silently swallows the rest of the footer into a formula.
_TYPST_SPECIAL = "\\#$[]*_@<>`~"


def _typst_escape(text: str) -> str:
    for ch in _TYPST_SPECIAL:
        text = text.replace(ch, "\\" + ch)
    return text


def _status_block(page: DailyPage) -> str:
    """The footer: pushed to the foot, separated by a hairline, and SMALLER than the page.

    Raw typst rather than markdown emphasis because none of that is expressible in markdown — as
    plain `*italics*` the status rendered at full body size, directly beneath "On the shelf", so
    the one line that shouts when something is broken read as another entry in the reading list.
    """
    return (
        "```{=typst}\n"
        "#v(1fr)\n"
        "#line(length: 100%, stroke: 0.3pt + luma(85%))\n"
        "#v(0.45em)\n"
        f"#text(size: 8pt, fill: luma(45%), style: \"italic\")[{_typst_escape(page.status.render())}]\n"
        "```"
    )


def _open_region() -> list[str]:
    return ["# Anything on your mind", "", _anchor("Q1"), "", _rules(4)]


def _render_read(page: DailyPage) -> str:
    lines = ["# Read", ""]
    for d in page.readings:
        tag = "  *re-read*" if d.shelf == "re-read" else ""
        lines += [f"{_anchor(d.anchor)} **{d.title}**{tag}", "", d.why]
        if d.grounding:
            lines += ["", f"`{d.grounding}`"]
        lines += ["", _rules(1)]

    st = page.reading_state
    if st.in_progress:
        lines += ["", "## In progress", ""]
        # Truncating the reasons was the FIRST attempt at keeping this section on one page and it
        # was not enough: with 3 proposals above it, five in-progress items still spilled the
        # status line onto a second, near-empty page (measured 2026-08-02 — a real 5-page PDF).
        # The list is a status glance, so the fix is to bound the ITEM COUNT and say what was
        # held back; `_MAX_IN_PROGRESS` is what fits beneath a full Read section, not a taste.
        shown = st.in_progress[:_MAX_IN_PROGRESS]
        for item in shown:
            link = f" — {item.links_to}" if item.links_to else ""
            # `yours` marks material he chose himself. It is the interesting case: nothing in the
            # pipeline decided it was relevant, so the link is a finding rather than a restatement.
            tag = "  *yours*" if item.owner_added else ""
            lines += [f"- **{_clip(item.title, 58)}**{link}{tag}"]
            # THE LINK IS THE POINT HERE, not the argument. This is a status list; printing the
            # full written reason for each of five in-progress items pushed the whole section onto
            # a second page. Only material he chose himself gets a line of it, because that is the
            # case he asked to see, and only a line.
            if item.why and item.owner_added:
                lines += [f"  {_clip(item.why, 110)}"]
        if len(st.in_progress) > len(shown):
            # Never silently truncate: a shorter list must not read as "this is everything".
            lines += [f"- *…and {len(st.in_progress) - len(shown)} more in progress*"]
    if st.proposed:
        oldest = f", oldest {st.oldest_days} days" if st.oldest_days is not None else ""
        lines += ["", f"**On the shelf:** {st.proposed} waiting{oldest}"]

    lines += ["", _status_block(page)]
    return "\n".join(lines)


def _render_think(page: DailyPage) -> str:
    lines = ["# Think", ""]
    per_item = _lines_for(len(page.threads), "think")
    current = ""
    for t in page.threads:
        if t.section != current:
            current = t.section
            lines += [f"## {current}", ""]
        # ASCII brackets, not U+2610 BALLOT BOX: the Typst body font has no glyph for it and it
        # rendered as a tofu box (verified 2026-07-30 — the character was absent from the
        # extracted text while a stray rectangle was drawn in its place). A tick box he must
        # recognise cannot depend on font coverage.
        box = f"{_TICKBOX} keep   " if t.tick else ""
        lines += [f"{_anchor(t.anchor)} {box}{t.headline}", ""]
        for prior in t.body:
            lines += [f"> {prior}", ""]
        lines += [f"*{t.context}*", "", _rules(per_item)]
    return "\n".join(lines)


def _render_recall(page: DailyPage) -> str:
    lines = ["# Recall", "", "Answer, then tick if you knew it. Answers overleaf.", ""]
    per_item = _lines_for(len(page.recalls), "recall")
    for r in page.recalls:
        lines += [f"{_anchor(r.anchor)} {r.prompt}", ""]
        if r.source:
            lines += [f"*{r.source}*", ""]
        # RIGHT-ALIGNED, because position is what says which item it belongs to. Set flush left
        # it sat directly above the NEXT question and read as that question's box, when it
        # terminates the one above it.
        lines += [_rules(per_item),
                  "```{=typst}\n#align(right)[#tickbox #text(size: 9pt, fill: luma(45%))[knew it]]\n```",
                  ""]
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

    HEADING LEVELS ARE THE PAGE'S HIERARCHY, so they moved up one when the date left: a SECTION
    (`# Read`) is the title of its page, and the Think subsections (`## Still open`) sit under it.
    `md2pdf` styles them by level, so this is the contract between the two modules.
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
        head = ["Nothing to surface today.", ""]
        return "\n".join(head + _open_region() + ["", _status_block(page)])

    # NO DATE HEADING. It used to open the document as an `# H1`, competing with the section
    # heading immediately beneath it for the role of page title. The date is per-DOCUMENT, not
    # per-section, so it belongs in the running header (`PageGeometry.running_header`) where it
    # appears on all four pages instead of only the first.
    return _PAGEBREAK.join(pages + [_render_back(page)])
