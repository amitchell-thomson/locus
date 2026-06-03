"""Proposition pass: atomic, self-contained claims (first-class retrieval units, decision A).

These are embedded and directly retrieved, so each must stand alone out of context — the
prompt enforces resolving pronouns/references and one assertion per proposition.
"""

from __future__ import annotations

from pydantic import BaseModel

from locus.ingest.llm import generate_structured


class Propositions(BaseModel):
    propositions: list[str]


def extract_propositions(title: str | None, text: str, **kw) -> list[str]:
    """Return the atomic propositions asserted in the section (possibly empty)."""
    user = (
        f"Section title: {title or '(untitled)'}\n\n"
        f"Section text:\n{text}\n\n"
        "Extract the propositions asserted in this section. Each proposition must be:\n"
        "- a single, self-contained factual claim stating one idea (resolve pronouns and "
        "references so it stands alone, with no dependence on surrounding context),\n"
        "- faithful to the text (do not infer beyond what is stated).\n"
        "Do NOT split an enumerated list into one proposition per item: when the text lists "
        "several items under a shared statement (a topic that 'covers A, B and C', a set of "
        "subtopics, a list of learning outcomes), capture that enumeration as a SINGLE "
        "proposition. But keep genuinely distinct claims as separate propositions — do not merge "
        "unrelated statements. Return an empty list if the section asserts no substantive claims."
    )
    return generate_structured(Propositions, user, **kw).propositions
