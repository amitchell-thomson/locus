"""The daily reMarkable page (agent-layer plan §9) — the one surface the owner touches.

**Aggregates; does not compute.** Every item on this page already exists somewhere in the
agent state: due review rows, `related_documents` joins, deterministic gap analysis, proposed
objects awaiting blessing. Nothing here calls a model and nothing here proposes anything new.
That is a deliberate boundary, not an economy: the page must render identically whether or not
last night's structure run succeeded, because §9's guardrail is that it "degrades silently if
an agent didn't run" — a composer that computed its own content could not offer that.

Longevity guardrails (§9, which outrank any feature here):
  - glanceable in ~10s, so hard caps: 3 connections, 5 recalls, 3 readings, 3 blessings;
  - NO guilt metrics — no unread counts, no streaks, no "N pending". The 43 objects awaiting
    blessing are never announced as 43; the page offers three and says nothing about a queue.
    A backlog the owner is reminded of daily is a chore, and a chore gets abandoned;
  - empty is a valid, calm state. A section with nothing due is omitted entirely rather than
    rendered as an empty heading, and a page with nothing at all still renders — it just says
    so in one line;
  - it earns its place by REPLACING hunting, not by adding one more thing to keep up with.

Every region the owner can write in carries a stable anchor (`R1`, `C2`, `B3`) recorded in
`daily_anchors`, which is what lets the annotated page be mapped back to the right item. The
anchors are assigned in render order and persisted with the page, so pull-back never has to
re-derive what was on it — `daily_anchors` is the record, not a reconstruction.
"""

from __future__ import annotations

import sqlite3
from dataclasses import dataclass, field
from datetime import date, datetime, timezone

from locus.agent import state
from locus.learn import gaps as learn_gaps
from locus.learn import review as learn_review
from locus.link.related import related_documents

# §9 hard caps. Deliberately small: the page is a prompt to think, not an inbox.
MAX_CONNECTIONS = 3
MAX_RECALLS = 5
MAX_READINGS = 3
MAX_BLESSINGS = 3

# Categories whose documents are the owner's OWN recent capture — the "why now" that makes a
# connection worth surfacing today rather than any other day.
_CAPTURE_CATEGORIES = ("note",)


@dataclass
class Anchor:
    """A numbered, writable region on the page and what it refers to."""

    anchor: str          # 'R1' | 'C2' | 'B3' — stable within a page date
    kind: str            # 'recall' | 'connection' | 'reading' | 'blessing'
    target_kind: str     # 'review_item' | 'doc' | 'object'
    target_key: str      # stable string key, never a row id that a re-ingest changes
    label: str = ""


@dataclass
class Connection:
    anchor: str
    source_title: str
    source_date: str
    other_title: str
    why_now: str
    shared: tuple[str, ...]


@dataclass
class Recall:
    anchor: str
    prompt: str
    source: str
    item_id: int


@dataclass
class ReadNext:
    anchor: str
    title: str
    reason: str
    object_id: int


@dataclass
class Blessing:
    anchor: str
    object_id: int
    type_: str
    title: str
    why: str
    grounding: str


@dataclass
class DailyPage:
    page_date: str
    connections: list[Connection] = field(default_factory=list)
    recalls: list[Recall] = field(default_factory=list)
    readings: list[ReadNext] = field(default_factory=list)
    blessings: list[Blessing] = field(default_factory=list)
    anchors: list[Anchor] = field(default_factory=list)

    @property
    def is_empty(self) -> bool:
        return not (self.connections or self.recalls or self.readings or self.blessings)


def _utcnow() -> str:
    return datetime.now(timezone.utc).isoformat()


# --- section builders (each a pure read) ------------------------------------------------------


def _recent_capture(conn: sqlite3.Connection, *, limit: int) -> list[sqlite3.Row]:
    """The owner's most recently captured own-writing, newest first.

    Ordered by `source_date` (when it was WRITTEN) rather than ingest time — the 2026-07-29
    capture fix established that as the meaningful date for handwriting, and "why now" is a
    claim about when he was thinking about something, not when a timer ran.
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


def build_connections(conn: sqlite3.Connection, *, limit: int = MAX_CONNECTIONS) -> list[Connection]:
    """Cross-corpus links out of recent capture — a pure `related_documents` join.

    Only links that LEAVE the note cluster are surfaced. The captured notes formed their own
    mutual cluster (shared=10 among themselves, measured 2026-07-29), so a nearest-neighbour
    query over recent capture returns sibling notes almost every time — true, but not news to
    the person who wrote both. A connection earns a slot here by reaching something he did not
    write: a paper, a project, a lecture. Note<->note deserves its own surface, not this one.
    """
    out: list[Connection] = []
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
            pair = (min(src["id"], rel.doc_id), max(src["id"], rel.doc_id))
            if pair in seen_pairs:
                continue
            seen_pairs.add(pair)
            shared = rel.shared_names[:3]
            out.append(
                Connection(
                    anchor="",
                    source_title=src["title"] or src["source_uri"],
                    source_date=src["source_date"] or "",
                    other_title=rel.title,
                    why_now=(
                        f"you wrote this on {src['source_date']}; it shares "
                        f"{rel.shared_count} concept{'s' if rel.shared_count != 1 else ''} "
                        f"with a document you have not linked it to"
                    ),
                    shared=tuple(shared),
                )
            )
            break  # one connection per note, so three notes are represented, not one
    return out[:limit]


def build_recalls(
    conn: sqlite3.Connection, *, today: date | None = None, limit: int = MAX_RECALLS
) -> list[Recall]:
    """Due SM-2 items, soonest first — `due_items` + `resolve_prompt`, both plain joins."""
    out: list[Recall] = []
    for item in learn_review.due_items(conn, today=today, limit=limit):
        prompt, source = learn_review.resolve_prompt(conn, item)
        out.append(Recall(anchor="", prompt=prompt, source=source, item_id=item.id))
    return out


def build_readings(conn: sqlite3.Connection, *, limit: int = MAX_READINGS) -> list[ReadNext]:
    """Blessed reading objects ranked by how many gaps they close (§12.3: gap-driven, not FIFO).

    `learn.gaps` is deterministic and model-free, so this stays inside the aggregate-only rule.
    Only `active` (blessed) readings appear: offering the owner a reading he has not yet agreed
    is a thing to read would be proposing, which is not this page's job.
    """
    scored: list[tuple[int, ReadNext]] = []
    for obj in state.list_objects(conn, type_="reading", status="active", limit=50):
        found = learn_gaps.gaps_for_object(conn, obj.id, limit=3)
        if not found:
            continue
        top = found[0]
        scored.append(
            (
                len(found),
                ReadNext(
                    anchor="",
                    title=obj.title,
                    reason=f"closes a gap on {getattr(top, 'name', '')}".rstrip(),
                    object_id=obj.id,
                ),
            )
        )
    scored.sort(key=lambda p: -p[0])
    return [r for _, r in scored[:limit]]


def _format_grounding(link: state.ObjectLink) -> str:
    """Human-readable grounding for the page.

    An entity target_key is `name\\x1ftype` — a unit separator, chosen because it cannot occur
    in a real entity name. It is also invisible when printed, so the raw key renders as
    "order processing event loopconcept". Split it back out for the page.
    """
    if link.target_kind == "entity":
        name, type_ = state.parse_entity_key(link.target_key)
        return f"{name} ({type_})"
    if link.target_kind == "doc":
        return link.target_key.rsplit("/", 1)[-1]
    return link.target_key


def build_blessings(conn: sqlite3.Connection, *, limit: int = MAX_BLESSINGS) -> list[Blessing]:
    """The oldest proposals still awaiting a decision, capped hard at three.

    Oldest-first is the fair queue: a proposal the owner has walked past for weeks is the one
    most owed an answer, and it also guarantees the backlog drains in order instead of the same
    few recent items re-offering themselves nightly. The cap is what keeps 43 pending from
    becoming a wall — and the count is never printed (§9: no guilt metrics).
    """
    rows = conn.execute(
        "SELECT * FROM objects WHERE status='proposed' ORDER BY created_at, id LIMIT ?",
        (limit,),
    ).fetchall()
    out: list[Blessing] = []
    for row in rows:
        obj = state.get_object(conn, row["id"])
        if obj is None:
            continue
        body = obj.body or {}
        why = (
            body.get("why")
            or body.get("summary")
            or body.get("rationale")
            or f"proposed from {len(obj.links)} grounded source(s)"
        )
        grounding = _format_grounding(obj.links[0]) if obj.links else "(no grounding link)"
        out.append(
            Blessing(
                anchor="",
                object_id=obj.id,
                type_=obj.type,
                title=obj.title,
                why=str(why).strip(),
                grounding=grounding,
            )
        )
    return out


# --- composition -------------------------------------------------------------------------------


def compose(conn: sqlite3.Connection, *, today: date | None = None) -> DailyPage:
    """Build the day's page. Pure read — persisting it is `persist()`, a separate step."""
    today = today or date.today()
    page = DailyPage(page_date=today.isoformat())
    page.connections = build_connections(conn)
    page.recalls = build_recalls(conn, today=today)
    page.readings = build_readings(conn)
    page.blessings = build_blessings(conn)

    for i, c in enumerate(page.connections, 1):
        c.anchor = f"C{i}"
        page.anchors.append(
            Anchor(c.anchor, "connection", "doc", c.other_title, label=c.source_title)
        )
    for i, r in enumerate(page.recalls, 1):
        r.anchor = f"R{i}"
        page.anchors.append(
            Anchor(r.anchor, "recall", "review_item", str(r.item_id), label=r.prompt[:120])
        )
    for i, d in enumerate(page.readings, 1):
        d.anchor = f"D{i}"
        page.anchors.append(
            Anchor(d.anchor, "reading", "object", str(d.object_id), label=d.title)
        )
    for i, b in enumerate(page.blessings, 1):
        b.anchor = f"B{i}"
        page.anchors.append(
            Anchor(b.anchor, "blessing", "object", str(b.object_id), label=b.title)
        )
    return page


def persist(
    conn: sqlite3.Connection,
    page: DailyPage,
    *,
    md_path: str | None = None,
    pdf_path: str | None = None,
    source_run: int | None = None,
) -> None:
    """Record the page and its anchors so the annotated version can be mapped back.

    Rebuilding a date REPLACES its anchors: the physical page the owner will write on is the
    latest one, and a stale anchor row would silently route an answer to whatever used to sit
    at that number. Annotations are keyed by (date, anchor) independently and are NOT touched
    here — evidence the owner produced outlives a rebuild of the page that prompted it.
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


# --- rendering ---------------------------------------------------------------------------------

# Ruled lines the owner writes on. Long enough for a sentence in handwriting, short enough that
# four regions still fit a reMarkable page without scrolling.
#
# `***` on its own line, and a PLAIN blank line before it. The first draft used a
# non-breaking space as the spacer and `---` as the rule, and neither survived pandoc:
# U+00A0 is not markdown whitespace, so the spacer line counted as text and the `---`
# under it parsed as a SETEXT HEADING UNDERLINE rather than a thematic break. (A pair of
# `---` fences around a blank line has a second bad reading too - pandoc's YAML metadata
# block.) Verified 2026-07-30: zero `#horizontalrule` reached the Typst body, so the page
# offered blank space with no lines on it. `***` has neither ambiguity.
_RULE = "\n***\n"


def _rules(n: int) -> str:
    return _RULE * n


def render(page: DailyPage) -> str:
    """Markdown for the page. Anchors are printed verbatim so the pull-back can find them."""
    lines: list[str] = [f"# {page.page_date}", ""]

    if page.is_empty:
        lines += ["Nothing to surface today.", ""]
        return "\n".join(lines)

    if page.connections:
        lines += ["## Connections", ""]
        for c in page.connections:
            lines += [
                f"**{c.anchor}. {c.source_title} → {c.other_title}**",
                "",
                f"*{c.why_now}*",
            ]
            if c.shared:
                lines += ["", f"Shared: {', '.join(c.shared)}"]
            lines += ["", _rules(2)]

    if page.recalls:
        lines += ["## Recall", ""]
        for r in page.recalls:
            lines += [f"**{r.anchor}.** {r.prompt}"]
            if r.source:
                lines += ["", f"*{r.source}*"]
            lines += ["", _rules(3)]

    if page.readings:
        lines += ["## Read next", ""]
        for d in page.readings:
            lines += [f"**{d.anchor}.** {d.title}", "", f"*{d.reason}*", "", _rules(1)]

    if page.blessings:
        lines += [
            "## Awaiting your call",
            "",
            "Tick to bless. Write to correct — corrections apply whether or not you tick.",
            "",
        ]
        for b in page.blessings:
            lines += [
                # ASCII brackets, not U+2610 BALLOT BOX: the Typst body font has no glyph for
                # it and it rendered as a tofu box (verified 2026-07-30 — the character was
                # absent from the extracted text while a stray rectangle was drawn in its
                # place). A tick box the owner must recognise cannot depend on font coverage.
                f"**{b.anchor}.**  `[   ]`   *{b.type_}* — {b.title}",
                "",
                f"{b.why}",
                "",
                f"Grounded in: {b.grounding}",
                "",
                _rules(3),
            ]

    return "\n".join(lines)
