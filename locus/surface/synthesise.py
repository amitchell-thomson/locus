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
    """Practice over the topic's own material: an object's gap-driven candidates when one matched,
    else propositions from the concepts the trajectories cover."""
    from locus.agent.state import parse_entity_key
    from locus.learn.practice import candidates_for_concept, candidates_for_object

    candidates = []
    for obj in ground.objects:
        candidates += candidates_for_object(conn, obj.id)
    if not candidates:
        for traj in ground.trajectories:
            if traj.subject_kind == "concept":
                candidates += candidates_for_concept(conn, parse_entity_key(traj.subject_key)[0])
    if not candidates:
        candidates = candidates_for_concept(conn, ground.topic)
    return generate_practice(
        conn, candidates, max_items=max_items, runner=runner, model=model
    )
