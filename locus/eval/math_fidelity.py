"""Math-fidelity judging: does a transcription preserve the math visible on a page image?

Claude (multimodal) compares a rendered page against a candidate transcription and scores
equation survival. Two uses (plan step 6 + step 7):
  - the math-OCR engine benchmark (scripts/benchmarks/judge_mathocr.py) — choosing the routing engine;
  - the corpus math-fidelity metric — the gate metric for the bulk ingest ("fraction of
    math-bearing sections whose formulas survived extraction").

The judge sees the image as ground truth; the text layer, an OCR output, or a stored section
text are all just candidates. Score = (correct + 0.5*partial) / equations_on_page.
"""

from __future__ import annotations

import base64
import json
import re
from pathlib import Path

from pydantic import BaseModel, Field

JUDGE_SYSTEM = (
    "You are a meticulous technical judge. You compare a rendered page image (ground truth) "
    "against a candidate transcription and score how faithfully the MATHEMATICS survived. "
    "Count display equations and substantive inline expressions (variables with structure: "
    "fractions, integrals, sub/superscripts, operators) — not lone variable names. "
    "Respond ONLY with the requested JSON."
)

JUDGE_PROMPT = """Compare the attached page image (ground truth) with this candidate transcription:

<transcription>
{transcription}
</transcription>

Return JSON with exactly these fields:
- "equations_on_page": number of distinct mathematical expressions visible on the page (display + substantive inline; cap at 30)
- "equations_correct": how many appear in the transcription mathematically correct (any notation: LaTeX, unicode, plain — judge the MATH, not the markup style)
- "equations_partial": how many appear but are garbled, truncated, or partially wrong
- "equations_missing": how many are absent from the transcription entirely
- "equations_hallucinated": expressions in the transcription that are NOT on the page
- "prose_fidelity": 0-5, how faithfully the non-math text is transcribed (5 = verbatim)
- "repetition_loop": true if the transcription degenerates into repeated text
- "notes": one or two short observations (worst error, or what was done well)
"""


class PageJudgement(BaseModel):
    equations_on_page: int = Field(ge=0)
    equations_correct: int = Field(ge=0)
    equations_partial: int = Field(ge=0)
    equations_missing: int = Field(ge=0)
    equations_hallucinated: int = Field(ge=0)
    prose_fidelity: int = Field(ge=0, le=5)
    repetition_loop: bool
    notes: str

    @property
    def math_fidelity(self) -> float:
        """0..1: fraction of the page's math that survived (partial counts half)."""
        if self.equations_on_page <= 0:
            return 1.0  # no math to lose
        score = (self.equations_correct + 0.5 * self.equations_partial) / self.equations_on_page
        return max(0.0, min(1.0, score))


def _extract_json(text: str) -> dict:
    """Parse the first JSON object in the response (judges occasionally add prose)."""
    m = re.search(r"\{.*\}", text, re.DOTALL)
    if not m:
        raise ValueError(f"no JSON object in judge response: {text[:200]!r}")
    return json.loads(m.group(0))


def judge_transcription(
    client, model: str, page_png: Path, transcription: str, *, max_chars: int = 24_000
) -> PageJudgement:
    """Judge one (page image, candidate transcription) pair. One repair retry on bad JSON."""
    image_b64 = base64.standard_b64encode(page_png.read_bytes()).decode()
    content = [
        {
            "type": "image",
            "source": {"type": "base64", "media_type": "image/png", "data": image_b64},
        },
        {"type": "text", "text": JUDGE_PROMPT.format(transcription=transcription[:max_chars])},
    ]
    messages = [{"role": "user", "content": content}]
    last: Exception | None = None
    for _ in range(2):
        resp = client.messages.create(
            model=model, system=JUDGE_SYSTEM, messages=messages, max_tokens=1000
        )
        text = "".join(b.text for b in resp.content if b.type == "text")
        try:
            return PageJudgement.model_validate(_extract_json(text))
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
