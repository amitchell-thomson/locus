"""Conservative fill-in of transcription uncertainty markers (agent-layer §8.1, Phase 1 ④).

Transcription flags what it couldn't read: `[illegible]` (unreadable) and `word[?]` (low-confidence
guess). This pass asks Claude — via the subscription `claude -p` runner (a cheap TEXT task, unlike
the metered vision transcription) — to resolve markers it can infer with HIGH confidence FROM
SURROUNDING CONTEXT, and leave the rest.

Two guards against putting words in the owner's mouth (failure mode #1):
  - **Deterministic application.** Claude only returns a resolution PER NUMBERED gap; the fills are
    applied in Python by exact marker span. The model never rewrites the note, so it cannot silently
    alter any text outside a marker — the transcription is preserved byte-for-byte except at gaps.
  - **AI-marked.** Every fill is wrapped `⟦…⟧`, so an inferred word is visibly not the owner's own
    (invariant 4). The raw transcription (with the original markers) is kept upstream, so a wrong
    fill is always recoverable.

Degrades safely: if the runner fails, the transcription is returned unchanged (markers intact).
"""

from __future__ import annotations

import re
from dataclasses import dataclass

from pydantic import BaseModel

from locus.agent import claude

# A gap is `[illegible]` or a token immediately followed by `[?]`.
_GAP_RE = re.compile(r"(?P<word>\S+?)\[\?\]|\[illegible\]")
# How much surrounding text to show Claude per gap (enough context to infer, not the whole note).
_CONTEXT_CHARS = 80


@dataclass
class FillResult:
    markdown: str      # transcription with high-confidence gaps resolved, each marked ⟦…⟧
    filled: int        # gaps resolved
    total_gaps: int    # gaps found


class _Fill(BaseModel):
    n: int
    resolution: str | None = None  # None = cannot confidently infer -> leave the marker


class _Fills(BaseModel):
    fills: list[_Fill]


def _build_prompt(text: str, matches: list[re.Match]) -> str:
    lines = []
    for i, m in enumerate(matches, start=1):
        lo = max(0, m.start() - _CONTEXT_CHARS)
        hi = min(len(text), m.end() + _CONTEXT_CHARS)
        kind = "illegible" if m.group(0) == "[illegible]" else "low-confidence guess"
        snippet = text[lo:m.start()] + "«GAP»" + text[m.end():hi]
        lines.append(f'{i}. [{kind}] …{snippet.strip()}…')
    return (
        "This is a transcription of the owner's handwritten notes. Some words are flagged: "
        "[illegible] = the transcriber could not read it; word[?] = a low-confidence guess. "
        "For each NUMBERED gap (marked «GAP» in its context), supply the correct word ONLY if the "
        "surrounding context lets you infer it with HIGH confidence. If you are not confident, "
        "return null — do NOT guess. These are dense quant-finance notes; do not invent jargon.\n\n"
        'Respond with ONLY JSON: {"fills": [{"n": 1, "resolution": "word or short phrase" or null}, …]}, '
        "one entry per gap.\n\nGAPS:\n" + "\n".join(lines)
    )


def fill_gaps(markdown: str, *, runner=None, model: str | None = None) -> FillResult:
    """Resolve high-confidence uncertainty markers in `markdown`, marking each fill ⟦…⟧.

    `runner`/`model` are the `claude -p` runner knobs (default: subscription Haiku). On any runner
    failure the transcription is returned unchanged (degrade, never block)."""
    matches = list(_GAP_RE.finditer(markdown))
    if not matches:
        return FillResult(markdown=markdown, filled=0, total_gaps=0)

    try:
        result = claude.run_structured(
            _build_prompt(markdown, matches), schema=_Fills, runner=runner, model=model,
        )
    except claude.ClaudeError:
        return FillResult(markdown=markdown, filled=0, total_gaps=len(matches))  # degrade: keep raw

    resolved = {f.n: f.resolution.strip() for f in result.fills if f.resolution and f.resolution.strip()}
    out: list[str] = []
    cursor = 0
    filled = 0
    for i, m in enumerate(matches, start=1):
        out.append(markdown[cursor:m.start()])
        res = resolved.get(i)
        if res:
            out.append(f"⟦{res}⟧")  # AI-marked fill (invariant 4)
            filled += 1
        else:
            out.append(m.group(0))  # leave the original marker untouched
        cursor = m.end()
    out.append(markdown[cursor:])
    return FillResult(markdown="".join(out), filled=filled, total_gaps=len(matches))
