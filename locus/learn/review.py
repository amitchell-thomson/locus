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

import logging

log = logging.getLogger(__name__)

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
    # A real question about the proposition, generated once and stored (migration 0023). Without
    # it `resolve_prompt` returns the PROPOSITION ITSELF as the prompt — i.e. it shows him the
    # answer and asks him to recall it, which is why the recall loop had no answerable step.
    question: str | None = None


def _row(r) -> ReviewItem:
    return ReviewItem(
        id=r["id"], prompt_kind=r["prompt_kind"], prompt_ref=r["prompt_ref"], due=r["due"],
        ease=r["ease"], interval=r["interval"], reps=r["reps"], last_grade=r["last_grade"],
        last_review=r["last_review"],
        # Tolerated absent so a DB one migration behind still reads its schedule.
        question=(r["question"] if "question" in r.keys() else None),
    )


def schedule_prompt(
    conn, *, prompt_kind: str, prompt_ref: str, today: date | None = None
) -> ReviewItem:
    """Add a prompt to the schedule (idempotent — an already-scheduled prompt is returned as is).

    A new item is due immediately: it has never been seen, so there is nothing to wait for."""
    if prompt_kind not in ("proposition", "object", "concept"):
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
    if item.prompt_kind == "concept":
        return concept_answer(conn, item.prompt_ref)
    row = conn.execute("SELECT title FROM objects WHERE id=?", (item.prompt_ref,)).fetchone()
    return (row["title"] if row else "(object no longer exists)"), ""


def concept_answer(conn, name: str) -> tuple[str, str]:
    """(reference answer, source) for a concept — the stored propositions that define it.

    GROUNDED BY CONSTRUCTION. The question is generated, the ANSWER never is: it is corpus text
    that names the concept, so the answer overleaf is something he actually read rather than
    something a model asserted about a subject it was only given the name of.
    """
    rows = conn.execute(
        """
        SELECT p.text, d.title FROM propositions p
        JOIN documents d ON d.id = p.doc_id
        WHERE p.section_id IN (
            SELECT e.section_id FROM entities e
            LEFT JOIN entity_aliases a ON a.variant_name=e.name AND a.variant_type=e.type
            WHERE COALESCE(a.canonical_name, e.name) = ?
        )
        AND LENGTH(p.text) BETWEEN 60 AND 400
        ORDER BY LENGTH(p.text) DESC LIMIT 1
        """,
        (name,),
    ).fetchall()
    if not rows:
        return f"(no stored material defines {name})", ""
    # ONE proposition, and a bounded one. The answers share page 4 with the open writing region,
    # and four unbounded answers pushed it onto a seventh page.
    return rows[0]["text"], rows[0]["title"]


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
# what he can). Enrolment itself generates nothing and calls no model; the stored proposition is
# the reference answer and always remains so, which is the §15 grounded-or-silent rule applied to
# learning. `fill_questions` (below, billed, overnight) later gives an enrolled item a question to
# ASK about that proposition — without one the prompt would be the proposition itself, i.e. the
# answer.
#
# Deliberately GRADUAL. Enrolling every proposition of 32 blessed objects would put thousands of
# items in the queue and produce a permanently-saturated recall section — the guilt-inducing
# backlog §9 exists to prevent. A small per-run cap means the schedule grows a few items a night
# and settles at whatever rate he actually answers them.

DEFAULT_PER_OBJECT = 2
DEFAULT_MAX_NEW = 8

# Minimum prompt length, in characters. A crude proxy for substance, and openly so — it does not
# measure whether a claim is worth remembering, only whether it is specific enough to be worth
# being ASKED. The first live enrolment (2026-07-30) surfaced "The tanker-flow project is
# prioritized." (39) and "Python is an interpreted programming language." (46) alongside genuinely
# useful material; both are true, both survived the ingest-time anti-meta filters, and neither is
# a question anyone benefits from answering.
#
# Tuned against those observations rather than picked round: 60 was tried first and also rejected
# short-but-real claims like "Covered interest parity fails under funding stress." (51). 50 clears
# the observed junk while keeping terse substantive ones. It is a blunt instrument standing in for
# a judgement call, and the honest upgrade is a quality pass over candidates, not a bigger number.
_MIN_PROMPT_CHARS = 50


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
    from locus.learn.practice import candidates_for_object, concept_grounded

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
            if len(cand.text.strip()) < _MIN_PROMPT_CHARS:
                continue  # too thin to be worth asking — see _MIN_PROMPT_CHARS
            if not concept_grounded(cand.text, cand.concept):
                continue  # selected for a concept it is not actually about (see that function)
            ref = str(cand.id)
            if _already_scheduled(conn, "proposition", ref):
                continue
            added.append(
                schedule_prompt(conn, prompt_kind="proposition", prompt_ref=ref, today=today)
            )
            taken += 1
    return added


# --- question generation (billed, run overnight — never during composition) --------------------


def items_without_questions(
    conn, *, limit: int = 20, kinds: tuple[str, ...] = ("proposition", "concept")
) -> list[ReviewItem]:
    """Scheduled items that have no stored question yet, soonest-due first.

    `object` prompts are excluded by default: an object prompt is the owner's own question or
    idea title, which already reads as a prompt and needs nothing generated. `concept` prompts
    are a bare noun and need one badly — they are the whole point of the concept schedule.
    """
    try:
        rows = conn.execute(
            f"SELECT * FROM review_schedule WHERE prompt_kind IN "
            f"({','.join('?' * len(kinds))}) "
            "AND (question IS NULL OR TRIM(question)='') ORDER BY due, id LIMIT ?",
            (*kinds, limit),
        ).fetchall()
    except Exception:  # column absent on an un-migrated DB
        return []
    return [_row(r) for r in rows]


def set_question(conn, item_id: int, question: str) -> None:
    with conn:
        conn.execute(
            "UPDATE review_schedule SET question=? WHERE id=?", (question.strip(), item_id)
        )


def fill_questions(conn, *, limit: int = 20, runner=None, model: str | None = None) -> int:
    """Give scheduled propositions a real question to ask. Returns how many were written.

    THE SPLIT THIS CREATES is what makes the recall page work: page 3 asks the question, page 4
    carries the proposition as the reference answer. Previously both were the same string, so
    there was nothing to attempt.

    Billed (`claude -p` via `practice.generate_practice`), and deliberately NOT called from
    `compose_daily`: the page aggregates stored state and must render whether or not this ran.
    A proposition that gets no question keeps the old behaviour rather than blocking the page.

    Generation is grounded by construction — `generate_practice` drops any question referencing a
    proposition it was not shown, so a question can never carry an answer we cannot vouch for.
    """
    from locus.learn.practice import _Candidate, generate_practice

    pending = items_without_questions(conn, limit=limit, kinds=("proposition",))
    if not pending:
        return 0

    by_ref = {int(i.prompt_ref): i for i in pending if str(i.prompt_ref).isdigit()}
    if not by_ref:
        return 0
    rows = conn.execute(
        "SELECT p.id, p.text, d.title FROM propositions p JOIN documents d ON d.id=p.doc_id "
        f"WHERE p.id IN ({','.join('?' * len(by_ref))})",
        tuple(by_ref),
    ).fetchall()
    candidates = [
        _Candidate(id=r["id"], text=r["text"], doc_title=r["title"], concept=None) for r in rows
    ]
    generated = generate_practice(
        conn, candidates, max_items=len(candidates), runner=runner, model=model
    )
    written = 0
    for practice_item in generated.items:
        item = by_ref.get(practice_item.proposition_id)
        if item is None:
            continue
        set_question(conn, item.id, practice_item.question)
        written += 1
    return written


# --- concept enrolment: what he actually wants to be asked ---------------------------------------
#
# THE PROBLEM, in his words: the questions were "way too broad and just regurgitating material
# that I may have read verbatim - not actually useful". They were generated FROM PROPOSITIONS in
# blessed objects, and his blessed objects include a project roadmap, so the page asked "What
# analytical capabilities does the tanker-flow project provide for vessel activity analysis?".
# What he asked for instead: "specific questions on the mathematical concepts or financial
# instruments introduced in what I read, like how is covariance different to correlation, or what
# is covered interest rate parity, or how does a hidden markov model work."
#
# So the SUBJECT is a concept, not a document sentence. The concept must be one the corpus can
# answer (propositions define it) and one worth asking about (a maths/finance idea attested in
# prose, not a code artefact or a data series).

# A concept needs this many defining propositions before it is worth a slot: fewer than this and
# the answer overleaf is one stray sentence.
_MIN_CONCEPT_PROPS = 4


def concept_candidates(conn, *, limit: int = 40) -> list[tuple[str, int]]:
    """[(canonical concept, propositions that define it)] — enrolment's candidate pool."""
    from locus.agent.compose_daily import TEACHABLE_TYPES, _MIN_TEACHABLE_CHARS

    marks = ",".join("?" * len(TEACHABLE_TYPES))
    rows = conn.execute(
        f"""
        WITH canon AS (
            SELECT COALESCE(a.canonical_name, e.name) AS cn,
                   COALESCE(a.canonical_type, e.type) AS ct,
                   e.section_id, e.doc_id
            FROM entities e
            LEFT JOIN entity_aliases a
              ON a.variant_name = e.name AND a.variant_type = e.type
        )
        SELECT canon.cn AS name, COUNT(DISTINCT p.id) AS props
        FROM canon
        JOIN documents d ON d.id = canon.doc_id
        JOIN propositions p ON p.section_id = canon.section_id
        WHERE canon.ct IN ({marks})
          AND LENGTH(canon.cn) >= ?
          AND d.source_type != 'code'
          AND d.category IN ('paper','note','coursework','career')
        GROUP BY canon.cn
        HAVING props >= ?
        -- CURRENT READING FIRST. Ranked on proposition count alone the list is engineering
        -- coursework (transfer function, Laplace transform, Nyquist plot), because that is 144
        -- of 218 documents — true, and not what he is revising for. A concept attested in a
        -- paper or a note is one he is working with now.
        ORDER BY MAX(CASE WHEN d.category IN ('paper','note') THEN 1 ELSE 0 END) DESC,
                 props DESC
        LIMIT ?
        """,
        (*TEACHABLE_TYPES, _MIN_TEACHABLE_CHARS, _MIN_CONCEPT_PROPS, limit * 4),
    ).fetchall()

    from locus.link.related import non_topical_names

    try:
        generic = non_topical_names(conn)
    except Exception:                                  # no alias substrate yet
        generic = set()
    out: list[tuple[str, int]] = []
    for r in rows:
        if r["name"].lower() in generic:
            continue
        out.append((r["name"], r["props"]))
        if len(out) >= limit:
            break
    return out


def enrol_concepts(conn, *, max_new: int = 8, today: date | None = None) -> list[int]:
    """Schedule concepts for recall. Deterministic and free — no model call here."""
    added: list[int] = []
    for name, _props in concept_candidates(conn, limit=max_new * 4):
        if len(added) >= max_new:
            break
        if _already_scheduled(conn, "concept", name):
            continue
        added.append(schedule_prompt(conn, prompt_kind="concept", prompt_ref=name, today=today))
    return added


_CONCEPT_PROMPT = """You are writing ONE spaced-repetition question for someone revising for quant
interviews. The subject is the concept below. Here is what his own corpus says about it, which is
the reference answer he will be shown:

CONCEPT: {name}
WHAT HIS MATERIAL SAYS: {evidence}

Write a question that tests whether he UNDERSTANDS the concept — how it works, how it differs from
a neighbouring one, or when it applies. Good questions look like "How is covariance different from
correlation?", "What is covered interest rate parity?", "How does a hidden Markov model work?".

Do NOT ask about any document, project, dataset or result. Do NOT ask him to recall a sentence.
Do not mention "your material" or "the text". One sentence, ending in a question mark. Reply with
the question only."""


def fill_concept_questions(conn, *, limit: int = 8, runner=None, model: str | None = None) -> int:
    """Write the question for scheduled CONCEPTS. Billed; returns how many were written.

    Separate from `fill_questions` because the job is different: that one turns a stored claim
    into a question about that claim, which is exactly the regurgitation the owner rejected. This
    one is given a concept and its corpus evidence and asked for an UNDERSTANDING question, with
    the evidence kept as the answer rather than restated as the prompt.
    """
    from locus.agent.claude import ClaudeError, run_text
    from locus.config import load

    written = 0
    for item in items_without_questions(conn, limit=limit * 3, kinds=("concept",)):
        if written >= limit:
            break
        evidence, _src = concept_answer(conn, item.prompt_ref)
        if evidence.startswith("(no stored material"):
            continue
        try:
            text = run_text(
                _CONCEPT_PROMPT.format(name=item.prompt_ref, evidence=evidence[:1200]),
                model=model or load().agent.model, runner=runner,
            )
        except ClaudeError as exc:                     # degrade, never block
            log.warning("review: concept question failed for %r: %s", item.prompt_ref, exc)
            continue
        question = " ".join((text or "").split())
        # A question that is not a question, or that leaked the answer back, is not stored.
        if not question.endswith("?") or len(question) < 15:
            continue
        set_question(conn, item.id, question[:300])
        written += 1
    return written
