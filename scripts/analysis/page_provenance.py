"""Every item on the daily page, and the table row it came from.

One-off (2026-08-03). The page is composed from eight different stores and the rendered PDF says
nothing about which. This prints the composed page item by item with its provenance, so each line
can be judged on whether it EARNED its slot rather than on whether it reads well.

Read-only (composition is aggregate-only by design). argv[1] = DB path.
"""

import sys
from datetime import date

sys.path.insert(0, ".")

from locus.agent import compose_daily as cd
from locus.db.connection import get_connection

DB = sys.argv[1] if len(sys.argv) > 1 else "vault/locus.db"
conn = get_connection(DB)
page = cd.compose(conn, today=date(2026, 8, 4))


def rule(title):
    print(f"\n{'=' * 78}\n{title}\n{'=' * 78}")


rule("PAGE 1 — READ")
r = page.reading
print(f"in_progress ({len(r.in_progress)})   <- reading_targets JOIN documents")
for x in r.in_progress:
    print(f"   - {getattr(x, 'title', x)}")
print(f"\nproposed shelf: {len(r.proposed)} shown, oldest {r.oldest_days} days"
      f"   <- reading_proposals WHERE status='proposed'")
for p in r.proposed:
    print(f"   - {p.title[:66]}")
    print(f"       why   : {(p.why or '')[:100]}")
    print(f"       source: why_long={'yes' if getattr(p, 'why_long', None) else 'no'} "
          f"score={getattr(p, 'score', '?')}")

rule("PAGE 2 — THINK")
for t in page.threads:
    print(f"\n[{t.section}]  kind={t.kind}  tick={t.tick}")
    print(f"   headline: {t.headline[:150]}")
    print(f"   context : {(t.context or '')[:120]}")
    print(f"   target  : {t.target_kind}:{str(t.target_key)[:70]}")
    print(f"   item_key: {t.item_key}")

rule("PAGE 3/4 — RECALL")
for q in page.recall:
    print(f"\n   prompt : {q.question[:150]}")
    print(f"   answer : {(q.answer or '')[:150]}")
    print(f"   ref    : kind={getattr(q, 'prompt_kind', '?')} ref={getattr(q, 'prompt_ref', '?')}")

rule("PAGE 4 — STATUS")
print(page.status)

rule("WHAT FED THE THINK PAGE — raw sources")
print("open threads (objects the DEVELOP section draws from):")
for o in conn.execute(
    "SELECT id,type,status,title,updated_at FROM objects "
    "WHERE status='active' AND type IN ('question','idea') ORDER BY updated_at LIMIT 12"
):
    print(f"   obj {o['id']:3d} [{o['type']:8s}] {o['title'][:64]}")
print("\nopen tensions (CHECK THIS):")
for x in conn.execute(
    "SELECT stance,conflicts_with FROM belief_tensions "
    "WHERE conflicts_with!='' AND dismissed_at IS NULL"
):
    print(f"   {x['stance'][:70]}")
print("\nconnection notes (CONNECT):")
for x in conn.execute("SELECT shared,src_uri,other_uri FROM connection_notes"):
    print(f"   [{x['shared']}] {x['src_uri'].split('/')[-1][:40]} -> {x['other_uri'].split('/')[-1][:40]}")
