"""Measure the right value for [retrieve].rough_penalty.

The 2026-07-29 label re-curation rejected 6 candidate queries with ONE shared diagnosis:
rough handwriting does not compete with polished prose. The penalty is SUBTRACTIVE on the
cross-encoder logit and currently 1.5, against a confidence floor of 0.22 — large enough to
push a rough note out of the pool entirely.

Lowering it can only PROMOTE rough candidates, so the note queries that already pass cannot
regress. The real risk is the other direction: a rough note intruding on a query whose
correct target is a paper/project/coursework doc. This sweep measures BOTH:

  RECOVERY  — do the 6 rejected note queries find their target?
  INTRUSION — on non-note queries, does the labelled target still surface, and how many
              rough notes appear above it?
"""
from __future__ import annotations

import sys

from locus import config as locus_config
from locus.db.connection import get_connection
from locus.retrieve import pipeline as rp

PENALTIES = [1.5, 0.75, 0.35, 0.0]

# The 6 rejected in re-curation: (query, expected-substring(s))
RECOVERY: list[tuple[str, list[str]]] = [
    ("Where are the mispricings in Central and Eastern European rates across tenors?",
     ["em-rates-trading"]),
    ("What improvements were proposed for price discovery tools and AI-assisted risk reporting?",
     ["em-ideas"]),
    ("Where does mean reversion appear in both my own trading notes and my regime modelling code?",
     ["speaker-sessions|em-rates-trading", "regime-conditioned-equity-ml"]),
    ("How do Markov regime models connect my handwritten risk notes to the research papers?",
     ["robert-training", "2605.30943v1|2603.16035v1"]),
    ("How does my internship work on rates relate to the Brevan Howard offer?",
     ["swaps-b3f4d16b|em-rates-trading|speaker-sessions",
      "Summer Intern Offer Letter|Intern employment contract"]),
    ("How does fixing risk in an emerging market currency swap decompose around a central bank meeting?",
     ["dashboard-"]),  # already passes at rank 1 — the control, must not move
]

# Non-note queries: does a rough note now displace a legitimate target?
INTRUSION: list[tuple[str, list[str]]] = [
    ("What detects regime shifts in the treasury market using unstructured data?", ["2605.30363v1"]),
    ("Which project derives trading signals from oil tanker movements and AIS vessel flows?", ["tanker-flow"]),
    ("How is the stability of a closed-loop control system assessed from its poles?", ["Introduction to Control Theory"]),
    ("What are the terms of the Brevan Howard summer internship offer?",
     ["Summer Intern Offer Letter|Intern employment contract"]),
    ("How can macroeconomic indicators improve time-series forecasting at mixed sampling frequencies?", ["2606.00624v1"]),
    ("What is gradient descent and what role does the learning rate play?", ["optimisation.pdf"]),
    ("Where is Black-Scholes Monte Carlo option pricing implemented with portfolio covariance stats?", ["alpha-fund"]),
    ("How does negative feedback set the gain and stability of an operational amplifier circuit?",
     ["A2 Electronics and Information Engineering"]),
]

cfg_full = locus_config.load()
conn = get_connection(cfg_full.paths.db)
uris = {r["id"]: r["source_uri"] for r in conn.execute("SELECT id, source_uri FROM documents")}
rough = {r["id"] for r in conn.execute("SELECT id FROM documents WHERE maturity='rough'")}
_real_load = rp.load


def patched(penalty: float):
    def _load(*a, **k):
        c = _real_load(*a, **k)
        c.retrieve.rough_penalty = penalty
        return c
    return _load


def run(query: str, expected: list[str]) -> tuple[list[int | None], int, str | None]:
    r = rp.retrieve(query, conn=conn)
    keys: list[tuple[str, int]] = []
    for c in r.survivors:
        if c.doc_id not in [d for _, d in keys]:
            keys.append((uris.get(c.doc_id) or "", c.doc_id))
    ranks: list[int | None] = []
    for pattern in expected:
        alts = [a.strip().lower() for a in pattern.split("|")]
        ranks.append(next((i for i, (u, _) in enumerate(keys, 1)
                           if any(a and a in u.lower() for a in alts)), None))
    first = next((i for i in ranks if i), None)
    n_rough_above = sum(1 for i, (_, d) in enumerate(keys, 1)
                        if d in rough and (first is None or i < first))
    return ranks, n_rough_above, r.confidence_band


for pen in PENALTIES:
    rp.load = patched(pen)
    print(f"\n{'='*74}\n  rough_penalty = {pen}\n{'='*74}")
    rec = 0
    print("-- RECOVERY (the 6 rejected note queries) --")
    for q, exp in RECOVERY:
        ranks, _, band = run(q, exp)
        hit = all(r is not None for r in ranks)
        rec += hit
        print(f"  {'OK  ' if hit else 'MISS'} ranks={ranks} band={band}  {q[:64]}")
    intr = 0
    print("-- INTRUSION (non-note queries: target rank, rough notes above it) --")
    for q, exp in INTRUSION:
        ranks, above, band = run(q, exp)
        hit = all(r is not None for r in ranks)
        intr += hit
        flag = f"  <-- {above} rough above" if above else ""
        print(f"  {'OK  ' if hit else 'MISS'} rank={ranks[0]}{flag}  {q[:58]}")
    print(f"  ==> recovery {rec}/{len(RECOVERY)}   intrusion-safe {intr}/{len(INTRUSION)}")

rp.load = _real_load
sys.exit(0)
