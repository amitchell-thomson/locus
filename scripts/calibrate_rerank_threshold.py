"""Calibrate [retrieve].min_rerank_score against the live corpus (remediation phase 4).

Runs the labelled retrieval queries (in-corpus positives) and a set of negative-control
queries (subjects the corpus does NOT cover — the 2026-06-05 evaluation's Black-Scholes
probe) through the full pipeline with the floor disabled, then reports the cross-encoder
score distributions and a suggested floor:

  - for each labelled query: the best score among survivors from EXPECTED docs (these must
    stay above the floor or recall regresses);
  - for each negative query: the best survivor score overall (these must fall below the
    floor so the query flags LOW CONFIDENCE).

The suggestion is the midpoint of the gap between the two distributions; if they overlap,
the script says so and prints the offending queries — pick by inspection, do not force it.

Usage: uv run python scripts/calibrate_rerank_threshold.py
(Needs Ollama for query embedding + the rerank extra. Read-only on the DB.)
"""

from __future__ import annotations

from locus.config import load
from locus.db.connection import get_connection
from locus.eval.retrieval_eval import LABELLED_QUERIES
from locus.retrieve import retrieve

# Subjects verified absent from the corpus (2026-06-05 evaluation negative control + kin).
NEGATIVE_QUERIES = [
    "How is the Black-Scholes formula derived and how do you hedge with the Greeks?",
    "What is the implied volatility smile and how does it affect option pricing?",
    "How do I configure a Kubernetes ingress controller with TLS certificates?",
    "What were the causes of the French Revolution?",
]


def main() -> None:
    cfg = load()
    if cfg.retrieve.min_rerank_score is not None:
        print(
            f"NOTE: min_rerank_score is currently set to {cfg.retrieve.min_rerank_score}; "
            "calibration runs measure raw behaviour best with it unset."
        )
    conn = get_connection(cfg.paths.db)
    try:
        positive_best: list[tuple[str, float]] = []  # (query, best expected-doc score)
        print("=== labelled (in-corpus) queries ===")
        for q in LABELLED_QUERIES:
            r = retrieve(q.query, conn=conn)
            by_doc: dict[int, float] = {}
            for c in r.survivors:
                if c.rerank_score is not None:
                    by_doc[c.doc_id] = max(by_doc.get(c.doc_id, float("-inf")), c.rerank_score)
            titles = {
                doc_id: (conn.execute(
                    "SELECT title FROM documents WHERE id=?", (doc_id,)
                ).fetchone() or {"title": ""})["title"] or ""
                for doc_id in by_doc
            }
            expected_scores = [
                s for d, s in by_doc.items()
                if any(p.lower() in titles[d].lower() for p in q.expected)
            ]
            if expected_scores:
                best = max(expected_scores)
                positive_best.append((q.query, best))
                print(f"  best-expected {best:+7.2f}  {q.query[:64]}")
            else:
                print(f"  MISS (no expected doc among survivors)  {q.query[:64]}")

        negative_best: list[tuple[str, float]] = []
        print("\n=== negative-control (out-of-corpus) queries ===")
        for nq in NEGATIVE_QUERIES:
            r = retrieve(nq, conn=conn)
            scores = [c.rerank_score for c in r.survivors if c.rerank_score is not None]
            best = max(scores) if scores else float("-inf")
            negative_best.append((nq, best))
            print(f"  best-survivor {best:+7.2f}  {nq[:64]}")

        if not positive_best or not negative_best:
            print("\nNot enough data to suggest a floor.")
            return
        floor_pos = min(s for _, s in positive_best)   # weakest signal we must keep
        ceil_neg = max(s for _, s in negative_best)    # strongest noise we must flag
        print(f"\nweakest expected-doc score : {floor_pos:+.2f}")
        print(f"strongest negative score   : {ceil_neg:+.2f}")
        if ceil_neg < floor_pos:
            suggestion = (floor_pos + ceil_neg) / 2
            print(f"clean separation — suggested min_rerank_score = {suggestion:.2f}")
        else:
            print(
                "DISTRIBUTIONS OVERLAP — no floor cleanly separates them. Inspect the "
                "queries above and choose: a floor below the weakest expected score "
                "(never regress recall) accepting that some negatives won't flag."
            )
    finally:
        conn.close()


if __name__ == "__main__":
    main()
