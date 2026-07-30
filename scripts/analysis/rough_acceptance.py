"""The 4 rough-note acceptance queries: does the target document surface at all?

These are the queries the 2026-07-29 sweep left failing and attributed to a §11.B extraction
ceiling ("dense idea-list notes whose summaries flatten into generic prose, so they never
enter the candidate pool"). Run this to check that attribution against the live pipeline.
"""
from __future__ import annotations

from locus.config import load
from locus.db.connection import get_connection
from locus.retrieve import pipeline as rp

QUERIES = [
    ("Where are the mispricings in Central and Eastern European rates across tenors?", "em-rates-trading"),
    ("What improvements were proposed for price discovery tools and AI-assisted risk reporting?", "em-ideas"),
    ("How do Markov regime models connect my handwritten risk notes to the research papers?", "robert-training"),
    ("How does my internship work on rates relate to the Brevan Howard offer?", "swaps-b3f4d16b"),
]

cfg = load()
conn = get_connection(cfg.paths.db)
uris = {r["id"]: r["source_uri"] or "" for r in conn.execute("SELECT id, source_uri FROM documents")}

hits = 0
for q, target in QUERIES:
    r = rp.retrieve(q, conn=conn)
    docs, seen = [], set()
    for c in r.survivors:
        if c.doc_id not in seen:
            seen.add(c.doc_id)
            docs.append(c.doc_id)
    tgt = {d for d, u in uris.items() if target in u.lower()}
    rank = next((i for i, d in enumerate(docs, 1) if d in tgt), None)
    hits += rank is not None
    print(f"  {'OK  ' if rank else 'MISS'} rank={str(rank):<5} band={str(r.confidence_band):<10} "
          f"{target:<20} {q[:52]}")
print(f"  ==> {hits}/{len(QUERIES)} targets surfaced")
