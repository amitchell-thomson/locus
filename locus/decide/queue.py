"""Every decision waiting on him, and — the load-bearing part — which surface it belongs to.

HIS RULE, verbatim: "Nothing in the tui should also ever be on the daily note, they should ALWAYS
be two separate approval things. I should not be able to approve the same thing on the daily page
and the tui." Two surfaces that can both resolve one item is not a convenience, it is a way to
lose a decision: he ticks it on paper, clears it in the terminal the next morning having forgotten,
and the second action silently overwrites the first — or worse, the flywheel records both and
learns from a judgement he made once.

So the split is computed HERE, in one place, and both surfaces read it. The daily page is for
things that need thinking with the source at hand; the TUI is for yes/no with no thinking in it
(his answer: "Thinking on the page, approving in the TUI"). Anything the page is currently
offering is subtracted from the TUI queue by `pending()`, so the rule holds even when a future
decision kind could legitimately appear in both — which is not hypothetical: a marked passage is a
Think item, and once mark-intent inference lands (step 4) an AMBIGUOUS intent on that same passage
becomes a TUI decision. Without the subtraction they would collide on the first one.

`tests/test_decide_queue.py` asserts the two key sets never intersect. That test is the invariant;
this module is only its implementation.
"""

from __future__ import annotations

import sqlite3
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone

from locus.agent import compose_daily as cd
from locus.agent import state

# Decision kinds the TUI owns. Each is a yes/no judgeable from a title and a line of context —
# that is the test for belonging here rather than on the page.
KIND_OBJECT = "object"          # a proposed object: bless it or drop it
KIND_DUPLICATE = "duplicate"    # two objects that look like one concept: merge or keep apart
KIND_ABANDONED = "abandoned"    # a reading that has sat untouched: why did you stop?
KIND_INTENT = "intent"          # a mark whose meaning the model was not sure of

TUI_KINDS = (KIND_OBJECT, KIND_DUPLICATE, KIND_INTENT, KIND_ABANDONED)

# How long a reading may sit in `In-Progress` with no new ink before it is worth asking about.
# Deliberately generous — two weeks of not opening a paper is as likely to mean a busy fortnight
# as a bad paper, which is why it ASKS rather than concluding (his answer: "prompting me to either
# say it wasnt useful or keep reading").
DEFAULT_ABANDON_DAYS = 14


@dataclass
class Decision:
    """One pending decision, rendered identically whatever kind it is."""

    key: str                       # stable identity, and what `acceptance_log` records
    kind: str
    title: str
    detail: str = ""               # why this is being asked
    grounding: str = ""            # what it rests on, so a yes/no is checkable
    accept_label: str = "keep"
    reject_label: str = "drop"
    ref: int = 0                   # row id the actions operate on
    extra: tuple[int, ...] = ()    # further rows (a merge folds these into `ref`)


@dataclass
class Queue:
    sections: dict[str, list[Decision]] = field(default_factory=dict)

    @property
    def total(self) -> int:
        return sum(len(v) for v in self.sections.values())

    def flat(self) -> list[Decision]:
        return [d for kind in TUI_KINDS for d in self.sections.get(kind, [])]


def _utcnow() -> str:
    return datetime.now(timezone.utc).isoformat()


# --- what the PAGE is offering (subtracted from the TUI) ---------------------------------------


def page_keys(conn: sqlite3.Connection) -> set[str]:
    """Item keys the daily page would offer right now.

    Composed rather than read from `daily_shown`, because the question is what the page is
    OFFERING, not what it has ever offered: an item shown last week and since resolved is not a
    live collision, and one about to appear tomorrow is.
    """
    try:
        page = cd.compose(conn)
    except sqlite3.OperationalError:
        return set()
    return {key for key, _kind in page.items_shown()}


# --- the kinds ---------------------------------------------------------------------------------


def object_decisions(conn: sqlite3.Connection, *, limit: int = 200) -> list[Decision]:
    """Proposed objects awaiting a blessing, oldest first.

    Oldest-first is the fair queue and the one that actually drains: it answers the proposal he
    has walked past longest rather than re-offering whatever was made most recently.

    This is the whole reason the TUI exists. Blessings left the daily page in the step-2 rebuild
    because he wants them "all at once so that I clear everything ASAP", and until this landed the
    only way through 48 pending objects was `locus objects --bless <id>`, one at a time.
    """
    rows = conn.execute(
        "SELECT id FROM objects WHERE status='proposed' ORDER BY created_at, id LIMIT ?",
        (limit,),
    ).fetchall()

    out: list[Decision] = []
    for row in rows:
        obj = state.get_object(conn, row["id"])
        if obj is None:
            continue
        body = obj.body or {}
        # `why` is the only rationale key any writer produces; the old `summary`/`rationale`
        # fallbacks were dead branches that made this look more tolerant than it was.
        why = body.get("why") or ""
        grounding = cd._format_grounding(obj.links[0]) if obj.links else "(no grounding link)"
        out.append(
            Decision(
                key=f"object:{obj.id}",
                kind=KIND_OBJECT,
                title=f"{obj.type} — {obj.title}",
                detail=" ".join(str(why).split()),
                grounding=f"grounded in {grounding}",
                accept_label="bless",
                reject_label="drop for good",
                ref=obj.id,
            )
        )
    return out


def _normalise(title: str) -> str:
    """Casefold, strip punctuation, drop a trailing plural — the cheap same-thing test.

    Deliberately the same shape as the deterministic tiers in `link/aliases.py`, because the two
    layers should agree on what counts as the same name.
    """
    import re

    return re.sub(r"[^a-z0-9]+", " ", title.lower()).strip().rstrip("s")


def _canonical_of(conn: sqlite3.Connection) -> dict[tuple[str, str], tuple[str, str]]:
    """`(title, type) -> canonical` from the alias substrate, when it exists."""
    try:
        return {
            (r["variant_name"].lower(), r["variant_type"]):
                (r["canonical_name"], r["canonical_type"])
            for r in conn.execute(
                "SELECT variant_name, variant_type, canonical_name, canonical_type "
                "FROM entity_aliases"
            )
        }
    except sqlite3.OperationalError:
        return {}


def duplicate_groups(conn: sqlite3.Connection, *, limit: int = 50) -> list[Decision]:
    """Objects that are the same concept wearing two names.

    TWO TIERS, and the order matters. The alias substrate is the principled one — if `locus link`
    has decided two surfaces share a canonical, then two objects titled with those surfaces are
    one concept by the system's own definition of sameness, and no new notion of similarity is
    invented here. The normalised-title tier is the cheap catch for objects whose titles never
    entered the substrate; today it is the only one that fires, on `Bootstrap` / `bootstrap`.

    A group he has said to keep apart is not asked about again: the refusal is recorded on the
    `link` surface and filtered here, so a "no" is durable rather than a daily irritation.
    """
    rows = conn.execute(
        "SELECT id, type, title, created_at FROM objects "
        "WHERE status IN ('active','proposed') ORDER BY created_at, id"
    ).fetchall()
    aliases = _canonical_of(conn)

    groups: dict[tuple, list[sqlite3.Row]] = {}
    for row in rows:
        canonical = aliases.get((row["title"].lower(), row["type"]))
        key = canonical if canonical else (_normalise(row["title"]), row["type"])
        groups.setdefault(key, []).append(row)

    settled = {
        key for key, counts in state.acceptance_counts(conn, surface=SURFACE_MERGE).items()
        if counts.get("rejected")
    }

    out: list[Decision] = []
    for key, members in groups.items():
        if len(members) < 2:
            continue
        decision_key = f"objects:{key[0]}\x1f{key[1]}"
        if decision_key in settled:
            continue
        keep, *rest = members            # oldest survives; it is the one with the longest history
        out.append(
            Decision(
                key=decision_key,
                kind=KIND_DUPLICATE,
                title=" = ".join(m["title"] for m in members),
                detail=(
                    f"merge into \"{keep['title']}\" (the oldest), archiving "
                    f"{len(rest)} duplicate(s)"
                ),
                grounding=(
                    "the alias substrate says these share a canonical"
                    if key in set(aliases.values())
                    else "identical once case and punctuation are ignored"
                ),
                accept_label="merge",
                reject_label="keep separate",
                ref=keep["id"],
                extra=tuple(m["id"] for m in rest),
            )
        )
        if len(out) >= limit:
            break
    return out


def merge_objects(conn: sqlite3.Connection, keep_id: int, drop_ids: tuple[int, ...]) -> int:
    """Fold duplicates into `keep_id`. Returns how many were folded in.

    ARCHIVES rather than deletes, and merges bodies additively through the same `merge_body` the
    structurer uses — so nothing he blessed is lost, and an undo is a status change rather than a
    resurrection. Links move across, because a duplicate's grounding is evidence for the concept,
    not for the row that happened to hold it.
    """
    keep = state.get_object(conn, keep_id)
    if keep is None:
        return 0
    folded = 0
    for drop_id in drop_ids:
        other = state.get_object(conn, drop_id)
        if other is None:
            continue
        state.upsert_object(conn, type_=keep.type, title=keep.title, body=other.body or {})
        state.add_links(conn, keep_id, other.links)
        state.set_status(conn, drop_id, "archived")
        folded += 1
    return folded


def uncertain_intents(conn: sqlite3.Connection, *, limit: int = 50) -> list[Decision]:
    """Marks the intent pass was not confident about — "infer, then let me correct".

    This is the correction step, and without it the floor would only mean "act on fewer marks"
    rather than "ask about the ones I could not read". A mark held here is doing nothing until he
    answers, which is the honest state: the system does not know what he meant.

    Accept/reject is a two-way choice and intent is three-way, so the card offers the model's
    guess as the accept and `idea` as the reject — the two that DO something. A mark that is
    neither is left alone, which is what `important` means anyway.
    """
    from locus.capture import intent as I

    out: list[Decision] = []
    for mark in I.uncertain_marks(conn, limit=limit):
        guess = mark["intent"] or ""
        wrote = " ".join((mark["note"] or "").split())
        passage = " ".join((mark["covered_text"] or "").split())
        out.append(
            Decision(
                key=f"mark:{mark['id']}",
                kind=KIND_INTENT,
                title=(wrote or passage or "(an unreadable mark)")[:110],
                detail=(
                    f"guessed {guess} at {mark['intent_confidence']:.0%} — too low to act on"
                ),
                grounding=(
                    f"{mark['doc_title'] or 'your reading'} p.{mark['pdf_page'] + 1}"
                    + (f" — \u201c{passage[:70]}\u201d" if wrote and passage else "")
                ),
                accept_label=f"yes, {guess}",
                reject_label="no, it was an idea",
                ref=mark["id"],
            )
        )
    return out


def abandoned_readings(
    conn: sqlite3.Connection, *, days: int = DEFAULT_ABANDON_DAYS, now: datetime | None = None
) -> list[Decision]:
    """Readings sitting in `In-Progress` with no ink for `days`.

    His requirement was that the answer must DO something: "I want to stress that this signal
    should 'do something' — ie. help tune what is recommended in the future or not just be a value
    that sits in a table and does nothing." `resolve()` writes it to `acceptance_log(surface=
    'discovery')`, which is the per-channel prior that already tunes what gets proposed.
    """
    from locus.reading import relevance as R

    now = now or datetime.now(timezone.utc)
    cutoff = now - timedelta(days=days)
    out: list[Decision] = []
    for target in R.in_progress(conn):
        if target["marks"]:
            continue                      # he has written on it: he is reading it
        stamp = target["last_swept"] or target["created_at"]
        try:
            seen = datetime.fromisoformat(stamp)
        except (TypeError, ValueError):
            continue
        if seen.tzinfo is None:
            seen = seen.replace(tzinfo=timezone.utc)
        if seen > cutoff:
            continue
        title = target["title"] or R.title_for(target["device_path"], target["source_uri"])
        out.append(
            Decision(
                key=f"reading:{target['id']}",
                kind=KIND_ABANDONED,
                title=title,
                detail=f"no marks in {(now - seen).days} days — wrong paper, or just not yet?",
                grounding=(
                    f"links to {target['subject_label']}" if target["subject_label"]
                    else "not linked to a project"
                ),
                accept_label="still reading",
                reject_label="wasn't useful",
                ref=target["id"],
            )
        )
    return out


def pending(
    conn: sqlite3.Connection, *, days: int = DEFAULT_ABANDON_DAYS, now: datetime | None = None
) -> Queue:
    """Everything awaiting a decision in the TUI, with anything the page offers removed."""
    on_the_page = page_keys(conn)
    queue = Queue()
    queue.sections[KIND_OBJECT] = [
        d for d in object_decisions(conn) if d.key not in on_the_page
    ]
    queue.sections[KIND_DUPLICATE] = [
        d for d in duplicate_groups(conn) if d.key not in on_the_page
    ]
    queue.sections[KIND_INTENT] = [
        d for d in uncertain_intents(conn) if d.key not in on_the_page
    ]
    queue.sections[KIND_ABANDONED] = [
        d for d in abandoned_readings(conn, days=days, now=now) if d.key not in on_the_page
    ]
    return queue


# --- applying a decision -----------------------------------------------------------------------

SURFACE_OBJECT = "object"
SURFACE_DISCOVERY = "discovery"
# A merge judgement is "are these two surfaces the same thing", which is precisely what the `link`
# surface means — it was defined for alias adjudication and never used. Reusing it puts both
# judgements in one history rather than two, and needs no migration to widen the CHECK. Keys are
# prefixed `objects:` so they cannot collide with an alias cluster key.
SURFACE_MERGE = "link"


def resolve(
    conn: sqlite3.Connection, decision: Decision, *, accept: bool, note: str = ""
) -> str:
    """Apply one decision. Returns a short human description of what happened.

    A correction he types is applied through `state.apply_owner_edit`, never `upsert_object`: the
    owner is the authority and the agent is not, and that asymmetry is enforced by using a
    different verb with a durable marker rather than by relaxing the additive merge.
    """
    if decision.kind == KIND_OBJECT:
        if note.strip():
            state.apply_owner_edit(
                conn, decision.ref, {"why": note.strip()}, source="decide"
            )
        state.set_status(conn, decision.ref, "active" if accept else "archived")
        state.log_acceptance(
            conn, surface=SURFACE_OBJECT, candidate_key=str(decision.ref),
            verdict="kept" if accept else "rejected",
        )
        return "blessed" if accept else "archived"

    if decision.kind == KIND_DUPLICATE:
        if accept:
            folded = merge_objects(conn, decision.ref, decision.extra)
            state.log_acceptance(
                conn, surface=SURFACE_MERGE, candidate_key=decision.key, verdict="kept"
            )
            return f"merged {folded} into one"
        state.log_acceptance(
            conn, surface=SURFACE_MERGE, candidate_key=decision.key, verdict="rejected"
        )
        return "kept separate"

    if decision.kind == KIND_INTENT:
        from locus.capture import intent as I

        guessed = conn.execute(
            "SELECT intent FROM pdf_annotations WHERE id=?", (decision.ref,)
        ).fetchone()
        chosen = (guessed["intent"] if accept and guessed else I.IDEA)
        I.set_owner_intent(conn, decision.ref, chosen)
        # His answer is authoritative and durable, so acting on it now is safe.
        I.act_on(conn, decision.ref)
        return f"recorded as {chosen}"

    if decision.kind == KIND_ABANDONED:
        # The signal has to reach the thing that decides what gets proposed, or asking was theatre.
        state.log_acceptance(
            conn, surface=SURFACE_DISCOVERY, candidate_key=decision.key,
            verdict="kept" if accept else "rejected",
        )
        with conn:
            conn.execute(
                "UPDATE reading_targets SET last_swept=? WHERE id=?", (_utcnow(), decision.ref)
            )
        return "kept reading" if accept else "recorded as not useful"

    return "no action"
