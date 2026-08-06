"""What separates a good CONNECT item from `while loop`? (2026-08-06). Read-only.

n=12 written notes is too few to fit a threshold on, so this does the only honest thing:
prints every candidate DISCRIMINATOR side by side for all 12, so the owner can label and
we can see which feature actually orders them. Guessing a number is what CLAUDE.md §3
forbids.

Features per (his_doc, other_doc, shared):
  df        doc_freq of the shared canonical corpus-wide
  ents_h/o  entity occurrences of the concept in each side
  secs_h/o  distinct sections of each side anchoring the concept
  syn_h/o   concept appears in that side's synthesis (thesis/method/result)
  nqual     how many OTHER qualifying canonicals the pair also shares
  cats      category of each side

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
from locus.agent.compose_daily import TEACHABLE_TYPES, _MIN_TEACHABLE_CHARS
from locus.link.related import _CANON_CTE, non_topical_names

generic = non_topical_names(conn)
marks = ",".join("?" * len(TEACHABLE_TYPES))
teachable = {
    r["n"].lower()
    for r in conn.execute(
        f"SELECT DISTINCT canonical_name AS n FROM entity_aliases "
        f"WHERE canonical_type IN ({marks})",
        TEACHABLE_TYPES,
    )
}
canon_docs = defaultdict(set)
for r in conn.execute(f"WITH {_CANON_CTE} SELECT canonical_name, doc_id FROM canon_docs"):
    canon_docs[r["canonical_name"]].add(r["doc_id"])


def qualifies(n):
    s = (n or "").strip()
    return (
        len(s) >= _MIN_TEACHABLE_CHARS
        and " " in s
        and s.lower() not in generic
        and s.lower() in teachable
    )


own_clause, own_params = state.owner_authored_sql("d")


def doc_of(uri):
    return conn.execute(
        f"SELECT d.id, d.title, d.category, d.source_type, ({own_clause}) AS own "
        f"FROM documents d WHERE d.source_uri=?",
        (*own_params, uri),
    ).fetchone()


def feats(doc_id, shared):
    """occurrences, distinct sections, in-synthesis for one side."""
    row = conn.execute(
        "SELECT COUNT(*) AS n, COUNT(DISTINCT e.section_id) AS s FROM entities e "
        "JOIN entity_aliases a ON a.variant_name=e.name AND a.variant_type=e.type "
        "WHERE e.doc_id=? AND a.canonical_name=?",
        (doc_id, shared),
    ).fetchone()
    syn = conn.execute(
        "SELECT COALESCE(thesis,'')||' '||COALESCE(method,'')||' '||COALESCE(result,'') AS t "
        "FROM documents WHERE id=?",
        (doc_id,),
    ).fetchone()
    in_syn = shared.lower() in (syn["t"] or "").lower()
    return row["n"], row["s"], in_syn


print(
    f"{'shared':30s} {'df':>3s} {'ent_h':>5s} {'sec_h':>5s} {'syn_h':>5s} "
    f"{'ent_o':>5s} {'sec_o':>5s} {'syn_o':>5s} {'nq':>3s}  cats"
)
print("-" * 110)
for r in conn.execute("SELECT * FROM connection_notes ORDER BY written_at, id"):
    h, o = doc_of(r["src_uri"]), doc_of(r["other_uri"])
    if not h or not o:
        print(f"{r['shared'][:30]:30s}  (missing doc: "
              f"{'src' if not h else ''}{'other' if not o else ''})")
        continue
    df = len(canon_docs.get(r["shared"], ()))
    eh, sh, yh = feats(h["id"], r["shared"])
    eo, so, yo = feats(o["id"], r["shared"])
    nq = sum(
        1
        for n, ds in canon_docs.items()
        if h["id"] in ds and o["id"] in ds and qualifies(n) and n != r["shared"]
    )
    ch = "own" if h["own"] else (h["category"] or "?")
    co = "own" if o["own"] else (o["category"] or "?")
    print(
        f"{r['shared'][:30]:30s} {df:3d} {eh:5d} {sh:5d} {str(yh)[0]:>5s} "
        f"{eo:5d} {so:5d} {str(yo)[0]:>5s} {nq:3d}  {ch}<->{co}"
    )

print("\n\nOther qualifying concepts each written pair ALSO shares (was the best one chosen?)")
print("-" * 110)
for r in conn.execute("SELECT * FROM connection_notes ORDER BY written_at, id"):
    h, o = doc_of(r["src_uri"]), doc_of(r["other_uri"])
    if not h or not o:
        continue
    alts = sorted(
        n for n, ds in canon_docs.items() if h["id"] in ds and o["id"] in ds and qualifies(n)
    )
    print(f"\n  chosen={r['shared']!r}")
    print(f"    all qualifying shared: {alts}")
