"""Answer the questions he wrote in the margin while reading.

THE GAP THIS CLOSES. `capture/intent` classifies a mark three ways and `not_understood` is the
most valuable of them — it is the one place he says, in his own hand, at the moment of confusion,
that he did not follow something. Nine such questions had accumulated ("what is a factor
covariance estimator r = Bf + e", "how are information ratio and sharpe ratio different?", "no
momentum in Japan??") and the only thing the signal earned was a corpus RE-READ slot on the Read
page: offer him a whole other document and hope it explains. The gate log showed that slot
rejecting 30 of 30 candidates, so in practice the signal earned nothing at all.

Answering the question directly is what he asked for, and the input is unusually good: the
question is already transcribed, already bound by geometry to the passage that prompted it, and
the passage names the vocabulary the answer needs.

WHAT IS NOT ANSWERED. Marginalia is not uniformly questions: "is this any good?" and "interesting"
name nothing to search on, and several passages are captured FRAGMENTS ("->s)", "ratio between a")
because the mark's geometry caught the tail of a line. A pass that answers everything would
produce confident prose about nothing, which is worse than the silence it replaced. The only
selection test that survived measurement is whether there is enough text to retrieve against —
see the note on `_MIN_QUESTION_CHARS` for the grammar-based gate that was built and rejected.

GROUNDED OR SILENT (invariant 3). Evidence comes from `surface/grounding.ground_topic` — the same
deterministic, floor-filtered retrieval the critique surface uses — and the answer must cite a key
it was actually given. A citation that does not resolve is dropped, and an answer left with no
resolving citation is not stored. "The corpus cannot answer this" is a true outcome and the one
the discovery channel exists to act on; a plausible invention is not.
"""

from __future__ import annotations

import json
import logging
import re
import sqlite3
from dataclasses import dataclass
from datetime import datetime, timezone

from pydantic import BaseModel, field_validator

log = logging.getLogger(__name__)

# A note has to be at least this long before it is worth searching on. Below it the mark is a
# reaction, not a question — "is this any good?" carries no vocabulary to retrieve against, and
# what comes back is whatever the passage fragment happened to say.
_MIN_QUESTION_CHARS = 18

# A QUESTION-SHAPE TEST WAS BUILT, MEASURED AND REJECTED. The first cut required a
# question-word opener, on the theory that it would separate real questions from notes-to-self.
# Run over his nine live marks it was wrong in both directions on exactly the two cases it was
# written for: it dropped "no momentum in Japan??" — a real question, and a good one, since the
# absence of momentum in Japan is a well-known anomaly — while passing "what is left to
# understand? -> research area return to this, unsure", which is him parking something.
#
# Marginalia does not have reliable grammar, so grammar cannot be the filter. What remains is the
# only test that held: is there enough here to search on? Everything else is left to
# grounded-or-silent, which is the real protection — an answer that cannot cite the corpus is
# never stored, so a vague note costs one retrieval and produces nothing.

# Failures to ground an answer before a question is PARKED and stops being offered. Three, not
# one: the corpus changes under it (every ingest, every `locus link`), so a single miss says
# nothing, while a question that has missed three separate nights is not one more retrieval away.
# Parking is a pause, never a verdict — `unpark_questions` clears every counter, and `locus status`
# names what is parked so it cannot quietly cease to exist.
#
# WITHOUT THIS THE RETRY IS ETERNAL. `pending_questions` selects marks with no `mark_answers` row
# and grounded-or-silent stores nothing when the corpus cannot answer, so the same mark returns
# every night: mark 11 did exactly that from 2026-08-04 onward, taking one of the four nightly
# slots each time. Two correct rules composing into a permanent occupant at the head of a bounded
# queue — the same shape as the recall and intent starvations.
_MAX_ANSWER_ATTEMPTS = 3

# Evidence units put in front of the model. Fewer than the critique surface uses: this is one
# question about one passage, and a wider net mostly adds neighbours of the same document.
_MAX_EVIDENCE = 8

# The stored answer shares a page with the questions themselves, so it is bounded like the recall
# answer is. Measured against the page budget, not chosen.
_MAX_ANSWER_CHARS = 420

# The evidence handles invented for the prompt ([S1], [S2]...). They must never reach the page.
_INTERNAL_KEY = re.compile(r"\[[A-Z]\d+\]")


@dataclass
class MarkQuestion:
    """One question he wrote, with the text it was written against.

    TWO FIELDS, because they are two different things and the second is usually the load-bearing
    one. `passage` is what the ink actually covered; `line` is the whole line it sits on, which
    `capture/annotate` has been storing all along.

    Marginalia is DEICTIC — it points. "what does variance physically mean here?" carries no
    subject at all; "here" is the entire content of the question, and it lives in the line, not in
    the note. Live, that mark's `covered_text` was the fragment "ratio between a" (15 chars, the
    tail of a wrapped line) while its `line_text` read "The % idio variance (which we also denote
    p) is the ratio between a portfolio's idio variance and...". Answering from the fragment
    produced a correct explanation of variance in general and a wrong answer to HIS question,
    which was about percent idiosyncratic variance.
    """

    mark_id: int
    question: str
    passage: str        # what the ink covered — may be a fragment, may be empty
    line: str           # the full line it sits on — usually the real context
    source: str
    page: int | None

    @property
    def context(self) -> str:
        """The best available text for what he was pointing AT."""
        return self.line if len(self.line) > len(self.passage) else self.passage


@dataclass
class MarkAnswer:
    mark_id: int
    question: str
    answer: str
    evidence: list[dict]
    source: str


class _Answer(BaseModel):
    answer: str
    citation_key: str

    @field_validator("answer")
    @classmethod
    def _no_internal_keys(cls, v: str) -> str:
        """Reject an answer that references the evidence handles.

        Live, one answer read "the estimator is that specific mathematical recipe — the one
        defined in [S2]". `S2` is an internal handle invented for the prompt; on the page it is
        noise pointing at nothing. Rejecting rather than stripping because the sentence is built
        around the reference ("the one defined in ") and stripping leaves it broken —
        `run_structured`'s repair retry gets a second attempt instead.
        """
        if _INTERNAL_KEY.search(v or ""):
            raise ValueError("answer must not reference evidence keys like [S2]")
        return v


def _drop_his_own_marks(conn, evidence):
    """Remove the reading-notes aggregation from the evidence — it IS his marks.

    `reading/sweep` collects every mark and its passage into
    `vault/notes/reading/<book>.md`, which `notes_sync` ingests as an ordinary note. That makes it
    retrievable, and it retrieves EXTREMELY well against a mark's own question, because it
    literally contains that question. Live (2026-08-03) two of four answers cited it, and the
    quoted "evidence" was the owner's own words read back to him:

        "## p.70  what is left to understand? -> research area return to this, unsure
         > exit at the same time in a crowded but are still not well understood"

    Answering his question with his question is circular, and it is the §24 failure — the page
    being read back as his own handwriting — in a new surface. The BOOK itself stays eligible:
    mark 8's answer correctly cites the actual formula from Advanced Portfolio Management.
    """
    from locus.agent.promote import READING_SUBDIR

    doc_ids = {e.doc_id for e in evidence if e.doc_id is not None}
    if not doc_ids:
        return evidence
    marks = ",".join("?" * len(doc_ids))
    excluded = {
        r["id"]
        for r in conn.execute(
            f"SELECT id, source_uri FROM documents WHERE id IN ({marks}) "
            f"AND (source_uri LIKE ? OR source_uri LIKE ?)",
            (*doc_ids, f"%/notes/{READING_SUBDIR}/%", f"notes/{READING_SUBDIR}/%"),
        )
    }
    if not excluded:
        return evidence
    return [e for e in evidence if e.doc_id not in excluded]


def _best_support(answer: str, evidence) -> tuple[object, float]:
    """The evidence unit that best supports the ANSWER, and its score.

    THE MODEL'S OWN CITATION IS NOT TRUSTED, because measured (2026-08-03, three live answers) it
    is arbitrary. Asked which key it used, it cited "Figure 4.2 Steps needed to generate a risk
    model. The figure is a block diagram..." for a correct answer about factor covariance
    estimation, and a passage distinguishing momentum from trend-following for an answer about
    Japan. Both keys RESOLVED, so the citation check passed — a model told to cite will cite, and
    enforcing that the key exists only catches invented ones, never irrelevant ones.

    Scoring against the ANSWER rather than the question is what the same three cases separate on:
    the fig-leaf citation scored 2.970 against its question (the HIGHEST of the three) and -0.710
    against its answer, while the one genuinely supporting citation scored 0.888 and 0.543. The
    question shares vocabulary with anything on the topic; only the answer shares it with the
    passage that actually backs the claim.

    NO FLOOR IS APPLIED YET, deliberately. Three points cannot calibrate a threshold, and the last
    threshold guessed on this codebase (`_REREAD_MIN_RERANK`) rejected 30 of 30 and sat dead. The
    score is recorded on every answer and logged, so a week of real data decides it instead.
    """
    try:
        from locus.retrieve.rerank import score_pairs

        scores = score_pairs(answer, [e.text for e in evidence])
    except Exception:                                # the `rerank` extra is optional
        return evidence[0], float("nan")
    best = max(range(len(evidence)), key=lambda i: scores[i])
    return evidence[best], float(scores[best])


_SYSTEM = (
    "You answer a specific question someone wrote in the margin of something they were reading. "
    "You are given the passage that prompted it and evidence from their own library. You never "
    "state anything the evidence does not support."
)

_TEMPLATE = """He wrote this question while reading:
{question}

The line he wrote it beside — this is what "this", "here" and "they" refer to:
{line}

The words his ink actually covered:
{passage}

Evidence from his own library (cite ONE key):
{evidence}

Answer HIS question directly, in two or three sentences. Explain the thing he did not follow —
the mechanism or the distinction — rather than restating the passage. Assume a strong quant
reader: no textbook throat-clearing, no "great question". If the evidence does not actually
answer it, say so plainly in the answer field and cite the closest key.

The keys are internal handles for this prompt only. NEVER write "[S1]" or "the passage" or "the
evidence" in the answer — he sees the answer, not this list, and a reference to a key points at
nothing.

Return ONLY JSON: {{"answer": "<the answer>", "citation_key": "<the key you used>"}}"""


def _has_enough_to_search(note: str) -> bool:
    """Is there enough here to retrieve against? See the note on `_MIN_QUESTION_CHARS`.

    Live, this keeps "no momentum in Japan??" (21 chars, a real question) and drops "is this any
    good?" (17) and the bare reactions "important" and "interesting...", which name nothing.
    """
    return len(" ".join((note or "").split())) >= _MIN_QUESTION_CHARS


def pending_questions(conn, *, limit: int = 20) -> list[MarkQuestion]:
    """Marks he flagged as not understood, that read as questions and have no answer yet.

    PARKED MARKS ARE EXCLUDED (`_MAX_ANSWER_ATTEMPTS`). A question the corpus has failed to ground
    three times would otherwise be re-offered every night forever, because nothing is stored when
    grounding fails and this query selects on the absence of a stored row.
    """
    try:
        rows = conn.execute(
            """
            SELECT a.id, a.note, a.covered_text, a.line_text, a.pdf_page, d.title
            FROM pdf_annotations a
            LEFT JOIN documents d ON d.source_uri = a.source_uri
            WHERE a.intent = 'not_understood'
              AND a.note IS NOT NULL AND TRIM(a.note) != ''
              AND COALESCE(a.answer_attempts, 0) < ?
              AND NOT EXISTS (SELECT 1 FROM mark_answers m WHERE m.mark_id = a.id)
            ORDER BY a.id
            """,
            (_MAX_ANSWER_ATTEMPTS,),
        ).fetchall()
    except sqlite3.OperationalError:          # pre-0035 DB: no counter, so nothing is parked
        rows = conn.execute(
            """
            SELECT a.id, a.note, a.covered_text, a.line_text, a.pdf_page, d.title
            FROM pdf_annotations a
            LEFT JOIN documents d ON d.source_uri = a.source_uri
            WHERE a.intent = 'not_understood'
              AND a.note IS NOT NULL AND TRIM(a.note) != ''
              AND NOT EXISTS (SELECT 1 FROM mark_answers m WHERE m.mark_id = a.id)
            ORDER BY a.id
            """
        ).fetchall()
    out: list[MarkQuestion] = []
    for r in rows:
        note = " ".join((r["note"] or "").split())
        if not _has_enough_to_search(note):
            log.info("answers: skipping mark %s — nothing to search on: %r", r["id"], note[:60])
            continue
        out.append(
            MarkQuestion(
                mark_id=r["id"],
                question=note,
                passage=" ".join((r["covered_text"] or "").split()),
                line=" ".join((r["line_text"] or "").split()),
                source=r["title"] or "",
                page=r["pdf_page"],
            )
        )
        if len(out) >= limit:
            break
    return out


def _search_text(q: MarkQuestion) -> str:
    """What to retrieve on: the question plus the line he wrote it against.

    The line carries the vocabulary the question omits, and omitting it is how "what does variance
    physically mean here?" retrieved a signals-and-systems lecture on random processes instead of
    the book's own section on percent idiosyncratic variance — the question alone names nothing
    but "variance", so the corpus answered a different question well.

    Still gated on length, because a 3-character fragment ("->s)") is noise that drags retrieval
    off the subject; `MarkQuestion.context` prefers the full line, which usually clears it.
    """
    context = q.context
    if len(context) >= 40:
        return f"{q.question} {context}"
    return q.question


def answer_question(
    conn, q: MarkQuestion, *, runner=None, model: str | None = None, retrieve_fn=None
) -> MarkAnswer | None:
    """Ground, answer, verify the citation, store. None when the corpus cannot answer it."""
    from locus.agent.claude import ClaudeError, run_structured
    from locus.config import load
    from locus.surface.grounding import ground_topic

    grounding = ground_topic(
        conn, _search_text(q), retrieve_fn=retrieve_fn,
        include_trajectories=False, include_gaps=False, max_evidence=_MAX_EVIDENCE,
    )
    grounding.evidence = _drop_his_own_marks(conn, grounding.evidence)
    if not grounding.evidence:
        log.info("answers: no evidence for mark %s", q.mark_id)
        return None

    by_key = {e.key: e for e in grounding.evidence}
    rendered = "\n".join(f"[{e.key}] ({e.source}) {e.text}" for e in grounding.evidence)
    prompt = _TEMPLATE.format(
        question=q.question,
        line=q.line or "(not captured)",
        passage=q.passage or "(not captured)",
        evidence=rendered,
    )
    try:
        out = run_structured(
            f"{_SYSTEM}\n\n{prompt}", schema=_Answer,
            model=model or load().agent.model, runner=runner,
        )
    except ClaudeError as exc:                       # advisory: degrade, never block
        log.warning("answers: model call failed for mark %s: %s", q.mark_id, exc)
        return None

    answer = " ".join((out.answer or "").split())
    if not answer:
        log.info("answers: empty answer for mark %s", q.mark_id)
        return None
    # The model's own key is read only as a sanity signal — the STORED citation is chosen
    # deterministically against the answer, for the reasons in `_best_support`.
    claimed = by_key.get((out.citation_key or "").strip())
    cited, support = _best_support(answer, grounding.evidence)
    if claimed is not None and claimed.key != cited.key:
        log.info(
            "answers: mark %s cited %s, best support is %s (%.3f)",
            q.mark_id, claimed.key, cited.key, support,
        )

    evidence = [
        {"key": cited.key, "text": cited.text[:400], "source": cited.source,
         "support": None if support != support else round(support, 3)}  # NaN-safe
    ]
    stored = MarkAnswer(
        mark_id=q.mark_id, question=q.question, answer=answer[:_MAX_ANSWER_CHARS],
        evidence=evidence, source=cited.source or q.source,
    )
    with conn:
        conn.execute(
            "INSERT INTO mark_answers (mark_id, question, answer, evidence, source, written_at) "
            "VALUES (?,?,?,?,?,?) ON CONFLICT(mark_id) DO UPDATE SET "
            "question=excluded.question, answer=excluded.answer, evidence=excluded.evidence, "
            "source=excluded.source, written_at=excluded.written_at",
            (stored.mark_id, stored.question, stored.answer, json.dumps(stored.evidence),
             stored.source, datetime.now(timezone.utc).isoformat()),
        )
    return stored


def write_answers(
    conn, *, limit: int = 4, runner=None, model: str | None = None, retrieve_fn=None
) -> int:
    """Answer up to `limit` pending questions overnight. Returns how many were stored."""
    from locus.observe import gates

    written = 0
    for q in pending_questions(conn, limit=limit * 3):
        if written >= limit:
            break
        got = answer_question(conn, q, runner=runner, model=model, retrieve_fn=retrieve_fn)
        gates.record(
            conn, "answers.grounded", rejected=got is None,
            value=None if got else q.question[:80],
        )
        if got is None:
            _record_failed_attempt(conn, q.mark_id)
        else:
            written += 1
            # NOT a gate — nothing is rejected on this — but the log is the cheapest place to
            # accumulate the support scores a floor would have to be calibrated against.
            support = (got.evidence[0].get("support") if got.evidence else None)
            gates.record(
                conn, "answers.support_score", rejected=False,
                value=f"{support} {got.question[:60]}",
            )
    return written


def _record_failed_attempt(conn, mark_id: int) -> None:
    """Count one failure to ground an answer, so a question cannot be retried forever.

    Silent on an un-migrated DB (the column arrives in 0035): a missing counter means every mark
    stays eligible, which is exactly the old behaviour.
    """
    try:
        with conn:
            conn.execute(
                "UPDATE pdf_annotations SET answer_attempts = COALESCE(answer_attempts,0) + 1 "
                "WHERE id=?",
                (mark_id,),
            )
    except sqlite3.OperationalError:
        log.debug("answers: no answer_attempts column; not counting the failure")


def parked_questions(conn) -> list[tuple[int, str]]:
    """(mark_id, question) for marks the corpus has failed to answer `_MAX_ANSWER_ATTEMPTS` times.

    Read by `locus status`. Parking has to be VISIBLE or it is indistinguishable from the question
    never having been asked — which is the failure this whole surface was built to end.
    """
    try:
        rows = conn.execute(
            "SELECT id, note FROM pdf_annotations "
            "WHERE COALESCE(answer_attempts,0) >= ? AND TRIM(COALESCE(note,'')) != '' "
            "AND NOT EXISTS (SELECT 1 FROM mark_answers m WHERE m.mark_id = pdf_annotations.id) "
            "ORDER BY id",
            (_MAX_ANSWER_ATTEMPTS,),
        ).fetchall()
    except sqlite3.OperationalError:
        return []
    return [(r["id"], " ".join((r["note"] or "").split())) for r in rows]


def unpark_questions(conn) -> int:
    """Clear every parking counter so the pass tries again. Returns how many were reset.

    The corpus is not what it was when the question failed — every ingest and every `locus link`
    changes what can be grounded — so parking is a pause, not a verdict.
    """
    try:
        with conn:
            cur = conn.execute(
                "UPDATE pdf_annotations SET answer_attempts = 0 "
                "WHERE COALESCE(answer_attempts,0) > 0"
            )
        return cur.rowcount
    except sqlite3.OperationalError:
        return 0


def open_answers(conn, *, limit: int = 4) -> list[MarkAnswer]:
    """Answered questions he has not dismissed — what the page prints. A pure join."""
    try:
        rows = conn.execute(
            "SELECT mark_id, question, answer, evidence, source FROM mark_answers "
            "WHERE dismissed_at IS NULL ORDER BY written_at DESC, id DESC LIMIT ?",
            (limit,),
        ).fetchall()
    except Exception:                                # table absent on an un-migrated DB
        return []
    out = []
    for r in rows:
        try:
            evidence = json.loads(r["evidence"] or "[]")
        except (TypeError, ValueError):
            evidence = []
        out.append(
            MarkAnswer(
                mark_id=r["mark_id"], question=r["question"], answer=r["answer"],
                evidence=evidence, source=r["source"] or "",
            )
        )
    return out
