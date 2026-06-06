"""Stage 7: query — retrieve, then a single Claude call (§7).

The product surface for asking the vault questions. Retrieval (Stage 6) assembles grounded
context + citations; this hands that context + the question to ONE Claude call and returns a
grounded answer. Query *modes* are a system-prompt lever only — same retrieval, different
persona/instruction (standard / gap-analysis / cross-domain synthesis / code / professional
framing / project recommendation).
"""

from __future__ import annotations

from dataclasses import dataclass, field

from locus.config import Config, load
from locus.retrieve import Facets, retrieve

DEFAULT_MODE = "standard"

# Mode = persona/framing only. Retrieval is identical across modes (§7).
QUERY_MODES: dict[str, str] = {
    "standard": "Answer the question directly and precisely from the retrieved material.",
    "gap": (
        "Focus on what is missing, unresolved, or unaddressed. Identify gaps, open questions, "
        "and untested assumptions in the retrieved material that bear on the question."
    ),
    "synthesis": (
        "Surface connections across domains and sources. Draw out non-obvious links (e.g. "
        "between engineering and quantitative finance) and synthesise a unified view from the "
        "retrieved material."
    ),
    "code": (
        "Answer code questions precisely. Include relevant function signatures and cite "
        "file:line references from the retrieved material when they are present."
    ),
    "framing": (
        "Frame the answer for professional and project context — how this knowledge applies to "
        "real engineering or quantitative-finance work, and how to articulate it."
    ),
    "project": (
        "Recommend concrete projects or next steps that build on the retrieved material, "
        "grounded in what it actually contains."
    ),
}

_PREAMBLE = "You are Locus, a precise research assistant answering over the owner's personal knowledge vault."

# Shared grounding rules (anti-hallucination): answer from context, cite, admit gaps.
_GROUNDING = (
    "Answer using ONLY the retrieved context provided. Ground every claim in it and cite the "
    "document/section you draw on (the context lists sources). If the context does not contain "
    "the answer, say so plainly — do not rely on outside knowledge or invent facts."
)


@dataclass
class QueryResult:
    question: str
    mode: str
    answer: str
    model: str
    citations: list[str] = field(default_factory=list)
    citation_details: list = field(default_factory=list)  # assemble.Citation, with rerank scores
    low_confidence: bool = False  # retrieval's coverage signal, passed through (see retrieve)
    confidence_band: str | None = None  # None | 'ambiguous' | 'absent' (see retrieve)
    figures_attached: int = 0  # actual figure images sent to Claude (step 11 tier 3)


def _system_prompt(mode: str) -> str:
    return f"{_PREAMBLE}\n\n{QUERY_MODES[mode]}\n\n{_GROUNDING}"


def _user_prompt(context: str, question: str, band: str | None = None) -> str:
    from locus.retrieve.pipeline import confidence_banner

    body = context or "(no relevant material was retrieved)"
    banner = confidence_banner(band)
    if banner:
        # The generation step must see the coverage signal, not just the (weak) material.
        # Band-aware: 'ambiguous' invites facet-bridging; 'absent' invites saying so.
        body = f"NOTE: {banner}\n\n{body}"
    return f"<retrieved_context>\n{body}\n</retrieved_context>\n\nQuestion: {question}"


def _user_content(retrieved, question: str) -> tuple[list[dict] | str, int]:
    """User-turn content: the text prompt, plus retrieved figure IMAGES (tier 3, §15.1).

    Claude is multimodal — when a retrieved unit is a figure, the actual image goes into
    the (uncached) user turn as a base64 image block, each preceded by a small text label
    so Claude can tie image to citation. The system prompt stays cached: per the caching
    invalidation hierarchy, message-content changes never touch the system-prefix cache.
    A missing/corrupt image silently degrades to text-only — caption+description are
    already in the assembled context. Returns (content, images_attached).
    """
    text = _user_prompt(retrieved.context, question, retrieved.confidence_band)
    if not retrieved.figures:
        return text, 0

    import base64

    from locus.retrieve.figure_images import load_figure_png

    blocks: list[dict] = [{"type": "text", "text": text}]
    attached = 0
    for fig in retrieved.figures:  # already best-first and capped at [figures].image_cap
        png = load_figure_png(fig.raw_path)
        if png is None:
            continue
        unit = "slide" if fig.kind == "slide" else "figure on p."
        label = f"[{unit}{fig.page} of \"{fig.doc_title}\"]"
        blocks.append({"type": "text", "text": label})
        blocks.append(
            {
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": "image/png",
                    "data": base64.standard_b64encode(png).decode("utf-8"),
                },
            }
        )
        attached += 1
    cited = getattr(retrieved, "figures_cited", 0)
    if attached and cited > attached:
        # Tell the model the truncation exists so it never claims to "see" an
        # unattached-but-cited figure (2026-06-06 audit: dangling figure citations).
        blocks.append(
            {
                "type": "text",
                "text": (
                    f"(Note: {cited} figures are cited in the context; only the "
                    f"{attached} most relevant are attached as images.)"
                ),
            }
        )
    return (blocks, attached) if attached else (text, 0)


def answer(
    question: str,
    *,
    mode: str = DEFAULT_MODE,
    conn=None,
    client=None,
    model: str | None = None,
    max_tokens: int = 16000,
    facets: Facets | None = None,
) -> QueryResult:
    """Retrieve context and answer the question with a single Claude call.

    `facets` optionally restricts retrieval to a date range / category (CLAUDE.md §16).
    """
    if mode not in QUERY_MODES:
        raise ValueError(f"unknown mode {mode!r}; choose from {sorted(QUERY_MODES)}")

    retrieved = retrieve(question, conn=conn, facets=facets)
    model = model or load().generation.model
    if client is None:
        import anthropic

        client = anthropic.Anthropic(api_key=Config.anthropic_api_key())

    content, figures_attached = _user_content(retrieved, question)
    resp = client.messages.create(
        model=model,
        max_tokens=max_tokens,
        # Stable system prompt cached; per-query context (text + any retrieved figure
        # images) goes in the (uncached) user turn — image changes never invalidate the
        # cached system prefix.
        system=[{"type": "text", "text": _system_prompt(mode), "cache_control": {"type": "ephemeral"}}],
        thinking={"type": "adaptive"},  # let Claude decide reasoning depth (synthesis is non-trivial)
        messages=[{"role": "user", "content": content}],
    )
    text = "".join(b.text for b in resp.content if getattr(b, "type", None) == "text")
    return QueryResult(
        question=question, mode=mode, answer=text, model=model,
        citations=retrieved.citations, citation_details=retrieved.citation_details,
        low_confidence=retrieved.low_confidence, confidence_band=retrieved.confidence_band,
        figures_attached=figures_attached,
    )
