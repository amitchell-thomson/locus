"""Why has CONNECT never once offered a coursework->quant connection?

One-off (2026-08-03). §16 justifies keeping 144 coursework documents on cross-domain
transfer ("eigenvectors in factor models vs modal analysis"), and 81 bridge canonicals
exist in the substrate — yet every connection_note and every thread link ever written is
quant<->quant. This prints, for each source the connection writer actually walks, the
related documents it sees and where coursework falls, so the cause is read rather than
guessed.

Read-only. argv[1] = DB path.
"""

import sqlite3
import sys

sys.path.insert(0, ".")
DB = sys.argv[1] if len(sys.argv) > 1 else "vault/locus.db"
sys.path.insert(0, ".")
from locus.db.connection import get_connection
conn = get_connection(DB)
conn.row_factory = sqlite3.Row

from locus.agent import compose_daily as cd
from locus.link.related import related_documents

srcs = cd._recent_capture(conn, limit=12)
print(f"_recent_capture returned {len(srcs)} sources\n")

cw_seen = 0
for s in srcs:
    cat = conn.execute("SELECT category FROM documents WHERE id=?", (s["id"],)).fetchone()
    print(f"--- src {s['id']} [{cat['category'] if cat else '?'}] {(s['source_uri'])[:70]}")
    rels = related_documents(conn, s["id"], top_n=8)
    if not rels:
        print("     (no related documents)")
    for i, r in enumerate(rels, 1):
        row = conn.execute(
            "SELECT category, title FROM documents WHERE id=?", (r.doc_id,)
        ).fetchone()
        c = row["category"] if row else "?"
        if c == "coursework":
            cw_seen += 1
        shared = ", ".join(r.shared_names[:3]) if r.shared_names else "-"
        flag = "  <== COURSEWORK" if c == "coursework" else ""
        print(f"   {i}. [{c:10s}] {str(row['title'])[:52]:52s} shared={shared}{flag}")
    print()

print(f"coursework appearances in top-8 across all sources: {cw_seen}")
