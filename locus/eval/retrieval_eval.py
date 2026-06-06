"""Labelled retrieval eval (PLAN.md step 7): recall@k + MRR over a fixed query set.

A retrieval miss is NOT Claude-recoverable (§15.0) — this is the definitive check that the
right content surfaces. Queries are labelled with expected documents by title substring
(stable across re-ingests, unlike ids). Two checks beyond plain recall (2026-06-05 round-3
evaluation, which found the eval set stale at 24 docs and structurally blind to two live
regressions):

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
    # Title substrings (case-insensitive); each entry is one expected doc. An entry may
    # carry '|'-separated alternatives ("Regime Shift|Neural Markov") satisfied by ANY
    # match — for queries where the corpus holds several legitimately substitutable
    # answers and ranking between them is model choice, not retrieval quality.
    expected: list[str]
    expected_paths: list[str] = field(default_factory=list)  # file-path substrings (code)
    cross_domain: bool = False


# Grounded in the current 30-doc corpus (24 prose + 5 code repos + 1 slide deck).
# Keep title substrings short and durable.
LABELLED_QUERIES: list[LabelledQuery] = [
    LabelledQuery(
        "What is the Biot number and what does it tell you about transient conduction?",
        ["Partial Differential Equations"],
    ),
    LabelledQuery(
        "How is Fourier analysis used both for signals and for solving PDEs?",
        ["Partial Differential Equations", "Time Frequency"],
    ),
    LabelledQuery(
        "How do regime-switching models in finance relate to state-space models in control theory?",
        # Either regime-modeling paper satisfies the finance side (the 2026-06-06 arrival,
        # an inspectable-neural-Markov-models paper, legitimately outranks the treasury one).
        ["Control Theory", "Regime Shift|Neural Markov"],
        cross_domain=True,
    ),
    LabelledQuery(
        "Which optimization techniques schedule shipping and trading over long horizons?",
        ["liquefied natural gas"],
        cross_domain=True,  # operations-research framing of a trading problem
    ),
    LabelledQuery(
        "What detects regime shifts in the treasury market using unstructured data?",
        ["Regime Shift"],
    ),
    LabelledQuery(
        "How does geometric Brownian motion arise in models of coordination and asset dynamics?",
        ["geometric Brownian motion"],
    ),
    LabelledQuery(
        "How does maximum likelihood estimation differ from Bayesian inference?",
        # Slug-titled source (ML-maths.pdf): the synthesis pass re-arbitrates a title on
        # every re-ingest, so the label keeps only the durable token ('Probability
        # Fundamentals' became 'Probability Concepts with Parameter Estimation').
        ["Probability"],
    ),
    LabelledQuery(
        "What is gradient descent and what role does the learning rate play?",
        ["Mathematical Optimization"],
    ),
    LabelledQuery(
        "How is the transfer function H(omega) used to analyse a linear system's response?",
        ["Time Frequency"],
    ),
    LabelledQuery(
        "How does Shannon's equation bound the capacity of a communication channel?",
        ["Sensors signals"],
    ),
    LabelledQuery(
        "What has Alec achieved academically?",
        ["Mitchell-Thomson"],
    ),
    LabelledQuery(
        "How do heavy tails affect predictability in energy-transition financial markets?",
        ["Transition-Energy"],
    ),
    # --- code corpus (step 10). The two regimes/ files carried the round-3 hallucinated
    # summaries ("electrical circuits" / "image recognition"): these queries fail unless
    # the actual source files surface, so a recurrence becomes a failing eval, not a
    # silent latent defect.
    LabelledQuery(
        "Show me the class that implements HMM regime detection in my regime ML project.",
        ["Regime-Conditioned"],
        expected_paths=["regimes/hmm.py"],
    ),
    LabelledQuery(
        "What metrics does the regime detection evaluation module compute in my regime ML code?",
        ["Regime-Conditioned"],
        expected_paths=["regimes/evaluation.py"],
    ),
    LabelledQuery(
        "Why are the HMM emission means initialised with KMeans, and how is the best seed chosen?",
        ["Regime-Conditioned"],
    ),
    LabelledQuery(
        "How does Locus rerank retrieval candidates with a cross-encoder?",
        ["Locus"],
        expected_paths=["retrieve/rerank.py"],
    ),
    # --- slides corpus (step 9).
    LabelledQuery(
        "What did my Citadel deck recommend about monetary policy?",
        ["Monetary Policy Recommendation"],
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
        # Slug-titled source (N456_1.pdf): arbitrated title varies per re-ingest
        # ('Signals and Systems' -> 'Signal Processing Techniques and Spectral Analysis').
        ["Spectral|Signals and Systems"],
    ),
    LabelledQuery(
        "The pipeline diagram where DINOv3 features match class prototypes and SAM segments regions.",
        ["Fine-Grained Semantic Segmentation"],
    ),
]


@dataclass
class QueryResult:
    query: LabelledQuery
    survivor_titles: list[str]  # doc titles in rank order (deduped, first occurrence)
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
    survivor_titles: list[str],
    survivor_paths: list[str] | None = None,
    confidence_band: str | None = None,
) -> QueryResult:
    """Score one query given the doc titles of the rank-ordered survivors (pure, testable)."""
    deduped: list[str] = []
    for t in survivor_titles:
        if t not in deduped:
            deduped.append(t)
    matched: list[str] = []
    first_rank: int | None = None
    for pattern in q.expected:
        alternatives = [a.strip().lower() for a in pattern.split("|")]  # any-of
        done = False
        for rank, title in enumerate(deduped, start=1):
            if done:
                break
            for alt in alternatives:
                if alt and alt in (title or "").lower():
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
        query=q, survivor_titles=deduped, survivor_paths=paths, matched=matched,
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

        r = retrieve(q.query, conn=conn, exclude=_answer_key)
        titles = []
        paths = []
        for c in r.survivors:
            row = conn.execute("SELECT title FROM documents WHERE id=?", (c.doc_id,)).fetchone()
            titles.append(row["title"] if row else f"doc {c.doc_id}")
            if c.file_path:
                paths.append(c.file_path)
        results.append(score_query(q, titles, paths, confidence_band=r.confidence_band))
    if excluded:
        import logging

        logging.getLogger(__name__).info(
            "retrieval eval: %d answer-key survivor(s) excluded from scoring", excluded
        )
    return results, aggregate(results)


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
            lines.append(f"      missing: {missing}  got: {[t[:40] for t in r.survivor_titles[:4]]}")
    lines.append("")
    for k, v in agg.items():
        lines.append(f"  {k:<24} {v:.3f}")
    return "\n".join(lines)
