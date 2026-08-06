"""How much material does each side of a connection actually have? (2026-08-06). Read-only.

Question 3 of the brief: would giving the model MORE of each side produce deeper links, or
just longer ones? That is unanswerable without knowing whether the current prompt is even
FULL. `connect._doc_text` caps each side at 1400 chars from synthesis + <=3 section
summaries matching the concept by LIKE. This measures, for every written note and every
eligible pool pair:

  used      chars actually handed to the model per side today
  capped    did it hit _MAX_SIDE_CHARS
  secs      section summaries the LIKE found (0..3)
  avail_*   what ELSE exists for that (doc, concept): sections, propositions, chunks,
            marked passages (pdf_annotations)

Also: the _SAMPLE_NAMES=5 leak — pairs dropped because the qualifying canonical was not
among the 5 rarest shared names, even though one exists.

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
from locus.link.related import _CANON_CTE, _SAMPLE_NAMES, non_topical_names


def hdr(s):
    print(f"\n{'=' * 78}\n{s}\n{'=' * 78}")


own_clause, own_params = state.owner_authored_sql("d")
docs = {
    r["id"]: dict(r)
    for r in conn.execute(
        f"SELECT d.id, d.title, d.category, d.source_type, d.source_uri, "
        f"({own_clause}) AS own FROM documents d",
        own_params,
    )
}
by_uri = {d["source_uri"]: d for d in docs.values()}

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


def available(doc_id, shared):
    """What else the DB holds for this (doc, concept), beyond what the prompt uses."""
    like = f"%{shared}%"
    secs_like = conn.execute(
        "SELECT COUNT(*) n, COALESCE(SUM(LENGTH(summary)),0) c FROM sections "
        "WHERE doc_id=? AND summary LIKE ?",
        (doc_id, like),
    ).fetchone()
    # sections anchoring the concept by ENTITY, not by substring — the join the prompt ignores
    secs_ent = conn.execute(
        "SELECT COUNT(DISTINCT s.id) n, COALESCE(SUM(LENGTH(s.summary)),0) c FROM sections s "
        "JOIN entities e ON e.section_id=s.id "
        "JOIN entity_aliases a ON a.variant_name=e.name AND a.variant_type=e.type "
        "WHERE s.doc_id=? AND a.canonical_name=?",
        (doc_id, shared),
    ).fetchone()
    props = conn.execute(
        "SELECT COUNT(*) n, COALESCE(SUM(LENGTH(text)),0) c FROM propositions "
        "WHERE doc_id=? AND text LIKE ?",
        (doc_id, like),
    ).fetchone()
    chunks = conn.execute(
        "SELECT COUNT(*) n FROM chunks WHERE doc_id=? AND raw_text LIKE ?", (doc_id, like)
    ).fetchone()
    ann = conn.execute(
        "SELECT COUNT(*) n FROM pdf_annotations WHERE source_uri=?",
        (docs[doc_id]["source_uri"],),
    ).fetchone()
    allsecs = conn.execute(
        "SELECT COUNT(*) n FROM sections WHERE doc_id=?", (doc_id,)
    ).fetchone()
    return dict(
        secs_like=secs_like["n"], secs_like_chars=secs_like["c"],
        secs_ent=secs_ent["n"], secs_ent_chars=secs_ent["c"],
        props=props["n"], prop_chars=props["c"], chunks=chunks["n"],
        marks=ann["n"], all_secs=allsecs["n"],
    )


hdr("A. the 12 written notes — what the prompt USED vs what EXISTS")
print(f"_MAX_SIDE_CHARS = {C._MAX_SIDE_CHARS}\n")
print(f"{'shared':26s} {'side':5s} {'used':>5s} {'cap':>3s} {'sLIKE':>5s} {'sENT':>4s} "
      f"{'secs':>4s} {'prop':>4s} {'chnk':>4s} {'mark':>4s}")
print("-" * 92)
for r in conn.execute("SELECT * FROM connection_notes ORDER BY written_at, id"):
    for label, uri in (("his", r["src_uri"]), ("other", r["other_uri"])):
        d = by_uri.get(uri)
        if not d:
            print(f"{r['shared'][:26]:26s} {label:5s}  (document not found: {uri[:50]})")
            continue
        _t, text = C._doc_text(conn, uri, r["shared"])
        a = available(d["id"], r["shared"])
        print(
            f"{r['shared'][:26]:26s} {label:5s} {len(text):5d} "
            f"{'Y' if len(text) >= C._MAX_SIDE_CHARS else '.':>3s} "
            f"{a['secs_like']:5d} {a['secs_ent']:4d} {a['all_secs']:4d} "
            f"{a['props']:4d} {a['chunks']:4d} {a['marks']:4d}"
        )

hdr("B. does the LIKE section lookup ever find anything?")
hits = Counter()
for r in conn.execute("SELECT * FROM connection_notes"):
    for uri in (r["src_uri"], r["other_uri"]):
        d = by_uri.get(uri)
        if not d:
            continue
        a = available(d["id"], r["shared"])
        hits[("LIKE", a["secs_like"] > 0)] += 1
        hits[("ENTITY", a["secs_ent"] > 0)] += 1
print(f"  sides where a section summary CONTAINS the concept string : "
      f"{hits[('LIKE', True)]} / {hits[('LIKE', True)] + hits[('LIKE', False)]}")
print(f"  sides where a section ANCHORS the concept as an entity     : "
      f"{hits[('ENTITY', True)]} / {hits[('ENTITY', True)] + hits[('ENTITY', False)]}")

hdr("C. marked passages — the depth source the prompt never reads")
rows = list(
    conn.execute(
        "SELECT source_uri, COUNT(*) n, SUM(LENGTH(COALESCE(covered_text,''))) c "
        "FROM pdf_annotations GROUP BY source_uri ORDER BY n DESC"
    )
)
print(f"  documents with marks: {len(rows)}")
for r in rows:
    d = by_uri.get(r["source_uri"])
    print(f"    n={r['n']:4d} chars={r['c'] or 0:7d}  "
          f"{(d['title'] if d else r['source_uri'])[:60]}")

hdr(f"D. the _SAMPLE_NAMES={_SAMPLE_NAMES} leak — qualifying concept outside the sample")
from locus.link.related import _shared_names, related_documents

cap_srcs = list(cd._recent_capture(conn, limit=12))
br_srcs = list(cd._bridge_sources(conn, limit=60))
leak = Counter()
examples = []
for s in cap_srcs + br_srcs:
    for rel in related_documents(conn, s["id"], top_n=5):
        a, b = s["id"], rel.doc_id
        true_shared = sorted(
            n for n, ds in canon_docs.items() if a in ds and b in ds and qualifies(n)
        )
        sampled_ok = any(qualifies(n) for n in (rel.shared_names or ()))
        if true_shared and not sampled_ok:
            leak["hidden"] += 1
            if len(examples) < 12:
                examples.append(
                    (docs[a]["title"], docs[b]["title"], list(rel.shared_names), true_shared)
                )
        elif true_shared:
            leak["visible"] += 1
        else:
            leak["none"] += 1
print(f"  pairs with a qualifying concept VISIBLE in the sample : {leak['visible']}")
print(f"  pairs with a qualifying concept HIDDEN by the sample  : {leak['hidden']}")
print(f"  pairs with no qualifying concept at all              : {leak['none']}")
for ht, ot, samp, true in examples:
    print(f"\n    {(ht or '')[:50]}  <->  {(ot or '')[:50]}")
    print(f"      sampled : {samp}")
    print(f"      hidden  : {true}")
