"""Diagnose WHY the rough-note acceptance queries miss: is the target absent from the raw
candidate pool (an EXTRACTION problem — nothing the reranker can fix) or present-but-demoted
(a RANKING problem)? Prints, per query, the target's units in the pre-rerank pool with their
retrieval arm, and the target's post-select doc rank."""
from __future__ import annotations

from locus.config import load
from locus.db.connection import get_connection
from locus.retrieve import pipeline as rp
from locus.retrieve.search import search

QUERIES = [
    ("Where are the mispricings in Central and Eastern European rates across tenors?", "em-rates-trading"),
    ("What improvements were proposed for price discovery tools and AI-assisted risk reporting?", "em-ideas"),
    ("How do Markov regime models connect my handwritten risk notes to the research papers?", "robert-training"),
    ("How does my internship work on rates relate to the Brevan Howard offer?", "swaps-b3f4d16b"),
]

cfg = load()
conn = get_connection(cfg.paths.db)
uris = {r["id"]: r["source_uri"] or "" for r in conn.execute("SELECT id, source_uri FROM documents")}
print(f"rough_penalty = {cfg.retrieve.rough_penalty}\n")

for q, target in QUERIES:
    tgt = {d for d, u in uris.items() if target in u.lower()}
    pool = search(conn, q, None)
    in_pool = [c for c in pool if c.doc_id in tgt]
    r = rp.retrieve(q, conn=conn)
    docs, seen = [], set()
    for c in r.survivors:
        if c.doc_id not in seen:
            seen.add(c.doc_id)
            docs.append(c.doc_id)
    rank = next((i for i, d in enumerate(docs, 1) if d in tgt), None)
    print("=" * 96)
    print(f"Q: {q}")
    print(f"   target={target} ids={sorted(tgt)}  POOL={len(pool)} units, target units in pool = {len(in_pool)}")
    for c in in_pool[:8]:
        print(f"      arm={getattr(c,'source',c.kind)!r:22} kind={c.kind:12} score={getattr(c,'score',None)}")
    print(f"   -> post-select doc rank: {rank}   band={r.confidence_band}")

# --- where exactly do the target's units die? cross-encoder scores vs the winners ------
print("\n\n" + "#" * 96)
print("# CROSS-ENCODER: target units vs pool winners")
print("#" * 96)
from locus.retrieve.rerank import score_pairs

for q, target in QUERIES:
    tgt = {d for d, u in uris.items() if target in u.lower()}
    pool = search(conn, q, None)
    if not pool:
        continue
    scores = score_pairs(q, [c.text for c in pool])
    ranked = sorted(zip(pool, scores), key=lambda p: -p[1])
    print("=" * 96)
    print(f"Q: {q}   floor={cfg.retrieve.min_rerank_score}")
    print("  TOP 6 pool units after rerank:")
    for c, s in ranked[:6]:
        print(f"    {s:+7.3f} doc={c.doc_id:<5} {c.kind:11} {uris[c.doc_id][-34:]}")
    tg = [(c, s) for c, s in ranked if c.doc_id in tgt]
    print(f"  TARGET units ({len(tg)}), best first:")
    for c, s in tg[:5]:
        pos = next(i for i, (cc, _) in enumerate(ranked, 1) if cc is c)
        print(f"    {s:+7.3f} rank={pos:<4} {c.kind:11} {c.text[:88]!r}")
