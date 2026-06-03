"""Entity pass: typed named entities (method, author, concept, ...).

The type is a closed vocabulary (grammar-constrained), which keeps the name+type identity
used downstream (Obsidian projection, entity-anchored retrieval) consistent across documents.
Section provenance (doc_id, section_id) is attached by the caller, not the model.
"""

from __future__ import annotations

from typing import Literal

from pydantic import BaseModel

from locus.ingest.llm import generate_structured

EntityType = Literal[
    "method", "dataset", "author", "concept", "ticker", "tool", "theorem", "metric", "other"
]


class Entity(BaseModel):
    name: str
    type: EntityType


class _Entities(BaseModel):
    entities: list[Entity]


def extract_entities(title: str | None, text: str, **kw) -> list[Entity]:
    """Return typed entities mentioned in the section (possibly empty)."""
    user = (
        f"Section title: {title or '(untitled)'}\n\n"
        f"Section text:\n{text}\n\n"
        "List the salient entities in this section. These are not only proper nouns: include "
        "technical concepts and models (e.g. 'LTI system', 'transfer function'), methods and "
        "algorithms, theorems, datasets, tools/software, metrics, people (authors), and stock "
        "tickers. For each give its name and the most specific type from: method, dataset, "
        "author, concept, ticker, tool, theorem, metric, other. Include only entities actually "
        "mentioned in the text; do not invent any. Return an empty list only if there are none."
    )
    return generate_structured(_Entities, user, **kw).entities
