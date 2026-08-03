"""Measure, per consumer, how much of its output the coursework mass owns.

One-off (2026-08-03). Answers the question the assessment left open: coursework is
65% of documents, but WHICH heuristics does that volume actually distort? Written
because the six existing coursework workarounds were each added on local evidence
and nobody has ever measured them together.

Read-only. Takes the DB path as argv[1] so it can run from a worktree that has no
config.toml of its own.
"""

import sqlite3
import sys

DB = sys.argv[1] if len(sys.argv) > 1 else "vault/locus.db"
db = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
db.row_factory = sqlite3.Row


def q(sql, *a):
    return db.execute(sql, a).fetchall()


CANON = """
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
"""

print("=== 1. canonical concept space: who owns the cross-doc canonicals ===")
r = q(CANON + """
  SELECT COUNT(*) n,
         SUM(CASE WHEN docs>=2 THEN 1 ELSE 0 END) crossdoc,
         SUM(CASE WHEN docs>=2 AND cw=docs THEN 1 ELSE 0 END) cw_only,
         SUM(CASE WHEN docs>=2 AND cw>0 AND cw<docs THEN 1 ELSE 0 END) bridge,
         SUM(CASE WHEN docs>=2 AND cw=0 THEN 1 ELSE 0 END) no_cw
  FROM tot""")[0]
print(f"  canonicals total {r['n']}, cross-doc {r['crossdoc']}")
print(f"    coursework-ONLY   {r['cw_only']}")
print(f"    BRIDGE (cw+other) {r['bridge']}")
print(f"    no coursework     {r['no_cw']}")

print("\n=== 2. objects: source document category ===")
for r in q("""
  SELECT d.category, COUNT(DISTINCT o.id) n FROM objects o
  JOIN object_links ol ON ol.object_id=o.id AND ol.target_kind='document'
  JOIN documents d ON CAST(d.id AS TEXT)=ol.target_key
  GROUP BY d.category ORDER BY n DESC"""):
    print(f"  {r['category']:12s} {r['n']}")

print("\n=== 3. review_schedule by prompt kind ===")
for r in q("SELECT prompt_kind, COUNT(*) n FROM review_schedule GROUP BY prompt_kind ORDER BY n DESC"):
    print(f"  {str(r['prompt_kind']):12s} {r['n']}")

print("\n=== 4. documents never structured, by category ===")
for r in q("""
  SELECT category, COUNT(*) n FROM documents WHERE structured_at IS NULL
  GROUP BY category ORDER BY n DESC"""):
    print(f"  {r['category']:12s} {r['n']}")

print("\n=== 5. the bridge canonicals — coursework reaching his own work ===")
for r in q(CANON + """
  SELECT cn, ct, docs, cw FROM tot
  WHERE docs>=2 AND cw>0 AND cw<docs ORDER BY (docs-cw) DESC, docs DESC LIMIT 25"""):
    print(f"  {r['docs']:3d} docs ({r['cw']} cw)  {r['cn']} [{r['ct']}]")
