"""Document synthesis pass (L1): thesis / method / result / limitations for the whole doc.

Operates over the section summaries (coarse, fits context) rather than raw text, so it sees
the whole document at a manageable size.
"""

from __future__ import annotations

from pydantic import BaseModel

from locus.ingest.llm import generate_structured


class DocSynthesis(BaseModel):
    thesis: str
    method: str
    result: str
    limitations: str


def synthesize_document(title: str | None, section_summaries: list[str], **kw) -> DocSynthesis:
    """Synthesise the document from its section summaries into the four L1 synthesis fields."""
    joined = "\n".join(f"- {s}" for s in section_summaries) or "(no section summaries)"
    user = (
        f"Document title: {title or '(untitled)'}\n\n"
        f"Section summaries:\n{joined}\n\n"
        "Synthesise the whole document into:\n"
        "- thesis: its central claim or purpose,\n"
        "- method: the approach, techniques, or structure used,\n"
        "- result: the key findings or content,\n"
        "- limitations: what it does not cover, caveats, or open ends.\n"
        "Be faithful to the summaries and concise."
    )
    return generate_structured(DocSynthesis, user, **kw)
