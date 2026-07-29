"""Practice questions generated from the owner's OWN stored propositions (plan §3.6, §8.4).

The constraint that makes this worth having: a question is only generated from a proposition
already in the corpus, and the proposition IS the reference answer. So every question is
answerable from material the owner has actually read or written, and grading later has something
real to grade against — rather than a model inventing quiz questions about a topic, half of which
his corpus cannot answer.

That also decides the division of labour. Python picks WHICH propositions (deterministic,
grounded, free); `claude -p` only turns a statement into a question. A returned question whose
`proposition_id` was not one we offered is dropped, not repaired — the same grounded-or-silent
enforcement the tension judge gets, for the same reason: a question with a made-up answer teaches
the owner something false.

Gap-driven, not FIFO (§12.3): selection prefers propositions attached to the concepts
`learn/gaps.py` says are thin, so practice goes where the grasp is weakest.
"""

from __future__ import annotations

import logging
from dataclasses import dataclass, field

from pydantic import BaseModel, Field

from locus.agent.claude import ClaudeError, run_structured
from locus.agent.state import parse_entity_key
from locus.config import load

log = logging.getLogger(__name__)

# Propositions offered to the model per run. The cap is the cost control AND the quality control:
# a long list produces filler questions off the weakest propositions.
_MAX_CANDIDATES = 12


@dataclass
class PracticeItem:
    question: str
    answer: str  # the source proposition, verbatim — the reference answer
    proposition_id: int
    source_title: str
    concept: str | None = None


@dataclass
class PracticeSet:
    items: list[PracticeItem] = field(default_factory=list)
    degraded: bool = False


class _Question(BaseModel):
    proposition_id: int
    question: str


class _Questions(BaseModel):
    questions: list[_Question] = Field(default_factory=list)


# --- candidate selection (deterministic) ---------------------------------------------------------


@dataclass
class _Candidate:
    id: int
    text: str
    doc_title: str
    concept: str | None
    doc_id: int | None = None  # lets callers restrict candidates to relevance-ranked documents


def candidates_for_concept(conn, name: str, *, limit: int = _MAX_CANDIDATES) -> list[_Candidate]:
    """Propositions from sections naming this canonical concept (alias-aware)."""
    rows = conn.execute(
        """
        SELECT DISTINCT p.id, p.text, p.doc_id, d.title
        FROM propositions p
        JOIN documents d ON d.id = p.doc_id
        WHERE p.section_id IN (
            SELECT e.section_id FROM entities e
            WHERE COALESCE(
                    (SELECT a.canonical_name FROM entity_aliases a
                      WHERE a.variant_name = e.name AND a.variant_type = e.type), e.name
                  ) = ?
        )
        ORDER BY p.id
        LIMIT ?
        """,
        (name, limit),
    ).fetchall()
    return [_Candidate(r["id"], r["text"], r["title"], name, r["doc_id"]) for r in rows]


def candidates_for_object(conn, object_id: int, *, limit: int = _MAX_CANDIDATES) -> list[_Candidate]:
    """Propositions from an object's own documents, PREFERRING the concepts flagged as gaps.

    Gap-driven ordering (§12.3): what he cannot explain comes before what he already can."""
    from locus.learn.gaps import _doc_ids_for_object, gaps_for_object

    out: list[_Candidate] = []
    seen: set[int] = set()
    for gap in gaps_for_object(conn, object_id):
        if gap.kind == "flagged":
            continue
        for cand in candidates_for_concept(conn, gap.subject, limit=limit):
            if cand.id not in seen:
                seen.add(cand.id)
                out.append(cand)
        if len(out) >= limit:
            return out[:limit]

    doc_ids = _doc_ids_for_object(conn, object_id)
    if doc_ids and len(out) < limit:
        placeholders = ",".join("?" * len(doc_ids))
        for r in conn.execute(
            f"SELECT p.id, p.text, p.doc_id, d.title FROM propositions p "
            f"JOIN documents d ON d.id=p.doc_id "
            f"WHERE p.doc_id IN ({placeholders}) ORDER BY p.id LIMIT ?",
            (*doc_ids, limit),
        ):
            if r["id"] not in seen:
                seen.add(r["id"])
                out.append(_Candidate(r["id"], r["text"], r["title"], None, r["doc_id"]))
    return out[:limit]


# --- generation ----------------------------------------------------------------------------------


_PROMPT = """\
Turn each statement below into ONE interview-style question whose correct answer is that \
statement. The statements come from the owner's own knowledge vault; you are helping him rehearse \
material he already has.

Rules:
- The question must be answerable from the statement alone.
- Do NOT restate the answer inside the question.
- Ask it the way an interviewer would — "why", "how", "what happens if" — not as a fill-in-the-blank.
- Skip any statement too trivial or too fragmentary to make a real question. Fewer, better.

Return ONLY JSON: {{"questions": [{{"proposition_id": <id>, "question": "<text>"}}]}}

STATEMENTS
{statements}
"""


def generate_practice(
    conn,
    candidates: list[_Candidate],
    *,
    max_items: int = 5,
    runner=None,
    model: str | None = None,
    on_result=None,
) -> PracticeSet:
    """Turn selected propositions into questions. The proposition stays as the reference answer.

    A question referencing a proposition that was not offered is DROPPED — it would carry an
    answer we cannot vouch for."""
    out = PracticeSet()
    if not candidates:
        return out
    by_id = {c.id: c for c in candidates}
    prompt = _PROMPT.format(
        statements="\n".join(f"[{c.id}] {c.text}" for c in candidates)
    )
    try:
        reply = run_structured(
            prompt, schema=_Questions, model=model or load().agent.model, runner=runner,
            on_result=on_result,
        )
    except ClaudeError as exc:
        log.warning("practice: generation failed: %s", exc)
        out.degraded = True
        return out

    for q in reply.questions:
        cand = by_id.get(q.proposition_id)
        if cand is None or not q.question.strip():
            log.debug("practice: dropped ungrounded question for id %s", q.proposition_id)
            continue
        out.items.append(PracticeItem(
            question=q.question.strip(), answer=cand.text, proposition_id=cand.id,
            source_title=cand.doc_title, concept=cand.concept,
        ))
        if len(out.items) >= max_items:
            break
    return out


def practice_for_object(
    conn, object_id: int, *, max_items: int = 5, runner=None, model: str | None = None
) -> PracticeSet:
    """The interview-prep entry point: practice questions for a project/concept object."""
    return generate_practice(
        conn, candidates_for_object(conn, object_id), max_items=max_items, runner=runner,
        model=model,
    )


def practice_for_concept_key(
    conn, subject_key: str, *, max_items: int = 5, runner=None, model: str | None = None
) -> PracticeSet:
    """Practice for a canonical concept addressed by its entity key."""
    name, _ = parse_entity_key(subject_key)
    return generate_practice(
        conn, candidates_for_concept(conn, name), max_items=max_items, runner=runner, model=model
    )


# Propositions taken from any single document in the fallback. A cap forces the round-robin to
# reach the 2nd/3rd ranked document rather than filling up on the 1st.
_PER_DOC_CAP = 3


def candidates_from_documents(conn, doc_ids: list[int], *, limit: int = _MAX_CANDIDATES) -> list[_Candidate]:
    """Propositions from specific documents — the grounded fallback when no concept name matches.

    Used by the synthesis surface for topics the corpus covers well under a different canonical
    name than the owner asked about ('portfolio construction' vs 'portfolio optimization'). Still
    the owner's own stored propositions; only the route to them is looser than a concept join.

    `doc_ids` MUST arrive in retrieval-relevance order, and is honoured in that order: an earlier
    version sorted by proposition id, which meant the lowest-numbered document won regardless of
    relevance. A live `synthesise("market making and arbitrage strategies")` then generated
    practice questions off a Python tutorial ("What is an IDE?") while the Optiver market-making
    repo sat further down the evidence list. Round-robin with a per-document cap so the top
    documents lead and one verbose document cannot monopolise the set."""
    ordered: list[int] = []
    for d in doc_ids:
        if d is not None and d not in ordered:
            ordered.append(d)
    if not ordered:
        return []
    by_doc: dict[int, list[_Candidate]] = {}
    placeholders = ",".join("?" * len(ordered))
    for r in conn.execute(
        f"SELECT p.id, p.text, p.doc_id, d.title FROM propositions p "
        f"JOIN documents d ON d.id=p.doc_id WHERE p.doc_id IN ({placeholders}) ORDER BY p.id",
        ordered,
    ):
        bucket = by_doc.setdefault(r["doc_id"], [])
        if len(bucket) < _PER_DOC_CAP:
            bucket.append(_Candidate(r["id"], r["text"], r["title"], None, r["doc_id"]))

    out: list[_Candidate] = []
    for rank in range(_PER_DOC_CAP):
        for doc_id in ordered:  # retrieval order, best document first
            bucket = by_doc.get(doc_id, [])
            if rank < len(bucket):
                out.append(bucket[rank])
                if len(out) >= limit:
                    return out
    return out
