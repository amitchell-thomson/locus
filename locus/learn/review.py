"""SM-2 spaced repetition over `review_schedule` (agent-layer plan §6.4, §3.6).

Textbook SuperMemo-2, deliberately unembellished — the algorithm is well-understood, and the
value here is that the prompts are the owner's OWN propositions and questions rather than a
generic deck.

  grade 0-5 (quality of recall). q < 3 is a lapse: repetitions reset and the item comes back
  tomorrow, but the ease factor is NOT reset — SM-2 lets ease carry the item's long-run
  difficulty across lapses, and resetting it makes a hard item oscillate forever.
  Intervals: 1 day, then 6, then round(interval x ease). Ease floors at 1.3.

No model, no network — pure arithmetic over stored rows, so the schedule is deterministic and
testable. `due` is a date string; the caller passes `today` so tests need no clock.
"""

from __future__ import annotations

from dataclasses import dataclass
from datetime import date, timedelta

# SM-2's floor: below this an item's interval barely grows and it dominates every review session.
_MIN_EASE = 1.3
_DEFAULT_EASE = 2.5


@dataclass
class ReviewItem:
    id: int
    prompt_kind: str  # 'proposition' | 'object'
    prompt_ref: str
    due: str
    ease: float
    interval: int
    reps: int
    last_grade: int | None = None
    last_review: str | None = None


def _row(r) -> ReviewItem:
    return ReviewItem(
        id=r["id"], prompt_kind=r["prompt_kind"], prompt_ref=r["prompt_ref"], due=r["due"],
        ease=r["ease"], interval=r["interval"], reps=r["reps"], last_grade=r["last_grade"],
        last_review=r["last_review"],
    )


def schedule_prompt(
    conn, *, prompt_kind: str, prompt_ref: str, today: date | None = None
) -> ReviewItem:
    """Add a prompt to the schedule (idempotent — an already-scheduled prompt is returned as is).

    A new item is due immediately: it has never been seen, so there is nothing to wait for."""
    if prompt_kind not in ("proposition", "object"):
        raise ValueError(f"unknown prompt_kind {prompt_kind!r}")
    today = today or date.today()
    existing = conn.execute(
        "SELECT * FROM review_schedule WHERE prompt_kind=? AND prompt_ref=?",
        (prompt_kind, str(prompt_ref)),
    ).fetchone()
    if existing:
        return _row(existing)
    with conn:
        conn.execute(
            "INSERT INTO review_schedule (prompt_kind, prompt_ref, due, ease, interval, reps) "
            "VALUES (?,?,?,?,0,0)",
            (prompt_kind, str(prompt_ref), today.isoformat(), _DEFAULT_EASE),
        )
    return _row(
        conn.execute(
            "SELECT * FROM review_schedule WHERE prompt_kind=? AND prompt_ref=?",
            (prompt_kind, str(prompt_ref)),
        ).fetchone()
    )


def due_items(conn, *, today: date | None = None, limit: int = 5) -> list[ReviewItem]:
    """Items due on or before `today`, soonest-due first. `limit` is the daily-page cap (§9)."""
    today = today or date.today()
    return [
        _row(r)
        for r in conn.execute(
            "SELECT * FROM review_schedule WHERE due <= ? ORDER BY due, id LIMIT ?",
            (today.isoformat(), limit),
        )
    ]


def next_interval(*, grade: int, ease: float, interval: int, reps: int) -> tuple[float, int, int]:
    """The SM-2 step: (new_ease, new_interval_days, new_reps). Pure arithmetic, no I/O.

    Ease is updated on EVERY grade including a lapse (that is what makes it track difficulty);
    repetitions and the interval reset on a lapse so the item is re-learned."""
    if not 0 <= grade <= 5:
        raise ValueError(f"grade must be 0-5, got {grade}")
    ease = max(_MIN_EASE, ease + (0.1 - (5 - grade) * (0.08 + (5 - grade) * 0.02)))
    if grade < 3:
        return ease, 1, 0
    reps += 1
    if reps == 1:
        return ease, 1, reps
    if reps == 2:
        return ease, 6, reps
    return ease, max(1, round(interval * ease)), reps


def grade_item(
    conn, item_id: int, grade: int, *, today: date | None = None
) -> ReviewItem | None:
    """Record a recall grade and reschedule. Returns the updated item (None if unknown)."""
    today = today or date.today()
    row = conn.execute("SELECT * FROM review_schedule WHERE id=?", (item_id,)).fetchone()
    if row is None:
        return None
    ease, interval, reps = next_interval(
        grade=grade, ease=row["ease"], interval=row["interval"], reps=row["reps"]
    )
    due = (today + timedelta(days=interval)).isoformat()
    with conn:
        conn.execute(
            "UPDATE review_schedule SET due=?, ease=?, interval=?, reps=?, last_grade=?, "
            "last_review=? WHERE id=?",
            (due, ease, interval, reps, grade, today.isoformat(), item_id),
        )
    return _row(conn.execute("SELECT * FROM review_schedule WHERE id=?", (item_id,)).fetchone())


def resolve_prompt(conn, item: ReviewItem) -> tuple[str, str]:
    """(prompt_text, source) for a scheduled item — the proposition's text, or an object's title.

    A prompt whose referent has been deleted (a re-ingest replaced the document) degrades to a
    placeholder rather than vanishing: the schedule row is the owner's review history, and losing
    it silently would be worse than showing a stale prompt he can retire."""
    if item.prompt_kind == "proposition":
        row = conn.execute(
            "SELECT p.text, d.title FROM propositions p JOIN documents d ON d.id=p.doc_id "
            "WHERE p.id=?",
            (item.prompt_ref,),
        ).fetchone()
        if row:
            return row["text"], row["title"]
        return "(source proposition no longer in the corpus)", ""
    row = conn.execute("SELECT title FROM objects WHERE id=?", (item.prompt_ref,)).fetchone()
    return (row["title"] if row else "(object no longer exists)"), ""


# --- enrolment ---------------------------------------------------------------------------------
#
# `review_schedule` sat at ZERO rows from the day it was built, because the only way in was
# `locus review --add-object <id>`, by hand, one object at a time. A spaced-repetition system
# nobody enrols into is furniture: the daily page's recall section was permanently empty, so the
# single most useful thing the page could do for interview prep never happened.
#
# Enrolment is deterministic and model-free. The source is the owner's BLESSED objects — the
# things he has explicitly said he cares about — and within each, `practice.candidates_for_object`
# orders propositions gap-first (§12.3: what he cannot yet explain in his own words comes before
# what he can). Nothing here generates a question; a stored proposition IS the prompt and stays
# its own reference answer, which is the §15 grounded-or-silent rule applied to learning.
#
# Deliberately GRADUAL. Enrolling every proposition of 32 blessed objects would put thousands of
# items in the queue and produce a permanently-saturated recall section — the guilt-inducing
# backlog §9 exists to prevent. A small per-run cap means the schedule grows a few items a night
# and settles at whatever rate he actually answers them.

DEFAULT_PER_OBJECT = 2
DEFAULT_MAX_NEW = 8


def _already_scheduled(conn, prompt_kind: str, prompt_ref: str) -> bool:
    return (
        conn.execute(
            "SELECT 1 FROM review_schedule WHERE prompt_kind=? AND prompt_ref=?",
            (prompt_kind, prompt_ref),
        ).fetchone()
        is not None
    )


def enrol_from_blessed_objects(
    conn,
    *,
    per_object: int = DEFAULT_PER_OBJECT,
    max_new: int = DEFAULT_MAX_NEW,
    today: date | None = None,
) -> list[ReviewItem]:
    """Add a few unscheduled propositions from blessed objects. Returns only the NEW items.

    Idempotent: an already-scheduled prompt is skipped rather than re-added, so running this
    nightly converges instead of accumulating duplicates. Objects are visited oldest-blessed
    first so the queue drains in a fair order rather than re-sampling whatever changed today.
    """
    from locus.learn.practice import candidates_for_object

    added: list[ReviewItem] = []
    rows = conn.execute(
        "SELECT id FROM objects WHERE status='active' ORDER BY updated_at, id"
    ).fetchall()

    for row in rows:
        if len(added) >= max_new:
            break
        taken = 0
        for cand in candidates_for_object(conn, row["id"]):
            if taken >= per_object or len(added) >= max_new:
                break
            ref = str(cand.id)
            if _already_scheduled(conn, "proposition", ref):
                continue
            added.append(
                schedule_prompt(conn, prompt_kind="proposition", prompt_ref=ref, today=today)
            )
            taken += 1
    return added
