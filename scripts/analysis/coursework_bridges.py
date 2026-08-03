"""Which coursework->quant bridges are GENUINE cross-domain transfer?

One-off (2026-08-03). The stated reason coursework stays in the corpus is §16's
cross-domain transfer ("eigenvectors in factor models vs modal analysis"). That claim
has never been checked. This applies the system's OWN definition of a topical concept
(`link.related.non_topical_names`) plus a specificity bar to the 125 bridge canonicals
and prints what survives, so the claim can be judged on names rather than a count.

Read-only. argv[1] = DB path.
"""

import sqlite3
import sys

sys.path.insert(0, ".")
DB = sys.argv[1] if len(sys.argv) > 1 else "vault/locus.db"
conn = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
conn.row_factory = sqlite3.Row

from locus.agent.compose_daily import TEACHABLE_TYPES
from locus.link.related import non_topical_names

generic = non_topical_names(conn)
print(f"non_topical_names: {len(generic)} filtered surfaces\n")

rows = conn.execute("""
  WITH canon AS (
    SELECT ea.canonical_name cn, ea.canonical_type ct, d.category,
           COUNT(DISTINCT e.doc_id) nd
    FROM entities e
    JOIN entity_aliases ea
      ON ea.variant_name = e.name AND ea.variant_type = e.type
    JOIN documents d ON d.id = e.doc_id
    GROUP BY cn, ct, d.category
  ),
  tot AS (
    SELECT cn, ct, SUM(nd) docs,
           SUM(CASE WHEN category='coursework' THEN nd ELSE 0 END) cw
    FROM canon GROUP BY cn, ct
  )
  SELECT cn, ct, docs, cw FROM tot WHERE docs>=2 AND cw>0 AND cw<docs
  ORDER BY docs DESC""").fetchall()

print(f"raw bridge canonicals: {len(rows)}\n")

kept, dropped = [], []
for r in rows:
    name, typ = r["cn"], r["ct"]
    why = None
    if typ not in TEACHABLE_TYPES:
        why = f"type={typ}"
    elif name.lower() in generic:
        why = "non_topical"
    elif len(name) < 6:
        why = "too short"
    elif not any(c.isalpha() for c in name) or name.startswith("\\"):
        why = "symbol"
    elif " " not in name and r["docs"] > 8:
        # a single common word is that word, not a shared idea
        why = "single common word"
    (dropped if why else kept).append((r, why))

print(f"=== SURVIVING BRIDGES: {len(kept)} ===")
for r, _ in kept:
    print(f"  {r['docs']:3d} docs ({r['cw']} cw)  {r['cn']} [{r['ct']}]")

print(f"\n=== DROPPED: {len(dropped)} (reasons) ===")
from collections import Counter
for why, n in Counter(w for _, w in dropped).most_common():
    print(f"  {n:4d}  {why}")
print("\n  examples:")
for r, why in dropped[:15]:
    print(f"    {r['cn']!r} [{r['ct']}] {r['docs']}d — {why}")
