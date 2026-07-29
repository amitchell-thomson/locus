"""Verify candidate eval labels against the LIVE pipeline before they are committed.

The label set asserts only relationships the system actually produces (see
retrieval_eval.py's docstring: two candidate bridges were DROPPED in the 2026-06-09
curation for being ungrounded). This runs each candidate query through production
retrieval and reports whether the intended target surfaces, at what rank, and what the
confidence banner would have said. A candidate that misses is NOT committed.
"""
from __future__ import annotations

import sys

from locus import config as locus_config
from locus.db.connection import get_connection
from locus.retrieve import retrieve

# (query, expected source_uri substring(s), is_cross_domain)
CANDIDATES: list[tuple[str, list[str], bool]] = [
    # --- handwriting notes: the Brevan Howard internship material (Loop A capture) ---
    ("How does fixing risk in an emerging market currency swap decompose around a central bank meeting?",
     ["dashboard-"], False),
    ("What is the difference between an FX swap, a cross-currency swap, and an interest rate swap?",
     ["swaps-b3f4d16b"], False),
    ("How can time-series momentum be captured in emerging market local interest rates?",
     ["swaps-momentum-strat"], False),
    ("What is a discount factor and how do day-count conventions affect swap valuation?",
     ["rates-foundations|jargon-sheet"], False),
    ("What did the Brevan Howard speakers say about risk management and technology in macro trading?",
     ["speaker-sessions"], False),
    ("Where are the mispricings in Central and Eastern European rates across tenors?",
     ["em-rates-trading"], False),
    ("What improvements were proposed for price discovery tools and AI-assisted risk reporting?",
     ["em-ideas"], False),
    ("What does Jane Street look for in candidates?", ["jane-steet-social"], False),
    ("How are hidden Markov models used for credit stress and risk attribution?",
     ["robert-training"], False),
    # --- bridges that matter now: my notes <-> my projects <-> the papers ---
    ("Where does mean reversion appear in both my own trading notes and my regime modelling code?",
     ["speaker-sessions|em-rates-trading", "regime-conditioned-equity-ml"], True),
    ("How do Markov regime models connect my handwritten risk notes to the research papers?",
     ["robert-training", "2605.30943v1|2603.16035v1"], True),
    ("How does my internship work on rates relate to the Brevan Howard offer?",
     ["swaps-b3f4d16b|em-rates-trading|speaker-sessions",
      "Summer Intern Offer Letter|Intern employment contract"], True),
    # --- replacement for the DEAD 'Dimensional Analysis' label (that doc was pruned) ---
    ("How does dimensional analysis reduce the number of variables in a fluid mechanics problem?",
     ["Fluid Mechanics|Thermofluids|Heat and Mass Transfer"], False),
]

cfg = locus_config.load()
conn = get_connection(cfg.paths.db)
uris = {r["id"]: r["source_uri"] for r in conn.execute("SELECT id, source_uri FROM documents")}

ok = bad = 0
for query, expected, cross in CANDIDATES:
    r = retrieve(query, conn=conn)
    keys: list[str] = []
    for c in r.survivors:
        u = uris.get(c.doc_id) or f"doc {c.doc_id}"
        if u not in keys:
            keys.append(u)
    print(f"\nQ: {query[:92]}")
    print(f"   banner: {r.confidence_band}")
    all_hit = True
    for pattern in expected:
        alts = [a.strip().lower() for a in pattern.split("|")]
        rank = next((i for i, k in enumerate(keys, 1)
                     if any(a and a in k.lower() for a in alts)), None)
        mark = "OK " if rank else "MISS"
        if not rank:
            all_hit = False
        print(f"   {mark} rank={rank} <- {pattern}")
    if not all_hit:
        print(f"   got: {[k.rsplit('/', 1)[-1][:38] for k in keys[:6]]}")
    if cross and r.confidence_band is not None:
        print(f"   !! cross-domain candidate would MISFIRE the banner ({r.confidence_band})")
        all_hit = False
    ok, bad = (ok + 1, bad) if all_hit else (ok, bad + 1)

print(f"\n=== {ok} candidates verified, {bad} rejected ===")
sys.exit(0)
