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
from locus.ingest.figures import rejection_reason as figure_rejection_reason
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
    # Numbers in summaries/propositions that the section source does not attest
    # (unattested_numbers; round-3 numeric-faithfulness carryover). Non-zero entries
    # almost always trace to a degraded math page (check the math-OCR gap flags).
    unattested_numbers: int = 0
    # Gap flags that are real knowledge gaps (audit-trail entries — math-OCR fallbacks,
    # degraded passes — excluded). Liveness signal: the 2026-06-05 evaluation found the gap
    # pass inert, and corpus-wide zero must be loud, not silent.
    semantic_gaps: int = 0
    # The zero-prop sections BY NAME — a bare count hides "the core method section has no
    # propositions" (2026-06-05 evaluation, regime paper §3).
    empty_prop_section_titles: list[str] = field(default_factory=list)
    entity_type_counts: dict[str, int] = field(default_factory=dict)
    # Figures (step 11): captured units + how findable they are. caption_only = the VLM
    # description failed QC at ingest (tier-1 preserve held; tier-2 findability degraded
    # to the caption). unsearchable = neither caption nor description — stored but no
    # vector, invisible to retrieval. suspect_descriptions re-applies the ingest QC
    # predicate to stored rows (figures profile docs only).
    figures: int = 0
    figures_caption_only: int = 0
    figures_unsearchable: int = 0
    suspect_figure_descriptions: int = 0
    # Pages whose math-OCR fell back to the raw text layer (gap audit lines). Counted so a
    # mass OCR failure is LOUD: the 2026-06-06 external audit found 255 OOM'd pages across
    # 14 docs that the audit-line classification had kept out of every headline number.
    ocr_fallback_pages: int = 0

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
# fallback notes, degraded-pass markers, and figure-capture/description degradation
# (ingest_pipeline._prepare/_prepare_doc).
_AUDIT_GAP = re.compile(
    r"^(math-OCR kept original text on |\w+ pass failed for section "
    r"|summary failed grounding for section "
    r"|figure description failed QC for |figure capture degraded: )"
)


def semantic_gaps(flags: list[str]) -> list[str]:
    """The gap_flags entries that are actual knowledge gaps, not pipeline audit lines."""
    return [g for g in flags if not _AUDIT_GAP.match(g)]


# --- numeric attestation (round-3 audit carryover: numeric faithfulness) -------------------
# A number stated in a DERIVED field (summary/proposition) that appears nowhere in the
# section source is either model arithmetic or inherited text-layer damage (the verified
# corpus instance: a degraded math page reading "K\n6 . 77" became "equals K/(6.77)" — true
# value K/6). Full numeric faithfulness is §11.B/C model-measurement scope; this predicate
# makes the failure mode COUNTABLE in audit QC so it never needs a manual re-check again.

# Trailing guard: no word char and no ".<digit>" continuation — but a sentence-final
# period ("...is 0.42.") must not block the match.
_NUMBER = re.compile(r"(?<![\w.])(\d{1,3}(?:,\d{3})+|\d+(?:\.\d+)?)(?!\w)(?!\.\d)")
_VULGAR = {"¼": ".25", "½": ".5", "¾": ".75", "⅓": ".33", "⅔": ".67", "⅕": ".2", "⅛": ".125"}
# "Nov-25" style month-abbreviated two-digit years, which summaries expand to "2025".
# Case-insensitive: doc_metrics feeds lowercased section source.
_YY = re.compile(
    r"\b(?:jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec)[a-z]*[-' ](\d{2})\b", re.IGNORECASE
)
# "11k" / "2.5k" shorthand, which summaries expand to the full number.
_K_SUFFIX = re.compile(r"\b(\d+(?:\.\d+)?)k\b", re.IGNORECASE)
_TRIVIAL = {0.0, 1.0, 2.0, 3.0}  # ordinals / list counts: attestation proves nothing


def _norm_number(n: str) -> str:
    n = n.replace(",", "")  # 24,000 == 24000
    return n.rstrip("0").rstrip(".") if "." in n else n


def _normalized_source(source: str) -> str:
    for glyph, dec in _VULGAR.items():
        source = source.replace(glyph, dec)  # "4¾" -> "4.75"
    # Strip thousands-grouping commas, but only inside a properly-bounded grouped number:
    # a bare "(?<=\d),(?=\d{3})" strip would merge digit LISTS ("{10,50,100,250,500}" ->
    # "50100250500"), destroying attestation for every middle member (live corpus case).
    source = re.sub(
        r"(?<![\d,])\d{1,3}(?:,\d{3})+(?![\d,])", lambda m: m.group(0).replace(",", ""), source
    )  # 24,000 -> 24000
    extra = {f"20{m}" for m in _YY.findall(source)}  # "Nov-25" attests 2025
    extra |= {_norm_number(str(float(m) * 1000)) for m in _K_SUFFIX.findall(source)}  # 11k
    # European decimal comma ("2,5 years" attests 2.5) — single trailing digit only, so
    # thousands groups and digit lists never match.
    extra |= {f"{a}.{b}" for a, b in re.findall(r"(?<!\d)(\d+),(\d)(?!\d)", source)}
    return source + ("\n" + " ".join(sorted(extra)) if extra else "")


def unattested_numbers(derived: str | None, source: str) -> list[str]:
    """Numbers in a derived field that the section source does not attest (see above).

    Attestation is a digit-boundary SUBSTRING search, deliberately lenient: source numbers
    embedded in formula runs ("9ω4", "4.5a") don't tokenize cleanly, and a QC predicate
    that cries wolf on every math-dense section is worse than one that under-counts.
    Trailing source zeros are tolerated ("95.2" attests against "95.20")."""
    if not derived:
        return []
    src = _normalized_source(source)
    out: list[str] = []
    for n in _NUMBER.findall(derived):
        norm = _norm_number(n)
        if float(norm) in _TRIVIAL:
            continue
        if not re.search(rf"(?<!\d){re.escape(norm)}0*(?!\d)", src):
            out.append(n)
    return out


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

    unattested_nums = 0
    for s in sections:
        sid = s["id"]
        source = _section_source(conn, sid)
        corrupted += has_corruption_signature(s["summary"])
        unattested_nums += len(unattested_numbers(s["summary"], source))

        props = [r["text"] for r in conn.execute(
            "SELECT text FROM propositions WHERE section_id=?", (sid,)
        )]
        prop_counts.append(len(props))
        if not props and profile.propositions:
            empty_prop_titles.append(s["title"] or "(untitled)")
        non_self_contained += sum(1 for p in props if not _is_self_contained(p))
        corrupted += sum(1 for p in props if has_corruption_signature(p))
        unattested_nums += sum(len(unattested_numbers(p, source)) for p in props)
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

    fig_rows = conn.execute(
        "SELECT caption, description FROM figures WHERE doc_id=?", (doc_id,)
    ).fetchall()
    figures_caption_only = sum(
        1 for f in fig_rows if not (f["description"] or "").strip() and (f["caption"] or "").strip()
    )
    figures_unsearchable = sum(
        1 for f in fig_rows
        if not (f["description"] or "").strip() and not (f["caption"] or "").strip()
    )
    suspect_fig_descs = sum(
        1 for f in fig_rows
        if (f["description"] or "").strip()
        and figure_rejection_reason(f["description"]) is not None
    )

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
        unattested_numbers=unattested_nums,
        semantic_gaps=len(semantic_gaps(json.loads(doc["gap_flags"] or "[]"))),
        empty_prop_section_titles=empty_prop_titles,
        empty_synthesis=not any(
            (doc[f] or "").strip() for f in ("thesis", "method", "result", "limitations")
        ),
        entity_type_counts=dict(type_counts.most_common()),
        figures=len(fig_rows),
        figures_caption_only=figures_caption_only,
        figures_unsearchable=figures_unsearchable,
        suspect_figure_descriptions=suspect_fig_descs,
        ocr_fallback_pages=sum(
            1 for g in json.loads(doc["gap_flags"] or "[]")
            if g.startswith("math-OCR kept original text on ")
        ),
    )


def corpus_metrics(conn) -> list[DocMetrics]:
    ids = [r["id"] for r in conn.execute("SELECT id FROM documents ORDER BY id")]
    return [doc_metrics(conn, i) for i in ids]


@dataclass
class AliasQC:
    """Corpus-level QC over the entity-alias substrate (step 12, `locus link`)."""

    variants: int
    clusters: int
    nontrivial_clusters: int
    merged_variants: int
    tier_counts: dict[str, int]
    cross_doc_canonicals: int  # canonicals spanning >1 doc — the link payload
    suspicious_merges: int  # llm-tier merges with zero lexical evidence (spot-check these)
    suspicious_examples: list[str]


def alias_qc(conn) -> AliasQC | None:
    """QC the stored entity_aliases table; None when the substrate isn't built yet.

    Suspicious merge = an llm-tier variant sharing no >=4-char token with its canonical
    (both sides having such tokens at all): exactly the merges with no lexical evidence,
    where a model error would hide. Counted + sampled, never auto-reverted — review with
    `locus audit`, fix by re-running `locus link --no-cache` after a prompt fix.
    """
    from locus.link.aliases import _tokens
    from locus.link.related import aliases_built

    if not aliases_built(conn):
        return None
    rows = conn.execute("SELECT * FROM entity_aliases").fetchall()
    tier_counts = Counter(r["tier"] for r in rows)
    cluster_sizes = Counter(r["cluster_id"] for r in rows)
    nontrivial = {cid for cid, n in cluster_sizes.items() if n > 1}

    suspicious: list[str] = []
    for r in rows:
        if r["tier"] != "llm":
            continue
        vt, ct = _tokens(r["variant_name"]), _tokens(r["canonical_name"])
        if vt and ct and not (vt & ct):
            suspicious.append(f"{r['variant_name']!r} -> {r['canonical_name']!r}")

    cross_doc = conn.execute(
        """
        SELECT COUNT(*) FROM (
            SELECT a.canonical_name, a.canonical_type
            FROM entities e
            JOIN entity_aliases a
              ON a.variant_name = e.name AND a.variant_type = e.type
            GROUP BY a.canonical_name, a.canonical_type
            HAVING COUNT(DISTINCT e.doc_id) > 1
        )
        """
    ).fetchone()[0]

    return AliasQC(
        variants=len(rows),
        clusters=len(cluster_sizes),
        nontrivial_clusters=len(nontrivial),
        merged_variants=sum(n for n in cluster_sizes.values() if n > 1),
        tier_counts=dict(tier_counts),
        cross_doc_canonicals=cross_doc,
        suspicious_merges=len(suspicious),
        suspicious_examples=suspicious[:8],
    )


def format_alias_qc(qc: AliasQC | None) -> str:
    if qc is None:
        return "ALIAS SUBSTRATE: not built (run `locus link`)"
    tiers = ", ".join(f"{t}:{n}" for t, n in sorted(qc.tier_counts.items()))
    lines = [
        "ALIAS SUBSTRATE (entity_aliases)",
        f"    variants {qc.variants} | clusters {qc.clusters} "
        f"({qc.nontrivial_clusters} non-trivial, {qc.merged_variants} variants merged)",
        f"    tiers: {tiers}",
        f"    cross-doc canonicals: {qc.cross_doc_canonicals}",
        f"    suspicious merges (llm, zero lexical evidence): {qc.suspicious_merges}",
    ]
    lines.extend(f"      - {ex}" for ex in qc.suspicious_examples)
    return "\n".join(lines)


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
            f"unattested numbers {d.unattested_numbers} | "
            f"semantic gaps {d.semantic_gaps} | "
            f"OCR-fallback pages {d.ocr_fallback_pages}"
        )
        top_types = ", ".join(f"{t}:{n}" for t, n in list(d.entity_type_counts.items())[:6])
        lines.append(f"    entity types: {top_types}")
        if d.figures:
            lines.append(
                f"    figures: {d.figures} | caption-only {d.figures_caption_only} | "
                f"unsearchable {d.figures_unsearchable} | "
                f"suspect descriptions {d.suspect_figure_descriptions}"
            )
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
        ocr_fb = sum(d.ocr_fallback_pages for d in docs)
        ocr_docs = sum(1 for d in docs if d.ocr_fallback_pages > 0)
        lines.append(f"    OCR fallbacks: {ocr_fb} page(s) across {ocr_docs} doc(s)")
        if ocr_fb > 20:
            lines.append(
                "    WARNING: heavy OCR fallback — the math-OCR engine is likely failing "
                "systematically (2026-06-06 audit failure mode: VRAM not evicted before GOT)."
            )
    return "\n".join(lines)


def _distribution(docs: list[DocMetrics]) -> str:
    """Corpus-level category and year distribution (the temporal/category facet overview)."""
    categories = Counter((d.category or "uncategorized") for d in docs)
    years = Counter((d.source_date[:4] if d.source_date else "unknown") for d in docs)
    cat_line = ", ".join(f"{c}:{n}" for c, n in categories.most_common())
    year_line = ", ".join(f"{y}:{n}" for y, n in sorted(years.items()))
    return f"DISTRIBUTION ({len(docs)} docs)\n    by category: {cat_line}\n    by year: {year_line}"
