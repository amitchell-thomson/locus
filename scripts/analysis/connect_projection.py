"""Supply x quality projection for CONNECT (2026-08-06). Read-only, no model calls.

Supply and quality have to move together, so counting a wider pool alone proves nothing.
For every pool pair involving HIS material (plus the coursework bridge the current rules
already serve), this computes the deterministic features a gate could use and reports how
many pairs survive each candidate predicate, per candidate SOURCE ARM.

Predicates measured (each derived from a measurement, none guessed as a final number —
they are the hypotheses the labelling experiment is meant to test):

  P_len  min(len(_doc_text(his)), len(_doc_text(other))) >= T
         measured: the one connection the owner called junk (`while loop`) has 288/319,
         every other written note has >= 635 on its thinner side.
  P_cent concept appears in a synthesis field on >=1 side, OR the pair shares >=2
         qualifying canonicals.  Rejects exactly `while loop` and `math fidelity` of 12.
  P_df   shared canonical doc_freq <= 12.

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
from locus.link import connect as C
from locus.link.related import _CANON_CTE, non_topical_names

own_clause, own_params = state.owner_authored_sql("d")
docs = {
    r["id"]: dict(r)
    for r in conn.execute(
        f"SELECT d.id, d.title, d.category, d.source_type, d.source_uri, d.source_date, "
        f"COALESCE(thesis,'')||' '||COALESCE(method,'')||' '||COALESCE(result,'') AS syn, "
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


def qualifies(n):
    s = (n or "").strip()
    return (
        len(s) >= _MIN_TEACHABLE_CHARS
        and " " in s
        and s.lower() not in generic
        and s.lower() in teachable
    )


qual = {n: ds for n, ds in canon_docs.items() if len(ds) >= 2 and qualifies(n)}
pairs = defaultdict(set)
for n, ds in qual.items():
    for a, b in itertools.combinations(sorted(ds), 2):
        pairs[(a, b)].add(n)

HIS = {"own-note", "code", "project"}
interest = {
    p for p in pairs
    if klass(docs[p[0]]) in HIS
    or klass(docs[p[1]]) in HIS
    or {klass(docs[p[0]]), klass(docs[p[1]])} in ({"coursework", "paper"}, {"paper"})
}
print(f"pairs under analysis: {len(interest)} "
      f"(his-side, coursework<->paper bridge, or paper<->paper)")

_text_cache: dict[tuple[int, str], int] = {}


def side_len(doc_id, shared):
    key = (doc_id, shared)
    if key not in _text_cache:
        _t, txt = C._doc_text(conn, docs[doc_id]["source_uri"], shared)
        _text_cache[key] = len(txt)
    return _text_cache[key]


# For each pair choose the concept the CURRENT rule would choose: rarest qualifying.
rows = []
for (a, b), names in interest and {p: pairs[p] for p in interest}.items():
    chosen = min(names, key=lambda n: (len(qual[n]), n))
    la, lb = side_len(a, shared=chosen), side_len(b, shared=chosen)
    in_syn = (
        chosen.lower() in (docs[a]["syn"] or "").lower()
        or chosen.lower() in (docs[b]["syn"] or "").lower()
    )
    rows.append(
        dict(
            a=a, b=b, shared=chosen, nqual=len(names), df=len(qual[chosen]),
            minlen=min(la, lb), in_syn=in_syn,
            ka=klass(docs[a]), kb=klass(docs[b]),
        )
    )

# --- candidate source arms ---------------------------------------------------------------
cap12 = {r["id"] for r in cd._recent_capture(conn, limit=12)}
cap_all = {i for i, d in docs.items() if d["own"] and d["source_date"]}
br = {r["id"] for r in cd._bridge_sources(conn, limit=60)}
code_ids = {i for i, d in docs.items() if d["source_type"] == "code"}


def arm_current(r):
    for s, t in ((r["a"], r["b"]), (r["b"], r["a"])):
        if s in cap12 and not docs[t]["own"]:
            return True
        if s in br and docs[t]["category"] == "coursework":
            return True
    return False


def arm_all_notes(r):
    for s, t in ((r["a"], r["b"]), (r["b"], r["a"])):
        if s in cap_all and not docs[t]["own"]:
            return True
        if s in br and docs[t]["category"] == "coursework":
            return True
    return False


def arm_plus_code(r):
    if arm_all_notes(r):
        return True
    # a repo of his against anything that is not another repo
    return (r["a"] in code_ids) != (r["b"] in code_ids)


def arm_plus_paper_paper(r):
    return arm_plus_code(r) or {r["ka"], r["kb"]} == {"paper"}


ARMS = [
    ("current (12 notes + bridge)", arm_current),
    ("+ all 46 owner-authored", arm_all_notes),
    ("+ code repos as a source", arm_plus_code),
    ("+ paper<->paper", arm_plus_paper_paper),
]

PREDS = [
    ("no quality gate", lambda r: True),
    ("P_cent (syn or nqual>=2)", lambda r: r["in_syn"] or r["nqual"] >= 2),
    ("P_len minlen>=600", lambda r: r["minlen"] >= 600),
    ("P_len minlen>=900", lambda r: r["minlen"] >= 900),
    ("P_cent AND P_len>=600", lambda r: (r["in_syn"] or r["nqual"] >= 2) and r["minlen"] >= 600),
    ("P_cent AND P_len>=600 AND df<=12",
     lambda r: (r["in_syn"] or r["nqual"] >= 2) and r["minlen"] >= 600 and r["df"] <= 12),
]

print(f"\n{'':34s}" + "".join(f"{p[0][:22]:>24s}" for p in PREDS))
for aname, afn in ARMS:
    sel = [r for r in rows if afn(r)]
    line = f"{aname:34s}"
    for _pn, pfn in PREDS:
        line += f"{sum(1 for r in sel if pfn(r)):>24d}"
    print(line)

print("\n\nBreakdown of the widest arm x tightest gate, by class:")
sel = [
    r for r in rows
    if arm_plus_paper_paper(r)
    and (r["in_syn"] or r["nqual"] >= 2)
    and r["minlen"] >= 600
    and r["df"] <= 12
]
for k, n in Counter(tuple(sorted((r["ka"], r["kb"]))) for r in sel).most_common():
    print(f"  {k[0]:11s} <-> {k[1]:11s} : {n}")

print("\n\nSample of NEW pairs the widened arm + gate would offer (code side, top 25):")
shown = 0
for r in sorted(sel, key=lambda r: -r["nqual"]):
    if arm_current(r):
        continue
    if "code" not in (r["ka"], r["kb"]):
        continue
    print(f"\n  [{r['ka']}] {(docs[r['a']]['title'] or '')[:55]}")
    print(f"  [{r['kb']}] {(docs[r['b']]['title'] or '')[:55]}")
    print(f"      shared={r['shared']!r} df={r['df']} nqual={r['nqual']} minlen={r['minlen']}")
    shown += 1
    if shown >= 25:
        break

print("\n\nSample of NEW paper<->paper pairs (top 12):")
shown = 0
for r in sorted(sel, key=lambda r: -r["nqual"]):
    if arm_current(r) or {r["ka"], r["kb"]} != {"paper"}:
        continue
    print(f"\n  {(docs[r['a']]['title'] or '')[:60]}")
    print(f"  {(docs[r['b']]['title'] or '')[:60]}")
    print(f"      shared={r['shared']!r} df={r['df']} nqual={r['nqual']} minlen={r['minlen']}")
    shown += 1
    if shown >= 12:
        break
