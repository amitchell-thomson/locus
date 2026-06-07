"""Claude adjudication of fuzzy alias clusters (plan step 12, CLAUDE.md §15.4).

The deterministic tiers in `aliases.py` only merge on hard evidence (case, punctuation,
attested acronym expansion, attested plural). What remains are lookalike clusters —
"LTI model" vs "Linear, Time-invariant (LTI) models", "fourier transform" stored under
three types — where the verdict is a judgement call. Per the owner's decision, that
judgement goes to the Claude API (the §11.B reasoning: this is generation-quality work,
and a wrong merge corrupts the link substrate), using the same forced-tool-use pattern
as eval/judge.py. The client is injectable so parsing is unit-testable without a key.

The adjudicator PROPOSES; `aliases.py` DISPOSES — hard guards there (min length,
same-section co-occurrence, member-surface snapping) override any verdict.
"""

from __future__ import annotations

from dataclasses import dataclass, field

from pydantic import BaseModel

from locus.config import load
from locus.eval.judge import _anthropic_client
from locus.ingest.entities import EntityType

# Bump to invalidate cached verdicts in pass_cache (prompt/schema changes).
PROMPT_VERSION = 1


@dataclass(frozen=True)
class ClusterMember:
    """One candidate surface presented for adjudication."""

    name: str
    type: str
    doc_titles: tuple[str, ...] = field(default_factory=tuple)  # up to 2, for context


class AliasGroup(BaseModel):
    member_indices: list[int]
    canonical_name: str
    canonical_type: EntityType


class AliasVerdict(BaseModel):
    """Partition of the presented members: each group is ONE real entity; a singleton
    group means that member is distinct from everything else in the cluster."""

    groups: list[AliasGroup]


_SYSTEM = (
    "You canonicalise entity names extracted from a personal knowledge base. You are given "
    "a small cluster of entity surfaces that LOOK similar (by embedding and token overlap). "
    "Partition them into groups where each group contains only surface variants of the SAME "
    "real entity: case variants, acronym vs expansion ('KL divergence' / 'Kullback-Leibler "
    "(KL) divergence'), punctuation/hyphenation variants, singular/plural, or the same "
    "concept stored under different type labels.\n"
    "Do NOT merge distinct entities that merely share words or a theme: 'Kalman filter' and "
    "'particle filter' are DISTINCT; 'VaR' (value at risk) and 'variance' are DISTINCT; "
    "'MSH(2)' and 'MSH(20)' are DISTINCT models. When in doubt, keep members separate — a "
    "missed merge is harmless, a wrong merge corrupts the knowledge graph.\n"
    "For each group choose canonical_name (MUST be copied verbatim from one of the group's "
    "members — never invent or edit a surface; prefer the most standard, complete form) and "
    "the single most accurate canonical_type. Every member index must appear in exactly one "
    "group. Use the partition tool to answer."
)


def _build_user(members: list[ClusterMember]) -> str:
    lines = []
    for i, m in enumerate(members):
        ctx = f" — appears in: {'; '.join(m.doc_titles)}" if m.doc_titles else ""
        lines.append(f"  {i}. {m.name!r} (type: {m.type}){ctx}")
    return "CANDIDATE CLUSTER:\n" + "\n".join(lines)


def adjudicate_cluster(
    members: list[ClusterMember],
    *,
    client=None,
    model: str | None = None,
    max_tokens: int = 1024,
) -> AliasVerdict:
    """Partition one lookalike cluster with Claude. Raises if no structured result returns."""
    client = client or _anthropic_client()
    model = model or load().generation.model
    tool = {
        "name": "partition",
        "description": "Record the alias partition for this cluster.",
        "input_schema": AliasVerdict.model_json_schema(),
    }
    resp = client.messages.create(
        model=model,
        max_tokens=max_tokens,
        system=_SYSTEM,
        tools=[tool],
        tool_choice={"type": "tool", "name": "partition"},
        messages=[{"role": "user", "content": _build_user(members)}],
    )
    for block in resp.content:
        if getattr(block, "type", None) == "tool_use":
            return AliasVerdict.model_validate(block.input)
    raise RuntimeError("Alias adjudicator returned no tool_use block")
