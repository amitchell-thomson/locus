"""Synthesise — "what do I know and think about X?" (plan §3.5 use mode 3, §8.4).

The distinguishing feature versus asking any model about X: this answers from the owner's corpus
AND includes his TRAJECTORY on the topic — what he used to think, what changed it, what he argues
now. That combination is the thing no general tool can produce, and it is why the trajectory is
rendered deterministically (from stored positions) rather than narrated by the model: the chain
must be his, even when the prose around it is not.

Same enforcement as critique: every point cites an evidence key it was given, and an
unresolvable citation is dropped rather than repaired.
"""

from __future__ import annotations

import logging
from dataclasses import dataclass, field

from pydantic import BaseModel, Field

from locus.agent.claude import ClaudeError, run_structured
from locus.config import load
from locus.evolve.trajectory import render_trajectory
from locus.learn.practice import PracticeSet, generate_practice
from locus.surface.grounding import GroundingSet, ground_topic

log = logging.getLogger(__name__)


@dataclass
class Point:
    text: str
    citation_key: str
    citation_text: str
    source: str


@dataclass
class SynthesisResult:
    topic: str
    summary: str = ""
    points: list[Point] = field(default_factory=list)
    trajectory_md: str = ""
    gaps: list[str] = field(default_factory=list)
    practice: PracticeSet | None = None
    dropped: int = 0
    degraded: bool = False
    low_confidence: bool = False

    def render(self) -> str:
        lines = [f"# What you know about {self.topic}"]
        if self.low_confidence:
            lines.append(
                "\n_LOW CONFIDENCE — the corpus holds little on this; this is what there is._"
            )
        if self.summary:
            lines.append(f"\n{self.summary}")
        if self.points:
            lines.append("\n## Grounded points")
            for p in self.points:
                lines.append(f"- {p.text}")
                lines.append(f"    — {p.source}: {p.citation_text[:200]}")
        if self.trajectory_md:
            lines.append("\n## How your view has moved")
            lines.append(self.trajectory_md)
        if self.gaps:
            lines.append("\n## Where it is thin")
            lines += [f"- {g}" for g in self.gaps]
        if self.practice and self.practice.items:
            lines.append("\n## Practice")
            for i, item in enumerate(self.practice.items, start=1):
                lines.append(f"{i}. {item.question}")
                lines.append(f"    _answer ({item.source_title}): {item.answer}_")
        return "\n".join(lines)


class _Point(BaseModel):
    text: str = ""
    citation_key: str = ""


class _Synthesis(BaseModel):
    summary: str = ""
    points: list[_Point] = Field(default_factory=list)


_PROMPT = """\
Synthesise what the owner of this knowledge vault knows about a topic, using ONLY the evidence \
below — his own corpus. This is not a general explanation of the topic; it is an account of what \
HE has read, built, and concluded.

Every point MUST cite one evidence key (e.g. "S2"). A point you cannot ground in the evidence \
below will be discarded, so do not pad. If the evidence is thin, say so in the summary rather \
than filling the gap from general knowledge — a short honest answer is the useful one.

Return ONLY JSON: {{"summary": "<2-4 sentences>", "points": [{{"text": "...", "citation_key": "S2"}}]}}

TOPIC
{topic}

{evidence}
"""


def synthesise(
    conn,
    topic: str,
    *,
    retrieve_fn=None,
    runner=None,
    model: str | None = None,
    with_practice: bool = False,
    practice_items: int = 3,
    on_result=None,
    grounding: GroundingSet | None = None,
) -> SynthesisResult:
    """Grounded "what I know about X", including the dated trajectory and (optionally) practice."""
    ground = grounding if grounding is not None else ground_topic(
        conn, topic, retrieve_fn=retrieve_fn
    )
    out = SynthesisResult(topic=topic, low_confidence=ground.low_confidence)
    out.trajectory_md = "\n\n".join(
        render_trajectory(t) for t in ground.trajectories if t.entries
    )
    out.gaps = [g.render() for g in ground.gaps]

    if with_practice:
        out.practice = _practice_for(
            conn, ground, max_items=practice_items, runner=runner, model=model
        )

    prompt = _PROMPT.format(topic=topic, evidence=ground.render())
    try:
        reply = run_structured(
            prompt, schema=_Synthesis, model=model or load().agent.model, runner=runner,
            on_result=on_result,
        )
    except ClaudeError as exc:
        log.warning("synthesise: model call failed: %s", exc)
        out.degraded = True
        return out

    by_key = ground.by_key()
    out.summary = reply.summary.strip()
    for p in reply.points:
        evidence = by_key.get(p.citation_key.strip())
        if evidence is None or not p.text.strip():
            out.dropped += 1
            continue
        out.points.append(Point(text=p.text.strip(), citation_key=evidence.key,
                                citation_text=evidence.text, source=evidence.source))
    return out


def _practice_for(
    conn, ground: GroundingSet, *, max_items: int, runner, model: str | None
) -> PracticeSet:
    """Choose what to practise: gap-driven WITHIN what retrieval judged relevant.

    Relevance and gap-drivenness are both real signals, and they rank differently — so the order
    they are applied decides the result. A live `synthesise("market making and arbitrage
    strategies")` matched the `market making` concept object, whose gaps point at Optibook setup
    material, and generated three questions about what a Python IDE is while the Optiver
    market-making repo sat at the top of the evidence. Relevance has to be the FILTER (the owner
    asked about this topic) and the gap ordering the SORT (§12.3) — not the other way round.

    So: take the object's gap-driven candidates, keep those whose document retrieval actually
    surfaced, then top up from the retrieved documents in relevance order. Every route ends at
    the owner's own stored propositions; only the path differs."""
    from locus.agent.state import parse_entity_key
    from locus.learn.practice import (
        candidates_for_concept, candidates_for_object, candidates_from_documents,
    )

    evidence_docs = [e.doc_id for e in ground.evidence if e.doc_id is not None]
    relevant = set(evidence_docs)

    gap_driven: list = []
    for obj in ground.objects:
        gap_driven += candidates_for_object(conn, obj.id)
    if not gap_driven:
        for traj in ground.trajectories:
            if traj.subject_kind == "concept":
                gap_driven += candidates_for_concept(conn, parse_entity_key(traj.subject_key)[0])
    if not gap_driven:
        gap_driven = candidates_for_concept(conn, ground.topic)

    # Gap-driven candidates the retrieval ALSO surfaced: both signals agree, so they lead.
    candidates = [c for c in gap_driven if c.doc_id in relevant] if relevant else list(gap_driven)
    seen = {c.id for c in candidates}
    for cand in candidates_from_documents(conn, evidence_docs):
        if cand.id not in seen:
            seen.add(cand.id)
            candidates.append(cand)
    if not candidates:  # no retrieval evidence at all — fall back to the gap-driven set
        candidates = gap_driven

    return generate_practice(
        conn, candidates, max_items=max_items, runner=runner, model=model
    )
