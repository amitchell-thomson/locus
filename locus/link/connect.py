"""The written reason two documents are worth connecting — and what to DO about it.

THE PROBLEM, in the owner's words. The Think page used to print a connection as a pair:

    keep  for regime projects overarching philosophy is that markets are not... ->
    Identification Verification for Structural Vector Autoregressions with Sparse
    Heterogeneous Markov Switching Heteroskedasticity
    both develop regime — you wrote yours on 2026-08-02

He called it "obscure and hard to read and understand", and he is right: it is two long titles
glued together and a bare noun. It states that an overlap exists and asks nothing. What he asked
for instead is "this paper 'x' discusses regime detection this way, and your note talks about it
that way. Could you use the paper's methods to improve the project's regime detection
performance? ... something directly useful to me."

That is not derivable from a join. `related_documents` knows the two documents share a canonical
concept; it cannot know HOW each treats it. So this is a model pass over two stored texts —
exactly the trade `discover/why.py` already makes, and for the same reason: the RANKING stays
deterministic (a shared canonical is a checkable fact), and the model only says in English what
two texts it was shown have to do with each other.

WHEN IT RUNS. Overnight, never at page-composition time, so `compose_daily` stays aggregate-only
and the page renders whether or not this succeeded (§18). The prose is stored in
`connection_notes` and the page just prints it; with no note, the connection is simply not
offered rather than falling back to the phrasing he rejected.

GROUNDED OR SILENT. The prompt sees only stored text: each document's synthesis and the section
summaries that ANCHOR the shared concept (the entity join that made the pair exist — a summary
substring match found strictly less, measured 18/24 vs 23/24 sides). The model picks which shared
concept the connection is built on and must name its pick on a final `CONCEPT:` line; a pick that
is not in the offered list is DROPPED rather than stored — the same rule `surface/critique`
applies to a claim citing evidence it was not given. Failure degrades to no connection, never to
a bad one.

THE 2026-08-06 REDESIGN was A/B-measured on 12 live pairs x 3 variants
(`scripts/analysis/connect_exp1.py`; outputs judged by hand):
  - DEEP CONTEXT (entity-anchored sections, README + project-object body for repos, 2800 chars)
    produced prose naming the owner's actual functions and constants (`run_backtest_mvo`, the
    flat 0.02 edge threshold) where the 1400-char LIKE-matched context produced generalities.
  - MODEL CHOICE IS A SAFETY PROPERTY, not a fluency one: on a junk pair (a spurious shared
    `Markov model` between a quant repo and VLE thermodynamics) Haiku CONFIDENTLY BLUFFED twice
    ("inspired by Le Châtelier's Principle"); Sonnet replied NO_CONNECTION. Hence
    `[agent].connect_model` defaults to sonnet while `[agent].model` stays haiku.
  - Sonnet overruns a 420-char instruction ~40% of the time: hence the bounded shorter-retry
    below rather than the old silent `prose[:400]` clip, which had printed prose cut mid-word.
"""

from __future__ import annotations

import logging
import sqlite3
from dataclasses import dataclass
from datetime import datetime, timezone

log = logging.getLogger(__name__)

# Chars of each side's stored text handed to the model. 1400 truncated 15 of 24 sides of the
# notes written before the redesign; 2800 covers synthesis + ~6 section summaries.
_MAX_SIDE_CHARS = 2800

# The instruction asks for under 420 chars (fits the page line budget with room); anything over
# _MAX_PROSE_CHARS after one shorter-retry is dropped. Never clip stored prose mid-sentence.
_TARGET_PROSE_CHARS = 420
_MAX_PROSE_CHARS = 600

# One retry with a terser instruction when the only defect is length/format (measured: a re-ask
# usually fixes a one-off malformed reply; unbounded loops are how cost caps die).
_FORMAT_RETRIES = 1

_CONCEPT_TAG = "CONCEPT:"

# The model may honestly find nothing (the pair is deterministically real; its usefulness is
# not). NO_CONNECTION is stored as an EMPTY note so the nightly writer does not re-pay for the
# same verdict; the page treats empty prose as nothing to show.
_NO_CONNECTION = "NO_CONNECTION"

_FOOTER = f"""Reply with the prompt only — no preamble, no "PROMPT:" prefix, and never refer to "Side A" or
"Side B" (say "your notes", "the paper", "your project" as fits). If the material offers nothing
genuinely useful, reply with exactly {_NO_CONNECTION}. End with a separate final line:
{_CONCEPT_TAG} <the one concept from the list above the connection is built on>"""

_SYSTEM = (
    "You connect a person's own work to something they have read. You are given stored text from "
    "two documents and the concepts they both develop. You never invent facts about either."
)

_TEMPLATE = """His own material:
{his_title}
{his_text}

What he read:
{other_title}
{other_text}

Concepts both develop: {concept_list}

Write ONE short prompt (2-3 sentences, under {target} characters) that tells him how the READ
material treats one of those concepts differently from his own, and asks a concrete question
about whether he could use it in his work. Be specific about the method or idea, not about the
fact that both mention it. Do not open with "This paper" boilerplate or restate the titles.
{footer}"""

# THE BRIDGE FRAMING. A coursework connection is not "something he read" — it is something he was
# TAUGHT, and the useful question runs the other way: not "should you adopt this method" but "the
# maths you already have notes on is the maths this work rests on; can you apply it?". §16 keeps
# 144 coursework documents on precisely this promise (`eigenvalue problem`, `Frobenius norm`,
# `central limit theorem` all bridge his lecture notes into his quant papers), and until
# 2026-08-03 it was never once delivered. Reusing the "what he read" template here would produce
# the wrong question — asking whether to adopt a second-year linear algebra lecture.
#
# The owner's ask (2026-08-06) sharpened it: what he wants from this framing is the moment of
# IDENTIFICATION — "the quant concept I am reading about IS the engineering concept I already
# studied" (his examples: eigendecomposition ~ PCA, control theory ~ Kalman filtering). So the
# prompt asks for the identification made precise on both sides, not just a reminder.
_BRIDGE_SYSTEM = (
    "You show a person that material they are working with and material they were taught earlier "
    "develop the same underlying idea in different vocabulary. You are given stored text from two "
    "documents and the concepts they both develop. You never invent facts about either."
)

_BRIDGE_TEMPLATE = """What he is working with now:
{his_title}
{his_text}

What he has already studied:
{other_title}
{other_text}

Concepts both develop: {concept_list}

Write ONE short prompt (2-3 sentences, under {target} characters) that makes the identification
precise: name the specific technique or result on EACH side and state how they are the same idea
in different vocabulary, then ask one concrete question that tests whether he can carry a result
from his study material over to the work in front of him. Assume he once knew this and may have
forgotten the connection — do not explain the concept from scratch, and do not suggest he read
the lecture notes.
{footer}"""

# THE PROJECT FRAMING (2026-08-06). The owner's stated top want: ideas from papers, his notes and
# his coursework FOR HIS CODE REPOS. The near side is a repo he wrote; the useful question names
# the technique in the material AND the part of the project it would change. The A/B run showed
# this framing plus repo context (README narrative + project-object body) is what makes the model
# name his actual modules instead of "your portfolio strategy".
_PROJECT_SYSTEM = (
    "You find concrete, actionable ideas for a person's own software and quant projects from "
    "material in their personal knowledge corpus. You are given stored text from both sides. "
    "You never invent facts about either side."
)

_PROJECT_TEMPLATE = """HIS PROJECT (code he wrote and maintains):
{his_title}
{his_text}

MATERIAL ({material_kind}):
{other_title}
{other_text}

Concepts both sides develop: {concept_list}

Write ONE prompt (2-4 sentences, under {target} characters) proposing a specific idea from the
material for the project. Name the technique precisely, say which part of the project it would
change, and end with a concrete question he could act on. Do not restate titles and do not
flatter.
{footer}"""


# Phrases that mark prose ABOUT THE TASK rather than about the two documents. Deliberately
# narrow: each names the prompt-writing job or addresses the person who set it, which a genuine
# connection never does — it is written to HIM, about texts he owns.
_REFUSAL_MARKERS = (
    "i don't see", "i do not see", "i cannot", "i can't", "could you clarify",
    "you've provided", "you have provided", "i want to write", "the material provided",
)

# Template-vocabulary leakage: the A/B run leaked one "PROMPT:" prefix and several "Side A/Side
# B" references. The prefix is stripped; a side-reference is a format failure (retried once).
_SIDE_MARKERS = ("side a", "side b")


@dataclass
class ConnectionNote:
    src_uri: str
    other_uri: str
    shared: str
    prose: str


def _doc_text(conn: sqlite3.Connection, uri: str, concepts: list[str]) -> tuple[str, str]:
    """(title, stored text) for one side — synthesis plus the summaries carrying the concepts.

    Sections are found by the ENTITY ANCHOR (the join that created the candidate), not by
    summary-substring: measured on the pre-redesign notes, LIKE found a section on 18 of 24
    sides, the anchor on 23 of 24. LIKE is kept only as a fallback for DBs with no alias
    substrate (seeded tests).

    For a code repo, the narrative is deliberately front-loaded: the project object's body (his
    blessed profile — approach, open threads, learnings) first, then README/markdown section
    summaries. CLAUDE.md §6: for a project repo the READMEs are often the most informative
    content.
    """
    row = conn.execute(
        "SELECT id, title, source_type, thesis, method, result FROM documents "
        "WHERE source_uri=?",
        (uri,),
    ).fetchone()
    if row is None:
        return "", ""
    parts: list[str] = []
    if row["source_type"] == "code":
        try:
            obj = conn.execute(
                "SELECT o.body FROM objects o JOIN object_links ol ON ol.object_id=o.id "
                "WHERE o.type='project' AND ol.target_kind='doc' AND ol.relation='implements' "
                "AND ol.target_key=? AND COALESCE(o.body,'')!='' LIMIT 1",
                (uri,),
            ).fetchone()
            if obj:
                parts.append(obj["body"])
        except sqlite3.OperationalError:            # agent tables absent (bare test DB)
            pass
    parts += [p for p in (row["thesis"], row["method"], row["result"]) if p]

    seen_secs: set[int] = set()
    if concepts:
        marks = ",".join("?" * len(concepts))
        try:
            for sec in conn.execute(
                f"SELECT DISTINCT s.id, s.summary, s.file_path FROM sections s "
                f"JOIN entities e ON e.section_id=s.id "
                f"JOIN entity_aliases a ON a.variant_name=e.name AND a.variant_type=e.type "
                f"WHERE s.doc_id=? AND a.canonical_name IN ({marks}) "
                f"AND COALESCE(s.summary,'')!='' "
                f"ORDER BY (s.file_path IS NULL OR lower(s.file_path) LIKE '%.md') DESC "
                f"LIMIT 6",
                (row["id"], *concepts),
            ):
                seen_secs.add(sec["id"])
                parts.append(sec["summary"])
        except sqlite3.OperationalError:
            pass
    if not seen_secs and concepts:
        like = f"%{concepts[0]}%"
        for sec in conn.execute(
            "SELECT id, summary FROM sections WHERE doc_id=? AND summary LIKE ? LIMIT 3",
            (row["id"], like),
        ):
            if sec["summary"]:
                seen_secs.add(sec["id"])
                parts.append(sec["summary"])
    if row["source_type"] == "code":
        for sec in conn.execute(
            "SELECT id, summary FROM sections WHERE doc_id=? "
            "AND lower(COALESCE(file_path,'')) LIKE '%.md' AND COALESCE(summary,'')!='' "
            "ORDER BY position LIMIT 3",
            (row["id"],),
        ):
            if sec["id"] not in seen_secs:
                parts.append(sec["summary"])
    return (row["title"] or uri), " ".join(parts)[:_MAX_SIDE_CHARS]


def _material_kind(conn: sqlite3.Connection, uri: str) -> str:
    from locus.agent import state

    clause, params = state.owner_authored_sql("d")
    row = conn.execute(
        f"SELECT category, ({clause}) AS own FROM documents d WHERE d.source_uri=?",
        (*params, uri),
    ).fetchone()
    if row is None:
        return "material from his corpus"
    if row["own"]:
        return "his own notes"
    if row["category"] == "coursework":
        return "his university coursework"
    return "a paper he read"


def _parse_reply(text: str, names: list[str]) -> tuple[str, object]:
    """(verdict, payload). Verdicts: OK (payload=(concept, body)) | no_connection | a reason."""
    t = " ".join((text or "").split())
    if not t:
        return "empty", None
    if t.strip().upper().startswith(_NO_CONNECTION):
        return "no_connection", None
    low = t.lower()
    if any(m in low for m in _REFUSAL_MARKERS):
        return "refusal", None
    if any(m in low for m in _SIDE_MARKERS):
        return "side_reference", None
    tag = low.rfind(_CONCEPT_TAG.lower())
    if tag == -1:
        return "no_concept_line", None
    picked = t[tag + len(_CONCEPT_TAG):].strip().strip(".").strip()
    match = next((n for n in names if n.lower() == picked.lower()), None)
    if match is None:
        # GROUNDED OR SILENT: the concept the model claims to have built on is not one it was
        # given. Checking membership catches invented picks the way citation-existence checks
        # catch invented keys.
        return "concept_not_offered", picked
    body = t[:tag].strip()
    if body.lower().startswith("prompt:"):
        body = body[len("prompt:"):].strip()
    if not body:
        return "empty_body", None
    if len(body) > _MAX_PROSE_CHARS:
        return "too_long", len(body)
    return "OK", (match, body)


def write_note(
    conn: sqlite3.Connection,
    *,
    src_uri: str,
    other_uri: str,
    shared: str,
    shared_all: tuple[str, ...] | list[str] = (),
    kind: str | None = None,
    runner=None,
    model: str | None = None,
    bridge: bool = False,
) -> ConnectionNote | None:
    """Compose and store one connection's prose. Returns None when it cannot be grounded.

    `kind` selects the framing: 'project' (idea for his repo), 'bridge' (same idea as what he
    studied), 'capture' (what he read vs his own material). `bridge=True` is the pre-redesign
    spelling of kind='bridge', kept so existing callers read unchanged.

    A NO_CONNECTION verdict stores an EMPTY note: the pair was paid for and answered, and the
    nightly writer must not re-ask (`pair_attempted`). The page shows nothing for empty prose.
    """
    from locus.agent.claude import ClaudeError, run_text
    from locus.config import load
    from locus.observe import gates

    kind = kind or ("bridge" if bridge else "capture")
    names = [n for n in (shared_all or ()) if n] or [shared]

    his_title, his_text = _doc_text(conn, src_uri, names)
    other_title, other_text = _doc_text(conn, other_uri, names)
    if not (his_text and other_text):
        return None

    concept_list = "; ".join(names)
    common = dict(
        his_title=his_title, his_text=his_text, other_title=other_title,
        other_text=other_text, concept_list=concept_list, target=_TARGET_PROSE_CHARS,
        footer=_FOOTER,
    )
    if kind == "project":
        system = _PROJECT_SYSTEM
        prompt = _PROJECT_TEMPLATE.format(
            material_kind=_material_kind(conn, other_uri), **common
        )
    elif kind == "bridge":
        system = _BRIDGE_SYSTEM
        prompt = _BRIDGE_TEMPLATE.format(**common)
    else:
        system = _SYSTEM
        prompt = _TEMPLATE.format(**common)

    cfg = load()
    model = model or getattr(cfg.agent, "connect_model", None) or cfg.agent.model

    verdict, payload = "empty", None
    ask = f"{system}\n\n{prompt}"
    for _attempt in range(_FORMAT_RETRIES + 1):
        try:
            text = run_text(ask, model=model, runner=runner)
        except ClaudeError as exc:                   # degrade, never block (ingest §7)
            log.warning("connect: model call failed for %s <-> %s: %s", src_uri, other_uri, exc)
            return None
        verdict, payload = _parse_reply(text, names)
        if verdict in ("OK", "no_connection", "refusal", "empty"):
            break
        # Format defects (overrun, side-references, missing/unknown concept line) usually fix on
        # one terser re-ask; bounded so a stubborn overrun costs one extra call, not a loop.
        ask = (
            f"{system}\n\n{prompt}\n\nYour previous reply was rejected ({verdict}). Reply again: "
            f"under {_TARGET_PROSE_CHARS} characters, then the final {_CONCEPT_TAG} line naming "
            f"one concept from the list verbatim."
        )
    gates.record(
        conn, "connect.write_verified", rejected=verdict != "OK",
        value=None if verdict == "OK" else f"{verdict}: {concept_list[:60]}",
    )

    if verdict == "no_connection":
        # An honest "nothing here" is a result worth keeping: without it the writer re-pays for
        # the same verdict every night (the pre-redesign skip check keyed on non-empty prose).
        _store(conn, src_uri, other_uri, names[0], "")
        log.info("connect: NO_CONNECTION for %s <-> %s", src_uri, other_uri)
        return None
    if verdict != "OK":
        log.info("connect: dropped (%s) for %r", verdict, concept_list)
        return None

    picked, body = payload
    _store(conn, src_uri, other_uri, picked, body)
    return ConnectionNote(src_uri, other_uri, picked, body)


def _store(conn: sqlite3.Connection, src: str, other: str, shared: str, prose: str) -> None:
    with conn:
        conn.execute(
            "INSERT INTO connection_notes (src_uri, other_uri, shared, prose, written_at) "
            "VALUES (?,?,?,?,?) ON CONFLICT(src_uri, other_uri, shared) DO UPDATE SET "
            "prose=excluded.prose, written_at=excluded.written_at",
            (src, other, shared, prose, datetime.now(timezone.utc).isoformat()),
        )


def stored_note(
    conn: sqlite3.Connection, *, src_uri: str, other_uri: str, shared: str
) -> str:
    """The stored prose for one exact (pair, concept), or ''."""
    try:
        row = conn.execute(
            "SELECT prose FROM connection_notes WHERE src_uri=? AND other_uri=? AND shared=?",
            (src_uri, other_uri, shared),
        ).fetchone()
    except sqlite3.OperationalError:
        return ""
    return (row["prose"] if row else "") or ""


def stored_pair_note(conn: sqlite3.Connection, *, src_uri: str, other_uri: str) -> str:
    """The newest non-empty prose for the PAIR, whatever concept it was stored under.

    The model picks the concept the prose is built on, so the stored `shared` need not equal the
    candidate's first qualifying concept — this lookup is what `compose_daily` reads.
    """
    try:
        row = conn.execute(
            "SELECT prose FROM connection_notes WHERE src_uri=? AND other_uri=? "
            "AND COALESCE(prose,'')!='' ORDER BY written_at DESC LIMIT 1",
            (src_uri, other_uri),
        ).fetchone()
    except sqlite3.OperationalError:
        return ""
    return (row["prose"] if row else "") or ""


def pair_attempted(conn: sqlite3.Connection, *, src_uri: str, other_uri: str) -> bool:
    """Has the overnight writer already answered this pair (including with NO_CONNECTION)?"""
    try:
        return (
            conn.execute(
                "SELECT 1 FROM connection_notes WHERE src_uri=? AND other_uri=? LIMIT 1",
                (src_uri, other_uri),
            ).fetchone()
            is not None
        )
    except sqlite3.OperationalError:
        return False
