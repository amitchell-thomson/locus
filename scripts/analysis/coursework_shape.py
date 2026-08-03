"""What the coursework mass actually IS — documents, and the units it owns.

One-off (2026-08-03), the first measurement behind CLAUDE.md §25. The standing claim was that
coursework "distorts every downstream heuristic"; this establishes the denominator that claim
rests on, before `coursework_harm.py` checks each consumer.

Read-only. argv[1] = DB path.
"""

import collections
import sqlite3
import sys

DB = sys.argv[1] if len(sys.argv) > 1 else "vault/locus.db"
db = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
db.row_factory = sqlite3.Row

print("=== documents by category ===")
for r in db.execute("SELECT category, COUNT(*) n FROM documents GROUP BY category ORDER BY n DESC"):
    print(f"  {r['category']:12s} {r['n']}")

print("\n=== coursework: source folders ===")
c: collections.Counter = collections.Counter()
for r in db.execute("SELECT source_uri FROM documents WHERE category='coursework'"):
    parts = [p for p in (r["source_uri"] or "").split("/") if p]
    key = parts[-1]
    for i, p in enumerate(parts):
        if p == "coursework" and i + 1 < len(parts):
            key = parts[i + 1]
            break
    c[key] += 1
for k, n in c.most_common(20):
    print(f"  {n:4d}  {k}")

print("\n=== units by category (why volume dominance is real) ===")
for r in db.execute("""
  SELECT d.category,
         COUNT(DISTINCT d.id) docs,
         (SELECT COUNT(*) FROM propositions p JOIN documents dd ON dd.id=p.doc_id
           WHERE dd.category=d.category) props,
         (SELECT COUNT(*) FROM entities e JOIN documents dd ON dd.id=e.doc_id
           WHERE dd.category=d.category) ents,
         (SELECT COUNT(*) FROM chunks ch JOIN documents dd ON dd.id=ch.doc_id
           WHERE dd.category=d.category) chunks
  FROM documents d GROUP BY d.category ORDER BY docs DESC"""):
    print(
        f"  {r['category']:12s} docs={r['docs']:4d} props={r['props']:6d} "
        f"ents={r['ents']:6d} chunks={r['chunks']:6d}"
    )
