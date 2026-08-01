"""Cross-document entity-alias resolution — the link substrate (CLAUDE.md §15.4, step 12).

Collapses surface variants of one real entity ("Bode Diagram"/"Bode diagram",
"Kullback-Leibler (KL) divergence"/"KL divergence", "fourier transform" stored under three
types) onto one canonical `(name, type)`, so cross-document entity co-occurrence — the
substrate of the "link" primary use (§1) — connects documents instead of fragmenting.

Tiers, in order of evidence strength:
  deterministic — casefold, punct/hyphen, attested acronym-expansion, attested cross-doc
    plural. Same-type only; merge on hard evidence; no inference.
  llm — remaining lookalike clusters (blocked by name-embedding cosine AND token overlap)
    adjudicated by the Claude API (adjudicate.py). Cross-type merges happen only here.

Output is the `entity_aliases` table (migration 0008): DERIVED + REGENERABLE, a TOTAL
mapping (singletons map to themselves, tier='identity'), rebuilt by delete + recompute.
The `entities` table is never mutated — per-section provenance is the ground truth.

Hard guards override any LLM verdict (the adjudicator proposes, this module disposes):
  - names shorter than `min_merge_len` never merge (homonym risk: 'var', 'P2', 'MA');
  - two surfaces co-occurring as distinct rows in the SAME section never merge — the
    author treated them as distinct (strongest backstop against theme-mate fusion);
  - canonical surfaces are snapped to an actual cluster member — never an invented string;
  - entities appearing only on code documents are excluded from clustering entirely
    (AST identifiers are exact; they still get identity rows for join totality).
"""

from __future__ import annotations

import hashlib
import re
import sqlite3
import time
from collections import defaultdict
from dataclasses import dataclass, field
from typing import Callable

from locus.config import load
from locus.ingest.embed import embed_texts
from locus.ingest.entities import normalize_name
from locus.link import adjudicate
from locus.link.adjudicate import AliasVerdict, ClusterMember

# Tier labels (also the CHECK vocabulary in migration 0008).
IDENTITY, CASEFOLD, PUNCT, ACRONYM, PLURAL, LLM = (
    "identity", "casefold", "punct", "acronym", "plural", "llm",
)

# "Expansion words (ACRO) trailing words" — the acronym must look like one (all-caps+digits,
# mirroring entities._ACRONYM), so "(see below)" never parses as an acronym.
_PAREN_ACRO = re.compile(r"^(.*?)\s*\(([A-Z][A-Z0-9]{1,9})\)\s*(.*)$")
_TOKEN = re.compile(r"[a-z0-9]{4,}")


@dataclass
class _Node:
    """One distinct stored entity identity `(name, type)` with its corpus evidence."""

    name: str
    type: str
    docs: set[int] = field(default_factory=set)
    sections: set[int] = field(default_factory=set)
    titles: list[str] = field(default_factory=list)  # up to 2 doc titles (LLM context)
    code_only: bool = True  # True until seen on any non-code document

    @property
    def doc_freq(self) -> int:
        return len(self.docs)


@dataclass
class AliasBuildReport:
    total_identities: int = 0
    clusters: int = 0
    nontrivial_clusters: int = 0
    merged_variants: int = 0  # nodes living in nontrivial clusters
    tier_counts: dict[str, int] = field(default_factory=dict)
    llm_candidate_clusters: int = 0
    llm_calls: int = 0
    cache_hits: int = 0
    oversize_skipped: int = 0
    oversize_chunked: int = 0  # oversize clusters adjudicated in pieces rather than dropped
    guard_splits: int = 0  # LLM groups rejected/split by hard guards
    cross_doc_canonicals: int = 0  # canonicals spanning >1 document (the link payload)

    def format(self) -> str:
        tiers = ", ".join(f"{t} {n}" for t, n in sorted(self.tier_counts.items())) or "(none)"
        return (
            f"identities {self.total_identities} -> clusters {self.clusters} "
            f"({self.nontrivial_clusters} non-trivial, {self.merged_variants} variants merged)\n"
            f"  tiers: {tiers}\n"
            f"  llm: {self.llm_candidate_clusters} candidate clusters, {self.llm_calls} API "
            f"calls, {self.cache_hits} cache hits, {self.oversize_chunked} oversize chunked, "
            f"{self.oversize_skipped} oversize skipped, "
            f"{self.guard_splits} guard splits\n"
            f"  cross-doc canonicals: {self.cross_doc_canonicals}"
        )


def _chunk_cluster(
    cluster: list[int], nodes, size: int, max_chunks: int
) -> list[list[int]]:
    """Split an oversize cluster into adjudication-sized pieces, or [] if it is too large.

    Sorted by normalised name so that surfaces differing only in case, punctuation or a dash sit
    adjacent and therefore land in the SAME chunk — chunking an unsorted cluster would scatter the
    variants it exists to merge.
    """
    if size <= 0 or len(cluster) > size * max_chunks:
        return []
    ordered = sorted(
        cluster,
        key=lambda i: nodes[i].name.casefold().replace("-", " ").replace("\u2013", " "),
    )
    return [ordered[i : i + size] for i in range(0, len(ordered), size)]


class _UnionFind:
    def __init__(self, n: int):
        self.parent = list(range(n))

    def find(self, i: int) -> int:
        while self.parent[i] != i:
            self.parent[i] = self.parent[self.parent[i]]
            i = self.parent[i]
        return i

    def union(self, a: int, b: int) -> bool:
        ra, rb = self.find(a), self.find(b)
        if ra == rb:
            return False
        self.parent[rb] = ra
        return True


# --- keys & helpers -------------------------------------------------------------------------


def _casefold_key(name: str) -> str:
    return normalize_name(name).lower()


def _punct_key(name: str) -> str | None:
    """Punctuation-insensitive key: hyphens/slashes/commas/periods -> space, apostrophes
    dropped, whitespace collapsed. None when no >=4-letter word remains (too little signal
    for a punctuation-only merge)."""
    s = name.lower().replace("'", "").replace("’", "")
    s = re.sub(r"[-–—/_,.]", " ", s)
    s = re.sub(r"\s+", " ", s).strip()
    return s if re.search(r"[a-z]{4,}", s) else None


def _tokens(name: str) -> frozenset[str]:
    """>=4-char tokens, plural-insensitive (trailing 's' stripped) — the Jaccard guard basis."""
    return frozenset(t.rstrip("s") or t for t in _TOKEN.findall(name.lower()))


def _jaccard(a: frozenset[str], b: frozenset[str]) -> float:
    if not a or not b:
        return 0.0
    return len(a & b) / len(a | b)


def pick_canonical_idx(indices: list[int], nodes: list[_Node]) -> int:
    """Deterministic canonical pick: most doc-attested surface, tie -> shortest, then alpha."""
    return min(
        indices,
        key=lambda i: (-nodes[i].doc_freq, len(nodes[i].name), nodes[i].name.lower(), nodes[i].type),
    )


# --- corpus loading -------------------------------------------------------------------------


def _load_nodes(conn: sqlite3.Connection) -> list[_Node]:
    by_key: dict[tuple[str, str], _Node] = {}
    for r in conn.execute(
        "SELECT e.name, e.type, e.doc_id, e.section_id, d.source_type, d.title "
        "FROM entities e JOIN documents d ON d.id = e.doc_id"
    ):
        key = (r["name"], r["type"])
        node = by_key.get(key)
        if node is None:
            node = by_key[key] = _Node(name=r["name"], type=r["type"])
        node.docs.add(r["doc_id"])
        if r["section_id"] is not None:
            node.sections.add(r["section_id"])
        if r["source_type"] != "code":
            node.code_only = False
        title = r["title"] or ""
        if title and title not in node.titles and len(node.titles) < 2:
            node.titles.append(title)
    return list(by_key.values())


# --- deterministic tiers --------------------------------------------------------------------


def _run_deterministic_tiers(
    nodes: list[_Node],
    eligible: list[int],
    uf: _UnionFind,
    tiers: dict[int, str],
    min_merge_len: int,
) -> None:
    """Casefold/punct/acronym/plural unions over eligible node indices. Mutates uf + tiers."""

    def mark(a: int, b: int, tier: str) -> None:
        if uf.union(a, b):
            tiers.setdefault(a, tier)
            tiers.setdefault(b, tier)

    # Index: (casefold_key, type) -> node indices. Also the attestation lookup for the
    # acronym/plural tiers (case-insensitive, same-type — cross-type merges are LLM-only).
    cf_index: dict[tuple[str, str], list[int]] = defaultdict(list)
    for i in eligible:
        cf_index[(_casefold_key(nodes[i].name), nodes[i].type)].append(i)

    # casefold: identical case-folded surface, same type. Short names are exempt FROM
    # merging (not from the index): case carries meaning at acronym length ('VaR' vs 'var').
    for (key, _), idxs in cf_index.items():
        if len(idxs) < 2 or len(key) < min_merge_len:
            continue
        for j in idxs[1:]:
            mark(idxs[0], j, CASEFOLD)

    # punct/hyphen: identical punctuation-insensitive surface, same type.
    p_index: dict[tuple[str, str], list[int]] = defaultdict(list)
    for i in eligible:
        pk = _punct_key(nodes[i].name)
        if pk and len(pk) >= min_merge_len:
            p_index[(pk, nodes[i].type)].append(i)
    for idxs in p_index.values():
        for j in idxs[1:]:
            mark(idxs[0], j, PUNCT)

    # acronym-expansion: "Expansion (ACRO) tail" links to attested same-type surfaces
    # derived from its own structure — the bare acronym, acronym+tail, and the expansion
    # without the parenthetical — each also tried with a naive plural/singular variant
    # ("LTI models" matches an attested "LTI model"). Evidence-based: only attested
    # surfaces link; nothing is invented. Exempt from min_merge_len — the parenthesised
    # acronym in the source IS the evidence ('LTI' is 3 chars and legitimate here).
    for i in eligible:
        m = _PAREN_ACRO.match(nodes[i].name)
        if not m:
            continue
        prefix, acro, tail = m.group(1).strip(), m.group(2), m.group(3).strip()
        if not prefix:  # "(ACRO) tail" with no expansion — not the pattern
            continue
        surfaces = {acro, f"{acro} {tail}".strip(), f"{prefix} {tail}".strip()}
        lookups: set[str] = set()
        for s in surfaces:
            lookups.add(s)
            lookups.add(s + "s")
            if s.endswith("es"):
                lookups.add(s[:-2])
            if s.endswith("s"):
                lookups.add(s[:-1])
        for s in lookups:
            for j in cf_index.get((_casefold_key(s), nodes[i].type), []):
                if j != i:
                    mark(i, j, ACRONYM)

    # cross-doc plural: 'Xs'/'Xes' -> attested same-type 'X' (the within-doc
    # merge_plural_variants contract, lifted corpus-wide).
    for i in eligible:
        name = nodes[i].name
        if len(name) < min_merge_len:
            continue
        for suffix in ("es", "s"):
            if not name.endswith(suffix):
                continue
            singular = name[: -len(suffix)]
            if len(singular) >= min_merge_len:
                for j in cf_index.get((_casefold_key(singular), nodes[i].type), []):
                    if j != i:
                        mark(i, j, PLURAL)
            break  # longest matching suffix only


# --- typo-class candidate edges (round-5 audit: PCMCI vs PCMIC never reached the LLM) -------


def _digits(name: str) -> str:
    return "".join(c for c in name if c.isdigit())


def _typo_edges(rep_indices: list[int], nodes: list[_Node]) -> set[tuple[int, int]]:
    """Same-type pairs within Damerau-Levenshtein distance ~1 (substitution, single
    insert/delete, adjacent transposition) — the typo class ('PCMCI'/'PCMIC',
    'BinSeg'/'BiaSeg') that BOTH blocking guards miss: single-token names have
    token-Jaccard 0 between variants, and an embedder may or may not pull them close.

    Found via deletion-variant hashing (SymSpell trick): two names are within the band
    iff one is in the other's single-deletion set, or their deletion sets intersect.
    Slight over-generation beyond Damerau-1 is fine — every edge still goes to the LLM
    for adjudication and through the hard guards; nothing here auto-merges.

    Guards: same type only; both names >= 5 chars (acronym homonyms are one substitution
    apart by chance); DIGIT-SEQUENCE EQUALITY — 'MSH(2)' and 'MSH(20)' are one edit apart
    and are DIFFERENT models, so any pair whose digits differ is rejected outright.

    Returns LOCAL index pairs (a, b), a < b, into rep_indices.
    """
    folded = [_casefold_key(nodes[i].name) for i in rep_indices]
    buckets: dict[str, list[int]] = defaultdict(list)
    for li, name in enumerate(folded):
        if len(name) < 5:
            continue
        for v in {name} | {name[:k] + name[k + 1 :] for k in range(len(name))}:
            buckets[v].append(li)
    edges: set[tuple[int, int]] = set()
    for lis in buckets.values():
        if len(lis) < 2:
            continue
        for x in range(len(lis)):
            for y in range(x + 1, len(lis)):
                a, b = lis[x], lis[y]
                if a == b or folded[a] == folded[b]:
                    continue  # identical surfaces are the casefold tier's business
                na, nb = nodes[rep_indices[a]], nodes[rep_indices[b]]
                if na.type != nb.type:
                    continue
                if _digits(na.name) != _digits(nb.name):
                    continue  # MSH(2) vs MSH(20): one edit, different models
                edges.add((a, b) if a < b else (b, a))
    return edges


# --- embedding-blocked candidate clusters ---------------------------------------------------


def _candidate_clusters(
    rep_indices: list[int],
    nodes: list[_Node],
    *,
    block_threshold: float,
    min_token_overlap: float,
    embed_fn: Callable[[list[str]], list[list[float]]],
) -> list[list[int]]:
    """Group deterministic-cluster representatives into lookalike components.

    An edge needs BOTH embedding cosine >= block_threshold AND token-Jaccard >=
    min_token_overlap — identical strings under different types pass trivially (1.0/1.0);
    theme-mates ("Kalman filter"/"particle filter") fail the token guard. Typo-class
    edges (_typo_edges) are unioned in besides — they fail the token guard by
    construction. Similarities are computed in row blocks so memory stays bounded
    post-pour.
    """
    import numpy as np

    if len(rep_indices) < 2:
        return []
    names = [nodes[i].name for i in rep_indices]
    distinct = sorted(set(names))
    vec_by_name = dict(zip(distinct, embed_fn(distinct)))
    mat = np.asarray([vec_by_name[n] for n in names], dtype=np.float32)
    # nomic vectors are unit-normalised (cosine == dot), but normalise defensively:
    # a test-injected embed_fn need not be.
    norms = np.linalg.norm(mat, axis=1, keepdims=True)
    norms[norms == 0] = 1.0
    mat /= norms
    toks = [_tokens(n) for n in names]

    uf = _UnionFind(len(rep_indices))
    block = 512
    for start in range(0, len(names), block):
        sims = mat[start : start + block] @ mat.T
        for bi, gj in zip(*np.nonzero(sims >= block_threshold)):
            a, b = start + int(bi), int(gj)
            if a >= b:  # upper triangle only (dedupe + skip self)
                continue
            if _jaccard(toks[a], toks[b]) >= min_token_overlap:
                uf.union(a, b)

    for a, b in _typo_edges(rep_indices, nodes):
        uf.union(a, b)

    comps: dict[int, list[int]] = defaultdict(list)
    for local, rep_idx in enumerate(rep_indices):
        comps[uf.find(local)].append(rep_idx)
    return [c for c in comps.values() if len(c) > 1]


# --- LLM adjudication + guards ---------------------------------------------------------------


def _verdict_cache_key(members: list[_Node], model: str) -> str:
    raw = (
        "\x1f".join(sorted(f"{m.name}\t{m.type}" for m in members))
        + f":alias:{model}:{adjudicate.PROMPT_VERSION}"
    )
    return hashlib.sha256(raw.encode()).hexdigest()


def _apply_guards(
    verdict: AliasVerdict,
    members: list[_Node],
    member_sections: list[set[int]],
    min_merge_len: int,
) -> tuple[list[tuple[list[int], str, str]], int]:
    """Validate an LLM verdict against the hard guards.

    Returns (groups, splits): each group is (member_index_list, canonical_name,
    canonical_type) with len >= 2 and the canonical snapped to an actual member surface.
    Guard failures split the offending group back to singletons — conservative: the
    deterministic clusters stand, only the LLM merge is rejected.
    """
    splits = 0
    out: list[tuple[list[int], str, str]] = []
    claimed: set[int] = set()
    for g in verdict.groups:
        idxs = [i for i in g.member_indices if 0 <= i < len(members) and i not in claimed]
        claimed.update(idxs)
        if len(idxs) < 2:
            continue
        # min length: short names never merge into a different surface.
        if any(len(members[i].name) < min_merge_len for i in idxs):
            splits += 1
            continue
        # co-occurrence: surfaces sharing a section are distinct by authorial evidence.
        cooccur = any(
            member_sections[a] & member_sections[b]
            for k, a in enumerate(idxs)
            for b in idxs[k + 1 :]
        )
        if cooccur:
            splits += 1
            continue
        # Snap the canonical to an actual member surface (never an invented string).
        snapped = next(
            (members[i] for i in idxs if members[i].name.lower() == g.canonical_name.lower()),
            None,
        )
        if snapped is not None:
            canon_name, canon_type = snapped.name, str(g.canonical_type)
        else:
            best_local = pick_canonical_idx(idxs, members)  # members is index-aligned
            canon_name, canon_type = members[best_local].name, members[best_local].type
        out.append((idxs, canon_name, canon_type))
    return out, splits


# --- the pass --------------------------------------------------------------------------------


def build_aliases(
    conn: sqlite3.Connection,
    *,
    use_llm: bool | None = None,
    use_cache: bool = True,
    runner=None,
    model: str | None = None,
    embed_fn: Callable[[list[str]], list[list[float]]] = embed_texts,
    log: Callable[[str], None] = lambda _msg: None,
) -> AliasBuildReport:
    """Rebuild the entity_aliases table from the stored entities. Regenerable; one transaction.

    `use_llm=None` follows config. `runner`/`embed_fn` are injectable for tests (`runner` is the
    headless-`claude -p` adjudication callable). LLM verdicts are cached in pass_cache keyed on
    cluster content + model + PROMPT_VERSION, so re-runs after new ingests only adjudicate
    new/changed clusters; `use_cache=False` re-adjudicates.
    """
    cfg = load().alias
    if use_llm is None:
        use_llm = cfg.use_llm
    report = AliasBuildReport()

    nodes = _load_nodes(conn)
    report.total_identities = len(nodes)
    if not nodes:
        with conn:
            conn.execute("DELETE FROM entity_aliases")
        return report

    eligible = [i for i, n in enumerate(nodes) if not n.code_only]
    uf = _UnionFind(len(nodes))
    tiers: dict[int, str] = {}

    _run_deterministic_tiers(nodes, eligible, uf, tiers, cfg.min_merge_len)
    log(f"deterministic tiers done over {len(eligible)} eligible identities")

    # Deterministic components (eligible only); one representative each enters blocking.
    comps: dict[int, list[int]] = defaultdict(list)
    for i in eligible:
        comps[uf.find(i)].append(i)
    rep_to_component = {
        pick_canonical_idx(members, nodes): members for members in comps.values()
    }
    rep_indices = list(rep_to_component)

    # Per-final-cluster canonical overrides decided by the LLM, keyed by union-find root.
    llm_canonical: dict[int, tuple[str, str]] = {}

    if use_llm:
        candidates = _candidate_clusters(
            rep_indices, nodes,
            block_threshold=cfg.block_threshold,
            min_token_overlap=cfg.min_token_overlap,
            embed_fn=embed_fn,
        )
        report.llm_candidate_clusters = len(candidates)
        gen_model = model or load().generation.model
        last_api_call = 0.0  # monotonic time of the previous adjudication call
        # OVERSIZE CLUSTERS ARE CHUNKED, NOT DROPPED.
        #
        # `max_cluster_size` is a COST guard — it bounds how much goes into one adjudication
        # prompt — but it was being applied as a judgement: anything larger was skipped entirely
        # and never adjudicated at all. Measured on the 2026-08-01 rebuild, that silently dropped
        # real concepts alongside the junk it was aimed at:
        #
        #   Fama and French / Fama-French factors / three-factor model / five-factor model  (9)
        #   Portfolio Optimisation / portfolio optimization / portfolio construction        (11)
        #
        # both skipped, while `portfolio construction` is simultaneously one of the live search
        # terms driving reading discovery — so the fragmentation propagated outward.
        #
        # Splitting preserves what the guard is actually for. Each chunk is a bounded prompt, so
        # cost per call is unchanged; members are sorted by normalised name first so near-
        # identical surfaces land in the same chunk rather than being separated arbitrarily. A
        # cluster too large even to chunk (`Laplace transform of ...`, 48 surfaces) is still
        # skipped, because at that size it is a topic rather than a concept.
        units: list[list[int]] = []
        for cluster in candidates:
            if len(cluster) <= cfg.max_cluster_size:
                units.append(cluster)
                continue
            chunks = _chunk_cluster(cluster, nodes, cfg.max_cluster_size, cfg.max_cluster_chunks)
            preview = ", ".join(nodes[i].name for i in cluster[:6])
            if not chunks:
                report.oversize_skipped += 1
                log(f"oversize cluster skipped ({len(cluster)} reps): {preview}, ...")
                continue
            report.oversize_chunked += 1
            log(f"oversize cluster chunked ({len(cluster)} reps -> {len(chunks)}): {preview}, ...")
            units.extend(chunks)

        for cluster in units:
            members = [nodes[i] for i in cluster]
            key = _verdict_cache_key(members, gen_model)
            verdict: AliasVerdict | None = None
            if use_cache:
                row = conn.execute(
                    "SELECT payload FROM pass_cache WHERE key=?", (key,)
                ).fetchone()
                if row:
                    verdict = AliasVerdict.model_validate_json(row["payload"])
                    report.cache_hits += 1
            if verdict is None:
                # Throttle: space adjudication calls so a full rebuild (hundreds of clusters)
                # stays under the subscription's rate limit. Cache hits skip this entirely.
                if cfg.api_call_interval > 0 and last_api_call:
                    wait = cfg.api_call_interval - (time.monotonic() - last_api_call)
                    if wait > 0:
                        time.sleep(wait)
                last_api_call = time.monotonic()
                verdict = adjudicate.adjudicate_cluster(
                    [ClusterMember(m.name, m.type, tuple(m.titles)) for m in members],
                    runner=runner, model=gen_model,
                )
                report.llm_calls += 1
                # Persist this verdict immediately, in its own commit, so a crash partway
                # through a rebuild never discards the adjudications already made — a re-run
                # resumes from the cache instead of repeating them (2026-06-09: was a single
                # executemany flushed only at the very end of the loop).
                with conn:
                    conn.execute(
                        "INSERT OR REPLACE INTO pass_cache (key, payload) VALUES (?,?)",
                        (key, verdict.model_dump_json()),
                    )
            # Members are deterministic-component REPRESENTATIVES; the co-occurrence guard
            # must see each component's FULL section set, not just the rep's.
            member_sections = [
                set().union(*(nodes[j].sections for j in rep_to_component[rep]))
                for rep in cluster
            ]
            groups, splits = _apply_guards(verdict, members, member_sections, cfg.min_merge_len)
            report.guard_splits += splits
            for idxs, canon_name, canon_type in groups:
                first = cluster[idxs[0]]
                for k in idxs[1:]:
                    if uf.union(first, cluster[k]):
                        tiers.setdefault(cluster[k], LLM)
                        tiers.setdefault(first, LLM)
                llm_canonical[uf.find(first)] = (canon_name, canon_type)

    # --- finalise clusters + write ------------------------------------------------------------
    final: dict[int, list[int]] = defaultdict(list)
    for i in range(len(nodes)):
        final[uf.find(i)].append(i)

    rows: list[tuple[str, str, str, str, int, str]] = []
    # Deterministic cluster ordering -> stable cluster_ids across rebuilds.
    ordered = sorted(
        final.values(),
        key=lambda c: min((nodes[i].name.lower(), nodes[i].type) for i in c),
    )
    for cluster_id, members in enumerate(ordered, start=1):
        root = uf.find(members[0])
        if root in llm_canonical:
            canon_name, canon_type = llm_canonical[root]
        else:
            best = pick_canonical_idx(members, nodes)
            canon_name, canon_type = nodes[best].name, nodes[best].type
        if len(members) > 1:
            report.nontrivial_clusters += 1
            report.merged_variants += len(members)
        cluster_docs: set[int] = set()
        for i in members:
            tier = tiers.get(i, IDENTITY) if len(members) > 1 else IDENTITY
            report.tier_counts[tier] = report.tier_counts.get(tier, 0) + 1
            rows.append((nodes[i].name, nodes[i].type, canon_name, canon_type, cluster_id, tier))
            cluster_docs.update(nodes[i].docs)
        if len(cluster_docs) > 1:
            report.cross_doc_canonicals += 1

    report.clusters = len(ordered)

    with conn:
        conn.execute("DELETE FROM entity_aliases")
        conn.executemany(
            "INSERT INTO entity_aliases "
            "(variant_name, variant_type, canonical_name, canonical_type, cluster_id, tier) "
            "VALUES (?,?,?,?,?,?)",
            rows,
        )
    log(report.format())
    return report
