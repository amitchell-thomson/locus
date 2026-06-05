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

import json
import re
from collections import Counter
from dataclasses import dataclass, field

from locus.ingest.entities import is_grounded, is_noise
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
    # LLM-generated fields (synthesis/summaries/propositions) carrying the JSON-escape LaTeX
    # corruption signature (has_corruption_signature). Must be 0 on a post-sanitizer ingest.
    corrupted_fields: int = 0
    # Gap flags that are real knowledge gaps (audit-trail entries — math-OCR fallbacks,
    # degraded passes — excluded). Liveness signal: the 2026-06-05 evaluation found the gap
    # pass inert, and corpus-wide zero must be loud, not silent.
    semantic_gaps: int = 0
    # The zero-prop sections BY NAME — a bare count hides "the core method section has no
    # propositions" (2026-06-05 evaluation, regime paper §3).
    empty_prop_section_titles: list[str] = field(default_factory=list)
    entity_type_counts: dict[str, int] = field(default_factory=dict)

    @property
    def non_self_contained_pct(self) -> float:
        return 100.0 * self.non_self_contained_props / self.propositions if self.propositions else 0.0

    @property
    def ungrounded_pct(self) -> float:
        return 100.0 * self.ungrounded_entities / self.entities if self.entities else 0.0


def has_corruption_signature(text: str | None) -> bool:
    """Detect the JSON-escape LaTeX corruption signature in an LLM-generated field.

    Unescaped LaTeX in model JSON turned `\\tau`/`\\frac`/`\\beta` into control chars at parse
    time (2026-06-05 evaluation; fixed by llm._sanitize_latex_escapes). Stored text showing
    (a) any control char that isn't a real newline/tab/CR, or (b) a TAB immediately followed
    by letters (the TAB+`au` residue of `\\tau`) carries that signature. Newline-based corruption
    (`\\nu` -> LF+`u`) is not separately matched: a bare LF before a word is legitimate text,
    and corrupt docs always carry the other signatures too.
    """
    if not text:
        return False
    if any(ord(c) < 32 and c not in "\n\t\r" for c in text):
        return True
    return bool(re.search(r"\t[a-z]{2,}", text))


# Audit-trail gap_flags written by the pipeline itself (not knowledge gaps): math-OCR
# fallback notes and degraded-pass markers (ingest_pipeline._prepare).
_AUDIT_GAP = re.compile(r"^(math-OCR kept original text on |\w+ pass failed for section )")


def semantic_gaps(flags: list[str]) -> list[str]:
    """The gap_flags entries that are actual knowledge gaps, not pipeline audit lines."""
    return [g for g in flags if not _AUDIT_GAP.match(g)]


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
        "SELECT title, source_type, source_date, category, thesis, method, result, "
        "limitations, gap_flags FROM documents WHERE id=?",
        (doc_id,),
    ).fetchone()
    # Mirror the ingest pass profile: code docs skip propositions by design and carry
    # exact AST entities (function names differing by a trailing 's' are distinct
    # functions, not redundant variants) — those predicates would cry wolf on every repo.
    from locus.ingest_pipeline import pass_profile

    profile = pass_profile(doc["source_type"])
    title = doc["title"]
    sections = conn.execute(
        "SELECT id, title, summary FROM sections WHERE doc_id=? ORDER BY position", (doc_id,)
    ).fetchall()

    prop_counts: list[int] = []
    empty_prop_titles: list[str] = []
    non_self_contained = 0
    ungrounded = 0
    redundant = 0
    suspect: Counter[str] = Counter()
    noise_ents = 0
    type_counts: Counter[str] = Counter()
    total_entities = 0
    corrupted = sum(
        1 for f in ("thesis", "method", "result", "limitations")
        if has_corruption_signature(doc[f])
    )

    for s in sections:
        sid = s["id"]
        source = _section_source(conn, sid)
        corrupted += has_corruption_signature(s["summary"])

        props = [r["text"] for r in conn.execute(
            "SELECT text FROM propositions WHERE section_id=?", (sid,)
        )]
        prop_counts.append(len(props))
        if not props and profile.propositions:
            empty_prop_titles.append(s["title"] or "(untitled)")
        non_self_contained += sum(1 for p in props if not _is_self_contained(p))
        corrupted += sum(1 for p in props if has_corruption_signature(p))
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
            if not is_grounded(e["name"], source):
                ungrounded += 1
            if is_noise(e["name"]):
                noise_ents += 1
        if profile.llm_entities:
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
        empty_prop_sections=(
            sum(1 for c in prop_counts if c == 0) if profile.propositions else 0
        ),
        over_fragmented_sections=sum(1 for c in prop_counts if c > OVER_FRAGMENTED),
        non_self_contained_props=non_self_contained,
        ungrounded_entities=ungrounded,
        redundant_entity_pairs=redundant,
        suspect_props=dict(suspect.most_common()),
        noise_entities=noise_ents,
        corrupted_fields=corrupted,
        semantic_gaps=len(semantic_gaps(json.loads(doc["gap_flags"] or "[]"))),
        empty_prop_section_titles=empty_prop_titles,
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
            f"empty synthesis {'YES' if d.empty_synthesis else 'no'} | "
            f"corrupted fields {d.corrupted_fields} | "
            f"semantic gaps {d.semantic_gaps}"
        )
        top_types = ", ".join(f"{t}:{n}" for t, n in list(d.entity_type_counts.items())[:6])
        lines.append(f"    entity types: {top_types}")
        if d.empty_prop_section_titles:
            shown = d.empty_prop_section_titles[:8]
            more = len(d.empty_prop_section_titles) - len(shown)
            lines.append(
                "    zero-prop sections: "
                + "; ".join(shown)
                + (f" (+{more} more)" if more > 0 else "")
            )
    if len(docs) > 1:
        lines.append("")
        lines.append(_distribution(docs))
        with_gaps = sum(1 for d in docs if d.semantic_gaps > 0)
        lines.append(f"    gap liveness: {with_gaps}/{len(docs)} docs with >=1 semantic gap")
        if with_gaps == 0:
            lines.append(
                "    WARNING: zero semantic gaps corpus-wide — the gap-flagging pass "
                "looks inert (2026-06-05 evaluation failure mode)."
            )
    return "\n".join(lines)


def _distribution(docs: list[DocMetrics]) -> str:
    """Corpus-level category and year distribution (the temporal/category facet overview)."""
    categories = Counter((d.category or "uncategorized") for d in docs)
    years = Counter((d.source_date[:4] if d.source_date else "unknown") for d in docs)
    cat_line = ", ".join(f"{c}:{n}" for c, n in categories.most_common())
    year_line = ", ".join(f"{y}:{n}" for y, n in sorted(years.items()))
    return f"DISTRIBUTION ({len(docs)} docs)\n    by category: {cat_line}\n    by year: {year_line}"
