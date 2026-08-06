"""The TRUE ceiling of CONNECT supply (2026-08-06). Read-only.

`connection_candidates` walks 12 recent notes + 60 papers/projects, takes the top-5
related docs of each and emits AT MOST ONE pair per source. That is a walk, not the pool.
This enumerates the pool directly from the substrate: every canonical that passes the
same gate (teachable type, multi-word, >=6 chars, not non-topical), every document pair
it induces, classified by what each side IS.

argv[1] = DB path.
"""

import itertools
import sqlite3
import sys
from collections import Counter, defaultdict

sys.path.insert(0, ".")
DB = sys.argv[1] if len(sys.argv) > 1 else "vault/locus.db"
from locus.db.connection import get_connection

conn = get_connection(DB)
conn.row_factory = sqlite3.Row

from locus.agent import state
from locus.agent.compose_daily import TEACHABLE_TYPES, _MIN_TEACHABLE_CHARS
from locus.link.related import _CANON_CTE, non_topical_names


def hdr(s):
    print(f"\n{'=' * 78}\n{s}\n{'=' * 78}")


# --- document classification -------------------------------------------------------------
own_clause, own_params = state.owner_authored_sql("d")
docs = {}
for r in conn.execute(
    f"SELECT d.id, d.title, d.category, d.source_type, d.source_uri, ({own_clause}) AS own "
    f"FROM documents d",
    own_params,
):
    docs[r["id"]] = dict(r)


def klass(d):
    if d["own"]:
        return "own-note"
    if d["source_type"] == "code":
        return "code"
    return d["category"] or "?"


hdr("0. corpus by class")
for k, n in Counter(klass(d) for d in docs.values()).most_common():
    print(f"  {k:12s} {n}")

# --- qualifying canonicals ---------------------------------------------------------------
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

hdr("1. canonical funnel")
print(f"  canonicals in canon_docs                : {len(canon_docs)}")
multi = {n: d for n, d in canon_docs.items() if len(d) >= 2}
print(f"  ... spanning >=2 documents              : {len(multi)}")


def passes(n):
    s = n.strip()
    if len(s) < _MIN_TEACHABLE_CHARS or " " not in s:
        return False
    if s.lower() in generic:
        return False
    return s.lower() in teachable


step_len = {n: d for n, d in multi.items() if len(n.strip()) >= _MIN_TEACHABLE_CHARS}
step_ws = {n: d for n, d in step_len.items() if " " in n.strip()}
step_gen = {n: d for n, d in step_ws.items() if n.strip().lower() not in generic}
qual = {n: d for n, d in step_gen.items() if n.strip().lower() in teachable}
print(f"  ... len >= {_MIN_TEACHABLE_CHARS}                            : {len(step_len)}")
print(f"  ... multi-word                          : {len(step_ws)}")
print(f"  ... not non-topical                     : {len(step_gen)}")
print(f"  ... in TEACHABLE_TYPES  == QUALIFYING   : {len(qual)}")
print(f"\n  dropped by TEACHABLE_TYPES alone        : {len(step_gen) - len(qual)}")

hdr("2. qualifying canonicals by doc_freq")
df = Counter(len(d) for d in qual.values())
for k in sorted(df):
    print(f"  doc_freq {k:3d}: {df[k]}")

# --- pair enumeration --------------------------------------------------------------------
# Cap: a canonical in N docs induces N*(N-1)/2 pairs. Very common canonicals produce
# combinatorial junk; report with and without a doc_freq cap.
PAIR_DF_CAP = 12

pairs_all = defaultdict(set)     # (a,b) -> {canonical, ...}
pairs_capped = defaultdict(set)
for n, ds in qual.items():
    for a, b in itertools.combinations(sorted(ds), 2):
        pairs_all[(a, b)].add(n)
        if len(ds) <= PAIR_DF_CAP:
            pairs_capped[(a, b)].add(n)

hdr("3. document pairs induced by qualifying canonicals")
print(f"  all pairs                       : {len(pairs_all)}")
print(f"  pairs from canonicals df<={PAIR_DF_CAP}     : {len(pairs_capped)}")


def classify_pairs(pairs, label):
    print(f"\n  --- {label} ---")
    byclass = Counter()
    for (a, b) in pairs:
        if a not in docs or b not in docs:
            continue
        ka, kb = klass(docs[a]), klass(docs[b])
        byclass[tuple(sorted((ka, kb)))] += 1
    for k, n in byclass.most_common(25):
        print(f"    {k[0]:11s} <-> {k[1]:11s} : {n}")
    return byclass


classify_pairs(pairs_all, "all pairs by class")
classify_pairs(pairs_capped, f"pairs, canonical df<={PAIR_DF_CAP}")

# --- what CONNECT actually wants: one side is HIS -----------------------------------------
HIS = {"own-note", "project", "code"}
hdr(f"4. pairs with >=1 side in {sorted(HIS)} (what CONNECT is for)")
for label, pairs in (("all", pairs_all), (f"df<={PAIR_DF_CAP}", pairs_capped)):
    n_his = sum(
        1
        for (a, b) in pairs
        if a in docs and b in docs and (klass(docs[a]) in HIS or klass(docs[b]) in HIS)
    )
    print(f"  {label:9s}: {n_his}")

# --- reachability: which of these does the CURRENT walk ever see? -------------------------
hdr("5. reachability of the current walk")
from locus.agent import compose_daily as cd
from locus.link.related import related_documents

cap_srcs = cd._recent_capture(conn, limit=12)
br_srcs = cd._bridge_sources(conn, limit=60)
print(f"  capture sources : {len(cap_srcs)}")
print(f"  bridge sources  : {len(br_srcs)}")

walked = set()
for s in list(cap_srcs) + list(br_srcs):
    for rel in related_documents(conn, s["id"], top_n=5):
        walked.add((min(s["id"], rel.doc_id), max(s["id"], rel.doc_id)))
print(f"  distinct pairs the top-5 walk touches at all : {len(walked)}")
qualified_walked = walked & set(pairs_all)
print(f"  ... of which carry a qualifying canonical    : {len(qualified_walked)}")
print(f"  pairs in the pool the walk NEVER sees        : {len(set(pairs_all) - walked)}")

# widened walk
for tn in (10, 20, 40):
    w = set()
    for s in list(cap_srcs) + list(br_srcs):
        for rel in related_documents(conn, s["id"], top_n=tn):
            w.add((min(s["id"], rel.doc_id), max(s["id"], rel.doc_id)))
    print(f"  top_n={tn:2d}: walk touches {len(w):5d}, qualifying {len(w & set(pairs_all)):5d}")
