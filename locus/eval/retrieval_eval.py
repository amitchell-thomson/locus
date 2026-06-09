"""Labelled retrieval eval (build step 7): recall@k + MRR over a fixed query set.

A retrieval miss is NOT Claude-recoverable (§15.0) — this is the definitive check that the
right content surfaces. Queries are labelled by **source_uri substring** (the 2026-06-09
re-curation): titles are re-arbitrated on every re-ingest and were wholesale rewritten by
`locus retitle`, which silently broke every title-substring label; `source_uri` is stable
across both retitle AND re-ingest (and, unlike the numeric id, survives a code-repo's
delete+rewrite on each commit). Stable keys in practice: arxiv ids for papers
(`2605.30363v1`), repo paths for code (`regime-conditioned-equity-ml`), and the **module
folder** for coursework (`Heat and Mass Transfer/`) — the pour created dense topic clusters
(16 heat-transfer, 23 control docs), so the right granularity for "does the corpus cover
this topic" is the module, not one brittle file. Two checks beyond plain recall (2026-06-05
round-3 evaluation, which found the eval set stale at 24 docs and structurally blind to two
live regressions):

  - `expected_paths`: file-level targets for code documents. Doc-level recall is satisfied
    by ANY unit of the repo doc, so a code query that returns prose-about-code instead of
    the source file scores 1.0 — these labels make "the source file itself surfaces" a
    measured property (the hallucinated-summary defect lived exactly there).
  - `cross_domain` queries assert the LOW CONFIDENCE banner does NOT fire: recall@k alone
    scored the floor-misfire rounds as passing while every user saw a warning banner.

Pure scoring is separated from pipeline execution so it is unit-testable without models.
"""

from __future__ import annotations

from dataclasses import dataclass, field


@dataclass
class LabelledQuery:
    query: str
    # source_uri substrings (case-insensitive); each entry is one expected doc. An entry may
    # carry '|'-separated alternatives ("2605.30943v1|2605.30363v1") satisfied by ANY match —
    # for queries where the corpus holds several legitimately substitutable answers and
    # ranking between them is model choice, not retrieval quality. Coursework entries use the
    # module-folder fragment ("Time Frequency Analysis/"), so any lecture in the module counts.
    expected: list[str]
    expected_paths: list[str] = field(default_factory=list)  # file-path substrings (code)
    cross_domain: bool = False
    # Run this query with the production self-exclusion lifted (retrieve(include_excluded=True)).
    # Only the query about Locus's OWN source needs it: doc 217 (the locus repo) is excluded
    # from production retrieval, so the labelled target is unreachable otherwise — and a user
    # asking about Locus's internals is exactly when they'd set the flag. Everything else stays
    # False so the eval measures the production pool.
    include_excluded: bool = False


# Grounded in the live post-pour corpus (306 docs; 2026-06-09 re-curation). Keys are
# source_uri substrings — see the module docstring. EXTEND whenever the corpus grows.
LABELLED_QUERIES: list[LabelledQuery] = [
    LabelledQuery(
        "What is the Biot number and what does it tell you about transient conduction?",
        ["Heat and Mass Transfer/"],  # conduction/Biot lives in L2 of the module
    ),
    LabelledQuery(
        "How is Fourier analysis used both for signals and for solving PDEs?",
        ["Partial Differential Equations/", "Time Frequency Analysis/"],
    ),
    LabelledQuery(
        "How do regime-switching models in finance relate to state-space models in control theory?",
        # Finance side: either regime paper (Neural Markov / treasury Regime Shift). Control
        # side: any control-theory doc ("Control Theory" matches the module folder AND the
        # pre-pour "Control Theory (II) Notes").
        ["Control Theory", "2605.30943v1|2605.30363v1"],
        cross_domain=True,
    ),
    LabelledQuery(
        "Which optimization techniques schedule shipping and trading over long horizons?",
        ["2511.08404v1"],  # Multistart LNS for LNG transportation+trading
        cross_domain=True,  # operations-research framing of a trading problem
    ),
    LabelledQuery(
        "What detects regime shifts in the treasury market using unstructured data?",
        ["2605.30363v1"],
    ),
    LabelledQuery(
        "How does geometric Brownian motion arise in models of coordination and asset dynamics?",
        ["2605.30950v1"],
    ),
    LabelledQuery(
        "How does maximum likelihood estimation differ from Bayesian inference?",
        ["Statistics and Probability/|ML-maths"],  # estimation: module folder or the ML-maths notes
    ),
    LabelledQuery(
        "What is gradient descent and what role does the learning rate play?",
        ["optimisation.pdf"],
    ),
    LabelledQuery(
        "How is the transfer function H(omega) used to analyse a linear system's response?",
        ["Time Frequency Analysis/|Control Theory"],  # transfer functions live in both modules
    ),
    LabelledQuery(
        "How does Shannon's equation bound the capacity of a communication channel?",
        ["Sensing, Signals and Communications/"],
    ),
    LabelledQuery(
        "What has Alec achieved academically?",
        ["Mitchell-Thomson_CV|Mitchell-Thomson CV"],  # any surviving CV (underscore or spaced)
    ),
    LabelledQuery(
        "How do heavy tails affect predictability in energy-transition financial markets?",
        ["2605.26890v1"],
    ),
    # --- code corpus (step 10). The two regimes/ files carried the round-3 hallucinated
    # summaries ("electrical circuits" / "image recognition"): these queries fail unless
    # the actual source files surface, so a recurrence becomes a failing eval, not a
    # silent latent defect.
    LabelledQuery(
        "Show me the class that implements HMM regime detection in my regime ML project.",
        ["regime-conditioned-equity-ml"],
        expected_paths=["regimes/hmm.py"],
    ),
    LabelledQuery(
        "What metrics does the regime detection evaluation module compute in my regime ML code?",
        ["regime-conditioned-equity-ml"],
        expected_paths=["regimes/evaluation.py"],
    ),
    LabelledQuery(
        "Why are the HMM emission means initialised with KMeans, and how is the best seed chosen?",
        ["regime-conditioned-equity-ml"],
    ),
    LabelledQuery(
        "How does Locus rerank retrieval candidates with a cross-encoder?",
        # The locus repo (doc 217) is excluded from production retrieval, and its repo-root
        # source_uri is a prefix of every vault doc's path, so it can't be keyed by source_uri
        # substring — the file-path target IS the discriminator here.
        [],
        expected_paths=["retrieve/rerank.py"],
        include_excluded=True,
    ),
    # --- slides corpus (step 9).
    LabelledQuery(
        "What did my Citadel deck recommend about monetary policy?",
        ["Policy Recommendation v1"],  # the .pptx (two near-dup copies; either satisfies)
    ),
    # --- figures (step 11): figure-shaped queries that the figure_vectors arm should
    # serve (block diagrams, plots, pipeline figures). Doc-level recall still passes via
    # text units, so these measure that figure phrasing surfaces the right documents —
    # the figure arm's contribution shows up in the survivors' kinds, not the score.
    LabelledQuery(
        "Show the block diagram of a sampled-data control system with a zero-order hold.",
        ["Control Theory"],
    ),
    LabelledQuery(
        "Which figure illustrates aliasing as overlapping spectra in the frequency domain?",
        ["Time Frequency Analysis/|N456_1"],  # aliasing/PSD: the time-freq module or N456 notes
    ),
    LabelledQuery(
        "The pipeline diagram where DINOv3 features match class prototypes and SAM segments regions.",
        ["7c8c0b9075b6"],  # FungiTastic segmentation paper (raw-store hash; original path lost)
    ),
    # --- entity-alias substrate (step 12): the query is phrased with a variant surface the
    # target doc does NOT use verbatim — the probability notes store the entity as
    # 'Kullback-Leibler (KL) divergence'; only the neural-Markov paper writes 'KL
    # divergence'. The acronym tier links them, so the alias-aware entity arm must bridge
    # the phrasing to BOTH documents.
    LabelledQuery(
        "Where does the KL divergence appear — in estimation theory and in regime-model "
        "evaluation?",
        ["Statistics and Probability/|ML-maths", "2605.30943v1"],
    ),
]


# Known related-document pairs (source_uri substrings, same matching rules as `expected`).
# Re-curated 2026-06-09 against the live related-docs layer: each pair was VERIFIED to surface
# mutually in the other's top-5 (the joins-only layer is deterministic — these assert genuine,
# currently-surfacing relationships, not aspirations). The pre-pour PDE<->Time-Frequency pair
# was DROPPED: post-pour each side has closer same-module siblings that crowd the cross-module
# link out of the top-5, so labelling it would assert a relationship the layer no longer
# produces. EXTEND as the corpus grows.
RELATED_PAIRS: list[tuple[str, str]] = [
    # finance papers — SVAR <-> Inspectable Neural Markov (mutual #2, 8 shared canonicals)
    ("2603.16035v1", "2605.30943v1"),
    # control theory I <-> II lecture notes (mutual #1, 68 shared canonicals)
    ("2A2A Control Theory (I) Notes", "2A2B Control Theory (II) Notes"),
    # regime cluster — treasury Regime Shift <-> SVAR (mutual within top-5: #1 and #4).
    # NB the Neural-Markov <-> Regime-Shift pair was NOT used: genuine but rank ~8, crowded
    # below closer finance siblings, so it is absent from the consumer-facing top-5.
    ("2605.30363v1", "2603.16035v1"),
]


@dataclass
class QueryResult:
    query: LabelledQuery
    survivor_keys: list[str]  # doc source_uris in rank order (deduped, first occurrence)
    survivor_paths: list[str] = field(default_factory=list)  # file paths among survivors
    matched: list[str] = field(default_factory=list)
    matched_paths: list[str] = field(default_factory=list)
    first_rank: int | None = None  # 1-based rank of the first expected doc
    confidence_band: str | None = None  # the banner the consumer would have seen

    @property
    def recall(self) -> float:
        """Joint doc + file recall: every expected doc AND expected file must surface."""
        want = len(self.query.expected) + len(self.query.expected_paths)
        got = len(self.matched) + len(self.matched_paths)
        return got / want if want else 1.0

    @property
    def reciprocal_rank(self) -> float:
        return 1.0 / self.first_rank if self.first_rank else 0.0

    @property
    def banner_misfire(self) -> bool:
        """A cross-domain query that retrieved its expected set but still warned the user."""
        return self.query.cross_domain and self.confidence_band is not None


def score_query(
    q: LabelledQuery,
    survivor_keys: list[str],
    survivor_paths: list[str] | None = None,
    confidence_band: str | None = None,
) -> QueryResult:
    """Score one query given the match-keys (source_uris) of the rank-ordered survivors.

    Pure and testable: the function is a generic substring matcher over whatever rank-ordered
    strings it is handed (production feeds source_uris; unit tests feed plain strings — the
    logic is identical either way).
    """
    deduped: list[str] = []
    for t in survivor_keys:
        if t not in deduped:
            deduped.append(t)
    matched: list[str] = []
    first_rank: int | None = None
    for pattern in q.expected:
        alternatives = [a.strip().lower() for a in pattern.split("|")]  # any-of
        done = False
        for rank, key in enumerate(deduped, start=1):
            if done:
                break
            for alt in alternatives:
                if alt and alt in (key or "").lower():
                    matched.append(pattern)
                    if first_rank is None or rank < first_rank:
                        first_rank = rank
                    done = True
                    break
    paths = survivor_paths or []
    matched_paths = [
        p for p in q.expected_paths if any(p.lower() in (sp or "").lower() for sp in paths)
    ]
    return QueryResult(
        query=q, survivor_keys=deduped, survivor_paths=paths, matched=matched,
        matched_paths=matched_paths, first_rank=first_rank, confidence_band=confidence_band,
    )


def evaluate_retrieval(conn, queries: list[LabelledQuery] | None = None) -> tuple[list[QueryResult], dict[str, float]]:
    """Run the full retrieval pipeline per labelled query and score recall@k / MRR.

    Answer-key guard: since step 10 the locus repo itself is in the corpus — including
    THIS file, whose chunks contain every labelled query verbatim. A candidate whose text
    contains the full query string is the eval's own answer key: it (correctly) wins the
    live ranking but invalidates the measurement, so it is excluded from the CANDIDATE
    POOL (not merely from scoring — a post-hoc exclusion left answer keys consuming top-k
    slots and out-competing the real .py targets, visibly depressing recall and
    file_recall). Exclusions are counted so contamination stays visible.
    """
    from locus.retrieve import retrieve

    queries = queries or LABELLED_QUERIES
    results: list[QueryResult] = []
    excluded = 0
    for q in queries:
        def _answer_key(c, _q=q.query.lower()):
            nonlocal excluded
            if _q in (c.text or "").lower():
                excluded += 1
                return True
            return False

        r = retrieve(
            q.query, conn=conn, exclude=_answer_key, include_excluded=q.include_excluded
        )
        keys = []
        paths = []
        for c in r.survivors:
            row = conn.execute("SELECT source_uri FROM documents WHERE id=?", (c.doc_id,)).fetchone()
            keys.append(row["source_uri"] if row else f"doc {c.doc_id}")
            if c.file_path:
                paths.append(c.file_path)
        results.append(score_query(q, keys, paths, confidence_band=r.confidence_band))
    if excluded:
        import logging

        logging.getLogger(__name__).info(
            "retrieval eval: %d answer-key survivor(s) excluded from scoring", excluded
        )
    return results, aggregate(results)


def score_links(
    conn,
    pairs: list[tuple[str, str]] | None = None,
    *,
    top_n: int = 5,
) -> tuple[list[str], dict[str, float]]:
    """Check labelled related-document pairs against the alias-substrate link layer.

    Each (a, b) source_uri-substring pair is checked in BOTH directions: some doc matching `a`
    must list a different doc matching `b` in its related-documents top-N, and vice versa.
    Returns (report lines, {'links_recall': fraction of pair-directions satisfied}); empty
    metrics when the substrate has not been built (`locus link`).
    """
    from locus.link.related import aliases_built, related_documents, resolve_stop_doc_freq

    pairs = pairs or RELATED_PAIRS
    if not aliases_built(conn):
        return (["  entity_aliases not built (run `locus link`) — links check skipped"], {})

    uris = {r["id"]: r["source_uri"] for r in conn.execute("SELECT id, source_uri FROM documents")}

    def _docs_matching(pattern: str) -> list[int]:
        pat = pattern.lower()
        return [doc_id for doc_id, uri in uris.items() if pat in (uri or "").lower()]

    lines: list[str] = []
    satisfied = 0
    total = 0
    for a_pat, b_pat in pairs:
        for src_pat, dst_pat in ((a_pat, b_pat), (b_pat, a_pat)):
            total += 1
            hit = False
            stop = resolve_stop_doc_freq(conn)  # reflect the production stop-entity guard
            for src_id in _docs_matching(src_pat):
                for rel in related_documents(conn, src_id, top_n=top_n, stop_doc_freq=stop):
                    if rel.doc_id != src_id and dst_pat.lower() in (uris.get(rel.doc_id) or "").lower():
                        hit = True
                        break
                if hit:
                    break
            satisfied += hit
            mark = "✓" if hit else "✗"
            lines.append(f"  {mark} {src_pat!r} -> {dst_pat!r} in related top-{top_n}")
    return lines, {"links_recall": satisfied / total if total else 1.0}


def aggregate(results: list[QueryResult]) -> dict[str, float]:
    if not results:
        return {}
    cross = [r for r in results if r.query.cross_domain]
    pathed = [r for r in results if r.query.expected_paths]
    agg = {
        "recall_at_k": sum(r.recall for r in results) / len(results),
        "mrr": sum(r.reciprocal_rank for r in results) / len(results),
        "full_recall_queries": sum(1 for r in results if r.recall == 1.0) / len(results),
    }
    if cross:
        agg["cross_domain_recall"] = sum(r.recall for r in cross) / len(cross)
        # Banner misfires on cross-domain queries: must be 0.0. recall@k alone is blind
        # to this (the right docs retrieve while the user is told the corpus lacks them).
        agg["cross_domain_banner_rate"] = sum(r.banner_misfire for r in cross) / len(cross)
    if pathed:
        # File-level recall over code queries: did the SOURCE FILE surface, not just
        # any unit of its repo document?
        agg["file_recall"] = sum(
            len(r.matched_paths) / len(r.query.expected_paths) for r in pathed
        ) / len(pathed)
    return agg


def format_results(results: list[QueryResult], agg: dict[str, float]) -> str:
    lines = []
    for r in results:
        flag = " [cross-domain]" if r.query.cross_domain else ""
        if r.banner_misfire:
            flag += f" [BANNER MISFIRE: {r.confidence_band}]"
        mark = "✓" if r.recall == 1.0 else ("~" if (r.matched or r.matched_paths) else "✗")
        lines.append(
            f"  {mark} recall {r.recall:.2f} rr {r.reciprocal_rank:.2f}{flag}  {r.query.query[:70]}"
        )
        missing = [e for e in r.query.expected if e not in r.matched]
        missing += [p for p in r.query.expected_paths if p not in r.matched_paths]
        if missing:
            # Show the TAIL of each survivor key (source_uris share a long common prefix).
            got = [k.rsplit("/", 1)[-1][:40] for k in r.survivor_keys[:4]]
            lines.append(f"      missing: {missing}  got: {got}")
    lines.append("")
    for k, v in agg.items():
        lines.append(f"  {k:<24} {v:.3f}")
    return "\n".join(lines)
