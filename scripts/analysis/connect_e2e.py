"""End-to-end verification of the redesigned CONNECT path (2026-08-06).

Runs the REAL nightly path — `cli._write_connection_notes` → `link/connect.write_note`
(live Sonnet calls) → `compose_daily.build_connections` — against a THROWAWAY snapshot
DB, then prints what the page would show. CLAUDE.md §3: verify against real output, not
against the tests.

argv[1] = throwaway DB path. argv[2] = how many notes to write (default 8).
"""

import sqlite3
import sys

sys.path.insert(0, ".")
DB = sys.argv[1]
N = int(sys.argv[2]) if len(sys.argv) > 2 else 8

from locus.db.connection import get_connection

conn = get_connection(DB)
conn.row_factory = sqlite3.Row

from locus.cli import _write_connection_notes

written = _write_connection_notes(conn, limit=N)
print(f"writer wrote {written} note(s)\n")

print("=" * 90)
print("connection_notes written this run (newest first):")
for r in conn.execute(
    "SELECT * FROM connection_notes ORDER BY written_at DESC LIMIT ?", (N + 4,)
):
    print(f"\n  [{r['written_at'][:16]}] shared={r['shared']!r}")
    print(f"    src   : {r['src_uri'][:90]}")
    print(f"    other : {r['other_uri'][:90]}")
    print(f"    prose : {r['prose'] or '(NO_CONNECTION verdict)'}")

print("\n" + "=" * 90)
print("what build_connections would print tomorrow (seen=empty, limit=3):")
from locus.agent import compose_daily as cd

for item in cd.build_connections(conn, limit=3, seen=set()):
    print(f"\n  headline: {item.headline}")
    print(f"  context : {item.context}")

print("\n" + "=" * 90)
print("gate log for this run:")
for g in conn.execute(
    "SELECT * FROM gate_log WHERE gate LIKE 'connect%' ORDER BY day DESC, gate LIMIT 6"
):
    print(f"  {g['day']} {g['gate']}: rejected={g['rejected']} passed={g['passed']} "
          f"samples={g['samples'][:100]}")
