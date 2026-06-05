"""Context assembly (§7, §11.G): coarse-to-fine, capped at the context token budget.

Doc synthesis and section summaries (densest meaning per token) are included first and always;
then propositions, then raw chunks (finest) fill the remaining budget. When over budget the
finest content is dropped first. Output is a context string + provenance citations, ready for a
single Claude call.
"""

from __future__ import annotations

from dataclasses import dataclass, field

from locus.config import load
from locus.ingest.chunk import count_tokens
from locus.retrieve.expand import Expanded


@dataclass
class Citation:
    """A provenance string plus the structure consumers need to annotate it.

    `rerank_score` is the best cross-encoder score among the units sharing this provenance —
    the confidence signal the 2026-06-05 evaluation found missing from the MCP surface.
    """

    text: str
    doc_id: int
    rerank_score: float | None = None


@dataclass
class AssembledContext:
    text: str
    citations: list[str] = field(default_factory=list)
    citation_details: list[Citation] = field(default_factory=list)
    included: int = 0
    dropped: int = 0


def _page_label(e: Expanded) -> str:
    if e.page_start and e.page_end:
        return f"pp {e.page_start}–{e.page_end}"
    return ""


def _provenance(e: Expanded) -> str:
    parts = [f'"{e.doc_title}"' if e.doc_title else f"doc {e.doc_id}"]
    if e.section_title:
        parts.append(f"§{e.section_title}")
    if e.file_path and e.line_start:
        parts.append(f"{e.file_path}:{e.line_start}-{e.line_end}")
    elif e.video_timestamp is not None:
        parts.append(f"?t={e.video_timestamp}")
    elif _page_label(e):
        parts.append(_page_label(e))
    return ", ".join(parts)


def assemble(expanded: list[Expanded], budget: int | None = None) -> AssembledContext:
    budget = budget or load().retrieve.context_token_budget

    # Group survivors by doc (in first-seen / rerank order), then by section.
    docs: dict[int, dict] = {}
    for e in expanded:
        d = docs.setdefault(
            e.doc_id,
            {"e": e, "sections": {}},
        )
        if e.section_id is not None:
            sec = d["sections"].setdefault(
                e.section_id, {"e": e, "propositions": [], "chunks": []}
            )
            if e.candidate.kind == "proposition":
                sec["propositions"].append(e)
            elif e.candidate.kind == "chunk":
                sec["chunks"].append(e)

    lines: list[str] = []
    citations: list[str] = []
    details: dict[str, Citation] = {}  # provenance -> detail (order-preserving in 3.7+)
    tokens = 0
    included = dropped = 0

    def cite(e: Expanded) -> None:
        # A proposition and a chunk from the same section share one provenance string;
        # list it once (order-preserving) instead of repeating it per included unit. The
        # detail keeps the best rerank score among the units behind the shared string.
        p = _provenance(e)
        if p not in citations:
            citations.append(p)
        score = e.candidate.rerank_score
        d = details.setdefault(p, Citation(text=p, doc_id=e.doc_id, rerank_score=score))
        if score is not None and (d.rerank_score is None or score > d.rerank_score):
            d.rerank_score = score

    def fits(text: str) -> bool:
        nonlocal tokens
        t = count_tokens(text)
        if tokens + t > budget:
            return False
        tokens += t
        return True

    for n, (doc_id, d) in enumerate(docs.items(), start=1):
        e = d["e"]
        header = (
            f"[{n}] Document: \"{e.doc_title}\"\n"
            f"    thesis: {e.thesis}\n    method: {e.method}\n"
            f"    result: {e.result}\n    limitations: {e.limitations}\n"
        )
        if not fits(header):  # coarse content should essentially always fit
            break
        lines.append(header)

        for sec in d["sections"].values():
            se = sec["e"]
            sec_head = f"  Section: \"{se.section_title}\" {_page_label(se)}\n"
            if se.section_summary:
                sec_head += f"    summary: {se.section_summary}\n"
            if fits(sec_head):
                lines.append(sec_head)

            if sec["propositions"]:
                lines.append("    claims:\n")
                for pe in sec["propositions"]:
                    block = f"      - {pe.candidate.text}\n"
                    if fits(block):
                        lines.append(block)
                        included += 1
                        cite(pe)
                    else:
                        dropped += 1

            for ce in sec["chunks"]:  # finest-grained: filled last, dropped first
                block = f"    excerpt ({_page_label(ce) or 'source'}): {ce.candidate.text}\n"
                if fits(block):
                    lines.append(block)
                    included += 1
                    cite(ce)
                else:
                    dropped += 1

    return AssembledContext(
        text="".join(lines), citations=citations, citation_details=list(details.values()),
        included=included, dropped=dropped,
    )
