"""Propose structured objects + belief positions from an ingested document (plan §6.2-6.3, §8.4).

After a note / conversation / repo lands in the corpus, this pass reads it back through the LOCAL
retrieval engine, asks `claude -p` what structure it implies, and records the survivors as
`status='proposed'`. The owner blesses (`active`) or ignores. Nothing here mutates the spine.

THE PRECISION BAR (the thing that decides whether this layer is useful or noise)
-------------------------------------------------------------------------------
Suggestion fatigue is failure mode #7 — a proposer that emits six plausible objects per note gets
switched off inside a week. So the model's output is treated as a CANDIDATE LIST, never as truth,
and every candidate must clear a gate that Python — not the model — enforces:

  1. **Concepts must be real canonical entities.** A proposed concept has to resolve, through
     `entity_aliases`, to a canonical entity that already spans >= `min_concept_docs` documents.
     The model cannot invent a concept the corpus does not already carry across sources; it can
     only tell us which of the owner's *existing* cross-document concepts this note engages.
  2. **Projects and readings must anchor to a real document.** The proposed anchor is matched
     against actual `source_uri`s; an unmatched anchor is dropped, not fuzzed into place.
  3. **Every object carries >= 1 grounding link** (invariant 3, grounded-or-silent). The source
     document is always one; retrieval adds cross-document support only for citations clearing
     the configured rerank floor.
  4. **Hard cap per document** (`max_objects_per_doc`, default 3). Ranked in the model's own
     order — it is asked to put the most consequential first.
  5. **Belief positions come only from owner-authored documents** (`belief_source_categories`,
     default `note` — handwriting and captured conversations) AND the stance must share heavy
     distinctive vocabulary with the source text. A paper's claim is the PAPER's position, not
     the owner's, and a paraphrase that drifts into the model's voice is failure mode #1: the
     trajectory is only worth having if every stance on it is really his.

STRUCTURE: `plan_for_document` decides (gates run, nothing is written); `apply_plan` writes. The
split is what makes `--dry-run` trustworthy — a dry run is the plan phase alone, so it cannot
write even by accident, and it is how the precision bar gets measured against the real corpus
before this is turned loose on it (plan §15's object-proposal precision check).

Degradation: a `claude -p` failure or an unparseable reply proposes nothing and marks the run
`degraded` — never blocks, never aborts a batch (ingest §7).
"""

from __future__ import annotations

import logging
from dataclasses import dataclass, field
from typing import Literal

from pydantic import AliasChoices, BaseModel, Field, field_validator, model_validator

from locus.agent import journal, state
from locus.agent.budget import BudgetLedger
from locus.agent.claude import ClaudeError, run_structured
from locus.agent.state import ObjectLink, entity_key
from locus.config import load
from locus.ingest.summarize import _GENERIC, _word_tokens

log = logging.getLogger(__name__)

# How much of a stance's distinctive vocabulary must appear in the source text for it to count as
# the owner's words rather than the model's. Deliberately far above the ingest summary guard's
# quarter-coverage bar (summarize.is_grounded): a summary may legitimately abstract, a QUOTED
# STANCE may not. At 0.6 a lightly-trimmed quote passes and a re-worded "improvement" fails.
_STANCE_MIN_COVERAGE = 0.6
# Chars of document text handed to the model as grounding. Bounds prompt cost on a long repo.
_MAX_DOC_CHARS = 6000


# --- what the model is asked for -------------------------------------------------------------


def _as_list(v):
    """Coerce a scalar into a one-element list.

    A live Haiku run (2026-07-28) returned `open_threads` as a prose string rather than a list on
    the majority of documents. Rejecting the whole proposal over container shape discards good
    candidates for a formatting slip; the PRECISION gates below are what protect quality, and
    they are unaffected by this. Tolerant parse, unchanged bar — the same trade `_extract_json`
    already makes by accepting prose around the JSON."""
    if v is None:
        return []
    if isinstance(v, str):
        return [v] if v.strip() else []
    return v


class ProposedObject(BaseModel):
    """One candidate object. `anchor` is what gates 1/2 check: a canonical entity name for a
    concept, a document source_uri (or a distinctive fragment of one) for a project/reading.

    Field aliases and coercions absorb the shapes a real Haiku run produced ('kind' for 'type',
    a missing 'title', a scalar where a list belongs) — see `_as_list`."""

    # `kind` observed live as an alias for `type`; accepted by population alias rather than
    # spending a retry on it.
    type: Literal["project", "concept", "question", "reading"] = Field(
        validation_alias=AliasChoices("type", "kind")
    )
    title: str = ""
    anchor: str = ""
    why: str = ""  # one line, grounded — shown to the owner when blessing
    approach: str = ""
    open_threads: list[str] = Field(default_factory=list)
    learnings: list[str] = Field(default_factory=list)
    mastery: str = ""  # thin | working | solid
    state: str = ""  # reading: queued | reading | done

    _coerce_lists = field_validator("open_threads", "learnings", mode="before")(_as_list)

    @model_validator(mode="after")
    def _title_falls_back_to_anchor(self):
        """A titleless candidate is named by its anchor rather than dropped.

        Safe because the anchor is precisely what gates 1-2 then verify: a concept's anchor must
        resolve to a canonical entity (and `_canonical_title` renames it to the canonical), and a
        project's must resolve to a real document. An anchor that resolves to nothing is rejected
        either way, so this cannot let an ungrounded object through."""
        if not self.title.strip() and self.anchor.strip():
            self.title = self.anchor.strip()
        return self


class ProposedPosition(BaseModel):
    """One dated stance. `stance` must be the owner's words, quoted from the source."""

    # 'position'/'claim' observed live as aliases for 'stance'; subject_kind defaults to concept
    # (the common case) rather than failing the whole reply when the model omits it.
    subject_kind: Literal["concept", "project"] = "concept"
    subject: str = ""
    stance: str = Field(default="", validation_alias=AliasChoices("stance", "position", "claim"))


class Proposals(BaseModel):
    objects: list[ProposedObject] = Field(default_factory=list)
    positions: list[ProposedPosition] = Field(default_factory=list)


# --- plan / result types -----------------------------------------------------------------------


@dataclass
class Rejection:
    kind: str  # 'object' | 'position'
    subject: str
    reason: str


@dataclass
class PlannedObject:
    type: str
    title: str
    body: dict
    links: list[ObjectLink]
    why: str = ""


@dataclass
class PlannedPosition:
    subject_kind: str
    # For a concept this is the resolved canonical key; for a project it is the project TITLE,
    # resolved to an object id only at apply time (the object may not exist until then).
    subject_ref: str
    stance: str
    dated_at: str


@dataclass
class Plan:
    doc_id: int
    doc_title: str = ""
    # The source note's path — stable across re-ingest, unlike doc_id (migration 0012).
    doc_uri: str | None = None
    objects: list[PlannedObject] = field(default_factory=list)
    positions: list[PlannedPosition] = field(default_factory=list)
    rejected: list[Rejection] = field(default_factory=list)
    degraded: bool = False


@dataclass
class ProposeResult:
    doc_id: int
    created: list[int] = field(default_factory=list)
    updated: list[int] = field(default_factory=list)
    positions: int = 0
    rejected: list[Rejection] = field(default_factory=list)
    degraded: bool = False
    plan: Plan | None = None

    @property
    def proposed(self) -> int:
        return len(self.created) + len(self.updated)


# --- grounding (deterministic, free) ----------------------------------------------------------


def _doc_row(conn, doc_id: int):
    return conn.execute("SELECT * FROM documents WHERE id=?", (doc_id,)).fetchone()


def _doc_text(conn, doc_id: int, *, limit: int = _MAX_DOC_CHARS) -> str:
    rows = conn.execute(
        "SELECT raw_text FROM chunks WHERE doc_id=? ORDER BY position", (doc_id,)
    ).fetchall()
    return "\n".join(r["raw_text"] for r in rows)[:limit]


def canonical_concepts(
    conn, doc_id: int, *, min_docs: int, exclude_types: list[str] | None = None,
    require_categories: list[str] | None = None,
) -> dict[str, tuple[str, str]]:
    """The doc's canonical entities that span >= `min_docs` documents — gate 1's whitelist.

    Joins the doc's raw entity surfaces through `entity_aliases` to their canonicals, then counts
    how many DISTINCT documents each canonical spans corpus-wide. Returns
    `casefolded canonical name -> (canonical_name, canonical_type)`.

    Degrades to raw entity names when the alias substrate has not been built (`locus link` not yet
    run): the cross-document count stays honest, it just cannot see surface variants as one
    concept."""
    rows = conn.execute(
        """
        WITH doc_canon AS (
            SELECT DISTINCT
                   COALESCE(a.canonical_name, e.name) AS cname,
                   COALESCE(a.canonical_type, e.type) AS ctype
            FROM entities e
            LEFT JOIN entity_aliases a
                   ON a.variant_name = e.name AND a.variant_type = e.type
            WHERE e.doc_id = ?
        )
        SELECT dc.cname AS cname, dc.ctype AS ctype, COUNT(DISTINCT e2.doc_id) AS docs
        FROM doc_canon dc
        JOIN entities e2
          ON COALESCE(
               (SELECT a2.canonical_name FROM entity_aliases a2
                 WHERE a2.variant_name = e2.name AND a2.variant_type = e2.type), e2.name
             ) = dc.cname
        GROUP BY dc.cname, dc.ctype
        HAVING docs >= ?
        """,
        (doc_id, min_docs),
    ).fetchall()
    # Gate 1c: a Concept object is an IDEA. A language, a platform, a person, a company and a
    # ticker are not, however central they are to the work — the first live run proposed `Python`
    # and `Optibook` (both entity type `tool`) as concepts to track. Filtering by KIND rather than
    # by frequency is the right instrument here: these are not rare, they are the wrong sort of
    # thing.
    excluded = {t.casefold() for t in (exclude_types or [])}
    out = {
        r["cname"].casefold(): (r["cname"], r["ctype"])
        for r in rows
        if (r["ctype"] or "").casefold() not in excluded
    }

    # Gate 1d: a concept must touch the owner's actual work, not only his coursework. Without
    # this the concept space is dominated by the 87% of cross-document canonicals reachable only
    # through 246 engineering coursework documents. The concepts that BRIDGE coursework into
    # papers/projects/career/notes survive by construction — they touch a required category.
    if require_categories:
        placeholders = ",".join("?" * len(require_categories))
        touching = {
            r["cname"].casefold()
            for r in conn.execute(
                f"""
                SELECT DISTINCT COALESCE(
                         (SELECT a.canonical_name FROM entity_aliases a
                           WHERE a.variant_name = e.name AND a.variant_type = e.type), e.name
                       ) AS cname
                FROM entities e JOIN documents d ON d.id = e.doc_id
                WHERE d.category IN ({placeholders})
                """,
                sorted(require_categories),
            )
        }
        out = {k: v for k, v in out.items() if k in touching}

    # Gate 1b: drop names too generic to BE a concept. A live dry-run (2026-07-28) offered
    # `state` and `ingest` as domain concepts for the tanker-flow repo — both clear the
    # cross-document bar trivially, because every code repo has them. The link layer already
    # tuned this exact judgement in rounds 6/7; reusing its definition rather than writing a
    # second one keeps the two surfaces from disagreeing about what a concept is.
    try:
        from locus.link.related import non_topical_names

        for name in non_topical_names(conn):
            out.pop(name, None)
    except Exception as exc:  # a missing alias substrate must not block proposing
        log.debug("propose: generic-name filter unavailable: %s", exc)
    return out


def _source_uris(conn) -> dict[str, str]:
    """casefolded source_uri -> source_uri, for gate 2's anchor match."""
    return {
        r["source_uri"].casefold(): r["source_uri"]
        for r in conn.execute("SELECT source_uri FROM documents WHERE source_uri IS NOT NULL")
    }


def _match_uri(anchor: str, uris: dict[str, str]) -> str | None:
    """Resolve an anchor to a real source_uri: exact, then unique substring. A fragment matching
    two documents is ambiguous and resolves to nothing — an anchor we cannot pin is not grounding."""
    key = anchor.strip().casefold()
    if not key:
        return None
    if key in uris:
        return uris[key]
    hits = [v for k, v in uris.items() if key in k or k in key]
    return hits[0] if len(hits) == 1 else None


def stance_is_grounded(stance: str, source: str) -> bool:
    """Is this stance the owner's words, or the model's? (gate 5, failure mode #1)

    Requires >= `_STANCE_MIN_COVERAGE` of the stance's distinctive tokens (generic vocabulary
    removed, compared by 6-char stem so morphology counts) to appear in the source text. A stance
    with almost no distinctive vocabulary says nothing attributable and fails."""
    distinctive = _word_tokens(stance) - _GENERIC
    if len(distinctive) < 2:
        return False
    stems = {t[:6] for t in _word_tokens(source)}
    hits = sum(1 for t in distinctive if t[:6] in stems)
    return hits >= _STANCE_MIN_COVERAGE * len(distinctive)


def _supporting_docs(conn, query: str, *, floor: float | None, retrieve_fn=None) -> list[int]:
    """Doc ids retrieval surfaces for this material ABOVE the rerank floor (gate 3's cross-doc
    support). Grounded-or-silent: a low-confidence or empty retrieval contributes nothing."""
    if retrieve_fn is None:
        from locus.retrieve import retrieve as _retrieve

        retrieve_fn = lambda q: _retrieve(q, conn=conn)  # noqa: E731
    try:
        result = retrieve_fn(query)
    except Exception as exc:  # retrieval is free but not infallible; never block proposing
        log.warning("propose: retrieval grounding failed: %s", exc)
        return []
    if getattr(result, "low_confidence", False):
        return []
    out: list[int] = []
    for c in getattr(result, "citation_details", []) or []:
        if floor is not None and (c.rerank_score is None or c.rerank_score < floor):
            continue
        if c.doc_id not in out:
            out.append(c.doc_id)
    return out


# --- the prompt ------------------------------------------------------------------------------


_PROMPT = """\
You are helping structure a personal knowledge vault. Below is ONE document the owner has just \
added, plus context from the rest of their corpus. Propose the structured objects it implies and \
any position the OWNER takes in it.

Return ONLY a JSON object in EXACTLY this shape. Use these key names verbatim. Every list field \
must be a JSON array of strings, even when it has one element:

{{
  "objects": [
    {{"type": "project", "title": "regime-ml", "anchor": "repos/regime-ml",
      "why": "one line: what in THIS document justifies the object",
      "approach": "how he is going about it",
      "open_threads": ["what is still unresolved"],
      "learnings": ["what he has concluded so far"]}},
    {{"type": "concept", "title": "regime detection", "anchor": "regime detection",
      "why": "...", "mastery": "thin"}},
    {{"type": "question", "title": "Do regimes persist out of sample?", "why": "..."}},
    {{"type": "reading", "title": "Ang - Asset Management", "anchor": "papers/ang.pdf",
      "why": "...", "state": "queued"}}
  ],
  "positions": [
    {{"subject_kind": "concept", "subject": "regime detection",
      "stance": "his own words, quoted from the document text"}}
  ]
}}

Object types:
- "project"  — work the owner is building. anchor = the repo/document path it lives in.
- "concept"  — an idea he is engaging with. anchor = the concept name, and it MUST be one of the \
CORPUS CONCEPTS listed below, copied EXACTLY. mastery is thin|working|solid.
- "question" — an open question this document raises. anchor may be omitted.
- "reading"  — something to read. anchor = its document path if already in the corpus. \
state is queued|reading|done.

Rules, strictly:
- Propose AT MOST {max_objects}, most consequential first. Fewer is better than padded.
- Never invent a concept that is not in the CORPUS CONCEPTS list.
- Omit a field rather than inventing a value for it. Use [] for an empty list, never a sentence.

A position is a view the OWNER HIMSELF expresses in this document — a stance, a decision, a \
conclusion he reached. Quote it in HIS OWN WORDS (verbatim or lightly trimmed); do not \
paraphrase, do not improve it. subject must be a CORPUS CONCEPT (subject_kind "concept") or a \
project title you proposed (subject_kind "project"). If the document states no view of his own — \
if it only reports what a source says — return "positions": []. That is the normal case for \
papers and lecture notes.

DOCUMENT
title: {title}
category: {category}
date: {date}
synthesis: {synthesis}

CORPUS CONCEPTS (the only permitted concept anchors):
{concepts}

RELATED DOCUMENTS IN THE CORPUS:
{related}

DOCUMENT TEXT
{text}
"""


def build_prompt(*, doc, concepts: list[str], related: list[str], text: str, max_objects: int) -> str:
    synthesis = " / ".join(str(doc[f]) for f in ("thesis", "method", "result") if doc[f]) or "(none)"
    return _PROMPT.format(
        max_objects=max_objects,
        title=doc["title"],
        category=doc["category"] or "—",
        date=doc["source_date"] or "—",
        synthesis=synthesis[:800],
        concepts="\n".join(f"- {c}" for c in concepts) or "- (none — propose no concepts)",
        related="\n".join(f"- {r}" for r in related) or "- (none)",
        text=text or "(no text)",
    )


# --- plan phase (gates run; NOTHING is written) -------------------------------------------------


def plan_for_document(
    conn,
    doc_id: int,
    *,
    ledger: BudgetLedger | None = None,
    runner=None,
    retrieve_fn=None,
    model: str | None = None,
) -> Plan:
    """Decide what to propose for one document. Pure with respect to the DB — reads only."""
    cfg = load()
    scfg = cfg.structure
    doc = _doc_row(conn, doc_id)
    if doc is None:
        raise ValueError(f"no document with id {doc_id}")
    plan = Plan(doc_id=doc_id, doc_title=doc["title"] or "", doc_uri=doc["source_uri"])

    # Invariant 5: agent-generated notes are corpus-excluded and must never feed the structurer —
    # that is exactly the feedback loop where the system starts learning from itself.
    if _is_generated(doc):
        plan.rejected.append(Rejection("object", plan.doc_title, "agent-generated source"))
        return plan

    concepts = canonical_concepts(
        conn, doc_id, min_docs=scfg.min_concept_docs,
        exclude_types=scfg.concept_exclude_entity_types,
        require_categories=scfg.concept_require_categories,
    )
    text = _doc_text(conn, doc_id)
    support = [
        d for d in _supporting_docs(
            conn, f"{doc['title']} {doc['thesis'] or ''}"[:400],
            floor=cfg.retrieve.min_rerank_score, retrieve_fn=retrieve_fn,
        )
        if d != doc_id
    ]
    related_titles = _titles(conn, support)

    prompt = build_prompt(
        doc=doc, concepts=sorted(name for name, _ in concepts.values()),
        related=list(related_titles.values()), text=text,
        max_objects=scfg.max_objects_per_doc,
    )
    try:
        proposals = run_structured(
            prompt, schema=Proposals, model=model or scfg.model or cfg.agent.model,
            runner=runner, on_result=(ledger.record if ledger else None),
        )
    except ClaudeError as exc:  # degrade, never block (ingest §7)
        log.warning("propose: model call failed for doc %s: %s", doc_id, exc)
        plan.degraded = True
        return plan

    uris = _source_uris(conn)
    support_uris = [u for u in (_uri_of(conn, d) for d in support) if u]

    for cand in proposals.objects:
        if len(plan.objects) >= scfg.max_objects_per_doc:
            plan.rejected.append(Rejection("object", cand.title, "over per-document cap"))
            continue
        links, reason = _gate_object(cand, doc=doc, concepts=concepts, uris=uris)
        if reason:
            plan.rejected.append(Rejection("object", cand.title or cand.anchor, reason))
            continue
        links += [ObjectLink("doc", u, "relates") for u in support_uris[: scfg.max_support_links]]
        plan.objects.append(
            PlannedObject(
                type=cand.type, title=_canonical_title(cand, concepts),
                body=_body_for(cand), links=links, why=cand.why,
            )
        )

    _plan_positions(
        plan, proposals.positions, doc=doc, concepts=concepts, text=text,
        allowed=scfg.belief_source_categories,
    )
    return plan


def _is_generated(doc) -> bool:
    uri = (doc["source_uri"] or "").replace("\\", "/")
    return "_generated/" in uri or uri.startswith("_generated")


def _uri_of(conn, doc_id: int) -> str | None:
    row = conn.execute("SELECT source_uri FROM documents WHERE id=?", (doc_id,)).fetchone()
    return row["source_uri"] if row else None


def _titles(conn, doc_ids: list[int]) -> dict[int, str]:
    if not doc_ids:
        return {}
    placeholders = ",".join("?" * len(doc_ids))
    return {
        r["id"]: r["title"]
        for r in conn.execute(
            f"SELECT id, title FROM documents WHERE id IN ({placeholders})", doc_ids
        )
    }


def _canonical_title(cand: ProposedObject, concepts: dict[str, tuple[str, str]]) -> str:
    """A concept object is titled by its CANONICAL name, not the model's phrasing — so two notes
    engaging the same concept under different surfaces land on ONE object (the whole point of the
    alias substrate)."""
    if cand.type == "concept":
        hit = concepts.get(cand.anchor.casefold()) or concepts.get(cand.title.casefold())
        if hit:
            return hit[0]
    return cand.title.strip()


def _gate_object(
    cand: ProposedObject, *, doc, concepts: dict[str, tuple[str, str]], uris: dict[str, str]
) -> tuple[list[ObjectLink], str | None]:
    """Apply gates 1-3 to one candidate. Returns (links, rejection_reason)."""
    if not cand.title.strip():
        return [], "empty title"
    doc_uri = doc["source_uri"]

    if cand.type == "concept":
        hit = concepts.get(cand.anchor.casefold()) or concepts.get(cand.title.casefold())
        if hit is None:
            return [], "concept is not a canonical entity spanning enough documents"
        return (
            [ObjectLink("entity", entity_key(*hit), "about"),
             ObjectLink("doc", doc_uri, "relates")],
            None,
        )

    if cand.type in ("project", "reading"):
        target = _match_uri(cand.anchor, uris)
        if target is None:
            # A project/reading the corpus cannot pin is exactly the ungrounded proposal the bar
            # exists to stop — UNLESS the source document IS the thing (a repo proposing itself).
            if cand.type == "project" and doc["category"] in ("project", "code"):
                target = doc_uri
            else:
                return [], "anchor does not resolve to a document in the corpus"
        links = [ObjectLink("doc", target, "implements" if cand.type == "project" else "reads")]
        if target != doc_uri:
            links.append(ObjectLink("doc", doc_uri, "relates"))
        return links, None

    # question: free-standing, but always grounded in what raised it.
    return [ObjectLink("doc", doc_uri, "raised_by")], None


def _body_for(cand: ProposedObject) -> dict:
    """Type-specific body (§6.2). Empty fields are dropped so the additive merge never writes
    blanks over something the owner has filled in."""
    if cand.type == "project":
        body = {"approach": cand.approach, "open_threads": cand.open_threads,
                "learnings": cand.learnings, "why": cand.why}
    elif cand.type == "concept":
        body = {"mastery": cand.mastery, "engagement": [], "why": cand.why}
    elif cand.type == "reading":
        body = {"state": cand.state or "queued", "why": cand.why}
    else:
        body = {"why": cand.why}
    return {k: v for k, v in body.items() if v not in (None, "", [])}


def _plan_positions(
    plan: Plan,
    positions: list[ProposedPosition],
    *,
    doc,
    concepts: dict[str, tuple[str, str]],
    text: str,
    allowed: list[str],
) -> None:
    """Gate 5. `dated_at` is the SOURCE date, so the trajectory is ordered by when the owner
    actually held the view — not by when capture caught up with it (§6.3)."""
    if not positions:
        return
    if (doc["category"] or "") not in allowed:
        for p in positions:
            plan.rejected.append(
                Rejection("position", p.subject,
                          f"category {doc['category']!r} is not owner-authored")
            )
        return
    dated_at = doc["source_date"] or (doc["ingested_at"] or "")[:10]
    planned_projects = {o.title.casefold() for o in plan.objects if o.type == "project"}
    for p in positions:
        stance = p.stance.strip().strip('"')
        if not stance_is_grounded(stance, text):
            plan.rejected.append(Rejection("position", p.subject, "stance is not in the owner's words"))
            continue
        if p.subject_kind == "concept":
            hit = concepts.get(p.subject.casefold())
            if hit is None:
                plan.rejected.append(
                    Rejection("position", p.subject, "subject is not a canonical concept")
                )
                continue
            ref = entity_key(*hit)
        else:
            if p.subject.strip().casefold() not in planned_projects:
                plan.rejected.append(
                    Rejection("position", p.subject, "no project object proposed for this subject")
                )
                continue
            ref = p.subject.strip()
        plan.positions.append(
            PlannedPosition(subject_kind=p.subject_kind, subject_ref=ref, stance=stance,
                            dated_at=dated_at)
        )


# --- apply phase (the only writer) --------------------------------------------------------------


def apply_plan(conn, plan: Plan, *, run_id: int | None = None, now=None) -> ProposeResult:
    """Write a plan's objects and positions. The ONLY place this module writes."""
    result = ProposeResult(doc_id=plan.doc_id, rejected=list(plan.rejected),
                           degraded=plan.degraded, plan=plan)
    kw = {"now": now} if now else {}
    titles_to_ids: dict[str, int] = {}
    for obj in plan.objects:
        oid, created = state.upsert_object(
            conn, type_=obj.type, title=obj.title, body=obj.body, source_run=run_id, **kw
        )
        state.add_links(conn, oid, obj.links)
        titles_to_ids[obj.title.casefold()] = oid
        (result.created if created else result.updated).append(oid)

    for pos in plan.positions:
        if pos.subject_kind == "project":
            oid = titles_to_ids.get(pos.subject_ref.casefold()) or _project_id(conn, pos.subject_ref)
            if oid is None:
                result.rejected.append(
                    Rejection("position", pos.subject_ref, "project object was not created")
                )
                continue
            key = str(oid)
        else:
            key = pos.subject_ref
        if state.record_position(
            conn, subject_kind=pos.subject_kind, subject_key=key, stance=pos.stance,
            dated_at=pos.dated_at, source_doc_id=plan.doc_id, source_uri=plan.doc_uri,
            source_run=run_id, **kw
        ):
            result.positions += 1
    return result


def _project_id(conn, title: str) -> int | None:
    row = conn.execute(
        "SELECT id FROM objects WHERE type='project' AND title=? COLLATE NOCASE", (title.strip(),)
    ).fetchone()
    return row["id"] if row else None


def propose_for_document(
    conn, doc_id: int, *, run_id: int | None = None, ledger: BudgetLedger | None = None,
    runner=None, retrieve_fn=None, model: str | None = None, now=None,
) -> ProposeResult:
    """Plan then apply for one document — the normal (non-dry) path."""
    plan = plan_for_document(
        conn, doc_id, ledger=ledger, runner=runner, retrieve_fn=retrieve_fn, model=model
    )
    return apply_plan(conn, plan, run_id=run_id, now=now)


# --- batch entry point ------------------------------------------------------------------------


@dataclass
class StructureRunResult:
    documents: int = 0
    created: int = 0
    updated: int = 0
    positions: int = 0
    rejected: int = 0
    degraded: int = 0
    cost_usd: float = 0.0
    per_doc: list[ProposeResult] = field(default_factory=list)


def structure_documents(
    conn,
    doc_ids: list[int],
    *,
    runner=None,
    retrieve_fn=None,
    model: str | None = None,
    dry_run: bool = False,
) -> StructureRunResult:
    """Run the proposer over several documents, journaled and budget-metered.

    `dry_run` runs the PLAN phase only — every gate executes against the real corpus, nothing is
    written, and the per-doc plans come back for inspection. That is how the precision bar is
    measured before this is turned loose corpus-wide (plan §15)."""
    cfg = load()
    ledger = BudgetLedger(cap_usd=cfg.agent.daily_cost_cap_usd)
    out = StructureRunResult()

    if dry_run:
        for doc_id in doc_ids:
            try:
                plan = plan_for_document(
                    conn, doc_id, ledger=ledger, runner=runner, retrieve_fn=retrieve_fn, model=model
                )
            except Exception as exc:
                log.warning("propose(dry): doc %s failed: %s", doc_id, exc)
                out.degraded += 1
                continue
            _fold(out, ProposeResult(
                doc_id=doc_id, created=[], updated=[], positions=len(plan.positions),
                rejected=list(plan.rejected), degraded=plan.degraded, plan=plan,
            ))
            out.created += len(plan.objects)  # "would create or update"
        out.cost_usd = ledger.spent_usd
        return out

    with journal.run(conn, "structure") as run:
        for doc_id in doc_ids:
            if ledger.over_cap():
                log.warning("propose: stopping early — daily cost cap reached")
                run.status = "degraded"
                break
            try:
                res = propose_for_document(
                    conn, doc_id, run_id=run.id, ledger=ledger, runner=runner,
                    retrieve_fn=retrieve_fn, model=model,
                )
            except Exception as exc:  # one doc never aborts the batch (§7)
                log.warning("propose: doc %s failed: %s", doc_id, exc)
                run.status = "degraded"
                out.degraded += 1
                continue
            _fold(out, res)
            out.created += len(res.created)
            out.updated += len(res.updated)
        run.stats = {
            "documents": out.documents, "created": out.created, "updated": out.updated,
            "positions": out.positions, "rejected": out.rejected, **ledger.stats(),
        }
    out.cost_usd = ledger.spent_usd
    return out


def _fold(out: StructureRunResult, res: ProposeResult) -> None:
    out.documents += 1
    out.positions += res.positions
    out.rejected += len(res.rejected)
    out.degraded += int(res.degraded)
    out.per_doc.append(res)
