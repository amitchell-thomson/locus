"""LLM-as-judge: Claude scores ingest outputs against the source section (CLAUDE.md §11.B/C).

A strong model (Claude) grades the weak local model's output on faithfulness, atomicity,
self-containment, and entity recall/precision. This is the systematic quality signal and the
basis for the qwen-vs-llama comparison (decision C). Uses Anthropic tool-use to force a
structured, validated score object. The client is injectable so parsing is unit-testable.
"""

from __future__ import annotations

from pydantic import BaseModel, Field

from locus.config import Config, load


class SectionScores(BaseModel):
    summary_faithfulness: int = Field(ge=1, le=5)
    proposition_faithfulness: int = Field(ge=1, le=5)
    proposition_atomicity: int = Field(ge=1, le=5)
    proposition_self_containment: int = Field(ge=1, le=5)
    entity_recall: int = Field(ge=1, le=5)
    entity_precision: int = Field(ge=1, le=5)
    notes: str = ""

    def mean(self) -> float:
        vals = [
            self.summary_faithfulness, self.proposition_faithfulness,
            self.proposition_atomicity, self.proposition_self_containment,
            self.entity_recall, self.entity_precision,
        ]
        return sum(vals) / len(vals)


_SYSTEM = (
    "You are a strict evaluator of an automated knowledge-extraction system. You are given a "
    "source section and the metadata a local model extracted from it. Score each dimension "
    "from 1 (poor) to 5 (excellent), judging ONLY against the source text:\n"
    "- summary_faithfulness: does the summary accurately reflect the source without invention?\n"
    "- proposition_faithfulness: are the propositions all supported by the source?\n"
    "- proposition_atomicity: is each a single claim (not fused, not a fragmented list item)?\n"
    "- proposition_self_containment: does each stand alone (no dangling pronouns/references)?\n"
    "- entity_recall: were the section's salient entities captured?\n"
    "- entity_precision: are the listed entities real and correctly typed (not noise/dupes)?\n"
    "Be calibrated and critical. Use the score tool to return your judgement."
)


def _build_user(source: str, summary: str, propositions: list[str], entities: list[tuple[str, str]]) -> str:
    props = "\n".join(f"  {i+1}. {p}" for i, p in enumerate(propositions)) or "  (none)"
    ents = "\n".join(f"  - {n} ({t})" for n, t in entities) or "  (none)"
    return (
        f"SOURCE SECTION:\n{source}\n\n"
        f"EXTRACTED SUMMARY:\n{summary}\n\n"
        f"EXTRACTED PROPOSITIONS:\n{props}\n\n"
        f"EXTRACTED ENTITIES:\n{ents}\n"
    )


def _anthropic_client():
    import anthropic

    return anthropic.Anthropic(api_key=Config.anthropic_api_key())


def judge_section(
    source: str,
    summary: str,
    propositions: list[str],
    entities: list[tuple[str, str]],
    *,
    client=None,
    model: str | None = None,
    max_tokens: int = 1024,
) -> SectionScores:
    """Score one section's extraction with Claude. Raises if no structured result is returned."""
    client = client or _anthropic_client()
    model = model or load().generation.model
    tool = {
        "name": "score",
        "description": "Record the evaluation scores for this section.",
        "input_schema": SectionScores.model_json_schema(),
    }
    resp = client.messages.create(
        model=model,
        max_tokens=max_tokens,
        system=_SYSTEM,
        tools=[tool],
        tool_choice={"type": "tool", "name": "score"},
        messages=[{"role": "user", "content": _build_user(source, summary, propositions, entities)}],
    )
    for block in resp.content:
        if getattr(block, "type", None) == "tool_use":
            return SectionScores.model_validate(block.input)
    raise RuntimeError("Judge returned no tool_use block")
