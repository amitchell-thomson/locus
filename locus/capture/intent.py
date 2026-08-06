"""What he MEANT by a mark, and therefore what should happen to it.

Asked what an underline means he gave three answers: "something I think is important, something I
dont understand, or an idea I have linking to the content of that passage (eg. I read part of a
paper that I think describes a concept or technique that would be useful to try in one of my
projects)." Three meanings that want three different fates:

    important        stays retrieval, and becomes a recall candidate. Never pushed at him — he
                     already knows it matters; that is what the mark said.
    not_understood   earns a corpus re-read a slot on the Read page — the ONLY thing that does,
                     because a generic gap-closer is what discredited that section — and becomes
                     a discovery query, since the corpus may simply not contain the answer.
    idea             becomes an `idea` object linked to the passage AND to the project it names,
                     returns on the Think page for development, and reaches the corpus through
                     `locus promote` once he has written on it more than once.

Until this existed every mark got the same fate — search fuel — which is why 26 have accumulated
and why the `idea` type has never once been populated.

WHAT IT IS SHOWN, and what it is not. The passage he marked, what he wrote beside it, the shape of
the mark, and the document's title. NOT his projects: this pass decides what he MEANT, not whether
the mark is useful, and offering it a list of projects invites it to manufacture a connection to
one. The project link is a separate, later step over material that is already classified `idea`.

CONFIDENCE IS LOAD-BEARING, not decoration. His answer was "infer, then let me correct", and a
low-confidence guess acted on silently is that answer with the correction removed. Below the floor
nothing happens except that the mark becomes a `locus decide` item. An intent HE set is never
overwritten by a re-run (`intent_by='owner'`).

THE SETTLE WINDOW is his: a mark is not classified until its page has been untouched for 12 hours,
so the pass never reacts to ink he is still in the middle of writing.
"""

from __future__ import annotations

import logging
import sqlite3
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone

from locus.agent.claude import ClaudeError, run_text
from locus.config import load

log = logging.getLogger(__name__)

IMPORTANT = "important"
NOT_UNDERSTOOD = "not_understood"
IDEA = "idea"
INTENTS = (IMPORTANT, NOT_UNDERSTOOD, IDEA)

BY_MODEL = "model"
BY_OWNER = "owner"

# Hours a mark's page must have been untouched before it is classified (his number).
DEFAULT_SETTLE_HOURS = 12
# Below this the guess is recorded but NOT acted on; the mark becomes a `locus decide` item.
DEFAULT_CONFIDENCE_FLOOR = 0.6

_PROMPT = """\
Someone marked a passage in "{title}" while reading. Decide what they MEANT by the mark.

THE PASSAGE THEY MARKED:
{passage}

{note_block}THE MARK ITSELF: {kind}{margin}

Exactly one of three things was meant:

  important       — they think this is worth remembering. A bare underline of a definition, a
                    result, or a claim, with no question and no proposal.
  not_understood  — they did not follow it. Questions, "?", "what is...", "how does...", or a
                    passage marked where the text is technical and nothing else was written.
  idea            — the passage gave them an idea for their OWN work: something to try, build,
                    test, or connect to a project of theirs. Usually a written note that proposes
                    or wonders about doing something.

Answer with ONLY this, and nothing else:

INTENT: <one of important, not_understood, idea>
CONFIDENCE: <0.0-1.0, how sure you are>
WHY: <one short sentence>

If the mark is a bare underline with nothing written and the passage carries no obvious
difficulty, `important` at moderate confidence is the honest answer. Do not reach for `idea`
unless they actually proposed doing something — that is the rarest of the three and the one whose
false positives cost the most.
"""


@dataclass
class Classified:
    mark_id: int
    intent: str = ""
    confidence: float = 0.0
    why: str = ""
    error: str = ""

    @property
    def ok(self) -> bool:
        return self.intent in INTENTS


def _parse(reply: str) -> tuple[str, float, str]:
    """Pull the three fields out of the reply, tolerating prose around them."""
    intent, confidence, why = "", 0.0, ""
    for raw in reply.splitlines():
        line = raw.strip().lstrip("-* ").strip()
        upper = line.upper()
        if upper.startswith("INTENT:"):
            value = line.split(":", 1)[1].strip().lower().strip(".`\"' ")
            # Tolerate 'not understood' / 'not-understood' for the underscored label.
            intent = value.replace(" ", "_").replace("-", "_")
        elif upper.startswith("CONFIDENCE:"):
            try:
                confidence = float(line.split(":", 1)[1].strip().strip("`\"' "))
            except ValueError:
                confidence = 0.0
        elif upper.startswith("WHY:"):
            why = line.split(":", 1)[1].strip()
    return intent, max(0.0, min(1.0, confidence)), why


def pending_marks(
    conn: sqlite3.Connection,
    *,
    settle_hours: int = DEFAULT_SETTLE_HOURS,
    now: datetime | None = None,
    limit: int = 100,
    force: bool = False,
) -> list[sqlite3.Row]:
    """Marks that have settled and have no intent yet.

    An intent HE set is never revisited: `intent_by='owner'` is durable, and a nightly pass that
    quietly re-guessed over his correction would make the correction pointless.

    A MARK WITH NOTHING ON IT IS NOT PENDING. `classify` rejects "nothing marked and nothing
    written" before it makes a call, so such a mark can never leave this pool — it stays NULL,
    stays pending, and is re-selected every night forever. They are not rare: highlights over a
    figure capture no text (§14), and 28 of the 42 marks waiting on 2026-08-06 were blank. Because
    the pool is ordered by id and the run is capped, those zombies sat at the FRONT and spent the
    cap: the 2026-08-06 run classified ids up to 131 and never reached 132-144, so his three
    newest written questions were never classified, `learn/answers.pending_questions` (which needs
    `intent='not_understood'`) never saw them, and the Answered page went hungry with the whole
    chain reporting success. The filter is on CURRENT content, not a durable skip flag, so a mark
    that gains a note later — transcription is bounded per run and lags capture — re-enters the
    pool by itself.
    """
    now = now or datetime.now(timezone.utc)
    cutoff = (now - timedelta(hours=settle_hours)).isoformat()
    sql = (
        "SELECT a.*, d.title AS doc_title FROM pdf_annotations a "
        "LEFT JOIN documents d ON d.source_uri = a.source_uri "
        "WHERE COALESCE(a.intent_by,'') != 'owner' AND a.captured_at <= ? "
        "AND (TRIM(COALESCE(a.covered_text,'')) != '' OR TRIM(COALESCE(a.note,'')) != '') "
    )
    if not force:
        sql += "AND (a.intent IS NULL OR TRIM(a.intent)='') "
    sql += "ORDER BY a.id LIMIT ?"
    try:
        return list(conn.execute(sql, (cutoff, limit)))
    except sqlite3.OperationalError:
        return []


def classify(row: sqlite3.Row, *, runner=None, model: str | None = None) -> Classified:
    """One mark -> its intent. Degrades to an error result; never raises."""
    passage = " ".join((row["covered_text"] or "").split())
    note = " ".join((row["note"] or "").split())
    if not passage and not note:
        return Classified(row["id"], error="nothing marked and nothing written")

    prompt = _PROMPT.format(
        title=row["doc_title"] or (row["source_uri"] or "").rsplit("/", 1)[-1],
        passage=passage or "(the mark covers no extractable text)",
        note_block=f"WHAT THEY WROTE BESIDE IT:\n{note}\n\n" if note else "",
        kind=row["kind"],
        margin=" in the margin" if row["in_margin"] else "",
    )
    try:
        reply = run_text(prompt, model=model or load().agent.model, runner=runner)
    except ClaudeError as exc:
        log.warning("intent: classification failed for mark %s: %s", row["id"], exc)
        return Classified(row["id"], error=str(exc))

    intent, confidence, why = _parse(reply)
    if intent not in INTENTS:
        return Classified(row["id"], error=f"unrecognised intent {intent!r}")
    return Classified(row["id"], intent=intent, confidence=confidence, why=why)


def store(conn: sqlite3.Connection, result: Classified, *, by: str = BY_MODEL) -> None:
    with conn:
        conn.execute(
            "UPDATE pdf_annotations SET intent=?, intent_confidence=?, intent_by=?, intent_at=? "
            "WHERE id=?",
            (result.intent, result.confidence, by, datetime.now(timezone.utc).isoformat(),
             result.mark_id),
        )


def set_owner_intent(conn: sqlite3.Connection, mark_id: int, intent: str) -> bool:
    """Record HIS answer. Durable: `pending_marks` never revisits an owner-set intent."""
    if intent not in INTENTS:
        return False
    store(conn, Classified(mark_id, intent=intent, confidence=1.0), by=BY_OWNER)
    return True


def classify_pending(
    conn: sqlite3.Connection,
    *,
    settle_hours: int = DEFAULT_SETTLE_HOURS,
    limit: int = 100,
    dry_run: bool = False,
    force: bool = False,
    runner=None,
    model: str | None = None,
    now: datetime | None = None,
) -> list[Classified]:
    """Classify every settled mark. `dry_run` genuinely writes nothing.

    The plan/apply split matters here more than usual: this is the first pass that creates
    durable structure from an INFERENCE rather than from something he ticked, so being able to
    look at the whole three-way split before anything acts on it is the difference between
    trusting it and hoping.
    """
    out: list[Classified] = []
    for row in pending_marks(
        conn, settle_hours=settle_hours, now=now, limit=limit, force=force
    ):
        result = classify(row, runner=runner, model=model)
        if result.ok and not dry_run:
            store(conn, result)
        out.append(result)
    return out


def counts(results: list[Classified]) -> dict[str, int]:
    tally = {intent: 0 for intent in INTENTS}
    tally["unclassified"] = 0
    for r in results:
        tally[r.intent if r.ok else "unclassified"] += 1
    return tally


# --- acting on an intent -------------------------------------------------------------------------


@dataclass
class Acted:
    mark_id: int
    intent: str
    outcome: str
    detail: str = ""


def _idea_title(text: str, limit: int = 70) -> str:
    words = " ".join(text.split())
    return words if len(words) <= limit else words[:limit].rsplit(" ", 1)[0] + "..."


def act_on(conn: sqlite3.Connection, mark_id: int, *, floor: float | None = None) -> Acted:
    """Give one classified mark its fate. Free and local — no model call.

    Deliberately separate from `classify`: the inference is billed and fallible, and acting is a
    deterministic join over its result. Keeping them apart is what lets the whole three-way split
    be inspected (`--dry-run`) before anything durable is created from it.
    """
    from locus.agent import state

    floor = load().capture.intent_confidence_floor if floor is None else floor
    row = conn.execute(
        "SELECT a.*, d.title AS doc_title FROM pdf_annotations a "
        "LEFT JOIN documents d ON d.source_uri = a.source_uri WHERE a.id=?",
        (mark_id,),
    ).fetchone()
    if row is None:
        return Acted(mark_id, "", "skipped", "no such mark")
    if row["object_id"]:
        return Acted(mark_id, row["intent"] or "", "already acted on")
    intent = row["intent"] or ""
    if intent not in INTENTS:
        return Acted(mark_id, intent, "skipped", "no intent")
    from locus.observe import gates

    held = (row["intent_confidence"] or 0.0) < floor and (row["intent_by"] != BY_OWNER)
    gates.record(
        conn, "capture.intent_confidence_floor", rejected=held,
        value=None if not held
        else f"conf={row['intent_confidence'] or 0.0:.2f} {intent}: {str(row['note'] or '')[:60]}",
    )
    if held:
        # His answer was "infer, then let me correct". Below the floor the correction step is the
        # whole point, so nothing is created and `locus decide` asks him.
        return Acted(mark_id, intent, "held", "below the confidence floor — ask in `locus decide`")

    if intent == IMPORTANT:
        # Nothing is pushed at him: the mark already said it matters, and re-offering it would be
        # the unread-count behaviour §9 forbids. It stays retrievable, which it already was.
        return Acted(mark_id, intent, "left in place", "retrieval only, nothing pushed")

    note = " ".join((row["note"] or "").split())
    passage = " ".join((row["covered_text"] or "").split())

    if intent == IDEA:
        # The TEXT is his, so the object is his and lands `active` — asking him to bless his own
        # handwriting is theatre. Only the ROUTING was inferred, and that is what the floor gates.
        title = _idea_title(note or passage)
        object_id, _created = state.upsert_object(conn, type_="idea", title=title)
        state.apply_owner_edit(
            conn, object_id, {"idea": note or passage, "from_mark": str(mark_id)},
            source=f"mark:{mark_id}",
        )
        state.set_status(conn, object_id, "active")

        links = [state.ObjectLink("doc", row["source_uri"], "raised_by")]
        projects = _project_for(conn, f"{note} {passage}")
        for pid, _ptitle in projects:
            # `target_key` for target_kind='object' is str(object_id) — see state.ObjectLink.
            # Storing the PROFILE LABEL here instead left the link pointing at nothing
            # resolvable, so the idea and the project it was about never actually connected.
            links.append(state.ObjectLink("object", str(pid), "relates"))
        state.add_links(conn, object_id, links)

        with conn:
            conn.execute(
                "UPDATE pdf_annotations SET object_id=? WHERE id=?", (object_id, mark_id)
            )
        where = f" -> {', '.join(t for _, t in projects)}" if projects else ""
        return Acted(mark_id, intent, "became an idea", f"{title}{where}")

    # NOT_UNDERSTOOD: it earns a corpus re-read a slot on the Read page — the only thing that
    # does. Nothing is created here; `compose_daily.build_rereads` reads the intent directly, so
    # a re-read cannot outlive the confusion that justified it.
    return Acted(mark_id, intent, "queued a re-read", (note or passage)[:60])


def _project_for(conn: sqlite3.Connection, text: str) -> list[tuple[int, str]]:
    """The PROJECT OBJECTS this idea is about — see `link/projects.py` for why it is two tiers.

    A LIST, because "a macro regime predictor in the tanker project" is genuinely about two of
    them; and often empty, because an idea with no project is still an idea. Inventing a link to
    the nearest project is the manufactured connection the classifier is deliberately not shown
    projects in order to avoid.
    """
    from locus.link.projects import projects_for

    return projects_for(conn, text)


def act_on_all(conn: sqlite3.Connection, *, limit: int = 100) -> list[Acted]:
    rows = conn.execute(
        "SELECT id FROM pdf_annotations WHERE intent IS NOT NULL AND object_id IS NULL "
        "ORDER BY id LIMIT ?",
        (limit,),
    ).fetchall()
    return [act_on(conn, r["id"]) for r in rows]


# A re-read is a SEARCH, so its seed has to be searchable. Marks 2 and 4 of the live book are
# `take the` and `NMVSPY` — a bare underline over a fragment the extractor recovered badly. Fed to
# retrieval those return noise, and a re-read justified by noise is exactly the useless suggestion
# the whole Read-page rework was about. A written question is the strong signal; a covered passage
# has to carry a real sentence to stand in for one.
_MIN_QUESTION_CHARS = 25


def reread_seed(row: sqlite3.Row) -> str:
    """The searchable question behind a not-understood mark, or '' if there isn't one."""
    note = " ".join((row["note"] or "").split())
    if len(note) >= _MIN_QUESTION_CHARS:
        return note
    passage = " ".join((row["covered_text"] or "").split())
    if len(passage) >= _MIN_QUESTION_CHARS:
        return passage
    return ""


def not_understood_marks(conn: sqlite3.Connection, *, limit: int = 20) -> list[sqlite3.Row]:
    """Passages he said he did not follow — the only thing that earns a corpus re-read a slot.

    Marks he actually WROTE on come first: "what is a factor covariance estimator" states the
    confusion, where a bare underline only locates it.
    """
    try:
        rows = list(conn.execute(
            "SELECT a.*, d.title AS doc_title FROM pdf_annotations a "
            "LEFT JOIN documents d ON d.source_uri = a.source_uri "
            "WHERE a.intent=? AND COALESCE(a.intent_confidence,0) >= ? "
            "ORDER BY (TRIM(COALESCE(a.note,'')) != '') DESC, a.captured_at DESC, a.id DESC "
            "LIMIT ?",
            (NOT_UNDERSTOOD, load().capture.intent_confidence_floor, limit * 4),
        ))
    except sqlite3.OperationalError:
        return []
    return [r for r in rows if reread_seed(r)][:limit]


def uncertain_marks(conn: sqlite3.Connection, *, limit: int = 50) -> list[sqlite3.Row]:
    """Marks the model was not sure about — `locus decide` asks him rather than guessing."""
    try:
        return list(conn.execute(
            "SELECT a.*, d.title AS doc_title FROM pdf_annotations a "
            "LEFT JOIN documents d ON d.source_uri = a.source_uri "
            "WHERE COALESCE(a.intent_by,'') = ? AND a.intent IS NOT NULL "
            "AND COALESCE(a.intent_confidence,0) < ? AND a.object_id IS NULL "
            "ORDER BY a.id LIMIT ?",
            (BY_MODEL, load().capture.intent_confidence_floor, limit),
        ))
    except sqlite3.OperationalError:
        return []
