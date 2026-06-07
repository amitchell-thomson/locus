"""Document synthesis pass (L1): thesis / method / result / limitations for the whole doc.

Operates over the section summaries (coarse, fits context) rather than raw text, so it sees
the whole document at a manageable size.

Validation is semantic, not just structural (2026-06-04 evaluation; build step 5): an
all-empty synthesis is schema-valid JSON, and one shipped silently — the doc header then
leads every assembled context with four blank fields. Field validators reject blank values,
which routes the failure through llm.py's bounded repair loop; if the model still cannot
produce a non-empty synthesis, IngestExtractionError quarantines the document loudly
instead of ingesting it with an empty L1.
"""

from __future__ import annotations

from pydantic import BaseModel, field_validator

from locus.ingest.llm import generate_structured


class DocSynthesis(BaseModel):
    thesis: str
    method: str
    result: str
    limitations: str
    # Title arbitration: the extractor's title heuristic (metadata -> page-1 font -> filename)
    # picks banners ("ENGINEERING SCIENCE" for a syllabus), tab titles ("... - Colab"), and
    # truncations. The synthesis pass sees every section summary — the right context to
    # confirm the candidate verbatim or correct it. Optional: None keeps the extractor title.
    title: str | None = None

    @field_validator("thesis", "method", "result", "limitations")
    @classmethod
    def _non_blank(cls, v: str) -> str:
        v = v.strip()
        if not v:
            raise ValueError(
                "synthesis fields must be non-empty; write a brief best-effort "
                "characterisation (e.g. 'Not stated in the document') rather than leaving "
                "a field blank"
            )
        return v


def synthesize_document(
    title: str | None, section_summaries: list[str], *, code: bool = False, **kw
) -> DocSynthesis:
    """Synthesise the document from its section summaries into the four L1 synthesis fields.

    `code=True` switches to the repository variant: sections are per-file summaries, and
    docs/plan files sort FIRST (ASCII: capitals before lowercase), which biased the generic
    prompt into titling repos after their documentation ("X Development Plan" — observed
    live, 2026-06-05). The variant pins the title to what the software does.
    """
    joined = "\n".join(f"- {s}" for s in section_summaries) or "(no section summaries)"
    if code:
        user = (
            f"Repository name: {title or '(unknown)'}\n\n"
            f"Per-file summaries (READMEs/plans first, then source files):\n{joined}\n\n"
            "Synthesise this SOFTWARE REPOSITORY into:\n"
            "- thesis: what the software does and why it exists,\n"
            "- method: its architecture — main components and how they fit together,\n"
            "- result: the current state/capabilities of the project,\n"
            "- limitations: known gaps, TODOs, or things it does not do.\n"
            "- title: a short name for the PROJECT ITSELF (at most 8 words), describing what "
            "the software does. Never title it after its documentation or planning files — "
            "'X Development Plan' or 'X Project Documentation' are wrong: the repository IS "
            "the software, the docs merely describe it.\n"
            "Be faithful to the summaries and concise. Every field must be non-empty: if the "
            "repository does not state one, give a brief best-effort characterisation instead "
            "of leaving it blank."
        )
    else:
        user = (
            f"Extracted title candidate: {title or '(none)'}\n\n"
            f"Section summaries:\n{joined}\n\n"
            "Synthesise the whole document into:\n"
            "- thesis: its central claim or purpose,\n"
            "- method: the approach, techniques, or structure used,\n"
            "- result: the key findings or content,\n"
            "- limitations: what it does not cover, caveats, or open ends.\n"
            "- title: the document's title. The candidate above was extracted heuristically and "
            "may be a page banner, a browser-tab suffix, a fragment, or a filename. If it is the "
            "document's actual title, return it VERBATIM; otherwise give the true title, or a "
            "faithful descriptive one (at most 12 words) based on the summaries.\n"
            "Be faithful to the summaries and concise. Every field must be non-empty: if the "
            "document does not state one, give a brief best-effort characterisation instead of "
            "leaving it blank."
        )
    return generate_structured(DocSynthesis, user, **kw)
