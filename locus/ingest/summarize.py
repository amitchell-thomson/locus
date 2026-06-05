"""Section summary pass (L2): a concise, faithful summary of one section.

Also emits a short semantic title: heading-poor PDFs fall back to pagination pseudo-titles
("Section (pp 6–9)" — 2026-06-05 evaluation), and the pipeline replaces those with this
title. One extra field on the existing pass; no additional model call.
"""

from __future__ import annotations

from pydantic import BaseModel

from locus.ingest.llm import generate_structured


class SectionSummary(BaseModel):
    summary: str
    title: str | None = None  # short semantic title; used only when the extractor had none


def summarize_section(title: str | None, text: str, **kw) -> SectionSummary:
    """Summarize the section. `.summary` is embedded as the L2 section vector; `.title` is a
    short semantic title the pipeline substitutes for pagination pseudo-titles."""
    user = (
        f"Section title: {title or '(untitled)'}\n\n"
        f"Section text:\n{text}\n\n"
        "Write a concise, faithful summary (3-5 sentences) capturing the section's key "
        "points, methods, and results. Also give a short descriptive title (3-8 words) "
        "naming what the section is actually about. Do not add information not present "
        "in the text."
    )
    return generate_structured(SectionSummary, user, **kw)
