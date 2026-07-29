"""What does the label set MEASURE, versus what the corpus is now FOR?

The 53-query set was curated against a 291-doc corpus that was 85% coursework. After the
2026-07-29 prune (306 -> 203, quant-focused) the balance of the labels is the question:
are we still measuring the material the owner actually queries?
"""
from __future__ import annotations

from collections import Counter

from locus import config as locus_config
from locus.db.connection import get_connection
from locus.eval.retrieval_eval import LABELLED_QUERIES

cfg = locus_config.load()
conn = get_connection(cfg.paths.db)
rows = [(r["id"], r["source_uri"], r["category"]) for r in
        conn.execute("SELECT id, source_uri, category FROM documents")]


def cats(pattern: str) -> set[str]:
    alts = [a.strip().lower() for a in pattern.split("|")]
    return {r[2] for r in rows if any(a and a in (r[1] or "").lower() for a in alts)}


label_cats: Counter = Counter()
for q in LABELLED_QUERIES:
    found: set[str] = set()
    for p in q.expected:
        found |= cats(p)
    if not found and q.expected_paths:
        found = {"code (excluded-pool)"}
    label_cats[",".join(sorted(found)) or "DEAD"] += 1

print("=== labelled queries, by category of their target ===")
for k, v in label_cats.most_common():
    print(f"  {k:<28} {v}")

print("\n=== corpus, by category ===")
for r in conn.execute("SELECT category, COUNT(*) c FROM documents GROUP BY category ORDER BY c DESC"):
    print(f"  {r['category']:<28} {r['c']}")

print("\n=== note documents (handwriting capture) — are ANY labelled? ===")
for r in conn.execute(
    "SELECT id, source_uri, title, maturity FROM documents WHERE category='note' ORDER BY id"
):
    labelled = any(
        any(a.strip().lower() in (r["source_uri"] or "").lower() for a in p.split("|"))
        for q in LABELLED_QUERIES for p in q.expected
    )
    print(f"  [{r['id']:3d}] {'LABELLED' if labelled else '   -    '} "
          f"{(r['maturity'] or '?'):<5} {r['title'][:58]}")
