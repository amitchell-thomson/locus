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

    ticked, no writing    -> bless (status -> active)
    ticked, with writing  -> apply the correction, then bless
    writing, not ticked   -> apply the correction, LEAVE IT PROPOSED
    neither               -> no-op; it is re-offered on a later page

"Wrote corrections but didn't tick" means *keep working on this* — not yes, and not no. Blessing
it would put words in the owner's mouth; discarding the writing would throw away the most
informative thing on the page. So the edit lands and the object stays in the queue.

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
    ticked: bool | None = Field(
        default=None,
        description="True if the [ ] box is ticked, False if clearly empty, null if no box.",
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
    "  - ticked: for a region with a `[ ]` box — true if there is a tick/cross/mark inside it, "
    "false if it is clearly empty. Use null if the region has no box at all.\n"
    "  - text: the HANDWRITING in that region, transcribed verbatim. Empty string if the ruled "
    "lines are blank. Do NOT include the printed text — only what was written by hand.\n\n"
    "Report a region even when it is entirely untouched (ticked false/null, text empty): a "
    "region the owner deliberately left alone is a real signal. Never invent a label that is "
    "not printed on the page."
)


@dataclass
class ExtractedRegion:
    anchor: str
    ticked: bool | None
    text: str

    @property
    def has_writing(self) -> bool:
        return bool(self.text.strip())


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
            if anchor:
                out[anchor] = ExtractedRegion(anchor, r.ticked, r.text.strip())
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
            "INSERT INTO annotations (page_date, anchor, ticked, text, outcome, source_run, "
            "captured_at, processed_at) VALUES (?,?,?,?,?,?,?,?) "
            "ON CONFLICT(page_date, anchor) DO UPDATE SET "
            "ticked=excluded.ticked, text=excluded.text, outcome=excluded.outcome, "
            "source_run=excluded.source_run, processed_at=excluded.processed_at",
            (
                page_date, r.anchor,
                None if r.ticked is None else int(r.ticked),
                r.text, outcome, source_run, stamp, stamp,
            ),
        )


# A recall answer is graded 0-5 for SM-2. Grading the ANSWER against the stored proposition is a
# judgement call and would need a model; what is deterministic — and what the schedule actually
# needs — is whether he attempted it. An attempted item advances, an untouched one does not.
_ATTEMPTED_GRADE = 4


def _route_recall(conn, anchor, region, r: ExtractedRegion) -> RouteOutcome:
    if not r.has_writing:
        return RouteOutcome(r.anchor, "recall", "untouched")
    try:
        item_id = int(anchor.target_key)
    except ValueError:
        return RouteOutcome(r.anchor, "recall", "error", "unparseable review item")
    row = conn.execute("SELECT * FROM review_schedule WHERE id=?", (item_id,)).fetchone()
    if row is None:
        return RouteOutcome(r.anchor, "recall", "error", "review item no longer exists")
    learn_review.grade_item(conn, item_id, grade=_ATTEMPTED_GRADE)
    state.log_acceptance(
        conn, surface=SURFACE_RECALL, candidate_key=str(item_id), verdict="kept"
    )
    return RouteOutcome(r.anchor, "recall", "graded", f"item {item_id} advanced")


def _route_connection(conn, anchor, region, r: ExtractedRegion) -> RouteOutcome:
    """A reaction to a surfaced connection is a free relevance judgement (§12.1).

    Written-on means the connection was worth his attention; untouched means it was not. Both
    go to `acceptance_log` — the whole point of the flywheel is that a rejection is data.
    """
    verdict = "kept" if r.has_writing else "rejected"
    state.log_acceptance(
        conn, surface=SURFACE_CONNECTION, candidate_key=anchor.target_key, verdict=verdict
    )
    if not r.has_writing:
        return RouteOutcome(r.anchor, "connection", "untouched")
    return RouteOutcome(r.anchor, "connection", "kept", anchor.target_key)


def _route_reading(conn, anchor, region, r: ExtractedRegion) -> RouteOutcome:
    verdict = "kept" if r.has_writing else "rejected"
    state.log_acceptance(
        conn, surface=SURFACE_READING, candidate_key=anchor.target_key, verdict=verdict
    )
    if not r.has_writing:
        return RouteOutcome(r.anchor, "reading", "untouched")
    return RouteOutcome(r.anchor, "reading", "kept", anchor.target_key)


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
    text = " ".join(r.text.split())
    title = text if len(text) <= 120 else text[:117] + "..."
    object_id, created = state.upsert_object(conn, type_="question", title=title)
    state.apply_owner_edit(
        conn, object_id, {"question": text}, source=f"daily:{page_date}#{r.anchor}"
    )
    state.set_status(conn, object_id, "active")
    return RouteOutcome(r.anchor, "question", "created" if created else "updated", title)


def _route_blessing(conn, anchor, region, r: ExtractedRegion, *, page_date: str) -> RouteOutcome:
    """The four-way table. See the module docstring for why 'writing, not ticked' is separate."""
    try:
        object_id = int(anchor.target_key)
    except ValueError:
        return RouteOutcome(r.anchor, "blessing", "error", "unparseable object id")
    obj = state.get_object(conn, object_id)
    if obj is None:
        return RouteOutcome(r.anchor, "blessing", "error", "object no longer exists")

    ticked = bool(r.ticked)
    wrote = r.has_writing

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
) -> dict[str, tuple[bool | None, str]]:
    """anchor -> (ticked, text) already routed for this page, for the re-pull guard."""
    out: dict[str, tuple[bool | None, str]] = {}
    for row in conn.execute(
        "SELECT anchor, ticked, text, outcome FROM annotations WHERE page_date=?", (page_date,)
    ):
        if row["outcome"] in (None, "error"):
            continue  # a region that failed to route is retried, not skipped
        ticked = None if row["ticked"] is None else bool(row["ticked"])
        out[row["anchor"]] = (ticked, (row["text"] or "").strip())
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
        if seen is not None and seen == (r.ticked, r.text.strip()):
            # Byte-identical to what we already routed for this region: re-applying would
            # advance the SM-2 schedule a second time for one answer and double-count the
            # acceptance judgement the flywheel reads. Idempotency is about the SIDE EFFECTS,
            # not just the annotation row. A region whose content CHANGED is re-applied, which
            # is what makes a re-scan after writing more actually work.
            result.outcomes.append(RouteOutcome(r.anchor, anchor.kind, "unchanged"))
            continue
        if anchor.kind == "recall":
            outcome = _route_recall(conn, anchor, r.anchor, r)
        elif anchor.kind == "connection":
            outcome = _route_connection(conn, anchor, r.anchor, r)
        elif anchor.kind == "reading":
            outcome = _route_reading(conn, anchor, r.anchor, r)
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
# The return leg needs no new transport. The on-device agent already pushes every CHANGED
# document over the tailnet into the staging dir as `<uuid>.pdf` (scripts/remarkable/receiver.py),
# which is how Loop A gets handwriting. Annotating the daily page changes it, so it arrives here
# on its own — the only thing missing was working out WHICH staged uuid is the page.
#
# Loop A deliberately EXCLUDES the `Locus` folder (invariant 5: our own pushed output must not be
# re-ingested as if it were the owner's writing). The pull-back is the exact inverse: it wants
# only that folder, and only the document named for the date in question.


def _pdf_hash(path: Path) -> str:
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def find_staged_page(
    page_date: str,
    *,
    staging_dir: str | Path | None = None,
    runner=None,
    rmapi_binary: str | None = None,
    folder: str | None = None,
) -> Path | None:
    """The staged `<uuid>.pdf` for a given daily page, or None if the device has not sent it.

    Matched on the device-side document NAME (`daily-<date>`), which is exactly what
    `cmd_daily` uploads, so the mapping is the filename contract and nothing more subtle.
    """
    from locus.capture.remarkable import build_uuid_index

    cfg = load()
    staging_dir = Path(staging_dir or cfg.capture.staging_dir)
    folder = folder or cfg.reading.target_folder
    if not staging_dir.is_dir():
        return None

    if runner is None:
        from locus.capture.remarkable import _subprocess_runner

        runner = _subprocess_runner(rmapi_binary or cfg.reading.rmapi_binary)

    # excluded_folders=() so the Locus folder IS considered — the inverse of Loop A's filter.
    index = build_uuid_index(runner, excluded_folders=())
    want = f"daily-{page_date}"
    for uuid, (name, top_folder, _modified) in index.items():
        if top_folder != folder or Path(name).stem != want:
            continue
        staged = staging_dir / f"{uuid}.pdf"
        if staged.is_file():
            return staged
    return None


def already_pulled(conn: sqlite3.Connection, page_date: str, pdf_path: str | Path) -> bool:
    """True when this exact file was already read for this page.

    Routing is idempotent regardless; this guard exists so a scheduled pull does not pay for a
    vision call on a page nobody has touched since the last run.
    """
    row = conn.execute(
        "SELECT pulled_hash FROM daily_pages WHERE page_date=?", (page_date,)
    ).fetchone()
    return bool(row and row["pulled_hash"] and row["pulled_hash"] == _pdf_hash(pdf_path))


def _record_pull(conn: sqlite3.Connection, page_date: str, pdf_path: str | Path) -> None:
    with conn:
        conn.execute(
            "UPDATE daily_pages SET pulled_hash=?, pulled_at=? WHERE page_date=?",
            (_pdf_hash(pdf_path), _utcnow(), page_date),
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

    With no `pdf_path`, the staged copy the device pushed is located automatically. Skips the
    (billed) vision pass when the file is byte-identical to the last one read, unless `force`.
    """
    page_date = page_date or date.today().isoformat()
    if pdf_path is None:
        pdf_path = find_staged_page(page_date)
        if pdf_path is None:
            return PullResult(page_date=page_date, status="not-on-device")
    if not force and already_pulled(conn, page_date, pdf_path):
        return PullResult(page_date=page_date, status="unchanged")

    regions = extract_regions(pdf_path, client=client)
    result = route_regions(conn, page_date, regions, source_run=source_run)
    _record_pull(conn, page_date, pdf_path)
    return result
