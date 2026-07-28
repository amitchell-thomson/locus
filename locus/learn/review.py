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
