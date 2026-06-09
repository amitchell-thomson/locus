"""Figure-description judging: is a description faithful to the figure image? (step 11.6)

Claude (multimodal) sees the actual figure PNG as ground truth and scores a candidate
description. Used by scripts/benchmarks/judge_figures.py to gate the llama.cpp vision engine against
the stored Ollama-served descriptions (the corpus baseline): identical weights served by a
different executor must produce descriptions of equal faithfulness — speed is judged
separately by the wall clock.
"""

from __future__ import annotations

import base64
import json
import re
from pathlib import Path

from pydantic import BaseModel, Field

JUDGE_SYSTEM = (
    "You are a meticulous technical judge. You compare a figure image (ground truth) "
    "against a candidate one-paragraph description written for text search. Score ONLY "
    "what can be verified against the pixels. Respond ONLY with the requested JSON."
)

JUDGE_PROMPT = """Compare the attached figure image (ground truth) with this candidate description:

<description>
{description}
</description>

Return JSON with exactly these fields:
- "faithfulness": 0-5 — does the description describe only what is actually visible (axes, labels, blocks, connections, trends)? 5 = every claim verifiable in the image; 0 = mostly wrong.
- "concreteness": 0-5 — does it name real, specific components/labels/quantities a text search could match? 5 = names the key visible elements; 0 = generic boilerplate.
- "hallucinated_elements": count of claims about elements NOT visible in the image (invented panels, blocks, delays, numbers, labels; blurred regions described as if readable).
- "notes": one short observation (the worst error, or what was done well)
"""


class FigureJudgement(BaseModel):
    faithfulness: int = Field(ge=0, le=5)
    concreteness: int = Field(ge=0, le=5)
    hallucinated_elements: int = Field(ge=0)
    notes: str


def _extract_json(text: str) -> dict:
    m = re.search(r"\{.*\}", text, re.DOTALL)
    if not m:
        raise ValueError(f"no JSON object in judge response: {text[:200]!r}")
    return json.loads(m.group(0))


def judge_description(
    client, model: str, figure_png: Path | bytes, description: str
) -> FigureJudgement:
    """Judge one (figure image, candidate description) pair. One repair retry on bad JSON."""
    png = figure_png if isinstance(figure_png, bytes) else figure_png.read_bytes()
    content = [
        {
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": "image/png",
                "data": base64.standard_b64encode(png).decode(),
            },
        },
        {"type": "text", "text": JUDGE_PROMPT.format(description=description[:8_000])},
    ]
    messages = [{"role": "user", "content": content}]
    last: Exception | None = None
    for _ in range(2):
        resp = client.messages.create(
            model=model, system=JUDGE_SYSTEM, messages=messages, max_tokens=800
        )
        text = "".join(b.text for b in resp.content if b.type == "text")
        try:
            return FigureJudgement.model_validate(_extract_json(text))
        except Exception as exc:  # malformed JSON / schema mismatch: one repair pass
            last = exc
            messages.append({"role": "assistant", "content": text})
            messages.append(
                {
                    "role": "user",
                    "content": f"Your output failed validation ({exc}). Return ONLY the JSON object.",
                }
            )
    raise RuntimeError(f"judge produced no valid JSON after retry: {last}")
