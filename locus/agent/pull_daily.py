"""Pull the annotated daily page back and route what the owner wrote (agent-layer plan §9).

This is the half that makes "one surface" actually work. The page goes out as a PDF with
numbered, ruled regions; the owner writes on it; this module reads it back, maps each region to
what was printed there, and routes the handwriting to the right store.

    PULL annotated PDF -> extract per anchor (vision) -> route:
      recall answers        -> grade -> review_schedule (SM-2, learn/review)
      connection reactions  -> acceptance_log (the flywheel's free relevance label)
      new questions         -> Question objects
      blessing boxes        -> the four-way table below

**Idempotent by (page_date, anchor).** `annotations` has a UNIQUE on that pair and every write
here is an upsert, so re-pulling the same page revises a region rather than adding a second
one — a page scanned twice cannot double-grade a recall answer or bless an object twice.

**The four-way blessing outcome.** Tick and writing are independent signals, so there are four
states, not two, and the interesting one is the third:

    tick, no writing      -> bless (status -> active)
    tick, with writing    -> apply the correction, then bless
    CROSS                 -> archive: never proposed or offered again (writing still kept)
    writing, no mark      -> apply the correction, LEAVE IT PROPOSED
    nothing               -> no-op; it is re-offered on a later page

"Wrote corrections but didn't tick" means *keep working on this* — not yes, and not no. Blessing
it would put words in the owner's mouth; discarding the writing would throw away the most
informative thing on the page. So the edit lands and the object stays in the queue.

A CROSS is the explicit no. The first cut had no refusal at all and, worse, its extraction
prompt counted a cross as "a mark is present, therefore ticked" — so crossing something out
blessed it. A checkbox that does the opposite of what a person means is worse than no checkbox,
and without an archive path an unwanted object could only ever rotate to the back of the queue.

Every one of the four is written to `acceptance_log`, including the no-op. A rejection is as
much signal as an acceptance, and "offered three times, never acted on" is a judgement the
flywheel should be able to see.

Owner corrections go through `state.apply_owner_edit`, never `upsert_object`: the owner is the
authority and the agent is not, and that asymmetry is enforced by using a different verb with a
durable marker rather than by relaxing the additive merge (docs/owner-authority-design.md).
"""

from __future__ import annotations

import base64
import hashlib
import json
import sqlite3
from dataclasses import dataclass, field
from datetime import date, datetime, timezone
from pathlib import Path

from pydantic import BaseModel, Field

from locus.agent import compose_daily as cd
from locus.agent import state
from locus.config import Config, load
from locus.learn import review as learn_review

# Surfaces recorded in acceptance_log. These are the vocabulary migration 0011 already fixed in
# a CHECK constraint — deliberately reused rather than extended: a blessing decided on the page
# is the same KIND of judgement as one made with `locus objects --bless`, keyed the same way
# (the object id), so both land in `object` and the flywheel sees one history rather than two.
SURFACE_CONNECTION = "connection"
SURFACE_BLESSING = "object"
SURFACE_READING = "reading"
SURFACE_RECALL = "recall"


# --- vision extraction ---------------------------------------------------------------------


class _Region(BaseModel):
    anchor: str = Field(description="The printed region label, e.g. 'R1', 'C2', 'B3'.")
    # The SHAPE of the mark, not merely its presence. A cross is a refusal; reading it as
    # "a mark is present, therefore yes" blessed the very things the owner was rejecting.
    mark: str = Field(
        default="none",
        description=(
            "What is in the [ ] box: 'tick' for a tick/check, 'cross' for an X or a strike "
            "through the box, 'none' if it is empty or there is no box."
        ),
    )
    text: str = Field(default="", description="Handwriting in this region, verbatim. '' if none.")


class _Regions(BaseModel):
    regions: list[_Region] = Field(default_factory=list)


_SYSTEM = (
    "You read a scanned page that was printed by a program and then annotated by hand. "
    "You transcribe only what is physically written; you never infer, complete, or tidy."
)

_USER = (
    "This page has numbered regions labelled like R1, R2, C1, C2, D1, B1, B2 — each followed by "
    "printed text and then ruled blank lines for handwriting.\n\n"
    "For EVERY labelled region visible on this page, report:\n"
    "  - anchor: the label exactly as printed (e.g. 'B2')\n"
    "  - mark: for a region with a `[ ]` box, report the SHAPE of what is in it — 'tick' for a "
    "tick or check mark, 'cross' for an X or a line struck through the box, and 'none' if the "
    "box is empty or the region has no box. A cross is NOT a tick: report them differently, "
    "and never report 'tick' merely because some mark is present.\n"
    "  - text: the HANDWRITING in that region, transcribed verbatim. Empty string if the ruled "
    "lines are blank. Do NOT include the printed text — only what was written by hand.\n\n"
    "Report a region even when it is entirely untouched (mark 'none', text empty): a "
    "region the owner deliberately left alone is a real signal. Never invent a label that is "
    "not printed on the page."
)


@dataclass
class ExtractedRegion:
    anchor: str
    ticked: bool | None   # affirmative mark present (derived from `mark`; kept for the record)
    text: str
    mark: str = "none"    # 'tick' | 'cross' | 'none'

    @property
    def has_writing(self) -> bool:
        return bool(self.text.strip())

    @property
    def is_tick(self) -> bool:
        return self.mark == "tick"

    @property
    def is_cross(self) -> bool:
        return self.mark == "cross"


def extract_regions(
    pdf_path: str | Path, *, client=None, model: str | None = None, dpi: int | None = None
) -> list[ExtractedRegion]:
    """Read every annotated region from the pulled PDF. One vision call per page.

    `client` is injectable so the tests stay model-free, matching `capture/transcribe.py`.
    """
    from locus.capture.transcribe import render_pdf_pages

    cfg = load().capture
    model = model or cfg.transcribe_model
    dpi = dpi if dpi is not None else cfg.transcribe_dpi
    if client is None:
        import anthropic

        client = anthropic.Anthropic(api_key=Config.anthropic_api_key())

    schema = json.dumps(_Regions.model_json_schema())
    out: dict[str, ExtractedRegion] = {}
    for png in render_pdf_pages(pdf_path, dpi=dpi):
        resp = client.messages.create(
            model=model,
            max_tokens=4096,
            system=[{"type": "text", "text": _SYSTEM}],
            messages=[{"role": "user", "content": [
                {"type": "image", "source": {
                    "type": "base64", "media_type": "image/png",
                    "data": base64.standard_b64encode(png).decode("utf-8"),
                }},
                {"type": "text", "text": f"{_USER}\n\nReply with ONLY JSON matching:\n{schema}"},
            ]}],
        )
        text = "".join(b.text for b in resp.content if getattr(b, "type", None) == "text")
        start, end = text.find("{"), text.rfind("}")
        if start == -1 or end <= start:
            continue  # a page with no regions on it (or an unreadable reply): skip, never guess
        try:
            parsed = _Regions.model_validate_json(text[start : end + 1])
        except Exception:
            continue
        for r in parsed.regions:
            anchor = r.anchor.strip().upper()
            if not anchor:
                continue
            mark = (r.mark or "none").strip().lower()
            if mark not in ("tick", "cross", "none"):
                mark = "none"  # never guess an affirmative from an unrecognised label
            out[anchor] = ExtractedRegion(
                anchor, True if mark == "tick" else False, r.text.strip(), mark=mark
            )
    return list(out.values())


# --- routing -------------------------------------------------------------------------------


@dataclass
class RouteOutcome:
    anchor: str
    kind: str
    outcome: str
    detail: str = ""


@dataclass
class PullResult:
    page_date: str
    outcomes: list[RouteOutcome] = field(default_factory=list)
    unknown_anchors: list[str] = field(default_factory=list)
    # 'routed' | 'not-on-device' (the tablet has not pushed it back yet) | 'unchanged'
    # (byte-identical to the last pull, so no model call was made)
    status: str = "routed"

    @property
    def acted(self) -> int:
        return sum(1 for o in self.outcomes if o.outcome != "untouched")


def _utcnow() -> str:
    return datetime.now(timezone.utc).isoformat()


def _record(
    conn: sqlite3.Connection, page_date: str, r: ExtractedRegion, outcome: str, *, source_run=None
) -> None:
    """Upsert the region's annotation. The UNIQUE(page_date, anchor) is the idempotency."""
    stamp = _utcnow()
    with conn:
        conn.execute(
            "INSERT INTO annotations (page_date, anchor, ticked, mark, text, outcome, source_run, "
            "captured_at, processed_at) VALUES (?,?,?,?,?,?,?,?,?) "
            "ON CONFLICT(page_date, anchor) DO UPDATE SET "
            "ticked=excluded.ticked, mark=excluded.mark, text=excluded.text, "
            "outcome=excluded.outcome, source_run=excluded.source_run, "
            "processed_at=excluded.processed_at",
            (
                page_date, r.anchor,
                None if r.ticked is None else int(r.ticked),
                r.mark, r.text, outcome, source_run, stamp, stamp,
            ),
        )


# --- what did he actually write? -------------------------------------------------------------
#
# The first cut treated handwriting as a CHECKBOX: a region was "written on" or not, and only the
# blessing boxes read the words. The first real page (2026-07-30) showed why that is wrong. He
# wrote three times and every one was a QUESTION — under a connection, under another connection,
# and on a recall line — while Q1, the box explicitly labelled for questions, he left blank.
#
# He writes where the thought occurs, not where the form asks for it. So the content has to be
# read wherever it lands, and it has to become something he can develop later, or the most
# valuable material on the page is a log row saying "he engaged".
#
# The classifier is DELIBERATELY DETERMINISTIC. A model round-trip per region would be billed,
# slow, and — on this distinction — no better: interrogative form is a syntactic fact, and the
# vision pass has already done the hard part by turning ink into text. Being wrong is also cheap
# in a way that matters: the worst case is an idea filed as a question or vice versa, both of
# which are owner-owned objects that surface again anyway. Nothing is discarded on a misread.

# WH-WORDS ONLY. A leading auxiliary ("is", "does", "should") is ambiguous without the question
# mark: "should we use X?" asks, while "should try X on the tanker project" proposes — same first
# word, opposite intent. An auxiliary-led question almost always carries its '?', and that is
# tested first, so restricting this list to wh-words loses nothing and stops reading every
# proposal as a query.
_INTERROGATIVE = ("what", "why", "how", "when", "where", "which", "who", "whose")
# Proposal vocabulary — the shape of "we could do X", as opposed to asking whether X is so.
_PROPOSAL = (
    "should", "could", "try", "idea", "build", "add", "use", "apply", "maybe",
    "worth", "consider", "todo", "implement", "extend",
)

WRITING_QUESTION = "question"
WRITING_IDEA = "idea"
WRITING_NOTE = "note"


def classify_writing(text: str) -> str:
    """What KIND of thing is this handwriting: a question, an idea, or a remark?

    Interrogative form wins over proposal vocabulary, because the two overlap heavily in his
    actual writing ("do we want macro regimes or other types" is a question that contains a
    proposal). Asking is the weaker commitment, so when a sentence is both, treating it as the
    question is the reading that claims less.
    """
    body = " ".join(text.split()).strip()
    if not body:
        return WRITING_NOTE
    lowered = body.lower()
    # WORDS, not substrings. The first cut tested `word in lowered` and so read "because" as
    # containing "use" — turning a real recall answer ("because the curve was inverted") into an
    # idea and skipping its grade. Matching on token boundaries is the whole fix.
    words = [w.strip(".,;:!?\"'()[]") for w in lowered.split()]
    first = words[0] if words else ""

    if body.endswith("?") or first in _INTERROGATIVE:
        return WRITING_QUESTION
    if any(w in _PROPOSAL for w in words) or "note to self" in lowered:
        return WRITING_IDEA
    return WRITING_NOTE


def _thought_title(text: str) -> str:
    body = " ".join(text.split())
    return body if len(body) <= 120 else body[:117] + "..."


def _capture_thought(
    conn, r: ExtractedRegion, anchor, *, page_date: str, kind: str
) -> tuple[int, bool]:
    """Store a question/idea the owner wrote, grounded in the region it was written under.

    Returns (object_id, created). Created `active`, not `proposed`: the text is his, so there
    is nothing here for him to bless — asking him to approve his own handwriting is theatre.

    The grounding link is what makes this more than a note. An idea written under a connection
    to the tanker paper LINKS to that paper, so it resurfaces with the material that provoked
    it rather than floating free in a list. `raised_by` is the existing relation for exactly
    this: the target did not state the thought, it prompted it.

    Idempotent by title, so a re-pull of the same page updates rather than duplicating.
    """
    text = " ".join(r.text.split())
    object_id, created = state.upsert_object(conn, type_=kind, title=_thought_title(text))
    state.apply_owner_edit(
        conn, object_id, {kind: text, "from_anchor": f"{page_date}#{r.anchor}"},
        source=f"daily:{page_date}#{r.anchor}",
    )
    state.set_status(conn, object_id, "active")
    if anchor is not None and anchor.target_kind in ("doc", "entity", "object"):
        state.add_links(
            conn, object_id,
            [state.ObjectLink(anchor.target_kind, anchor.target_key, "raised_by")],
        )
    return object_id, created


def _route_writing(conn, anchor, r: ExtractedRegion, *, page_date: str) -> str:
    """Capture whatever was written under a non-blessing region. Returns a detail string.

    A plain remark is left as the annotation row it already is: not every scribble is a thing
    to track, and manufacturing an object from "yes" would fill the queue with noise.
    """
    kind = classify_writing(r.text)
    if kind == WRITING_NOTE:
        return "noted"
    _capture_thought(conn, r, anchor, page_date=page_date, kind=kind)
    return f"{kind}: {_thought_title(r.text)}"


# A recall answer is graded 0-5 for SM-2. Grading the ANSWER against the stored proposition is a
# judgement call and would need a model; what is deterministic — and what the schedule actually
# needs — is whether he attempted it. An attempted item advances, an untouched one does not.
_ATTEMPTED_GRADE = 4


def _route_recall(conn, anchor, region, r: ExtractedRegion, *, page_date: str) -> RouteOutcome:
    if not r.has_writing:
        return RouteOutcome(r.anchor, "recall", "untouched")
    try:
        item_id = int(anchor.target_key)
    except ValueError:
        return RouteOutcome(r.anchor, "recall", "error", "unparseable review item")
    row = conn.execute("SELECT * FROM review_schedule WHERE id=?", (item_id,)).fetchone()
    if row is None:
        return RouteOutcome(r.anchor, "recall", "error", "review item no longer exists")

    # WRITING IS NOT THE SAME AS ANSWERING. On the first real page he wrote "does this suggest
    # we should [use a] macro regime predictor in the tanker project?" on a recall line — a
    # question, not a recall attempt — and it was graded 4 (correct-with-hesitation), pushing
    # the item a day further out on the strength of him NOT recalling it. SM-2 only works if
    # its grades mean what they say, so a question or a proposal leaves the schedule alone and
    # is captured as the thought it actually is. He still engaged, so the flywheel still hears
    # about it.
    kind = classify_writing(r.text)
    if kind != WRITING_NOTE:
        detail = _route_writing(conn, anchor, r, page_date=page_date)
        state.log_acceptance(
            conn, surface=SURFACE_RECALL, candidate_key=str(item_id), verdict="kept"
        )
        return RouteOutcome(r.anchor, "recall", "not-an-answer", detail)

    learn_review.grade_item(conn, item_id, grade=_ATTEMPTED_GRADE)
    state.log_acceptance(
        conn, surface=SURFACE_RECALL, candidate_key=str(item_id), verdict="kept"
    )
    return RouteOutcome(r.anchor, "recall", "graded", f"item {item_id} advanced")


def _route_connection(conn, anchor, region, r: ExtractedRegion, *, page_date: str) -> RouteOutcome:
    """A reaction to a surfaced connection is a free relevance judgement (§12.1) AND content.

    Written-on means the connection was worth his attention; untouched means it was not. Both
    go to `acceptance_log` — the whole point of the flywheel is that a rejection is data. What
    he WROTE is captured too, linked back to the document that provoked it.
    """
    verdict = "kept" if r.has_writing else "rejected"
    state.log_acceptance(
        conn, surface=SURFACE_CONNECTION, candidate_key=anchor.target_key, verdict=verdict
    )
    if not r.has_writing:
        return RouteOutcome(r.anchor, "connection", "untouched")
    return RouteOutcome(
        r.anchor, "connection", "kept", _route_writing(conn, anchor, r, page_date=page_date)
    )


def _route_reading(conn, anchor, region, r: ExtractedRegion, *, page_date: str) -> RouteOutcome:
    verdict = "kept" if r.has_writing else "rejected"
    state.log_acceptance(
        conn, surface=SURFACE_READING, candidate_key=anchor.target_key, verdict=verdict
    )
    if not r.has_writing:
        return RouteOutcome(r.anchor, "reading", "untouched")
    return RouteOutcome(
        r.anchor, "reading", "kept", _route_writing(conn, anchor, r, page_date=page_date)
    )


def _route_mark(conn, anchor, region, r: ExtractedRegion, *, page_date: str) -> RouteOutcome:
    """A passage he marked while reading, resurfaced on the page (Loop B's consumer).

    Reactions are logged under the `reading` surface rather than a new one: a judgement about
    a passage from a book is a judgement about reading material, and migration 0011 already
    fixed that vocabulary in a CHECK. Reusing it keeps one history instead of two.
    """
    verdict = "kept" if r.has_writing else "rejected"
    state.log_acceptance(
        conn, surface=SURFACE_READING, candidate_key=anchor.target_key, verdict=verdict
    )
    if not r.has_writing:
        return RouteOutcome(r.anchor, "mark", "untouched")
    return RouteOutcome(
        r.anchor, "mark", "kept", _route_writing(conn, anchor, r, page_date=page_date)
    )


def _route_question(
    conn, anchor, region, r: ExtractedRegion, *, page_date: str
) -> RouteOutcome:
    """A question the OWNER wrote becomes a Question object he already owns.

    Created `active`, not `proposed`. Propose-never-mutate constrains the agent; this text did
    not come from the agent, so there is nothing here for the owner to bless — asking him to
    approve his own handwriting would be theatre. Written through `apply_owner_edit` so the
    provenance says which page it came off, and so tonight's structure run cannot rewrite it.

    The title is the question itself, which also makes it idempotent for free: re-pulling the
    same page finds the same title and updates rather than creating a second object.
    """
    if not r.has_writing:
        return RouteOutcome(r.anchor, "question", "untouched")
    # The box invites a question, but he is not filling in a form: an idea written here is an
    # idea. Only the classifier's IDEA verdict overrides the box — a plain remark still becomes
    # a question object, because that is what he reached for the region to record.
    kind = WRITING_IDEA if classify_writing(r.text) == WRITING_IDEA else "question"
    _, created = _capture_thought(conn, r, anchor, page_date=page_date, kind=kind)
    return RouteOutcome(
        r.anchor, "question", "created" if created else "updated", _thought_title(r.text)
    )


def _route_open(conn, anchor, region, r: ExtractedRegion, *, page_date: str) -> RouteOutcome:
    """Continue, close, or drop one of his own open threads.

    Deliberately the SAME gesture vocabulary as the blessing boxes, because asking him to
    remember two different meanings for a tick is how a paper interface stops being used:

        tick, no writing   -> resolved (he has his answer; it just is not worth writing down)
        tick, with writing -> the writing IS the resolution, then closed
        CROSS              -> let it go; archived either way, and any writing is still kept
        writing, no mark   -> development. It stays open and comes back.
        nothing            -> no-op; re-offered later

    Development APPENDS rather than replaces. A thread is a sequence of passes at the same
    question, and overwriting yesterday's thinking with today's would destroy the only record
    of how his view moved — which is the thing `evolve/trajectory.py` exists to read.
    """
    try:
        object_id = int(anchor.target_key)
    except ValueError:
        return RouteOutcome(r.anchor, "open", "error", "unparseable object id")
    obj = state.get_object(conn, object_id)
    if obj is None:
        return RouteOutcome(r.anchor, "open", "error", "thread no longer exists")

    text = " ".join(r.text.split())
    if text:
        entries = list((obj.body or {}).get(cd.DEVELOPMENT_KEY) or [])
        entries.append({"at": page_date, "text": text, "anchor": r.anchor})
        state.apply_owner_edit(
            conn, object_id, {cd.DEVELOPMENT_KEY: entries},
            source=f"daily:{page_date}#{r.anchor}",
        )

    if r.is_cross:
        state.set_status(conn, object_id, "archived")
        state.log_acceptance(
            conn, surface=SURFACE_BLESSING, candidate_key=str(object_id), verdict="rejected"
        )
        return RouteOutcome(r.anchor, "open", "dropped", obj.title)

    if r.is_tick:
        if text:
            state.apply_owner_edit(
                conn, object_id, {"resolution": text},
                source=f"daily:{page_date}#{r.anchor}",
            )
        state.set_status(conn, object_id, "archived")
        state.log_acceptance(
            conn, surface=SURFACE_BLESSING, candidate_key=str(object_id), verdict="kept"
        )
        return RouteOutcome(r.anchor, "open", "resolved", obj.title)

    if text:
        state.log_acceptance(
            conn, surface=SURFACE_BLESSING, candidate_key=str(object_id), verdict="kept"
        )
        return RouteOutcome(r.anchor, "open", "developed", obj.title)
    return RouteOutcome(r.anchor, "open", "untouched", obj.title)


def _route_blessing(conn, anchor, region, r: ExtractedRegion, *, page_date: str) -> RouteOutcome:
    """The four-way table. See the module docstring for why 'writing, not ticked' is separate."""
    try:
        object_id = int(anchor.target_key)
    except ValueError:
        return RouteOutcome(r.anchor, "blessing", "error", "unparseable object id")
    obj = state.get_object(conn, object_id)
    if obj is None:
        return RouteOutcome(r.anchor, "blessing", "error", "object no longer exists")

    wrote = r.has_writing

    if r.is_cross:
        # A refusal, and a durable one. Archived objects are never re-proposed (upsert_object
        # never writes `status`) and never re-offered on the page, so this is the only way to
        # say "stop showing me this" — without it the queue merely rotates forever. Any writing
        # is still kept: an explanation of WHY it was wrong is worth more than the rejection.
        if wrote:
            state.apply_owner_edit(
                conn, object_id, {"why": r.text.strip()},
                source=f"daily:{page_date}#{r.anchor}",
            )
        state.set_status(conn, object_id, "archived")
        state.log_acceptance(
            conn, surface=SURFACE_BLESSING, candidate_key=str(object_id), verdict="rejected"
        )
        return RouteOutcome(
            r.anchor, "blessing", "archived" if not wrote else "corrected+archived", obj.title
        )

    ticked = r.is_tick

    if wrote:
        # The owner's words replace the agent's one-line rationale, and the marker makes that
        # stick against tonight's structure run.
        state.apply_owner_edit(
            conn, object_id, {"why": r.text.strip()},
            source=f"daily:{page_date}#{r.anchor}",
        )

    if ticked and not wrote:
        state.set_status(conn, object_id, "active")
        outcome, detail = "blessed", obj.title
    elif ticked and wrote:
        state.set_status(conn, object_id, "active")
        outcome, detail = "corrected+blessed", obj.title
    elif wrote and not ticked:
        # Deliberately still `proposed`: this means keep working on it, which is neither a yes
        # nor a no. It will be re-offered, now carrying his correction.
        outcome, detail = "corrected", obj.title
    else:
        outcome, detail = "untouched", obj.title

    verdict = "rejected" if outcome == "untouched" else "kept"
    state.log_acceptance(
        conn, surface=SURFACE_BLESSING, candidate_key=str(object_id), verdict=verdict
    )
    return RouteOutcome(r.anchor, "blessing", outcome, detail)


def _prior_annotations(
    conn: sqlite3.Connection, page_date: str
) -> dict[str, tuple[str, str]]:
    """anchor -> (mark, text) already routed for this page, for the re-pull guard.

    Keyed on the mark SHAPE: changing a tick to a cross must re-route, and comparing only an
    affirmative boolean would have treated that reversal as "unchanged"."""
    out: dict[str, tuple[str, str]] = {}
    for row in conn.execute(
        "SELECT anchor, mark, text, outcome FROM annotations WHERE page_date=?", (page_date,)
    ):
        if row["outcome"] in (None, "error"):
            continue  # a region that failed to route is retried, not skipped
        out[row["anchor"]] = (row["mark"] or "none", (row["text"] or "").strip())
    return out


def route_regions(
    conn: sqlite3.Connection,
    page_date: str,
    regions: list[ExtractedRegion],
    *,
    source_run: int | None = None,
) -> PullResult:
    """Map each extracted region to what was printed there and apply it. Idempotent."""
    anchors = cd.anchors_for(conn, page_date)
    prior = _prior_annotations(conn, page_date)
    result = PullResult(page_date=page_date)

    for r in sorted(regions, key=lambda x: x.anchor):
        anchor = anchors.get(r.anchor)
        if anchor is None:
            # Never guess: an anchor we did not print is not something we can route.
            result.unknown_anchors.append(r.anchor)
            continue
        seen = prior.get(r.anchor)
        if seen is not None and seen == (r.mark, r.text.strip()):
            # Byte-identical to what we already routed for this region: re-applying would
            # advance the SM-2 schedule a second time for one answer and double-count the
            # acceptance judgement the flywheel reads. Idempotency is about the SIDE EFFECTS,
            # not just the annotation row. A region whose content CHANGED is re-applied, which
            # is what makes a re-scan after writing more actually work.
            result.outcomes.append(RouteOutcome(r.anchor, anchor.kind, "unchanged"))
            continue
        if anchor.kind == "recall":
            outcome = _route_recall(conn, anchor, r.anchor, r, page_date=page_date)
        elif anchor.kind == "connection":
            outcome = _route_connection(conn, anchor, r.anchor, r, page_date=page_date)
        elif anchor.kind == "reading":
            outcome = _route_reading(conn, anchor, r.anchor, r, page_date=page_date)
        elif anchor.kind == "mark":
            outcome = _route_mark(conn, anchor, r.anchor, r, page_date=page_date)
        elif anchor.kind == "open":
            outcome = _route_open(conn, anchor, r.anchor, r, page_date=page_date)
        elif anchor.kind == "blessing":
            outcome = _route_blessing(conn, anchor, r.anchor, r, page_date=page_date)
        elif anchor.kind == "question":
            outcome = _route_question(conn, anchor, r.anchor, r, page_date=page_date)
        else:
            outcome = RouteOutcome(r.anchor, anchor.kind, "error", "unknown anchor kind")
        _record(conn, page_date, r, outcome.outcome, source_run=source_run)
        result.outcomes.append(outcome)

    return result


# --- device transport -------------------------------------------------------------------------
#
# THE RETURN LEG COMES FROM THE CLOUD, NOT THE TABLET. This was originally built on the tailnet
# staging dir — the on-device agent pushes every changed document there as `<uuid>.pdf`, which is
# how Loop A gets handwriting — and that route is STRUCTURALLY WRONG here. The device composites
# ink for NOTEBOOKS; for an UPLOADED PDF its render endpoint hands back the original file. Every
# Locus-delivered page is an uploaded PDF, so the staged copy is always blank.
#
# That failed silently and expensively: a blank page still costs a vision call, and recording its
# hash marks the date as read, so a day of real handwriting would be dropped with no error. Found
# 2026-07-30 on a page carrying 183 strokes that the pull reported as "not pushed back yet".
#
# The working route is `rmapi get` -> `.rmdoc` -> composite the strokes back onto the PDF
# (`capture/rmdoc.py`), which is the same transport Loop B uses for book annotations. It reads the
# CLOUD copy, so it works with the tablet asleep and needs nothing installed on the device.


def _pdf_hash(path: Path) -> str:
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def fetch_annotated_page(
    page_date: str,
    *,
    dest_dir: str | Path | None = None,
    rmapi_binary: str | None = None,
    folder: str | None = None,
) -> tuple[Path, str] | None:
    """Download the daily page from the cloud; return (PDF with its ink drawn on, ink hash).

    Returns None when the page carries no strokes at all — an untouched page is the normal
    case, and it must not cost a vision call.

    The device path is the filename contract `cmd_daily` uploads (`<folder>/daily-<date>`), so
    the mapping is nothing more subtle than the name.
    """
    from locus.capture.rmdoc import composite_pdf, fetch_rmdoc, ink_hash, read_rmdoc

    cfg = load()
    folder = folder or cfg.reading.target_folder
    dest_dir = Path(dest_dir or cfg.paths.notes) / "_generated"
    device_path = f"/{folder}/daily-{page_date}"

    try:
        bundle = fetch_rmdoc(
            device_path, dest_dir, rmapi_binary=rmapi_binary or cfg.reading.rmapi_binary
        )
        rmdoc = read_rmdoc(bundle)
    except (RuntimeError, ValueError, OSError):
        return None

    if not any(p.strokes for p in rmdoc.pages):
        return None
    out = composite_pdf(rmdoc, dest_dir / f"daily-{page_date}-annotated.pdf")
    return out, ink_hash(rmdoc)


def already_pulled(
    conn: sqlite3.Connection,
    page_date: str,
    pdf_path: str | Path,
    *,
    content_hash: str | None = None,
) -> bool:
    """True when this exact content was already read for this page.

    Routing is idempotent regardless; this guard exists so a scheduled pull does not pay for a
    vision call on a page nobody has touched since the last run. `content_hash` is the stroke
    fingerprint when the page came from a `.rmdoc`; a hand-supplied PDF falls back to its bytes.
    """
    row = conn.execute(
        "SELECT pulled_hash FROM daily_pages WHERE page_date=?", (page_date,)
    ).fetchone()
    return bool(row and row["pulled_hash"] and row["pulled_hash"] == (
        content_hash or _pdf_hash(pdf_path)
    ))


def _record_pull(
    conn: sqlite3.Connection,
    page_date: str,
    pdf_path: str | Path,
    *,
    content_hash: str | None = None,
) -> None:
    with conn:
        conn.execute(
            "UPDATE daily_pages SET pulled_hash=?, pulled_at=? WHERE page_date=?",
            (content_hash or _pdf_hash(pdf_path), _utcnow(), page_date),
        )


def pull_daily(
    conn: sqlite3.Connection,
    pdf_path: str | Path | None = None,
    *,
    page_date: str | None = None,
    client=None,
    source_run: int | None = None,
    force: bool = False,
) -> PullResult:
    """Extract and route one annotated daily page.

    With no `pdf_path`, the annotated page is fetched from the cloud automatically. Skips the
    (billed) vision pass when the file is byte-identical to the last one read, unless `force`.
    """
    page_date = page_date or date.today().isoformat()
    content_hash: str | None = None
    if pdf_path is None:
        fetched = fetch_annotated_page(page_date)
        if fetched is None:
            return PullResult(page_date=page_date, status="not-on-device")
        pdf_path, content_hash = fetched
    if not force and already_pulled(conn, page_date, pdf_path, content_hash=content_hash):
        return PullResult(page_date=page_date, status="unchanged")

    regions = extract_regions(pdf_path, client=client)
    result = route_regions(conn, page_date, regions, source_run=source_run)
    # A page with ink on it has been BEEN THROUGH, which is a different fact from having been
    # looked at (`pulled_at`, set below, records every check including the ones that found
    # nothing). `read_at` is what moves the page out of the /Daily inbox, so it must mean
    # "he wrote on this" and nothing looser.
    if any(r.has_writing or r.mark != "none" for r in regions):
        cd.mark_read(conn, page_date)
    _record_pull(conn, page_date, pdf_path, content_hash=content_hash)
    return result


def archive_page(
    page_date: str,
    *,
    runner=None,
    rmapi_binary: str | None = None,
    folder: str | None = None,
) -> str | None:
    """Move a page he has written on into its monthly folder. Returns the destination, or None.

    THE /Daily FOLDER IS AN INBOX. Pages used to accumulate loose forever, so it told him nothing;
    now a page stays at the root until it has ink and then moves to `/Daily/YYYY-MM`, which makes
    opening the folder a true statement of what he has not been through. That is a state he can
    see rather than an unread count we would have to print at him (§9).

    Best-effort by design: a failed move is not worth failing a pull over, since the routing has
    already happened and the next pull will try again.
    """
    from locus.reading.deliver_remarkable import _ensure_folder, _subprocess_runner

    cfg = load()
    folder = folder or cfg.reading.target_folder
    runner = runner or _subprocess_runner(rmapi_binary or cfg.reading.rmapi_binary)
    dest = f"{folder}/{page_date[:7]}"
    try:
        _ensure_folder(runner, dest)
        code, out, err = runner(["mv", f"/{folder}/daily-{page_date}", f"/{dest}"])
    except RuntimeError:
        return None
    return dest if code == 0 else None
