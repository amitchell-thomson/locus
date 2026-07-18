"""Code concept pass: the domain concepts a repository implements/studies (CLAUDE.md §1.2).

Cross-corpus *linking* is a primary use of Locus, and its biggest gap is code. Code docs skip
the raw-section entity pass (`llm_entities=False`) because entity extraction from raw code
yields identifiers (`fetch_feed`, `ZScore`), not the domain concepts that bridge a project to
the papers it is built on — and `link/related.py` filters those identifiers as boilerplate.
Measured (2026-06-09): 6/12 code docs share entities with nothing; all 3 Obsidian-graph
orphans are code/project docs (e.g. an "Alpha Fund … Black-Scholes" repo that links to none of
the Black-Scholes papers, because its entities are function names).

This pass fills the gap WITHOUT touching raw code — the §11.B-weak task it deliberately avoids.
It reads the repository's NARRATIVE (the doc synthesis, README/markdown, and per-file
summaries — all doc/section-level prose where an 8B model is reliable, the same surface
`synthesize`/`summarize` already use for code) and emits `concept`/`method`/`tool`/`dataset`/
`metric`/`theorem` entities in the SAME (name, type) space as paper entities. Once `locus link`
runs, its deterministic tiers merge "regime switching" on the regime repo with "regime
switching" in the regime papers, and the project finally links. See
docs/code-concept-extraction-plan.md.

A doc-level single call, like synthesis (and like synthesis, not pass_cached). Grounding +
stoplist + cap guards mirror the entity pass: a missed concept is fragmentation, a junk
concept is a false cross-domain link, so the guards lean conservative.
"""

from __future__ import annotations

from typing import Literal

from pydantic import BaseModel

from locus.ingest.entities import Entity, is_grounded, is_noise, normalize_name
from locus.ingest.llm import generate_structured

PROMPT_VERSION = 1

# Concepts are the "what it implements/studies" subset of the entity vocabulary; author/
# ticker/organization/other don't bridge a project to research, so they're out of scope here.
ConceptType = Literal["concept", "method", "tool", "dataset", "metric", "theorem"]

# Generic terms that, as entities, would link everything to everything (a code repo and a
# physics PDF both "use data" and "have an API"). The corpus-aware stop-entity guard in
# related.py is the second line of defence; this keeps them out of the substrate entirely.
DEFAULT_STOPLIST = frozenset(
    {
        "python", "api", "data", "function", "class", "method", "module", "library",
        "framework", "algorithm", "model", "code", "software", "pipeline", "database",
        "cli", "interface", "script", "test", "tool", "system", "application", "file",
        "input", "output", "dataset", "repository", "project",
    }
)


class _Concept(BaseModel):
    name: str
    type: ConceptType
    evidence: str  # a short span from the narrative (steers the model toward grounded output)


class _Concepts(BaseModel):
    concepts: list[_Concept]


def extract_code_concepts(
    title: str | None,
    synthesis_text: str,
    section_summaries: list[str],
    readme_text: str = "",
    *,
    stoplist: frozenset[str] | set[str] | None = None,
    max_concepts: int = 40,
    max_narrative_chars: int = 12000,
    markdown_fraction: float = 0.75,
    **kw,
) -> list[Entity]:
    """Domain concepts the repository implements/studies, as normalised + filtered entities.

    Inputs are the repo's already-extracted narrative (synthesis + README/markdown-doc text +
    section summaries) — never raw code. Markdown docs are the domain-concept-bearing surface,
    so they get `markdown_fraction` of the num_ctx-bounded budget and the code-file summaries
    the remainder (CLAUDE.md §1.2 — Link). Returns at most `max_concepts` `Entity` objects,
    deduped by (name, type), each grounded in the narrative shown and absent from `stoplist`
    (`None` => the built-in `DEFAULT_STOPLIST`)."""
    stop = {s.casefold() for s in (stoplist if stoplist is not None else DEFAULT_STOPLIST)}
    # Weight markdown FAR above code. A repo's domain concepts live in its prose docs (README,
    # analysis/*.md, design notes); its code-file summaries describe implementation and yield
    # mostly identifiers, not the concepts that bridge a project to its papers (CLAUDE.md §1.2).
    # So the markdown docs get the majority (`markdown_fraction`) of the num_ctx-bounded budget
    # and the code summaries a bounded remainder — a large repo (regime-ml has 88 files)
    # overflows num_ctx when ALL summaries are joined and the model degrades to one junk concept
    # (observed 2026-06-09). Synthesis is the densest signal and is always included whole; if
    # the docs are short the remainder grows, so a doc-light repo still gets its summaries.
    docs = (readme_text or "")[: int(max_narrative_chars * markdown_fraction)]
    budget = max(0, max_narrative_chars - len(synthesis_text) - len(docs))
    summ_lines: list[str] = []
    used = 0
    for s in section_summaries:
        line = f"- {s}"
        if not s or used + len(line) > budget:
            continue
        summ_lines.append(line)
        used += len(line)
    summaries = "\n".join(summ_lines) or "(none)"
    # Grounding source = exactly the text we show the model, so a concept must be attested in
    # the narrative, not merely in the model's (fabricable) `evidence` field.
    narrative = "\n".join(filter(None, [synthesis_text, docs, summaries]))
    user = (
        f"Project: {title or '(unknown)'}\n\n"
        f"Project description (synthesis):\n{synthesis_text}\n\n"
        + (
            "Project documentation (README / design docs — the PRIMARY source of domain "
            f"concepts):\n{docs}\n\n"
            if docs
            else ""
        )
        + f"Per-file code summaries (supplementary — mostly implementation detail):\n{summaries}\n\n"
        + "This is a SOFTWARE PROJECT. List the DOMAIN CONCEPTS, models, methods, techniques, "
        "datasets, theorems, and metrics this project implements, uses, or studies — the ideas "
        "that connect it to academic papers and coursework on the same subjects. Examples: "
        "'regime switching', 'Markov-switching model', 'Black-Scholes', 'Monte Carlo', 'Kalman "
        "filter', 'liquefied natural gas', 'AIS data', 'covariance estimation', 'mean "
        "reversion', 'modern portfolio theory'. For each give its name, the most specific type "
        "from concept/method/tool/dataset/metric/theorem, and a short evidence quote from the "
        "text above. Do NOT list: code identifiers (function, class, file, or variable names "
        "such as 'fetch_feed' or 'BaseClient'), programming languages, generic software or "
        "library terms (API, pipeline, database, CLI, module, NumPy, FastAPI), or the project's "
        "own name. When a concept is a specialised compound, ALSO list the base concept it "
        "specialises, provided that base is supported by the text: 'Markov-switching model' -> "
        "also 'Markov model'; 'Geometric Brownian Motion' -> also 'Brownian motion'; "
        "'mean-variance optimization' -> also 'portfolio optimization'. Base concepts are what "
        "connect a project to the papers and coursework on the same subject, so they matter most. "
        "Only concepts actually supported by the text; do not invent. Return an empty list if "
        "there are none."
    )
    raw = generate_structured(_Concepts, user, **kw).concepts
    out: list[Entity] = []
    seen: set[tuple[str, str]] = set()
    for c in raw:
        name = normalize_name(c.name)
        if not name or is_noise(name) or name.casefold() in stop:
            continue
        if not is_grounded(name, narrative):
            continue
        key = (name, str(c.type))
        if key in seen:
            continue
        seen.add(key)
        out.append(Entity(name=name, type=c.type))
        if len(out) >= max_concepts:
            break
    return out
