"""Section summary pass (L2): a concise, faithful summary of one section."""

from __future__ import annotations

from pydantic import BaseModel

from locus.ingest.llm import generate_structured


class SectionSummary(BaseModel):
    summary: str


def summarize_section(title: str | None, text: str, **kw) -> str:
    """Return a concise summary of the section. Embedded as the L2 section vector."""
    user = (
        f"Section title: {title or '(untitled)'}\n\n"
        f"Section text:\n{text}\n\n"
        "Write a concise, faithful summary (3-5 sentences) capturing the section's key "
        "points, methods, and results. Do not add information not present in the text."
    )
    return generate_structured(SectionSummary, user, **kw).summary
