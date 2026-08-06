"""Which pool pairs can the CURRENT rules EVER reach? (2026-08-06). Read-only.

`connect_pool.py` showed 3,784 pairs carry a qualifying canonical while the live walk
touches 100. Ranking explains some of that. This separates the two causes by asking a
question ranking cannot affect: is the pair ELIGIBLE at all — is one side in the source
set, and does the far side pass the source's target rule?

  capture arm : source must be owner-authored AND among the 12 most recent by source_date;
                target must NOT be owner-authored.
  bridge  arm : source category in (paper, project) AND source_type != 'code';
                target category must be 'coursework'.

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

from locus.agent import compose_daily as cd
from locus.agent import state
from locus.agent.compose_daily import TEACHABLE_TYPES, _MIN_TEACHABLE_CHARS
from locus.link.related import _CANON_CTE, non_topical_names

own_clause, own_params = state.owner_authored_sql("d")
docs = {
    r["id"]: dict(r)
    for r in conn.execute(
        f"SELECT d.id, d.title, d.category, d.source_type, d.source_uri, d.source_date, "
        f"({own_clause}) AS own FROM documents d",
        own_params,
    )
}


def klass(d):
    if d["own"]:
        return "own-note"
    if d["source_type"] == "code":
        return "code"
    return d["category"] or "?"


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

print("=" * 78)
print("1. canonical funnel (re-print)")
print("=" * 78)
multi = {n: d for n, d in canon_docs.items() if len(d) >= 2}
step_len = {n: d for n, d in multi.items() if len(n.strip()) >= _MIN_TEACHABLE_CHARS}
step_ws = {n: d for n, d in step_len.items() if " " in n.strip()}
step_gen = {n: d for n, d in step_ws.items() if n.strip().lower() not in generic}
qual = {n: d for n, d in step_gen.items() if n.strip().lower() in teachable}
print(f"  canonicals in canon_docs        : {len(canon_docs)}")
print(f"  spanning >=2 documents          : {len(multi)}")
print(f"  len >= {_MIN_TEACHABLE_CHARS}                        : {len(step_len)}")
print(f"  multi-word                      : {len(step_ws)}")
print(f"  not non-topical                 : {len(step_gen)}")
print(f"  in TEACHABLE_TYPES (QUALIFYING) : {len(qual)}")

pairs = defaultdict(set)
for n, ds in qual.items():
    for a, b in itertools.combinations(sorted(ds), 2):
        pairs[(a, b)].add(n)
print(f"\n  pairs induced                   : {len(pairs)}")

# --- eligibility -------------------------------------------------------------------------
cap_ids = {r["id"] for r in cd._recent_capture(conn, limit=12)}
cap_all = {i for i, d in docs.items() if d["own"] and d["source_date"]}
br_ids = {r["id"] for r in cd._bridge_sources(conn, limit=60)}

print(f"\n  capture sources (limit 12)      : {len(cap_ids)}")
print(f"  ALL owner-authored w/ a date    : {len(cap_all)}")
print(f"  bridge sources (limit 60)       : {len(br_ids)}")


def eligible(a, b):
    for s, t in ((a, b), (b, a)):
        if s in cap_ids and not docs[t]["own"]:
            return "capture"
        if s in br_ids and docs[t]["category"] == "coursework":
            return "bridge"
    return None


def eligible_uncapped(a, b):
    """Same rules, but the capture arm walks ALL his writing, not the 12 newest."""
    for s, t in ((a, b), (b, a)):
        if s in cap_all and not docs[t]["own"]:
            return "capture"
        if s in br_ids and docs[t]["category"] == "coursework":
            return "bridge"
    return None


print("\n" + "=" * 78)
print("2. eligibility of the 3,784-pair pool under CURRENT rules")
print("=" * 78)
elig = Counter()
elig_classes = Counter()
inelig_classes = Counter()
for (a, b) in pairs:
    e = eligible(a, b)
    elig[e or "INELIGIBLE"] += 1
    key = tuple(sorted((klass(docs[a]), klass(docs[b]))))
    (elig_classes if e else inelig_classes)[key] += 1
for k, n in elig.most_common():
    print(f"  {k:12s} : {n}")
print("\n  eligible pairs by class:")
for k, n in elig_classes.most_common(15):
    print(f"    {k[0]:11s} <-> {k[1]:11s} : {n}")
print("\n  INELIGIBLE pairs by class (top 15):")
for k, n in inelig_classes.most_common(15):
    print(f"    {k[0]:11s} <-> {k[1]:11s} : {n}")

print("\n" + "=" * 78)
print("3. counterfactual: capture arm walks ALL his writing, not the newest 12")
print("=" * 78)
e2 = Counter()
e2_classes = Counter()
for (a, b) in pairs:
    e = eligible_uncapped(a, b)
    e2[e or "INELIGIBLE"] += 1
    if e:
        e2_classes[tuple(sorted((klass(docs[a]), klass(docs[b]))))] += 1
for k, n in e2.most_common():
    print(f"  {k:12s} : {n}")
print("\n  eligible pairs by class:")
for k, n in e2_classes.most_common(15):
    print(f"    {k[0]:11s} <-> {k[1]:11s} : {n}")

print("\n" + "=" * 78)
print("4. the arms that do not exist — pool pairs by class, HIS side involved")
print("=" * 78)
HIS = {"own-note", "code", "project"}
want = Counter()
for (a, b) in pairs:
    ka, kb = klass(docs[a]), klass(docs[b])
    if ka in HIS or kb in HIS:
        want[tuple(sorted((ka, kb)))] += 1
for k, n in want.most_common():
    print(f"    {k[0]:11s} <-> {k[1]:11s} : {n}")

print("\n" + "=" * 78)
print("5. code repos in the corpus (never walked as a source)")
print("=" * 78)
for i, d in docs.items():
    if d["source_type"] == "code":
        npairs = sum(1 for (a, b) in pairs if a == i or b == i)
        print(f"  [{i}] cat={d['category']:11s} pool_pairs={npairs:4d}  {(d['title'] or '')[:60]}")

print("\n" + "=" * 78)
print("6. non-code projects (bridge sources) and their pool reach")
print("=" * 78)
for i in sorted(br_ids):
    d = docs[i]
    npairs = sum(1 for (a, b) in pairs if a == i or b == i)
    cw = sum(
        1
        for (a, b) in pairs
        if (a == i and docs[b]["category"] == "coursework")
        or (b == i and docs[a]["category"] == "coursework")
    )
    print(f"  [{i}] cat={d['category']:9s} pool={npairs:4d} coursework={cw:3d}  "
          f"{(d['title'] or '')[:55]}")
