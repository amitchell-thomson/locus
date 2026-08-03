"""Do his PROJECTS and PAPERS reach coursework through a real shared concept?

One-off (2026-08-03). `connect_sources.py` showed CONNECT walks only `_recent_capture` —
recent handwriting — whose notes are short and name few entities, so the 81 bridge
canonicals are structurally unreachable. This asks the complementary question: starting
from his projects and papers instead, does `related_documents` surface coursework with a
SUBSTANTIVE shared concept? If yes, the cross-domain capability exists and is merely
unrouted; if no, coursework genuinely earns nothing.

Read-only. argv[1] = DB path.
"""

import sys

sys.path.insert(0, ".")
DB = sys.argv[1] if len(sys.argv) > 1 else "vault/locus.db"

from locus.agent.compose_daily import TEACHABLE_TYPES  # noqa: F401  (documents the bar)
from locus.db.connection import get_connection
from locus.link.related import non_topical_names, related_documents

conn = get_connection(DB)
generic = non_topical_names(conn)

srcs = conn.execute("""
  SELECT id, title, category FROM documents
  WHERE category IN ('project','paper') ORDER BY category, id""").fetchall()
print(f"{len(srcs)} project/paper sources\n")

hits = []
for s in srcs:
    for r in related_documents(conn, s["id"], top_n=6):
        row = conn.execute(
            "SELECT category, title FROM documents WHERE id=?", (r.doc_id,)
        ).fetchone()
        if not row or row["category"] != "coursework":
            continue
        good = [
            n for n in (r.shared_names or [])
            if n.lower() not in generic and len(n) >= 6 and " " in n
        ]
        if good:
            hits.append((s, row, good))

print(f"=== project/paper -> coursework with a substantive shared concept: {len(hits)} ===\n")
for s, row, good in hits:
    print(f"  [{s['category']}] {str(s['title'])[:48]:48s}")
    print(f"     -> {str(row['title'])[:60]}")
    print(f"        shared: {', '.join(good[:4])}\n")
