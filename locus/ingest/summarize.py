"""Section summary pass (L2): a concise, faithful summary of one section.

Also emits a short semantic title: heading-poor PDFs fall back to pagination pseudo-titles
("Section (pp 6–9)" — 2026-06-05 evaluation), and the pipeline replaces those with this
title. One extra field on the existing pass; no additional model call.

GROUNDING GUARD (2026-06-05 round-3 evaluation, the headline finding): the local model
will, without warning, emit a fluent summary about a DIFFERENT topic entirely — code files
summarised as "basic electrical circuits" / "image recognition" — and for code there are
no propositions to catch the drift, so the hallucination feeds L2 retrieval silently.
Every generated summary is therefore checked for lexical grounding in its own source: it
must share distinctive vocabulary with the unit it claims to summarise. An ungrounded
summary gets ONE sterner regeneration; if that also fails, a deterministic fallback
(module signature for code, leading text for prose) replaces it — degraded but faithful
by construction, and flagged via `SectionSummary.grounded` so the pipeline records a gap.
"""

from __future__ import annotations

import logging
import re

from pydantic import BaseModel

from locus.ingest.llm import fit_to_window, generate_structured

log = logging.getLogger(__name__)

# Bumped whenever a summary prompt below changes wording: it is an ingredient of the
# pass-cache key (ingest_pipeline), so a prompt edit invalidates cached summaries without
# a manual flush. "2": grounding guard added — cached round-2 summaries include the
# hallucinated ones, so they must regenerate. "3": input now window-bounded
# (fit_to_window) — sections larger than num_ctx previously slid out of the context and
# produced topic-coherent hallucinations the guard cannot catch (round-5 audit:
# CLAUDE.md, 12k tokens, summarised as an 'image compression' paper); v2-cached
# summaries of oversized sections are untrustworthy and must regenerate. "4":
# template-echo rejection added (round-6 audit: summarising THIS file echoed the prompt
# scaffold as a filled-in 'example_module' template, and grounding passed it because the
# file's own string literals attest summary-vocabulary).
PROMPT_VERSION = "4"


class SectionSummary(BaseModel):
    summary: str
    title: str | None = None  # short semantic title; used only when the extractor had none
    # False when both generation attempts failed the grounding guard and the deterministic
    # fallback below was used. The pipeline surfaces this as a doc gap flag.
    grounded: bool = True


# --- grounding guard ------------------------------------------------------------------

# Vocabulary any summary uses regardless of subject — matching on these proves nothing.
# The hallucinated summaries this guard exists for were built almost entirely from them
# ("The document describes a new algorithm for ... with improved accuracy").
_GENERIC = frozenset(
    """document section module file describes discusses presents provides contains
    defines covers explains introduces summarizes outlines details implements
    information overview content text code source functions function classes class
    methods method system approach algorithm technique model framework process
    architecture component components feature features improved improving efficiency accurate
    basic various different several within their there these those which while
    using based include includes including handles handling manages managing
    supports related package library utility utilities helper helpers main public
    private data value values type types object objects
    this that with from into then when where what have will each every other
    also more most some such than them they only over under between across
    through both very well used uses ensure ensures allows allowing serves
    providing provided implements implementations functionality designed""".split()
)

_CAMEL_BOUNDARY = re.compile(r"(?<=[a-z0-9])(?=[A-Z])")
_WORD = re.compile(r"[A-Za-z]{4,}")


def _word_tokens(text: str) -> set[str]:
    """Lowercased alpha tokens (>=4 chars), with camelCase/snake_case split so source
    identifiers (`HMMRegimeDetector`, `fit_best_of_n_seeds`) yield their natural words."""
    text = _CAMEL_BOUNDARY.sub(" ", text.replace("_", " "))
    return {m.group(0).lower() for m in _WORD.finditer(text)}


# Prompt-scaffold fingerprints. A summary carrying any of these is the model echoing the
# instructions/template instead of summarising — observed (round-6 audit) on the one file
# whose SOURCE contains the prompt strings as literals: summarising locus/ingest/
# summarize.py produced "Source file: example_module\nThis module defines key functions
# for data processing... public API consists of process_data() and handle_input()",
# which sailed through is_grounded because the prompt text in the file attests generic
# summary vocabulary. Deterministic fingerprints, not similarity: the legit fallback
# writes "Source file {title}." (no colon), so "Source file:" is unambiguous echo.
_TEMPLATE_ECHO = (
    "example_module",
    "process_data()",
    "handle_input()",
    "\\n",  # a literal backslash-n: the model transcribed the f-string, not text
    "(unknown path)",
)


def is_template_echo(summary: str) -> bool:
    """True when the summary is the prompt scaffold echoed back, not a real summary."""
    if summary.startswith("Source file:"):
        return True
    return any(t in summary for t in _TEMPLATE_ECHO)


def is_grounded(summary: str, source: str) -> bool:
    """Does the summary share distinctive vocabulary with the text it summarises?

    Distinctive = the summary's word tokens minus generic summarising vocabulary,
    compared by 6-char stem so morphology ("propagates" vs "propagation") still counts.
    The bar is deliberately low — at least 2 distinctive tokens appearing in the source,
    covering at least a quarter of the summary's distinctive vocabulary — so any honest
    summary passes trivially (verified against all 260 stored code summaries: honest
    ones score 1/2 to all of their distinctive tokens; the hallucinated ones 0-1), while
    an off-topic hallucination and all-boilerplate filler ("contains information on
    system architecture") both fail.
    """
    distinctive = _word_tokens(summary) - _GENERIC
    if not distinctive:
        return False  # nothing but generic filler: says nothing about THIS unit
    stems = {t[:6] for t in _word_tokens(source)}
    hits = sum(1 for t in distinctive if t[:6] in stems)
    if len(distinctive) == 1:
        return hits == 1
    return hits >= 2 and hits * 4 >= len(distinctive)


def _fallback(title: str | None, text: str, code: bool) -> SectionSummary:
    """Deterministic, faithful-by-construction summary used when generation won't ground.

    Code: a signature summary (docstring first line + def/class names) — exactly the
    audit-recommended fallback, and a decent L2 embedding target for the file. Prose:
    the leading text verbatim (raw source is the one thing that cannot hallucinate).
    """
    if code:
        parts = [f"Source file {title}." if title else "Source file."]
        doc = re.match(r'\s*(?:[rub]*)?("""|\'\'\')\s*(.+)', text)
        if doc:
            first_line = doc.group(2).splitlines()[0].strip().strip("\"'")
            if first_line:
                parts.append(first_line if first_line.endswith(".") else first_line + ".")
        classes = re.findall(r"^class\s+(\w+)", text, re.M)
        funcs = re.findall(r"^\s*(?:async\s+)?def\s+(\w+)", text, re.M)
        if classes:
            parts.append("Defines classes: " + ", ".join(classes[:12]) + ".")
        if funcs:
            parts.append("Defines functions: " + ", ".join(funcs[:12]) + ".")
        return SectionSummary(summary=" ".join(parts), title=None, grounded=False)
    lead = " ".join(text.split())[:400]
    prefix = f"{title}: " if title else ""
    return SectionSummary(summary=f"{prefix}{lead}", title=None, grounded=False)


def summarize_section(title: str | None, text: str, *, code: bool = False, **kw) -> SectionSummary:
    """Summarize the section. `.summary` is embedded as the L2 section vector; `.title` is a
    short semantic title the pipeline substitutes for pagination pseudo-titles.

    `code=True` switches to the source-file variant (repo ingest): module responsibility +
    key definitions + public API, instead of the prose key-points framing.

    The text fed to the model is window-bounded (fit_to_window): repo file sections are
    one-section-per-file and can exceed num_ctx, where the unbounded source slid out of
    the window and yielded pure hallucination (round-5 audit). The grounding guard and
    the prose fallback still see the FULL text — attestation must not weaken just
    because the prompt was truncated.
    """
    full_text = text
    text = fit_to_window(text)
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
    source = f"{title or ''}\n{full_text}"
    out = generate_structured(SectionSummary, user, pass_name="summarize", **kw)
    if not is_template_echo(out.summary) and is_grounded(out.summary, source):
        return out
    log.warning("ungrounded summary for %r (%r); regenerating", title, out.summary[:80])
    retry = user + (
        "\n\nIMPORTANT: your summary MUST use the actual names and terms that appear in the "
        "material above (function/class names, methods, subject terms). Describe only what is "
        "literally there."
    )
    out = generate_structured(SectionSummary, retry, pass_name="summarize", **kw)
    if not is_template_echo(out.summary) and is_grounded(out.summary, source):
        return out
    log.warning("summary failed grounding twice for %r; deterministic fallback used", title)
    return _fallback(title, full_text, code)
