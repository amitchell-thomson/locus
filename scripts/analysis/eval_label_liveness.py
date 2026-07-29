"""Check every labelled eval target against the LIVE corpus.

Post-prune (306 -> 203 docs) some labels point at documents that no longer exist. A label
whose target was deliberately deleted measures nothing; this reports which.
"""
from __future__ import annotations

from locus import config as locus_config
from locus.db.connection import get_connection
from locus.eval.retrieval_eval import LABELLED_QUERIES, RELATED_PAIRS

cfg = locus_config.load()
conn = get_connection(cfg.paths.db)
rows = [(r["id"], r["source_uri"], r["category"]) for r in
        conn.execute("SELECT id, source_uri, category FROM documents")]


def matches(pattern: str) -> list[tuple[int, str, str]]:
    alts = [a.strip().lower() for a in pattern.split("|")]
    return [r for r in rows if any(a and a in (r[1] or "").lower() for a in alts)]


print("=== LABELLED QUERIES ===")
dead = 0
for i, q in enumerate(LABELLED_QUERIES):
    missing = [p for p in q.expected if not matches(p)]
    if missing:
        dead += 1
        print(f"[{i:2d}] DEAD {missing}")
        print(f"      q: {q.query[:88]}")
print(f"\n{dead} of {len(LABELLED_QUERIES)} queries have a dead target")

print("\n=== RELATED PAIRS ===")
for a, b in RELATED_PAIRS:
    ma, mb = matches(a), matches(b)
    if not ma or not mb:
        print(f"  DEAD  {a!r} <-> {b!r}   (a={len(ma)} docs, b={len(mb)} docs)")

print("\n=== CORPUS SHAPE ===")
for r in conn.execute("SELECT category, COUNT(*) c FROM documents GROUP BY category ORDER BY c DESC"):
    print(f"  {r['category']:<12} {r['c']}")
