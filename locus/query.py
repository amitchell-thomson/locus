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


def _system_prompt(mode: str) -> str:
    return f"{_PREAMBLE}\n\n{QUERY_MODES[mode]}\n\n{_GROUNDING}"


def _user_prompt(context: str, question: str) -> str:
    body = context or "(no relevant material was retrieved)"
    return f"<retrieved_context>\n{body}\n</retrieved_context>\n\nQuestion: {question}"


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

    resp = client.messages.create(
        model=model,
        max_tokens=max_tokens,
        # Stable system prompt cached; per-query context goes in the (uncached) user turn.
        system=[{"type": "text", "text": _system_prompt(mode), "cache_control": {"type": "ephemeral"}}],
        thinking={"type": "adaptive"},  # let Claude decide reasoning depth (synthesis is non-trivial)
        messages=[{"role": "user", "content": _user_prompt(retrieved.context, question)}],
    )
    text = "".join(b.text for b in resp.content if getattr(b, "type", None) == "text")
    return QueryResult(
        question=question, mode=mode, answer=text, model=model, citations=retrieved.citations
    )
