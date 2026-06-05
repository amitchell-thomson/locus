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

# Tokens too generic to identify a topic — ignored when grounding a gap claim against the
# document text (filter_gaps below).
_GENERIC_TOKENS = frozenset(
    "document section study paper chapter course lecture note notes does not provide "
    "provided explain explained explains detail details detailed discussion discuss "
    "discussed discusses information specific specifically about there this that with "
    "from into applied used using uses steps data method methods analysis include "
    "included includes cover covered covers coverage mention mentioned mentions exact "
    "also further more other their these those such what when where which while would "
    "could should missing absent without lacks lacking unclear how the and for are "
    "between based given state stated states".split()
)


def _distinctive_tokens(text: str) -> set[str]:
    return {
        t for t in re.findall(r"[a-zA-Z][a-zA-Z0-9\-]{3,}", text.lower())
        if t not in _GENERIC_TOKENS
    }


def filter_gaps(gaps: list[str], hints: list[str], doc_text: str) -> list[str]:
    """Precision filter: keep only gap claims the evidence supports.

    The 2026-06-05 second evaluation caught the failure mode this guards against: the model
    sees section *summaries*, so detail compressed out of a summary reads as 'absent' and it
    emits false gaps ('does not detail θC' while the text states θC=0.8). Keep a gap only if
    (a) it is backed by an explicit deferral hint (evidence-anchored 'mentioned but not
    covered'), or (b) its distinctive terms are genuinely absent from the document text
    (majority unattested). A gap naming a topic the document discusses at length is far more
    likely a summarisation artifact than a real absence — precision over recall here, because
    a false gap tells the owner they lack knowledge they have.
    """
    doc_lower = doc_text.lower()
    hint_token_sets = [_distinctive_tokens(h) for h in hints]
    kept: list[str] = []
    for gap in gaps:
        tokens = _distinctive_tokens(gap)
        if not tokens:
            continue
        hint_backed = any(len(tokens & ht) >= 2 for ht in hint_token_sets)
        attested = sum(1 for t in tokens if t in doc_lower)
        if hint_backed or attested < len(tokens) / 2:
            kept.append(gap)
    return kept


def deferral_hints(texts: Sequence[str], max_hints: int = _MAX_HINTS) -> list[str]:
    """Sentences in the raw text that explicitly defer or exclude a topic (deduplicated).

    The window includes the PRECEDING sentence: deferral sentences usually point at their
    topic by pronoun ('This method is not covered in the A2 course') and the topic's name
    sits one sentence earlier — without it the hint cannot anchor a gap (filter_gaps's
    hint-backing needs the topic tokens present).
    """
    hints: list[str] = []
    for text in texts:
        for m in _DEFERRAL.finditer(text):
            start = text.rfind(".", 0, m.start()) + 1
            prev = text.rfind(".", 0, max(0, start - 1)) + 1  # one sentence further back
            end = text.find(".", m.end())
            end = end + 1 if end != -1 else min(len(text), m.end() + 120)
            sentence = " ".join(text[prev:end].split())[:400]
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
        "concise sentence naming the specific missing topic. IMPORTANT: you are reading "
        "SUMMARIES, which compress detail by design — never claim the document 'does not "
        "explain' or 'lacks detail on' something a summary mentions; the detail almost "
        "certainly exists in the full text. Only report a topic as a gap if it is deferred "
        "explicitly, assumed as a prerequisite, or genuinely absent from every summary. "
        "Return an empty list if nothing qualifies."
    )
    raw = generate_structured(_Gaps, "\n".join(parts), **kw).gaps
    if sections:
        return filter_gaps(raw, hints, "\n".join(raw_text for _, _, raw_text in sections))
    return raw
