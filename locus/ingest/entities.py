"""Entity pass: typed named entities (method, author, concept, ...).

The type is a closed vocabulary (grammar-constrained), which keeps the name+type identity
used downstream (Obsidian projection, entity-anchored retrieval) consistent across documents.
Section provenance (doc_id, section_id) is attached by the caller, not the model.

Hygiene (2026-06-04 evaluation; build step 5): names are surface-normalised at write time
(whitespace, surrounding punctuation, leading articles) and noise is filtered — equation/
figure labels ("equation 1.36") and bare symbols ("β", "f(t,x,y,z)") are not entities, and
they degrade the entity-anchored retrieval arm. Plural/singular variants are merged per
document with `merge_plural_variants`, evidence-based (only when the singular form is itself
attested), so irregular words ("Fourier series") are never mangled. Full *cross-document*
alias resolution stays a separate pass (build step 12).
"""

from __future__ import annotations

import re
from typing import Literal

from pydantic import BaseModel

from locus.ingest.llm import generate_structured

EntityType = Literal[
    "method", "dataset", "author", "concept", "ticker", "tool", "theorem", "metric",
    "organization", "other"
]


class Entity(BaseModel):
    name: str
    type: EntityType


class _Entities(BaseModel):
    entities: list[Entity]


# --- name hygiene -------------------------------------------------------------------------

_ARTICLE = re.compile(r"^(?:the|a|an)\s+", re.IGNORECASE)
# Numbered artefact labels are references, not entities.
_LABEL = re.compile(
    r"^(?:equation|eq\.?|figure|fig\.?|table|section|example|problem|chapter|page|lecture)"
    r"\s*\(?\s*\d",
    re.IGNORECASE,
)
_WORD3 = re.compile(r"[A-Za-z]{3,}")
_ACRONYM = re.compile(r"[A-Z][A-Z0-9]+")  # 'ML', 'AI', 'LTI', 'H2' — all-caps short names


def normalize_name(name: str) -> str:
    """Conservative surface normalisation: collapse whitespace, strip wrapping punctuation
    and a leading article. Case is preserved (it carries meaning: 'Kalman', 'LTI')."""
    name = re.sub(r"\s+", " ", name).strip()
    name = name.strip("\"'`“”‘’()[]{}.,;:")
    return _ARTICLE.sub("", name).strip()


def is_noise(name: str) -> bool:
    """True for names that are not entities: numbered labels, bare symbols, and structurally
    malformed extractions.

    A real name has an alphabetic word of 3+ letters or is an all-caps acronym ('ML', 'LTI');
    'β', 'f(t,x,y,z)' and 'equation 1.36' have neither. Unbalanced brackets anywhere mark a
    truncated extraction ('SVD (', 'MAP (' — 2026-06-05 evaluation); normalize_name only
    strips string *ends*, so these must be rejected, not repaired. Shared with the audit
    metrics so `locus audit` reports how many *stored* entities would fail today's filter.
    """
    if not name or _LABEL.match(name):
        return True
    if any(name.count(o) != name.count(c) for o, c in ("()", "[]", "{}")):
        return True
    return not (_WORD3.search(name) or _ACRONYM.fullmatch(name))


def is_grounded(name: str, source: str) -> bool:
    """True when the name is attested in the section text it was extracted from.

    Any 3+ letter token of the name appearing in the source counts (loose on purpose: minor
    surface variation must not over-reject); names with no such token (acronyms like 'LTI')
    pass — the acronym arm of `is_noise` already vetted their shape. Applied at ingest to stop
    cross-document bleed and hallucinated entities (2026-06-05 evaluation: control-theory
    entities stored on an econometrics section); shared with the audit metrics.
    """
    tokens = [
        t for t in re.sub(r"[^a-z0-9 ]", " ", name.lower()).split() if len(t) >= 3
    ]
    if not tokens:
        return True
    source_lower = source.lower()
    return any(t in source_lower for t in tokens)


def merge_plural_variants(section_entities: list[list[Entity]]) -> None:
    """Within one document, rewrite 'Xs'/'Xes' to 'X' (same type) when 'X' is itself attested.

    Evidence-based: 'Laplace transforms' collapses onto an attested 'Laplace transform', but
    'Fourier series' is left alone ('Fourier serie' is attested nowhere). Mutates in place;
    the per-section UNIQUE constraint dedupes any resulting same-section duplicates at write.
    """
    seen = {(e.name, e.type) for sec in section_entities for e in sec}
    for sec in section_entities:
        for e in sec:
            for suffix in ("es", "s"):
                if e.name.endswith(suffix):
                    singular = e.name[: -len(suffix)]
                    if len(singular) >= 3 and (singular, e.type) in seen:
                        e.name = singular
                        break


def extract_entities(title: str | None, text: str, **kw) -> list[Entity]:
    """Return typed entities mentioned in the section (possibly empty), normalised + filtered."""
    user = (
        f"Section title: {title or '(untitled)'}\n\n"
        f"Section text:\n{text}\n\n"
        "List the salient entities in this section. These are not only proper nouns: include "
        "technical concepts and models (e.g. 'LTI system', 'transfer function'), methods and "
        "algorithms, theorems, datasets, tools/software, metrics, people (authors), "
        "organizations, and stock tickers. For each give its name and the most specific type "
        "from: method, dataset, author, concept, ticker, tool, theorem, metric, organization, "
        "other. Typing rules with examples: 'author' is a PERSON who wrote something (Rudolf "
        "Kalman, Zhang) — an institution (FOMC, IMF, the Federal Reserve, MIT) is "
        "'organization', never 'author'. 'ticker' is an exchange symbol (AAPL, SPY) — a policy "
        "or programme (QE3, Basel III) is 'concept', not 'ticker'. When an entity is a "
        "specialised compound, ALSO list the base concept it specialises as its own entity, "
        "provided the base concept is itself discussed in the text: 'Markov-switching VAR' -> "
        "also 'Markov model' and 'VAR'; 'rolling-window PCMCI' -> also 'PCMCI'; 'regime shift "
        "detection' -> also 'regime shift'. Base concepts are what connect documents to each "
        "other — a section about regime-switching models IS about Markov models. Include only "
        "entities actually mentioned in the text; do not invent any. Do NOT list numbered "
        "references (equation/figure/table numbers) or bare mathematical symbols — they are "
        "not entities. Return an empty list only if there are none."
    )
    raw = generate_structured(_Entities, user, **kw).entities
    out: list[Entity] = []
    seen: set[tuple[str, str]] = set()
    for e in raw:
        name = normalize_name(e.name)
        if is_noise(name) or not is_grounded(name, text):
            continue
        key = (name, str(e.type))
        if key in seen:
            continue
        seen.add(key)
        out.append(Entity(name=name, type=e.type))
    return out
