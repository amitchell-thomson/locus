"""Gate hypothesis: shared concept ATTESTED IN BOTH SIDES' assembled context (2026-08-06).

The 'Markov model' probe showed the failure the entity gate cannot see: a qualifying,
teachable concept whose anchoring on one side is an extraction hallucination (a VLE
thermodynamics section). If the concept string does not literally appear in the text we
hand the model, the model is being asked to bluff a connection.

Test: for each of the 12 written notes plus known probes, is the concept present
(casefold) in side_text_v2 of each side? Read-only.

argv[1] = DB path.
"""

import sqlite3
import sys
from collections import defaultdict

sys.path.insert(0, ".")
DB = sys.argv[1] if len(sys.argv) > 1 else "vault/locus.db"
from locus.db.connection import get_connection

conn = get_connection(DB)
conn.row_factory = sqlite3.Row

from locus.agent import state

own_clause, own_params = state.owner_authored_sql("d")
docs_by_uri = {}
docs = {}
for r in conn.execute(
    f"SELECT d.id, d.title, d.category, d.source_type, d.source_uri, "
    f"d.thesis, d.method, d.result, ({own_clause}) AS own FROM documents d",
    own_params,
):
    docs[r["id"]] = dict(r)
    docs_by_uri[r["source_uri"]] = dict(r)


def side_text_v2(doc_id, concepts, cap=2800):
    d = docs[doc_id]
    parts = [p for p in (d["thesis"], d["method"], d["result"]) if p]
    marks_c = ",".join("?" * len(concepts))
    anchored = conn.execute(
        f"SELECT DISTINCT s.id, s.summary, s.file_path FROM sections s "
        f"JOIN entities e ON e.section_id=s.id "
        f"JOIN entity_aliases a ON a.variant_name=e.name AND a.variant_type=e.type "
        f"WHERE s.doc_id=? AND a.canonical_name IN ({marks_c}) "
        f"AND COALESCE(s.summary,'')!='' "
        f"ORDER BY (s.file_path IS NULL OR lower(s.file_path) LIKE '%.md') DESC LIMIT 6",
        (doc_id, *concepts),
    ).fetchall()
    parts += [r["summary"] for r in anchored]
    if d["source_type"] == "code":
        seen = {r["id"] for r in anchored}
        for r in conn.execute(
            "SELECT id, summary FROM sections WHERE doc_id=? "
            "AND lower(COALESCE(file_path,'')) LIKE '%.md' AND COALESCE(summary,'')!='' "
            "ORDER BY position LIMIT 3",
            (doc_id,),
        ):
            if r["id"] not in seen:
                parts.append(r["summary"])
    return " ".join(parts)[:cap]


def attested(doc_id, concept):
    return concept.lower() in side_text_v2(doc_id, [concept]).lower()


print("The 12 written notes (owner-implied labels: while loop = junk):")
print(f"{'shared':30s} {'his':>4s} {'other':>6s}  verdict-if-gated")
for r in conn.execute("SELECT * FROM connection_notes ORDER BY written_at, id"):
    h = docs_by_uri.get(r["src_uri"])
    o = docs_by_uri.get(r["other_uri"])
    if not h or not o:
        print(f"{r['shared'][:30]:30s}  (doc missing)")
        continue
    ah, ao = attested(h["id"], r["shared"]), attested(o["id"], r["shared"])
    verdict = "PASS" if (ah and ao) else "REJECT"
    print(f"{r['shared'][:30]:30s} {str(ah)[0]:>4s} {str(ao)[0]:>6s}  {verdict}")

print("\nProbes:")
for a, b, concept in ((204, 453, "Markov model"),):
    print(f"  {concept!r} {docs[a]['title'][:30]} <-> {docs[b]['title'][:30]}: "
          f"his={attested(a, concept)} other={attested(b, concept)}")
