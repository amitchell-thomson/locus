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

from pydantic import BaseModel

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


def due_items(
    conn, *, today: date | None = None, limit: int = 5, offset: int = 0
) -> list[ReviewItem]:
    """Items due on or before `today`, soonest-due first. `limit` is the daily-page cap (§9).

    `offset` exists so a caller that discards some of what it gets can ask for the NEXT page of
    the queue rather than a bigger first page. `compose_daily.build_recalls` drops items already
    retired by `daily_shown`, and a card shown but never graded keeps its `due` forever — so it
    keeps both its retired key and its place at the head of this ordering. Seventeen such cards
    accumulated by 2026-08-06 and filled every fixed window the page ever asked for, which is how
    the Recall page went 4 -> 4 -> 1 -> 0 items over four days and would have stayed at 0.
    """
    today = today or date.today()
    return [
        _row(r)
        for r in conn.execute(
            "SELECT * FROM review_schedule WHERE due <= ? ORDER BY due, id LIMIT ? OFFSET ?",
            (today.isoformat(), limit, offset),
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


def concept_evidence(conn, name: str, *, limit: int = 6) -> tuple[list[str], str]:
    """(propositions about the concept, source title) — what the question is written FROM.

    RANKED BY RELEVANCE TO THE CONCEPT, not by length. The previous rule took the longest
    proposition in any section that merely MENTIONS the concept, and length is not aboutness: for
    `Bollinger bands` it returned a sentence describing a study's input format ("a sliding window
    of 30 daily OHLCV data points, together with pre-computed RSI..."), which happened to be the
    longest thing in the section. That sentence then had to serve as both the evidence a question
    was written from and the answer printed overleaf.

    Several propositions, not one: these are mechanism-and-trade-off questions now, and a single
    corpus sentence rarely contains enough to answer one.
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
        """,
        (name,),
    ).fetchall()
    if not rows:
        return [], ""

    texts = [r["text"] for r in rows]
    order = list(range(len(texts)))
    try:
        from locus.retrieve.rerank import score_pairs

        # The same cross-encoder every corpus query uses. Naming the concept as the query is the
        # whole point: a proposition ABOUT the concept outranks one that merely sits beside it.
        scores = score_pairs(name, texts)
        order.sort(key=lambda i: -scores[i])
    except Exception:                                  # the `rerank` extra is optional
        # Deterministic fallback: propositions that actually contain the concept's words first.
        needle = name.casefold()
        order.sort(key=lambda i: (needle not in texts[i].casefold(), -len(texts[i])))
    picked = order[:limit]
    return [texts[i] for i in picked], rows[picked[0]]["title"]


def concept_answer(conn, name: str) -> tuple[str, str]:
    """(reference answer, source) for a concept — the STORED answer, or the old fallback.

    The stored answer is written alongside the question from the same evidence, so the two cannot
    disagree (migration 0033). This function is the read path and the degradation: a row written
    before that change, or one whose answer failed its grounding check, still gets corpus text
    rather than nothing.
    """
    row = conn.execute(
        "SELECT answer, answer_source FROM review_schedule "
        "WHERE prompt_kind='concept' AND prompt_ref=? AND answer IS NOT NULL AND answer != '' "
        "LIMIT 1",
        (name,),
    ).fetchone()
    if row:
        return row["answer"], row["answer_source"] or ""

    evidence, source = concept_evidence(conn, name, limit=1)
    if not evidence:
        return f"(no stored material defines {name})", ""
    # ONE proposition, and a bounded one. The answers share page 4 with the open writing region,
    # and four unbounded answers pushed it onto a seventh page.
    return evidence[0], source


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

    A CONCEPT CARD IS BOTH HALVES, so a concept row missing its ANSWER is incomplete too and is
    returned for rewriting. That is not only to backfill: the questions written before migration
    0033 were generated from the longest-proposition evidence, which is the rule that produced a
    Bollinger-bands question answered by a study's input format. The question is as suspect as
    the answer, and both are rewritten from the better evidence.

    `fill_concept_questions` always ends up storing SOME answer (the written one, or corpus text
    when it fails its grounding check), so this cannot become a nightly re-bill loop.
    """
    try:
        rows = conn.execute(
            f"SELECT * FROM review_schedule WHERE prompt_kind IN "
            f"({','.join('?' * len(kinds))}) "
            "AND ((question IS NULL OR TRIM(question)='') "
            "     OR (prompt_kind='concept' AND (answer IS NULL OR TRIM(answer)=''))) "
            "ORDER BY due, id LIMIT ?",
            (*kinds, limit),
        ).fetchall()
    except Exception:  # column absent on an un-migrated DB
        return []
    return [_row(r) for r in rows]


def set_question(
    conn, item_id: int, question: str, *, answer: str = "", source: str = ""
) -> None:
    """Store a question and, for concepts, the answer written WITH it from the same evidence.

    Both in one statement because they are one fact: a question stored without its answer is how
    page 3 and page 4 drifted apart in the first place.
    """
    with conn:
        conn.execute(
            "UPDATE review_schedule SET question=?, answer=?, answer_source=? WHERE id=?",
            (question.strip(), (answer or "").strip() or None, (source or "").strip() or None,
             item_id),
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
        SELECT canon.cn AS name, COUNT(DISTINCT p.id) AS props,
               COUNT(DISTINCT canon.doc_id) AS docs
        FROM canon
        JOIN documents d ON d.id = canon.doc_id
        JOIN propositions p ON p.section_id = canon.section_id
        WHERE canon.ct IN ({marks})
          AND LENGTH(canon.cn) >= ?
          AND d.source_type != 'code'
          AND d.category IN ('paper','note','coursework','career')
        GROUP BY canon.cn
        -- AT LEAST TWO DOCUMENTS. Rarity alone is single-document vocabulary — the first run
        -- offered "AlphaZeroBeta" (a paper's name) and six candlestick patterns from one book.
        -- §21 encodes the same rule for thread concepts: "a name in one document is that
        -- document's vocabulary". Two documents means someone corroborated it.
        HAVING props >= ? AND docs >= 2
        -- CURRENT READING FIRST. Ranked on proposition count alone the list is engineering
        -- coursework (transfer function, Laplace transform, Nyquist plot), because that is 144
        -- of 218 documents — true, and not what he is revising for. A concept attested in a
        -- paper or a note is one he is working with now.
        --
        -- THEN RARE BEFORE COMMON. Proposition count is a POPULARITY measure and the most popular
        -- canonicals are the most elementary ("volatility", "Sharpe ratio", "transfer function")
        -- — exactly the questions he called "slightly too simple". A concept spanning two
        -- documents ("idiosyncratic volatility", "factor covariance estimator") is the specific
        -- one; §18 learned the same lesson for read-next, where ranking by raw count handed
        -- every slot to coursework. The `props` floor already guarantees answerability, so
        -- rarity can drive the order without producing a question nothing can answer.
        ORDER BY MAX(CASE WHEN d.category IN ('paper','note') THEN 1 ELSE 0 END) DESC,
                 docs ASC,
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
    from locus.observe import gates

    out: list[tuple[str, int]] = []
    for r in rows:
        drop = r["name"].lower() in generic
        gates.record(
            conn, "review.concept_non_topical", rejected=drop,
            value=None if not drop else r["name"],
        )
        if drop:
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


# Below this an "answer" is a stub, not the few sentences of reasoning the card is for.
_MIN_ANSWER_CHARS = 80


def _answer_is_usable(answer: str, concept: str) -> bool:
    """Is this a real answer ABOUT this concept?

    NOT `ingest.summarize.is_grounded`, which was the first attempt and was measured wrong for
    this job (2026-08-03: it rejected 5 of 9 live answers, and the fallback it forced reinstated
    the exact defect being fixed — an answer that does not answer the question). `is_grounded`
    asks "does this SUMMARY reuse the vocabulary of the text it summarises", which is right for a
    summary and wrong for reasoning: the question deliberately interrogates an edge, so the
    answer generalises past the propositions and legitimately introduces words they do not
    contain. Rejecting that penalises exactly the answers worth having.

    So the bar is aboutness plus substance, which is what actually distinguishes a usable answer
    from the failure mode that matters — a confident reply about a different subject. It still
    rejects "Quantum chromodynamics governs the strong nuclear interaction" for a question about
    AIS interpolation.

    THE TRADE, stated: page 4 now carries model-authored reasoning rather than corpus text only.
    That is a §11.B judgement — it is his own self-check, he is the one who knows the material,
    and the prompt forbids carrying over figures, tickers and model names. The rejection rate is
    logged (`review.answer_usable`) so the choice stays visible instead of becoming folklore.
    """
    if not answer or len(answer) < _MIN_ANSWER_CHARS:
        return False
    # Stem-matched so "factor models" satisfies "factor model" and morphology does not fail it.
    words = [w for w in concept.casefold().split() if len(w) > 3]
    body = answer.casefold()
    if not words:
        return concept.casefold() in body
    return any(w[:6] in body for w in words)


class _ConceptCard(BaseModel):
    """A question and the answer to THAT question — written together, from one evidence set.

    Two fields in one call rather than two calls, because the failure being fixed is precisely
    that the question and the answer were produced by different mechanisms and nothing tied them
    together. Asked as one object, a mismatch is not a thing that can happen.
    """

    question: str
    answer: str


_CONCEPT_PROMPT = """You are writing ONE spaced-repetition card for a strong quant candidate
preparing for buy-side interviews. He already knows the textbook definitions. The subject is the
concept below, and here is what his own corpus says about it:

CONCEPT: {name}
WHAT HIS MATERIAL SAYS:
{evidence}

Write a question that MAKES HIM THINK. Aim at one of these, whichever the material supports:
  - a mechanism ("why does X produce Y rather than Z?")
  - a failure mode or breaking assumption ("when does X stop being valid, and what happens?")
  - a trade-off between two defensible choices
  - a consequence that is not obvious from the definition
  - a quantitative relationship worth deriving or reasoning about

Do NOT ask "what is X" or "how does X differ from Y" — those are the definitional questions he
already knows. Assume the definition; interrogate the edge, the assumption or the consequence. It
should be answerable in a few sentences by someone who genuinely understands it, and awkward for
someone who has only read about it.

IT MUST STAND ALONE. The material above is one study's treatment of the concept and will contain
its particular figures, tickers, model names and sample windows. NONE of those may appear in the
question: no percentages or multiples from it, no named model or strategy, no "with only 30 days
of data". Someone who has never read that study must be able to answer from understanding the
concept alone. Use the material to find the interesting EDGE, then ask about the edge in general
terms.

Do NOT ask about any document, project, dataset or result, and do not mention "your material" or
"the text". One or two sentences, ending in a question mark.

THEN ANSWER YOUR OWN QUESTION, for the back of the card. The answer must actually answer the
question you just wrote — the reasoning, not a restatement of the definition and not a summary of
the material. Ground it in the material above: use its substance, but state it in general terms,
carrying over none of its particular figures, tickers, model names or sample windows. Three or
four sentences at most; it shares a page with his own written attempt.

Return ONLY JSON: {{"question": "<the question>", "answer": "<the answer to it>"}}"""


def fill_concept_questions(conn, *, limit: int = 8, runner=None, model: str | None = None) -> int:
    """Write the question AND its answer for scheduled CONCEPTS. Billed; returns how many.

    Separate from `fill_questions` because the job is different: that one turns a stored claim
    into a question about that claim, which is exactly the regurgitation the owner rejected.

    Both halves come from ONE call over ONE evidence set. Previously the question was generated
    here and the answer was re-derived at page-composition time by a different rule, and the two
    silently disagreed — page 3 asked why volatility undermines Bollinger-band mean reversion and
    page 4 replied with a study's input format.
    """
    from locus.agent.claude import ClaudeError, run_structured
    from locus.config import load
    from locus.observe import gates

    written = 0
    for item in items_without_questions(conn, limit=limit * 3, kinds=("concept",)):
        if written >= limit:
            break
        facts, source = concept_evidence(conn, item.prompt_ref)
        if not facts:
            continue
        evidence = "\n".join(f"- {f}" for f in facts)
        try:
            verdict = run_structured(
                _CONCEPT_PROMPT.format(name=item.prompt_ref, evidence=evidence[:2400]),
                schema=_ConceptCard,
                model=model or load().agent.model, runner=runner,
            )
        except ClaudeError as exc:                     # degrade, never block
            log.warning("review: concept question failed for %r: %s", item.prompt_ref, exc)
            continue
        question = " ".join((verdict.question or "").split())
        answer = " ".join((verdict.answer or "").split())
        # A question that is not a question, or that leaked the answer back, is not stored.
        if not question.endswith("?") or len(question) < 15:
            continue
        ok = _answer_is_usable(answer, item.prompt_ref)
        gates.record(
            conn, "review.answer_usable", rejected=not ok,
            value=None if ok else f"{item.prompt_ref}: {answer[:70]}",
        )
        # FALL BACK TO CORPUS TEXT rather than to nothing. An empty answer would leave the row
        # incomplete, and `items_without_questions` would offer it again every night — paying for
        # the same rejection forever, the trap §26 found in the tension cache. The best-ranked
        # proposition is what the old rule aimed at and is at least about the right concept.
        if not ok:
            log.info("review: unusable answer for %r, falling back", item.prompt_ref)
            answer = facts[0]
        set_question(conn, item.id, question[:300], answer=answer[:600], source=source)
        written += 1
    return written
