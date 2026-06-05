"""Section summary pass (L2): a concise, faithful summary of one section.

Also emits a short semantic title: heading-poor PDFs fall back to pagination pseudo-titles
("Section (pp 6–9)" — 2026-06-05 evaluation), and the pipeline replaces those with this
title. One extra field on the existing pass; no additional model call.
"""

from __future__ import annotations

from pydantic import BaseModel

from locus.ingest.llm import generate_structured

# Bumped whenever a summary prompt below changes wording: it is an ingredient of the
# pass-cache key (ingest_pipeline), so a prompt edit invalidates cached summaries without
# a manual flush.
PROMPT_VERSION = "1"


class SectionSummary(BaseModel):
    summary: str
    title: str | None = None  # short semantic title; used only when the extractor had none


def summarize_section(title: str | None, text: str, *, code: bool = False, **kw) -> SectionSummary:
    """Summarize the section. `.summary` is embedded as the L2 section vector; `.title` is a
    short semantic title the pipeline substitutes for pagination pseudo-titles.

    `code=True` switches to the source-file variant (repo ingest): module responsibility +
    key definitions + public API, instead of the prose key-points framing.
    """
    if code:
        user = (
            f"Source file: {title or '(unknown path)'}\n\n"
            f"Source code:\n{text}\n\n"
            "This is a source file from a software project. Write a concise, faithful "
            "summary (3-5 sentences) of what this module does: its responsibility, the key "
            "functions/classes it defines and what each is for, and how it fits the wider "
            "codebase if evident. Name the public API. Do not transcribe code into the "
            "summary. Also give a short descriptive title (3-8 words) naming what the "
            "module is actually for. Do not add information not present in the code."
        )
    else:
        user = (
            f"Section title: {title or '(untitled)'}\n\n"
            f"Section text:\n{text}\n\n"
            "Write a concise, faithful summary (3-5 sentences) capturing the section's key "
            "points, methods, and results. Write plain prose: name what equations express, but "
            "never transcribe equations or LaTeX into the summary. Also give a short descriptive "
            "title (3-8 words) naming what the section is actually about. Do not add information "
            "not present in the text."
        )
    return generate_structured(SectionSummary, user, **kw)
