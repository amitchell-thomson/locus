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
        "Extract the atomic propositions asserted in this section. Each proposition must be:\n"
        "- a single, self-contained factual claim (resolve pronouns and references so it "
        "stands alone, with no dependence on surrounding context),\n"
        "- faithful to the text (do not infer beyond what is stated),\n"
        "- one concise sentence.\n"
        "Return an empty list if the section asserts no substantive claims."
    )
    return generate_structured(Propositions, user, **kw).propositions
