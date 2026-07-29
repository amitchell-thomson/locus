"""Measure whether promoting shared sub-phrases would unify fragmented concepts.

THE PROBLEM (measured 2026-07-29): 81% of canonical entities span exactly ONE document, so they
can never become Concept objects. The corpus has no bare `machine learning` at all — only `ML`,
`deep learning`, and hapax compounds (`hybrid econometric-machine learning systems`, `financial
machine learning research`, `VARIMA-machine learning architectures`). The alias layer cannot help:
its deterministic tiers need the same string, and its LLM tier merges lookalikes into an EXISTING
member surface — so with no short surface attested, there is nothing to merge into.

THE CANDIDATE FIX: don't merge surfaces into each other — extract the shared SUB-PHRASE and
promote it to a canonical in its own right. `machine learning` appears inside compounds from N
different documents, so it is attested cross-document even though it never appears alone.

This script measures the yield before any of it is built: how many new cross-document concepts
would appear, and what the junk rate looks like. Read-only.
"""

from __future__ import annotations

import re
import sqlite3
import sys
from collections import defaultdict

sys.path.insert(0, str(__import__("pathlib").Path(__file__).resolve().parents[2]))

from locus.ingest.summarize import _GENERIC  # noqa: E402

DB = "/home/alec/server-projects/locus/vault/locus.db"
# Sub-phrase lengths worth promoting. 1 word is too generic ("model", "risk"); >4 is already
# as specific as the compound it came from.
NGRAM_MIN, NGRAM_MAX = 2, 4
# Structural words that must not start or end a promoted phrase — a phrase hinging on them
# ("of machine", "learning for") is a fragment, not a concept.
_EDGE_STOP = {
    "a", "an", "the", "of", "for", "in", "on", "to", "with", "and", "or", "by", "from", "at",
    "as", "via", "using", "based", "its", "their", "this", "that", "these", "those", "is", "are",
}
_WORD = re.compile(r"[A-Za-z][A-Za-z0-9+#-]*")


def ngrams(name: str):
    words = [w.lower() for w in _WORD.findall(name)]
    for n in range(NGRAM_MIN, NGRAM_MAX + 1):
        for i in range(len(words) - n + 1):
            gram = words[i : i + n]
            if gram[0] in _EDGE_STOP or gram[-1] in _EDGE_STOP:
                continue
            # An all-generic phrase says nothing distinctive about any document.
            if all(w in _GENERIC or w in _EDGE_STOP for w in gram):
                continue
            yield " ".join(gram)


def main() -> None:
    conn = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
    conn.row_factory = sqlite3.Row

    # canonical -> set(doc_ids)
    canon_docs: dict[str, set[int]] = defaultdict(set)
    for r in conn.execute(
        """
        SELECT COALESCE(a.canonical_name, e.name) AS cname, e.doc_id
        FROM entities e
        LEFT JOIN entity_aliases a ON a.variant_name=e.name AND a.variant_type=e.type
        """
    ):
        canon_docs[r["cname"]].add(r["doc_id"])

    existing_cross = {c for c, d in canon_docs.items() if len(d) >= 2}
    existing_lower = {c.lower() for c in canon_docs}
    print(f"canonicals                     : {len(canon_docs)}")
    print(f"  already cross-document (>=2) : {len(existing_cross)}")
    print(f"  single-document              : {len(canon_docs) - len(existing_cross)}")

    # Promoted phrase -> docs reached through every canonical containing it.
    phrase_docs: dict[str, set[int]] = defaultdict(set)
    phrase_parents: dict[str, set[str]] = defaultdict(set)
    for canon, docs in canon_docs.items():
        for g in set(ngrams(canon)):
            phrase_docs[g] |= docs
            phrase_parents[g].add(canon)

    # A promotion is only interesting if it is NEW (not already a canonical) and reaches >=2 docs
    # through >=2 DIFFERENT parent surfaces — one parent means we just renamed a single concept.
    promoted = {
        p: d for p, d in phrase_docs.items()
        if len(d) >= 2 and p not in existing_lower and len(phrase_parents[p]) >= 2
    }
    print(f"\nNEW cross-document concepts from promotion: {len(promoted)}")
    reachable = set().union(*(phrase_parents[p] for p in promoted)) if promoted else set()
    newly = {c for c in reachable if len(canon_docs[c]) < 2}
    print(f"  single-doc canonicals they absorb        : {len(newly)}")

    print("\nTOP 30 BY DOCUMENT REACH")
    for p, d in sorted(promoted.items(), key=lambda kv: -len(kv[1]))[:30]:
        parents = sorted(phrase_parents[p])[:3]
        print(f"  {len(d):>3} docs  {p:<34} <- {', '.join(x[:30] for x in parents)}")

    for probe in ("machine learning", "neural network", "portfolio optimization", "regime"):
        hit = phrase_docs.get(probe)
        print(f"\nPROBE {probe!r}: {len(hit) if hit else 0} docs, "
              f"{len(phrase_parents.get(probe, ()))} parent surfaces")
        for parent in sorted(phrase_parents.get(probe, ()))[:6]:
            print(f"    <- {parent}  ({len(canon_docs[parent])} docs)")


if __name__ == "__main__":
    main()
