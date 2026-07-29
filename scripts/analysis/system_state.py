from locus import config as locus_config
from locus.db.connection import get_connection

cfg = locus_config.load()
conn = get_connection(cfg.paths.db)
q = lambda s: conn.execute(s).fetchall()

print("=== agent objects ===")
for r in q("SELECT type, status, COUNT(*) c FROM objects GROUP BY type, status ORDER BY type, status"):
    print(f"  {r['type']:<10} {r['status']:<10} {r['c']}")
print("  total:", q("SELECT COUNT(*) c FROM objects")[0]["c"])

print("\n=== belief positions per subject ===")
for r in q("SELECT subject_kind, subject_key, COUNT(*) c FROM belief_positions "
           "GROUP BY subject_kind, subject_key ORDER BY c DESC LIMIT 12"):
    print(f"  {r['c']}x {r['subject_kind']}: {r['subject_key'][:60]}")

print("\n=== other agent tables ===")
for t in ("object_links", "review_schedule", "acceptance_log", "agent_runs"):
    print(f"  {t:<18} {q(f'SELECT COUNT(*) c FROM {t}')[0]['c']}")

print("\n=== maturity ===")
for r in q("SELECT maturity, COUNT(*) c FROM documents GROUP BY maturity"):
    print(f"  {str(r['maturity']):<8} {r['c']}")

print("\n=== canonical concept span (fragmentation) ===")
rows = q("""SELECT a.canonical_name, COUNT(DISTINCT e.doc_id) d
            FROM entity_aliases a JOIN entities e
              ON e.name = a.name AND e.type = a.type
            GROUP BY a.canonical_name, a.canonical_type""")
from collections import Counter
c = Counter(r["d"] for r in rows)
tot = sum(c.values())
multi = sum(v for k, v in c.items() if k >= 2)
print(f"  canonicals: {tot}   spanning >=2 docs: {multi} ({multi/tot:.1%})")
print("  top cross-doc concepts:")
for r in sorted(rows, key=lambda r: -r["d"])[:12]:
    print(f"    {r['d']:3d} docs  {r['canonical_name'][:52]}")
