"""How much of the link substrate is engineering coursework, and what would pruning cost?

The owner's question (2026-07-29): the corpus is 246/295 engineering coursework, and he suspects
it dilutes the system. This measures the claim instead of arguing it — what share of entities,
propositions and CROSS-DOCUMENT concepts come from coursework, how much of that is reachable only
through coursework, and what the eval labels depend on. Read-only; recommends nothing by itself.
"""

from __future__ import annotations

import sqlite3
from collections import defaultdict

DB = "/home/alec/server-projects/locus/vault/locus.db"
QUANT = ("paper", "project", "career", "note")


def main() -> None:
    conn = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
    conn.row_factory = sqlite3.Row

    cats = {r["id"]: r["category"] for r in conn.execute("SELECT id, category FROM documents")}
    total_docs = len(cats)
    cw_docs = {d for d, c in cats.items() if c == "coursework"}
    print(f"documents: {total_docs}  (coursework {len(cw_docs)}, other {total_docs - len(cw_docs)})")

    for table, label in (("propositions", "propositions"), ("chunks", "chunks"),
                         ("entities", "entity rows"), ("figures", "figures")):
        rows = conn.execute(f"SELECT doc_id, COUNT(*) n FROM {table} GROUP BY doc_id").fetchall()
        cw = sum(r["n"] for r in rows if r["doc_id"] in cw_docs)
        tot = sum(r["n"] for r in rows)
        print(f"  {label:<14} {tot:>7}   coursework {cw:>7} ({cw/tot:.0%})")

    # Canonical entity -> the categories of the documents it appears in.
    canon_cats: dict[str, set[str]] = defaultdict(set)
    canon_docs: dict[str, set[int]] = defaultdict(set)
    for r in conn.execute(
        """
        SELECT COALESCE(a.canonical_name, e.name) AS cname, e.doc_id
        FROM entities e
        LEFT JOIN entity_aliases a ON a.variant_name=e.name AND a.variant_type=e.type
        """
    ):
        canon_docs[r["cname"]].add(r["doc_id"])
        canon_cats[r["cname"]].add(cats.get(r["doc_id"]) or "?")

    cross = {c for c, d in canon_docs.items() if len(d) >= 2}
    cw_only = {c for c in cross if canon_cats[c] == {"coursework"}}
    quant_touching = {c for c in cross if canon_cats[c] & set(QUANT)}
    bridging = {c for c in cross if "coursework" in canon_cats[c] and canon_cats[c] & set(QUANT)}

    print(f"\ncross-document canonicals            : {len(cross)}")
    print(f"  reachable ONLY through coursework  : {len(cw_only)} ({len(cw_only)/len(cross):.0%})"
          "   <- lost entirely if coursework is pruned")
    print(f"  touching paper/project/career/note : {len(quant_touching)} "
          f"({len(quant_touching)/len(cross):.0%})")
    print(f"  BRIDGING coursework <-> quant work : {len(bridging)}"
          "   <- the genuinely valuable overlap")
    print("\n  bridging examples:")
    for c in sorted(bridging, key=lambda x: -len(canon_docs[x]))[:15]:
        print(f"    {len(canon_docs[c]):>3} docs  {c[:46]:<48} {sorted(canon_cats[c])}")

    # What survives a prune: cross-doc concepts among non-coursework documents only.
    keep = {d for d in cats if d not in cw_docs}
    survive = {c for c, d in canon_docs.items() if len(d & keep) >= 2}
    print("\nIF COURSEWORK WERE REMOVED ENTIRELY:")
    print(f"  cross-document canonicals surviving : {len(survive)} (from {len(cross)})")

    # Eval-label exposure: how many labelled queries point at coursework documents.
    try:
        import sys
        sys.path.insert(0, "/home/alec/server-projects/locus")
        from locus.eval.retrieval_eval import LABELS  # type: ignore

        cw_labels = 0
        for lab in LABELS:
            keys = getattr(lab, "expected", None) or getattr(lab, "relevant", None) or []
            if any("coursework" in str(k) for k in keys):
                cw_labels += 1
        print(f"  labelled eval queries citing coursework: {cw_labels}/{len(LABELS)}")
    except Exception as exc:
        print(f"  (eval labels not introspectable here: {exc})")


if __name__ == "__main__":
    main()
