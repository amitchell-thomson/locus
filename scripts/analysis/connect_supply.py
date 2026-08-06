"""CONNECT supply + quality baseline (2026-08-06). Read-only.

Measures what the CONNECT surface can offer TODAY, before any change:
  A. everything ever written (connection_notes) and everything ever shown
  B. today's candidate pool from the two live sources, with notes / no notes
  C. what the substantive_shared gate rejected (gate_log)

argv[1] = DB path.
"""

import sqlite3
import sys

sys.path.insert(0, ".")
DB = sys.argv[1] if len(sys.argv) > 1 else "vault/locus.db"
from locus.db.connection import get_connection

conn = get_connection(DB)
conn.row_factory = sqlite3.Row


def hdr(s):
    print(f"\n{'=' * 78}\n{s}\n{'=' * 78}")


hdr("A. connection_notes ever written")
rows = list(conn.execute("SELECT * FROM connection_notes ORDER BY written_at"))
print(f"total notes: {len(rows)}")
for r in rows:
    print(f"\n  [{r['written_at'][:10]}] shared={r['shared']!r}")
    print(f"    src   : {r['src_uri']}")
    print(f"    other : {r['other_uri']}")
    print(f"    prose : {r['prose']}")

hdr("A2. connection items ever SHOWN (daily_shown)")
try:
    shown = list(
        conn.execute(
            "SELECT * FROM daily_shown WHERE item_key LIKE 'conn:%' ORDER BY shown_at"
        )
    )
    print(f"shown connections: {len(shown)}")
    for s in shown:
        print(f"  {s['shown_at'][:10]}  {s['item_key'][:110]}")
except sqlite3.OperationalError as e:
    print("daily_shown:", e)

hdr("B. today's candidate pool (live connection_candidates)")
from locus.agent import compose_daily as cd
from locus.link.connect import stored_note

cands = cd.connection_candidates(conn)
print(f"candidates returned: {len(cands)}  "
      f"(capture={sum(1 for c in cands if not c.bridge)}, "
      f"bridge={sum(1 for c in cands if c.bridge)})")
withnote = 0
for c in cands:
    note = stored_note(conn, src_uri=c.src_uri, other_uri=c.other_uri, shared=c.shared)
    withnote += bool(note)
    print(f"\n  {'BRIDGE ' if c.bridge else 'CAPTURE'}  shared={c.shared!r}  note={'YES' if note else 'no'}")
    print(f"    his   : [{c.src_id}] {c.src_title[:80]}")
    print(f"    other : [{c.other_id}] {c.other_title[:80]}")
print(f"\ncandidates with stored prose: {withnote}/{len(cands)}")

hdr("C. gate_log: connect gates")
try:
    for g in conn.execute(
        "SELECT * FROM gate_log WHERE gate LIKE 'connect%' ORDER BY day DESC LIMIT 20"
    ):
        print(dict(g))
except sqlite3.OperationalError as e:
    print("gate_log:", e)
