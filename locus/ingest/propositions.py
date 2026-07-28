"""Proposition pass: atomic, self-contained claims (first-class retrieval units, decision A).

These are embedded and directly retrieved, so each must stand alone out of context — the
prompt enforces resolving pronouns/references and one assertion per proposition.

A deterministic post-filter (2026-06-04 evaluation; build step 5) rejects the 8B model's
recurring failure shapes — meta-statements about the document ("X is discussed in this
section"), echoes of the section title, fragments, and fluent sentences written *around* a
formula the extractor dropped ("the inner product is given by .") — the last being worse than
nothing, because they retrieve as authoritative and contain no content. When the filter
rejects everything the model produced, the pass retries once with the failure called out;
after that, a proposition-free section is recorded honestly rather than padded with noise.
"""

from __future__ import annotations

import logging
import re

from pydantic import BaseModel

from locus.ingest.llm import generate_structured

log = logging.getLogger(__name__)


class Propositions(BaseModel):
    propositions: list[str]


# Meta-statements describe the document instead of asserting subject-matter content.
_META = re.compile(
    r"\b(?:is|are|was|were)\s+(?:discussed|covered|presented|described|introduced|explained|"
    r"outlined|reviewed|summari[sz]ed|mentioned|provided|examined|addressed|highlighted)\b"
    r"|\bin\s+this\s+(?:section|document|chapter|course|lecture|note)\b"
    r"|\b(?:this|the)\s+(?:section|document|chapter|course|lecture|note)s?\s+(?:discusses|"
    r"covers|presents|introduces|describes|explains|outlines|reviews|focuses|highlights)\b",
    re.IGNORECASE,
)

# A sentence that trails off into a copula/preposition right where the (lost) math should
# be is the signature of a formula dropped at extraction.
_DROPPED_FORMULA = re.compile(
    r"\b(?:given\s+by|defined\s+as|denoted\s+by|expressed\s+as|written\s+as|equal\s+to|"
    r"of\s+the\s+form|takes?\s+the\s+form)\s*[.,;:]?\s*$",
    re.IGNORECASE,
)

_MIN_WORDS = 4


def _normalize(text: str) -> str:
    return re.sub(r"[^a-z0-9]+", " ", text.lower()).strip()


def rejection_reason(text: str, title: str | None = None) -> str | None:
    """Why this proposition is noise, or None if it is acceptable.

    Shared with the audit metrics so `locus audit` can report how many *stored* propositions
    would fail today's filter (relevant until the corpus is re-ingested).
    """
    t = text.strip()
    if not t or t[:1].islower():
        return "fragment"  # truncated text or a non-sentence (claims start capitalised)
    if len(t.split()) < _MIN_WORDS:
        return "too-short"
    if _META.search(t):
        return "meta"
    if _DROPPED_FORMULA.search(t):
        return "dropped-formula"
    if title:
        nt, np = _normalize(title), _normalize(t)
        # The proposition merely repeats (part of) the section title.
        if nt and np and (np == nt or np in nt):
            return "title-echo"
    return None


def filter_propositions(
    props: list[str], title: str | None = None
) -> tuple[list[str], list[tuple[str, str]]]:
    """Split raw model output into (kept, [(rejected, reason), ...]), order-preserving."""
    kept: list[str] = []
    rejected: list[tuple[str, str]] = []
    for p in props:
        reason = rejection_reason(p, title)
        if reason is None:
            kept.append(p)
        else:
            rejected.append((p, reason))
    return kept, rejected


_PROMPT_RULES = (
    "Extract the propositions asserted in this section. Each proposition must be:\n"
    "- a single, self-contained factual claim stating one idea (resolve pronouns and "
    "references so it stands alone, with no dependence on surrounding context),\n"
    "- faithful to the text (do not infer beyond what is stated),\n"
    "- about the SUBJECT MATTER, never about the document: do not write meta-statements "
    "like 'X is discussed in this section' or 'this chapter covers Y'.\n"
    "If a formula or symbol a claim depends on is missing or garbled in the text, OMIT that "
    "claim entirely — never write a sentence around the gap (e.g. 'the inner product is "
    "given by .').\n"
    "Do NOT split an enumerated list into one proposition per item: when the text lists "
    "several items under a shared statement (a topic that 'covers A, B and C', a set of "
    "subtopics, a list of learning outcomes), capture that enumeration as a SINGLE "
    "proposition. But keep genuinely distinct claims as separate propositions — do not merge "
    "unrelated statements. Return an empty list if the section asserts no substantive claims."
)


# Math-dense sections (PageFlags/has_math at extraction) lose their claims when the model
# skips formula-bearing statements wholesale: the 2026-06-05 evaluation found a paper's core
# method section with ZERO propositions while its abstract had nine. The variant line asks
# for the relationship in words instead of omission (the dropped-formula filter still rejects
# sentences written around an illegible gap).
_MATH_HEAVY_RULE = (
    "\nThis section is math-dense. When a formula's symbols are legible, state the "
    "relationship it expresses in WORDS as a proposition (e.g. 'The output equals the "
    "convolution of the input with the impulse response') rather than skipping it. Only "
    "omit a claim when its formula is actually missing or garbled."
)


def extract_propositions(
    title: str | None, text: str, math_heavy: bool = False, **kw
) -> list[str]:
    """Return the atomic propositions asserted in the section (possibly empty), filtered.

    One bounded retry when the section came out proposition-free — whether the model
    produced output the filter rejected entirely, or produced nothing at all (the latter
    previously shipped silently: 2026-06-05 evaluation). `math_heavy` adds a prompt rule
    asking for formula relationships in words (set from the extractor's has_math flag).
    """
    rules = _PROMPT_RULES + (_MATH_HEAVY_RULE if math_heavy else "")
    user = (
        f"Section title: {title or '(untitled)'}\n\n"
        f"Section text:\n{text}\n\n"
        f"{rules}"
    )
    raw = generate_structured(Propositions, user, pass_name="propositions", **kw).propositions
    kept, rejected = filter_propositions(raw, title)

    if not kept:
        if raw:
            reasons = ", ".join(sorted({r for _, r in rejected}))
            complaint = (
                f"Your previous attempt was rejected entirely ({reasons}): it contained only "
                "meta-statements, title echoes, fragments, or claims written around missing "
                "formulas. Extract only substantive, complete factual claims about the subject "
                "matter; if the section asserts none, return an empty list."
            )
        else:
            log.warning(
                "propositions: model returned zero propositions for section %r; retrying", title
            )
            complaint = (
                "Your previous attempt returned no propositions. Re-read the section: if it "
                "asserts substantive factual claims, extract them; return an empty list only "
                "if it genuinely asserts none."
            )
        raw = generate_structured(Propositions, f"{user}\n\n{complaint}", pass_name="propositions", **kw).propositions
        kept, rejected = filter_propositions(raw, title)
        if not kept:
            log.warning("propositions: section %r is proposition-free after retry", title)

    if rejected:
        log.info(
            "propositions: filtered %d/%d for section %r (%s)",
            len(rejected), len(rejected) + len(kept), title,
            ", ".join(sorted({r for _, r in rejected})),
        )
    return kept
