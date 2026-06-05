"""Gap-flagging pass (L1): what the document is missing, leaves open, or does not address.

Feeds the gap-analysis query mode. The 2026-06-05 evaluation found this pass inert (`gaps: 0`
on every document): given only the synthesis as context, the model saw nothing missing and the
prompt invited an empty list. Repaired by grounding the pass in evidence — the per-section
summaries (what the document actually covers, so mentioned-but-not-covered topics are visible)
plus deterministic deferral phrases scanned from the raw text ("not covered in this course",
"beyond the scope", …), which are confirmed gaps the model only has to phrase.
"""

from __future__ import annotations

import re
from typing import Sequence

from pydantic import BaseModel

from locus.ingest.llm import generate_structured


class _Gaps(BaseModel):
    gaps: list[str]


# Explicit deferrals in source text: deterministic evidence that a topic is mentioned but not
# covered. Retrieval already surfaces these lines verbatim (the root-locus probe in the
# 2026-06-05 evaluation); the ingest pass must record them as stored gaps.
_DEFERRAL = re.compile(
    r"(?:not covered in this (?:course|book|paper|document|chapter|module|lecture)"
    r"|beyond the scope"
    r"|outside the scope"
    r"|out of (?:the )?scope"
    r"|we (?:do|will|shall) not (?:cover|discuss|address|consider|treat|prove)"
    r"|left (?:to|as an exercise for) the reader"
    r"|assumed (?:to be )?familiar"
    r"|omitted for brevity"
    r"|will not be (?:covered|discussed|addressed|treated)"
    r"|is deferred to)",
    re.IGNORECASE,
)

_MAX_HINTS = 12
_MAX_SUMMARY_CHARS = 300  # per-section cap so long docs stay inside the model's context


def deferral_hints(texts: Sequence[str], max_hints: int = _MAX_HINTS) -> list[str]:
    """Sentences in the raw text that explicitly defer or exclude a topic (deduplicated)."""
    hints: list[str] = []
    for text in texts:
        for m in _DEFERRAL.finditer(text):
            start = text.rfind(".", 0, m.start()) + 1
            end = text.find(".", m.end())
            end = end + 1 if end != -1 else min(len(text), m.end() + 120)
            sentence = " ".join(text[start:end].split())
            if sentence and sentence not in hints:
                hints.append(sentence)
            if len(hints) >= max_hints:
                return hints
    return hints


def flag_gaps(
    title: str | None,
    context: str,
    sections: Sequence[tuple[str | None, str, str]] | None = None,
    **kw,
) -> list[str]:
    """Return flagged gaps for the document (possibly empty).

    `context` is the document synthesis. `sections` is (title, summary, raw_text) per section:
    summaries ground the coverage picture; raw text is scanned (deterministically, here — the
    model never sees it whole) for explicit deferral phrases fed back as confirmed gaps.
    """
    parts = [
        f"Document title: {title or '(untitled)'}",
        f"\nDocument overview:\n{context}",
    ]
    hints: list[str] = []
    if sections:
        coverage = "\n".join(
            f"- {sec_title or '(untitled section)'}: {summary[:_MAX_SUMMARY_CHARS]}"
            for sec_title, summary, _ in sections
        )
        parts.append(f"\nSection-by-section coverage:\n{coverage}")
        hints = deferral_hints([raw for _, _, raw in sections])
        if hints:
            parts.append(
                "\nExplicit deferrals found in the text (each is a CONFIRMED gap — "
                "rephrase each as a gap statement):\n" + "\n".join(f"- {h}" for h in hints)
            )
    parts.append(
        "\nIdentify the knowledge gaps a reader of this document is left with: topics that are "
        "mentioned or relied on but not covered (the explicit deferrals above, if any, are "
        "confirmed examples), questions raised but not answered, assumptions stated but not "
        "tested, and prerequisite material assumed rather than explained. Each gap is one "
        "concise sentence naming the specific missing topic. Ground every gap in the material "
        "above — do not invent gaps the material does not suggest. Return an empty list only "
        "if the document is genuinely self-contained."
    )
    return generate_structured(_Gaps, "\n".join(parts), **kw).gaps
