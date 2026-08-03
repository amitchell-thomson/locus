"""Is CHECK THIS empty because there are too few positions, or because the judge says no?

One-off (2026-08-03). `belief_tensions` holds 16 rows, all of them "judged, none found"
markers (empty `conflicts_with`, `dismissed_at == written_at`), so the Think page's
CHECK THIS section renders nothing every day. Two candidate causes, and they need
different fixes: not enough positions to contradict, or a judge that will not fire.

This re-runs `find_tensions` over EVERY stored position and prints what it saw — how
many neighbour claims were retrieved, and what verdict came back. Billed (one small
`claude -p` call per position, subscription runner).

Read-mostly: `find_tensions` does not write. argv[1] = DB path.
"""

import sys

sys.path.insert(0, ".")

from locus.db.connection import get_connection
from locus.evolve import trajectory as T

DB = sys.argv[1] if len(sys.argv) > 1 else "vault/locus.db"
conn = get_connection(DB)

rows = conn.execute(
    "SELECT id, subject_kind, subject_key, stance FROM belief_positions ORDER BY id"
).fetchall()
print(f"{len(rows)} stored positions; _MAX_NEIGHBOURS={T._MAX_NEIGHBOURS} "
      f"_MAX_DISTANCE={T._MAX_DISTANCE}\n")

found = 0
for r in rows:
    stance = r["stance"]
    neigh = (
        T._neighbour_positions(conn, stance, r["subject_kind"], r["subject_key"])
        + T._neighbour_propositions(conn, stance)
    )[: T._MAX_NEIGHBOURS]
    print(f"--- position {r['id']} [{r['subject_kind']}] {stance[:72]}")
    print(f"    neighbours retrieved: {len(neigh)}")
    if not neigh:
        print("    -> NO NEIGHBOURS: the judge is never even called\n")
        continue
    tensions = T.find_tensions(
        conn, stance, subject_kind=r["subject_kind"], subject_key=r["subject_key"]
    )
    if tensions:
        found += len(tensions)
        for t in tensions:
            print(f"    *** TENSION vs: {t.conflicts_with[:80]}")
            print(f"        reason: {t.reason[:80]}")
    else:
        print("    -> judge found none")
    print()

print(f"TOTAL tensions across {len(rows)} positions: {found}")
