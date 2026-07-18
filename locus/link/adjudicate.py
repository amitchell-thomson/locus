"""Claude adjudication of fuzzy alias clusters (plan step 12, CLAUDE.md §15.4).

The deterministic tiers in `aliases.py` only merge on hard evidence (case, punctuation,
attested acronym expansion, attested plural). What remains are lookalike clusters —
"LTI model" vs "Linear, Time-invariant (LTI) models", "fourier transform" stored under
three types — where the verdict is a judgement call. Per the owner's decision, that
judgement goes to Claude (the §11.B reasoning: this is generation-quality work, and a wrong
merge corrupts the link substrate).

Mechanism (2026-06-28): the adjudication runs through **headless `claude -p`** (the owner's
Claude Code subscription) rather than the billed Anthropic API. The CLI has no forced-tool-use,
so the model is asked to return the verdict as a strict JSON object which we parse into the same
`AliasVerdict` schema. Verdicts are cached content-keyed in pass_cache (mechanism-agnostic), so
this drop-in change reuses every prior verdict and only spends a CLI call on a new/changed
cluster. The runner is injectable so parsing is unit-testable without spawning a subprocess.

The adjudicator PROPOSES; `aliases.py` DISPOSES — hard guards there (min length,
same-section co-occurrence, member-surface snapping) override any verdict.
"""

from __future__ import annotations

import json
import subprocess
import tempfile
import typing
from collections.abc import Callable
from dataclasses import dataclass, field

from pydantic import BaseModel, ValidationError

from locus.config import load
from locus.ingest.entities import EntityType

# Bump to invalidate cached verdicts in pass_cache (prompt/schema changes). NB the API->CLI switch
# did NOT bump this: a cached verdict is the same partition regardless of which Claude produced it,
# so the substrate refresh reuses prior verdicts and only adjudicates new clusters via the CLI.
PROMPT_VERSION = 1

# A `claude -p` adjudication should never take this long; bounds a wedged subprocess.
_CLI_TIMEOUT_S = 180

# How a verdict is requested over the CLI. (prompt, model) -> the model's raw text response.
Runner = Callable[[str, "str | None"], str]


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


def _entity_types() -> list[str]:
    """The allowed canonical_type values, for the prompt (EntityType is a Literal/enum)."""
    args = typing.get_args(EntityType)
    if args:
        return [str(a) for a in args]
    return [e.value for e in EntityType]  # type: ignore[attr-defined]


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
    "group."
)


def _build_prompt(members: list[ClusterMember]) -> str:
    lines = []
    for i, m in enumerate(members):
        ctx = f" — appears in: {'; '.join(m.doc_titles)}" if m.doc_titles else ""
        lines.append(f"  {i}. {m.name!r} (type: {m.type}){ctx}")
    cluster = "CANDIDATE CLUSTER:\n" + "\n".join(lines)
    return (
        f"{_SYSTEM}\n\n{cluster}\n\n"
        f"canonical_type must be one of: {', '.join(_entity_types())}.\n"
        "Respond with ONLY a JSON object (no prose, no markdown code fences) of exactly this "
        'shape: {"groups": [{"member_indices": [0, 2], "canonical_name": "<verbatim surface>", '
        '"canonical_type": "<type>"}, {"member_indices": [1], "canonical_name": "...", '
        '"canonical_type": "..."}]}'
    )


def _claude_cli_runner(prompt: str, model: str | None) -> str:
    """Run one headless `claude -p` and return the assistant's text.

    Uses `--output-format json` (a stable envelope around the reply) and a neutral working
    directory so the call does NOT pick up this repo's CLAUDE.md or project MCP servers — each
    adjudication is a clean, cheap, single-shot classification.
    """
    cmd = ["claude", "-p", prompt, "--output-format", "json"]
    if model:
        cmd += ["--model", model]
    try:
        proc = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=_CLI_TIMEOUT_S,
            cwd=tempfile.gettempdir(),
        )
    except FileNotFoundError as e:
        raise RuntimeError(
            "`claude` CLI not found on PATH — alias adjudication uses headless Claude Code"
        ) from e
    except subprocess.TimeoutExpired as e:
        raise RuntimeError(f"`claude -p` timed out after {_CLI_TIMEOUT_S}s") from e
    if proc.returncode != 0:
        raise RuntimeError(f"`claude -p` failed (exit {proc.returncode}): {proc.stderr[:500]}")
    try:
        envelope = json.loads(proc.stdout)
    except json.JSONDecodeError as e:
        raise RuntimeError(f"`claude -p` did not return JSON: {proc.stdout[:300]!r}") from e
    if envelope.get("is_error"):
        raise RuntimeError(f"`claude -p` reported an error: {envelope.get('result')!r}")
    return envelope.get("result", "")


def _parse_verdict(text: str) -> AliasVerdict:
    """Extract the JSON verdict object from the model's reply and validate it.

    Tolerant of leading/trailing prose or code fences: slices from the first '{' to the last
    '}'. Raises RuntimeError (caught/surfaced by the caller) on anything unparseable.
    """
    start, end = text.find("{"), text.rfind("}")
    if start == -1 or end <= start:
        raise RuntimeError(f"adjudicator returned no JSON object: {text[:200]!r}")
    try:
        obj = json.loads(text[start : end + 1])
        return AliasVerdict.model_validate(obj)
    except (json.JSONDecodeError, ValidationError) as e:
        raise RuntimeError(f"adjudicator returned invalid verdict: {text[:200]!r}") from e


def adjudicate_cluster(
    members: list[ClusterMember],
    *,
    runner: Runner | None = None,
    model: str | None = None,
) -> AliasVerdict:
    """Partition one lookalike cluster with Claude (headless `claude -p` by default).

    `runner` is injectable: the default spawns `claude -p`; tests pass a fake returning canned
    JSON. Raises RuntimeError if no valid verdict comes back.
    """
    runner = runner or _claude_cli_runner
    model = model or load().generation.model
    return _parse_verdict(runner(_build_prompt(members), model))
