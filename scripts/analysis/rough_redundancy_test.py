"""Does the child-redundancy rule delete the document?

select() demotes a 'section' candidate when ANY proposition/chunk of the same section is in
the pool, on the assumption that expansion re-attaches the summary to that child for free.
That assumption holds only if the child is actually SELECTED. Check whether the em-ideas
section (pool rank 1) is suppressed by a child that ranks 42nd and is never selected.
"""
from __future__ import annotations

from locus.config import load
from locus.db.connection import get_connection
from locus.retrieve.rerank import score_pairs, select
from locus.retrieve.search import search

Q = "What improvements were proposed for price discovery tools and AI-assisted risk reporting?"
TARGET = 478

cfg = load()
conn = get_connection(cfg.paths.db)
pool = search(conn, Q, None)
for c, s in zip(pool, score_pairs(Q, [c.text for c in pool])):
    c.rerank_score = s
pool.sort(key=lambda c: c.rerank_score, reverse=True)

print("target units in pool:")
for c in pool:
    if c.doc_id == TARGET:
        r = pool.index(c) + 1
        print(f"  rank={r:<4} {c.kind:11} section_id={c.section_id} score={c.rerank_score:+.3f}")

sel = select(pool, cfg.retrieve.rerank_top_k, cfg.retrieve.per_doc_cap,
             min_score=cfg.retrieve.min_rerank_score)
print(f"\nselected {len(sel)} units; target present: {any(c.doc_id == TARGET for c in sel)}")
print("selected docs:", [c.doc_id for c in sel])

# counterfactual: suppress a section only when a child of it is actually selected
sections_with_units = {c.section_id for c in pool if c.kind in ("proposition", "chunk", "figure")}
tgt_secs = {c.section_id for c in pool if c.doc_id == TARGET and c.kind == "section"}
print("\ntarget section ids:", tgt_secs,
      "| have a child in pool:", {s for s in tgt_secs if s in sections_with_units})
child_ranks = [pool.index(c) + 1 for c in pool
               if c.section_id in tgt_secs and c.kind in ("proposition", "chunk", "figure")]
print("ranks of those children:", child_ranks, "-> selected?",
      [c.kind for c in sel if c.section_id in tgt_secs])
