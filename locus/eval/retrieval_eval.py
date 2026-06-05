"""Labelled retrieval eval (PLAN.md step 7): recall@k + MRR over a fixed query set.

A retrieval miss is NOT Claude-recoverable (§15.0) — this is the definitive check that the
right content surfaces. Queries are labelled with expected documents by title substring
(stable across re-ingests, unlike ids). Two queries are deliberately cross-domain
(engineering ↔ quant): the system's headline capability is retrieving the *set*, not the
single best-matching document.

Pure scoring is separated from pipeline execution so it is unit-testable without models.
"""

from __future__ import annotations

from dataclasses import dataclass, field


@dataclass
class LabelledQuery:
    query: str
    expected: list[str]  # title substrings (case-insensitive); each is one expected doc
    cross_domain: bool = False


# Grounded in the current 24-doc corpus. Keep title substrings short and durable.
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
        ["Control Theory", "Regime Shift"],
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
        ["mathreview"],
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
]


@dataclass
class QueryResult:
    query: LabelledQuery
    survivor_titles: list[str]  # doc titles in rank order (deduped, first occurrence)
    matched: list[str] = field(default_factory=list)
    first_rank: int | None = None  # 1-based rank of the first expected doc

    @property
    def recall(self) -> float:
        return len(self.matched) / len(self.query.expected)

    @property
    def reciprocal_rank(self) -> float:
        return 1.0 / self.first_rank if self.first_rank else 0.0


def score_query(q: LabelledQuery, survivor_titles: list[str]) -> QueryResult:
    """Score one query given the doc titles of the rank-ordered survivors (pure, testable)."""
    deduped: list[str] = []
    for t in survivor_titles:
        if t not in deduped:
            deduped.append(t)
    matched: list[str] = []
    first_rank: int | None = None
    for pattern in q.expected:
        for rank, title in enumerate(deduped, start=1):
            if pattern.lower() in (title or "").lower():
                matched.append(pattern)
                if first_rank is None or rank < first_rank:
                    first_rank = rank
                break
    return QueryResult(query=q, survivor_titles=deduped, matched=matched, first_rank=first_rank)


def evaluate_retrieval(conn, queries: list[LabelledQuery] | None = None) -> tuple[list[QueryResult], dict[str, float]]:
    """Run the full retrieval pipeline per labelled query and score recall@k / MRR."""
    from locus.retrieve import retrieve

    queries = queries or LABELLED_QUERIES
    results: list[QueryResult] = []
    for q in queries:
        r = retrieve(q.query, conn=conn)
        titles = []
        for c in r.survivors:
            row = conn.execute("SELECT title FROM documents WHERE id=?", (c.doc_id,)).fetchone()
            titles.append(row["title"] if row else f"doc {c.doc_id}")
        results.append(score_query(q, titles))
    return results, aggregate(results)


def aggregate(results: list[QueryResult]) -> dict[str, float]:
    if not results:
        return {}
    cross = [r for r in results if r.query.cross_domain]
    agg = {
        "recall_at_k": sum(r.recall for r in results) / len(results),
        "mrr": sum(r.reciprocal_rank for r in results) / len(results),
        "full_recall_queries": sum(1 for r in results if r.recall == 1.0) / len(results),
    }
    if cross:
        agg["cross_domain_recall"] = sum(r.recall for r in cross) / len(cross)
    return agg


def format_results(results: list[QueryResult], agg: dict[str, float]) -> str:
    lines = []
    for r in results:
        flag = " [cross-domain]" if r.query.cross_domain else ""
        mark = "✓" if r.recall == 1.0 else ("~" if r.matched else "✗")
        lines.append(
            f"  {mark} recall {r.recall:.2f} rr {r.reciprocal_rank:.2f}{flag}  {r.query.query[:70]}"
        )
        missing = [e for e in r.query.expected if e not in r.matched]
        if missing:
            lines.append(f"      missing: {missing}  got: {[t[:40] for t in r.survivor_titles[:4]]}")
    lines.append("")
    for k, v in agg.items():
        lines.append(f"  {k:<24} {v:.3f}")
    return "\n".join(lines)
