"""Structural ingest-quality metrics — deterministic, no API, computed from the DB.

These catch the common 8B-model failure modes without judging correctness:
  - over-fragmentation  : sections with far too many propositions (e.g. list explosion)
  - non-self-contained  : propositions starting with a dangling pronoun/demonstrative
  - ungrounded entities : entity names whose tokens never appear in the section source
  - redundant entities  : near-duplicate entity names within a section (name+type churn)
  - empty coverage      : sections that yielded no propositions

QC (2026-06-04 evaluation; PLAN.md step 5): the audit also re-applies the ingest-time hygiene
predicates to *stored* rows — suspect propositions (meta/title-echo/fragment/dropped-formula),
noise entities, and an empty document synthesis. Non-zero counts on documents ingested before
the hygiene pass quantify exactly what the step-7 re-ingest will clean.

They are a fast first pass; the LLM-as-judge (judge.py) assesses actual faithfulness.
"""

from __future__ import annotations

import re
from collections import Counter
from dataclasses import dataclass, field

from locus.ingest.entities import is_noise
from locus.ingest.propositions import rejection_reason

# A proposition opening with one of these is almost certainly not self-contained.
_PRONOUN_STARTS = {
    "it", "this", "that", "these", "those", "they", "them", "he", "she",
    "its", "their", "his", "her", "such", "here", "there", "they're", "it's",
}
# Sections with more propositions than this are flagged as likely over-fragmented.
OVER_FRAGMENTED = 30
_WORD = re.compile(r"[A-Za-z0-9']+")


@dataclass
class DocMetrics:
    doc_id: int
    title: str
    source_date: str | None
    category: str | None
    sections: int
    propositions: int
    entities: int
    props_per_section_mean: float
    props_per_section_max: int
    empty_prop_sections: int
    over_fragmented_sections: int
    non_self_contained_props: int
    ungrounded_entities: int
    redundant_entity_pairs: int
    # QC against the ingest-time hygiene predicates (re-applied to stored rows).
    suspect_props: dict[str, int] = field(default_factory=dict)  # reason -> count
    noise_entities: int = 0
    empty_synthesis: bool = False
    entity_type_counts: dict[str, int] = field(default_factory=dict)

    @property
    def non_self_contained_pct(self) -> float:
        return 100.0 * self.non_self_contained_props / self.propositions if self.propositions else 0.0

    @property
    def ungrounded_pct(self) -> float:
        return 100.0 * self.ungrounded_entities / self.entities if self.entities else 0.0


def _normalize(name: str) -> str:
    return re.sub(r"[^a-z0-9 ]", " ", name.lower()).strip()


def _section_source(conn, section_id: int) -> str:
    rows = conn.execute(
        "SELECT raw_text FROM chunks WHERE section_id=? ORDER BY position", (section_id,)
    ).fetchall()
    return "\n".join(r["raw_text"] for r in rows).lower()


def _is_self_contained(text: str) -> bool:
    words = _WORD.findall(text)
    return not (words and words[0].lower() in _PRONOUN_STARTS)


def _is_grounded(name: str, source_lower: str) -> bool:
    tokens = [t for t in _normalize(name).split() if len(t) >= 3]
    return any(t in source_lower for t in tokens) if tokens else True


def _redundant_pairs(names: list[str]) -> int:
    """Count name pairs in a section where one normalized name contains the other."""
    norms = sorted({_normalize(n) for n in names if n.strip()}, key=len)
    count = 0
    for i, short in enumerate(norms):
        for long in norms[i + 1 :]:
            if short and short != long and short in long:
                count += 1
    return count


def doc_metrics(conn, doc_id: int) -> DocMetrics:
    doc = conn.execute(
        "SELECT title, source_date, category, thesis, method, result, limitations "
        "FROM documents WHERE id=?",
        (doc_id,),
    ).fetchone()
    title = doc["title"]
    sections = conn.execute(
        "SELECT id, title FROM sections WHERE doc_id=? ORDER BY position", (doc_id,)
    ).fetchall()

    prop_counts: list[int] = []
    non_self_contained = 0
    ungrounded = 0
    redundant = 0
    suspect: Counter[str] = Counter()
    noise_ents = 0
    type_counts: Counter[str] = Counter()
    total_entities = 0

    for s in sections:
        sid = s["id"]
        source = _section_source(conn, sid)

        props = [r["text"] for r in conn.execute(
            "SELECT text FROM propositions WHERE section_id=?", (sid,)
        )]
        prop_counts.append(len(props))
        non_self_contained += sum(1 for p in props if not _is_self_contained(p))
        for p in props:
            reason = rejection_reason(p, s["title"])
            if reason is not None:
                suspect[reason] += 1

        ents = conn.execute(
            "SELECT name, type FROM entities WHERE section_id=?", (sid,)
        ).fetchall()
        total_entities += len(ents)
        for e in ents:
            type_counts[e["type"]] += 1
            if not _is_grounded(e["name"], source):
                ungrounded += 1
            if is_noise(e["name"]):
                noise_ents += 1
        redundant += _redundant_pairs([e["name"] for e in ents])

    total_props = sum(prop_counts)
    return DocMetrics(
        doc_id=doc_id,
        title=title,
        source_date=doc["source_date"],
        category=doc["category"],
        sections=len(sections),
        propositions=total_props,
        entities=total_entities,
        props_per_section_mean=total_props / len(sections) if sections else 0.0,
        props_per_section_max=max(prop_counts) if prop_counts else 0,
        empty_prop_sections=sum(1 for c in prop_counts if c == 0),
        over_fragmented_sections=sum(1 for c in prop_counts if c > OVER_FRAGMENTED),
        non_self_contained_props=non_self_contained,
        ungrounded_entities=ungrounded,
        redundant_entity_pairs=redundant,
        suspect_props=dict(suspect.most_common()),
        noise_entities=noise_ents,
        empty_synthesis=not any(
            (doc[f] or "").strip() for f in ("thesis", "method", "result", "limitations")
        ),
        entity_type_counts=dict(type_counts.most_common()),
    )


def corpus_metrics(conn) -> list[DocMetrics]:
    ids = [r["id"] for r in conn.execute("SELECT id FROM documents ORDER BY id")]
    return [doc_metrics(conn, i) for i in ids]


def format_metrics(docs: list[DocMetrics]) -> str:
    if not docs:
        return "No documents ingested yet."
    lines = []
    for d in docs:
        lines.append(f"[{d.doc_id}] {d.title}")
        lines.append(f"    date {d.source_date or '—'} | category {d.category or '—'}")
        lines.append(
            f"    sections {d.sections} | propositions {d.propositions} "
            f"(mean {d.props_per_section_mean:.1f}/sec, max {d.props_per_section_max}) | "
            f"entities {d.entities}"
        )
        lines.append(
            f"    flags: over-fragmented sections {d.over_fragmented_sections} | "
            f"empty-prop sections {d.empty_prop_sections} | "
            f"non-self-contained props {d.non_self_contained_props} ({d.non_self_contained_pct:.1f}%) | "
            f"ungrounded entities {d.ungrounded_entities} ({d.ungrounded_pct:.1f}%) | "
            f"redundant entity pairs {d.redundant_entity_pairs}"
        )
        suspect = ", ".join(f"{r}:{n}" for r, n in d.suspect_props.items()) or "none"
        lines.append(
            f"    QC: suspect props {sum(d.suspect_props.values())} ({suspect}) | "
            f"noise entities {d.noise_entities} | "
            f"empty synthesis {'YES' if d.empty_synthesis else 'no'}"
        )
        top_types = ", ".join(f"{t}:{n}" for t, n in list(d.entity_type_counts.items())[:6])
        lines.append(f"    entity types: {top_types}")
    if len(docs) > 1:
        lines.append("")
        lines.append(_distribution(docs))
    return "\n".join(lines)


def _distribution(docs: list[DocMetrics]) -> str:
    """Corpus-level category and year distribution (the temporal/category facet overview)."""
    categories = Counter((d.category or "uncategorized") for d in docs)
    years = Counter((d.source_date[:4] if d.source_date else "unknown") for d in docs)
    cat_line = ", ".join(f"{c}:{n}" for c, n in categories.most_common())
    year_line = ", ".join(f"{y}:{n}" for y, n in sorted(years.items()))
    return f"DISTRIBUTION ({len(docs)} docs)\n    by category: {cat_line}\n    by year: {year_line}"
