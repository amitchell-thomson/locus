"""Gap-flagging pass (L1): what the document is missing, leaves open, or does not address.

Feeds the gap-analysis query mode. Operates over the synthesis + summaries as context.
"""

from __future__ import annotations

from pydantic import BaseModel

from locus.ingest.llm import generate_structured


class _Gaps(BaseModel):
    gaps: list[str]


def flag_gaps(title: str | None, context: str, **kw) -> list[str]:
    """Return flagged gaps for the document (possibly empty). `context` is synthesis + summaries."""
    user = (
        f"Document title: {title or '(untitled)'}\n\n"
        f"Document overview:\n{context}\n\n"
        "Identify gaps in this document: unanswered questions, missing analysis, untested "
        "assumptions, or topics a reader would expect but that are absent. Each gap is one "
        "concise sentence. Be specific and grounded in the overview; return an empty list if "
        "there are no notable gaps."
    )
    return generate_structured(_Gaps, user, **kw).gaps
