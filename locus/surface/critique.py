"""Critique — stress-test a project or a piece of reasoning against the owner's own corpus.

Use mode 1, the co-priority (plan §1, §3.5). The value is specifically that the challenges come
from what HE has read and previously concluded, not from a model's general knowledge — a generic
"have you considered overfitting?" is worthless; "your note of 2026-05-01 says changepoint
detection is the honest baseline, and this approach is the HMM you rejected" is not.

So each challenge must cite an evidence key it was given, and `critique()` DROPS any challenge
whose citation does not resolve. That is enforced after the model, not requested of it — a model
told to cite will cite, including keys it invented. Dropping is also why the prompt asks for few,
sharp challenges: with verification in place, padding just gets discarded.
"""

from __future__ import annotations

import logging
from dataclasses import dataclass, field

from pydantic import BaseModel, Field

from locus.agent.claude import ClaudeError, run_structured
from locus.config import load
from locus.surface.grounding import GroundingSet, ground_object, ground_topic

log = logging.getLogger(__name__)


@dataclass
class Challenge:
    point: str
    citation_key: str
    citation_text: str
    source: str


@dataclass
class CritiqueResult:
    target: str
    challenges: list[Challenge] = field(default_factory=list)
    strengths: list[str] = field(default_factory=list)
    open_threads: list[str] = field(default_factory=list)
    gaps: list[str] = field(default_factory=list)
    dropped: int = 0  # challenges discarded for citing nothing real
    degraded: bool = False
    low_confidence: bool = False

    def render(self) -> str:
        lines = [f"# Critique — {self.target}"]
        if self.low_confidence:
            lines.append(
                "\n_LOW CONFIDENCE — the corpus holds little on this; treat the critique as thin._"
            )
        if self.strengths:
            lines.append("\n## What holds up")
            lines += [f"- {s}" for s in self.strengths]
        lines.append("\n## Challenges")
        if not self.challenges:
            lines.append("_No challenge could be grounded in your corpus._")
        for c in self.challenges:
            lines.append(f"- {c.point}")
            lines.append(f"    — grounded in: {c.citation_text[:200]} ({c.source})")
        if self.open_threads:
            lines.append("\n## Open threads you recorded")
            lines += [f"- {t}" for t in self.open_threads]
        if self.gaps:
            lines.append("\n## Where your grasp is thin")
            lines += [f"- {g}" for g in self.gaps]
        return "\n".join(lines)


class _Challenge(BaseModel):
    point: str = ""
    citation_key: str = ""


class _Critique(BaseModel):
    strengths: list[str] = Field(default_factory=list)
    challenges: list[_Challenge] = Field(default_factory=list)


_PROMPT = """\
You are stress-testing the owner of a knowledge vault against HIS OWN material. Below is what he \
is working on or claiming, and the evidence his corpus holds.

Write challenges that only someone who had read his corpus could write. Every challenge MUST \
cite one evidence key from the list (e.g. "S3"); a challenge you cannot ground in a specific \
piece of the evidence below does not belong in the output — it will be discarded. Prefer three \
sharp, grounded challenges to eight generic ones. Where his own recorded positions contradict \
what he is now doing, say so plainly — that is the most useful thing you can surface.

Also list what genuinely holds up (briefly, and only if the evidence supports it).

Return ONLY JSON: {{"strengths": ["..."], "challenges": [{{"point": "...", "citation_key": "S3"}}]}}

TARGET
{target}

{evidence}
"""


def critique(
    conn,
    target: str,
    *,
    object_id: int | None = None,
    retrieve_fn=None,
    runner=None,
    model: str | None = None,
    on_result=None,
    grounding: GroundingSet | None = None,
) -> CritiqueResult:
    """Stress-test `target` against the corpus. `object_id` centres it on a structured object.

    Grounding is deterministic and free; only the challenge-writing is a model call. Degrades to
    the deterministic half (open threads + gaps) if that call fails — those are still worth
    having, and they are the part that cannot be wrong."""
    ground = grounding if grounding is not None else (
        ground_object(conn, object_id, retrieve_fn=retrieve_fn) if object_id is not None
        else ground_topic(conn, target, retrieve_fn=retrieve_fn)
    )
    out = CritiqueResult(target=target, low_confidence=ground.low_confidence)
    for obj in ground.objects:
        out.open_threads += list(obj.body.get("open_threads", []))
    out.gaps = [g.render() for g in ground.gaps]

    prompt = _PROMPT.format(target=target, evidence=ground.render())
    try:
        reply = run_structured(
            prompt, schema=_Critique, model=model or load().agent.model, runner=runner,
            on_result=on_result,
        )
    except ClaudeError as exc:
        log.warning("critique: model call failed: %s", exc)
        out.degraded = True
        return out

    by_key = ground.by_key()
    out.strengths = [s.strip() for s in reply.strengths if s.strip()]
    for ch in reply.challenges:
        evidence = by_key.get(ch.citation_key.strip())
        if evidence is None or not ch.point.strip():
            out.dropped += 1  # cites nothing real -> not a grounded challenge
            continue
        out.challenges.append(Challenge(
            point=ch.point.strip(), citation_key=evidence.key, citation_text=evidence.text,
            source=evidence.source,
        ))
    return out
