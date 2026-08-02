"""The written reason a paper was proposed — what of his it bears on, and what he could do with it.

THE PROBLEM THIS SOLVES, in his words: "I want to see what it links to (projects/ thoughts-wise)
and why its relevant/ how it might help generate ideas/ develop projects". The deterministic
`why` cannot say that. It reads:

    closest to your work on Alpha Fund (fit 0.76; nearest existing material 0.79)

which is a statement about cosine distances. It is the same failure as the old connection line
("shares 3 concepts with a document you have not linked it to") — true, checkable, and no reason
for anyone to care. A similarity score is evidence that a link exists; it is not the link.

WHY THIS IS A MODEL PASS WHEN NOTHING ELSE IN DISCOVERY IS. The Phase-4 design deliberately ruled
one out ("every ranking term is a join or an arithmetic count"), and that stands for RANKING —
scoring papers with a model would add spend, latency and a hallucination surface to a pipeline
that needs none. Explaining the top-ranked handful is a different job: the ranking already
decided, and what is left is to say in English what two stored texts have to do with each other.

WHEN IT RUNS, AND WHY THAT MATTERS. At proposal time, not at page-composition time, so
`compose_daily` stays aggregate-only and the page renders identically whether or not this ran.
A proposal still unaccepted after `rewrite_after_days` has its reason REWRITTEN against his
current threads: a paper proposed three weeks ago was justified against work he has since moved
past, and a stale reason is exactly what made the old read-next slot worthless. The rewrite is
also what earns a proposal a second appearance on the Read page — `compose_daily.build_readings`
keys the no-repeat ledger on `why_written_at`, so a repeat always carries new text.

GROUNDED OR SILENT. The prompt is given only stored text: the paper's own abstract, and the
facets of the subject it matched (a project's synthesis and section summaries, built by
`discover/profiles.py`). A reason that does not so much as name the subject it was supposed to
connect is DROPPED rather than stored — the same rule `surface/critique` applies when it discards
a claim citing evidence it was not shown. Failure degrades to the deterministic `why`, which is
still true; it never blocks a proposal or a page.
"""

from __future__ import annotations

import logging
import sqlite3
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone

from locus.agent.claude import ClaudeError, run_text
from locus.config import load

log = logging.getLogger(__name__)

# Facets offered per subject. The synthesis says what the project IS; a few section summaries say
# what it actually contains. Beyond a handful the prompt becomes a wall of repo boilerplate and
# the reason gets vaguer, not sharper.
_MAX_FACETS = 6
_FACET_CHARS = 700

# The reason is read on a tablet, in a slot on a page that holds four of them, so brevity is a
# §9 guardrail rather than a preference. The cap sits ABOVE what the prompt asks for on purpose:
# when it sat at the target length every single reply hit it (6/6 at 415-422 chars, live) and was
# cut mid-sentence — losing the "what you could do with it" half, which is the valuable one.
# A ceiling should catch the outlier, not fire every time.
_MAX_REASON_CHARS = 520

DEFAULT_REWRITE_AFTER_DAYS = 7


# FREE TEXT, NOT A SCHEMA. The first cut wrapped the reason in a one-field pydantic model and
# went through `run_structured`; the model answered in excellent prose and every reply was
# discarded as "no JSON object in reply" (live, 6/6, 2026-08-02). A single string is not a
# structure, and demanding JSON around it only adds a way for a good answer to be thrown away.
# The validation that matters here is semantic (does it name his project?) and is applied below.


_PROMPT = """\
You are explaining to a quant-track student why one specific paper was put on his reading shelf.

HIS WORK — "{label}", one of his own projects:
{facets}

THE PAPER:
{title}
{abstract}

Write AT MOST 2 sentences, under 60 words total, saying:
  1. what specifically in "{label}" this paper bears on — NAME the project, then name the part, \
using his own words from the material above;
  2. what he could concretely DO with it — a method to try, a check to run, an assumption to test.

Rules:
  - Use ONLY the material above. If the connection is weak, say so plainly rather than inventing \
one; a candid "this only touches X loosely" is more useful to him than a confident overstatement.
  - Do not restate the paper's title or summarise its abstract back to him.
  - No preamble, no "this paper", no flattery. Write as if continuing his own notes.
"""


def _trim(reason: str, limit: int = _MAX_REASON_CHARS) -> str:
    """Cut at the last COMPLETE sentence that fits, never mid-clause.

    A dangling "Concretely: implement the..." is worse than one sentence fewer: the fragment reads
    as a system that ran out of room rather than as a finished thought, and the half being lost was
    always the actionable one.
    """
    if len(reason) <= limit:
        return reason
    window = reason[:limit]
    cut = max(window.rfind(". "), window.rfind("? "), window.rfind("! "))
    if cut > limit // 3:
        return window[: cut + 1]
    return window.rsplit(" ", 1)[0] + "..."


@dataclass
class Written:
    proposal_id: int
    ok: bool
    reason: str = ""
    detail: str = ""


def _subject_facets(conn: sqlite3.Connection, evidence_key: str) -> tuple[str, str]:
    """(label, rendered facet text) for a proposal's `evidence_key`, e.g. `project:Alpha Fund`.

    Returns ('', '') when the subject has no stored profile — in which case there is nothing
    honest to write and the caller keeps the deterministic `why`.
    """
    kind, _, label = (evidence_key or "").partition(":")
    if not label:
        return "", ""
    rows = conn.execute(
        "SELECT facet, text FROM discovery_profiles WHERE subject_kind=? AND label=? "
        # synthesis first: it says what the project IS, where a section summary says what one
        # file does. Ordering by id alone put a test module in front of the thesis.
        "ORDER BY (facet != 'synthesis'), id LIMIT ?",
        (kind, label, _MAX_FACETS),
    ).fetchall()
    if not rows:
        return "", ""
    body = "\n".join(f"- {' '.join((r['text'] or '').split())[:_FACET_CHARS]}" for r in rows)
    return label, body


def _grounded(reason: str, label: str, facets: str) -> bool:
    """Does the reason actually engage with HIS material, or is it a generic paper summary?

    REUSES `ingest.summarize.is_grounded`, the same distinctive-vocabulary predicate the section
    summaries are gated on — one definition of "grounded" across the codebase, and one already
    calibrated against 260 real summaries.

    The first cut demanded the project's LABEL appear verbatim, and it rejected 6 good reasons out
    of 6 on the first live run: the model had done exactly what it was asked and written "Your HMM
    regime training..." rather than repeating the string "regime-ml". Naming the label is now a
    sufficient condition, not a necessary one — sharing his project's vocabulary counts too.
    """
    from locus.ingest.summarize import is_grounded

    if not reason.strip():
        return False
    if label.casefold() in reason.casefold():
        return True
    return is_grounded(reason, facets)


def compose_reason(
    conn: sqlite3.Connection,
    *,
    title: str,
    source_text: str,
    evidence_key: str,
    ident: int = 0,
    runner=None,
    model: str | None = None,
) -> Written:
    """Write one reason from stored text. Shared by proposed papers and books he added himself.

    `source_text` is whatever describes the item: a paper's abstract, or — for a book that is not
    in the corpus — the passages he underlined, which say more about why it matters to him than
    an abstract would (`reading/relevance.evidence_text`).
    """
    label, facets = _subject_facets(conn, evidence_key)
    if not facets:
        return Written(ident, False, detail=f"no stored profile for {evidence_key!r}")

    prompt = _PROMPT.format(
        label=label,
        facets=facets,
        title=title,
        abstract=" ".join((source_text or "").split())[:2000] or "(no description available)",
    )
    try:
        reply = run_text(prompt, model=model or load().agent.model, runner=runner)
    except ClaudeError as exc:
        log.warning("why: generation failed for %s: %s", ident, exc)
        return Written(ident, False, detail=str(exc))

    # Markdown emphasis is noise on an e-ink page rendered through typst.
    reason = " ".join(reply.replace("**", "").replace("*", "").split())
    if not _grounded(reason, label, facets):
        # Silent rather than wrong: the deterministic `why` still renders, and it is true.
        log.info("why: dropped an ungrounded reason for %s", ident)
        return Written(ident, False, detail="reason never named the subject")
    return Written(ident, True, reason=_trim(reason))


def write_reason(
    conn: sqlite3.Connection, proposal_id: int, *, runner=None, model: str | None = None
) -> Written:
    """Compose and store one PROPOSAL's written reason. Degrades, never raises."""
    row = conn.execute(
        "SELECT id, title, abstract, evidence_key FROM reading_proposals WHERE id=?",
        (proposal_id,),
    ).fetchone()
    if row is None:
        return Written(proposal_id, False, detail="no such proposal")

    out = compose_reason(
        conn, title=row["title"], source_text=row["abstract"] or "",
        evidence_key=row["evidence_key"], ident=proposal_id, runner=runner, model=model,
    )
    if out.ok:
        with conn:
            conn.execute(
                "UPDATE reading_proposals SET why_long=?, why_written_at=? WHERE id=?",
                (out.reason, datetime.now(timezone.utc).isoformat(), proposal_id),
            )
    return out


def write_target_reason(
    conn: sqlite3.Connection, target_id: int, *, runner=None, model: str | None = None
) -> Written:
    """The same reason, for a book he chose himself (`reading_targets`).

    Its `evidence_key` is assembled from the link `reading/relevance` stored, so a book and a
    proposed paper resolve to a project through identical machinery.
    """
    from locus.reading.relevance import evidence_text, title_for

    row = conn.execute("SELECT * FROM reading_targets WHERE id=?", (target_id,)).fetchone()
    if row is None:
        return Written(target_id, False, detail="no such reading target")
    if not (row["subject_label"] or ""):
        return Written(target_id, False, detail="not linked to a project yet")

    text, _basis = evidence_text(conn, row)
    out = compose_reason(
        conn,
        title=row["title"] or title_for(row["device_path"], row["source_uri"]),
        source_text=text,
        evidence_key=f"{row['subject_kind']}:{row['subject_label']}",
        ident=target_id, runner=runner, model=model,
    )
    if out.ok:
        with conn:
            conn.execute(
                "UPDATE reading_targets SET why_long=?, why_written_at=? WHERE id=?",
                (out.reason, datetime.now(timezone.utc).isoformat(), target_id),
            )
    return out


def targets_needing_a_reason(
    conn: sqlite3.Connection,
    *,
    rewrite_after_days: int = DEFAULT_REWRITE_AFTER_DAYS,
    now: datetime | None = None,
) -> list[int]:
    """Linked reading targets whose reason is missing or stale."""
    now = now or datetime.now(timezone.utc)
    cutoff = (now - timedelta(days=rewrite_after_days)).isoformat()
    try:
        rows = conn.execute(
            "SELECT id FROM reading_targets WHERE COALESCE(subject_label,'') != '' "
            "AND (why_long IS NULL OR TRIM(why_long)='' OR COALESCE(why_written_at,'') < ?) "
            "ORDER BY COALESCE(last_swept, created_at) DESC",
            (cutoff,),
        ).fetchall()
    except sqlite3.OperationalError:
        return []
    return [r["id"] for r in rows]


def needs_a_reason(
    conn: sqlite3.Connection,
    *,
    rewrite_after_days: int = DEFAULT_REWRITE_AFTER_DAYS,
    now: datetime | None = None,
) -> list[int]:
    """Proposals on the shelf with no written reason, or one that has gone stale.

    Only `status='proposed'`: a paper he has already accepted needs no argument for reading it,
    and one he rejected needs none at all.
    """
    now = now or datetime.now(timezone.utc)
    cutoff = (now - timedelta(days=rewrite_after_days)).isoformat()
    rows = conn.execute(
        "SELECT id FROM reading_proposals WHERE status='proposed' "
        "AND (why_long IS NULL OR TRIM(why_long)='' OR COALESCE(why_written_at,'') < ?) "
        "ORDER BY COALESCE(score, 0) DESC, id",
        (cutoff,),
    ).fetchall()
    return [r["id"] for r in rows]


def write_missing(
    conn: sqlite3.Connection,
    *,
    limit: int = 10,
    rewrite_after_days: int = DEFAULT_REWRITE_AFTER_DAYS,
    runner=None,
    model: str | None = None,
    now: datetime | None = None,
) -> list[Written]:
    """Fill in every missing reason and refresh every stale one, newest-ranked first."""
    pending = needs_a_reason(conn, rewrite_after_days=rewrite_after_days, now=now)[:limit]
    return [write_reason(conn, pid, runner=runner, model=model) for pid in pending]
