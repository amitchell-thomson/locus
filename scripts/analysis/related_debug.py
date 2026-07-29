"""Why does 'downside-risk-prediction' -> 'tanker-flow' miss the related top-5, when the
reverse direction passes? Prints the full ranked neighbour list for the project docs and
for the handwriting notes (candidate new pairs)."""
from __future__ import annotations

from locus import config as locus_config
from locus.db.connection import get_connection
from locus.link.related import related_documents, resolve_stop_doc_freq

cfg = locus_config.load()
conn = get_connection(cfg.paths.db)
uris = {r["id"]: r["source_uri"] for r in conn.execute("SELECT id, source_uri FROM documents")}
titles = {r["id"]: r["title"] for r in conn.execute("SELECT id, title FROM documents")}
stop = resolve_stop_doc_freq(conn)
print(f"stop_doc_freq = {stop}\n")


def show(pattern: str, top_n: int = 8) -> None:
    for doc_id, uri in uris.items():
        if pattern.lower() not in (uri or "").lower():
            continue
        print(f"=== [{doc_id}] {titles[doc_id][:64]}")
        for i, rel in enumerate(related_documents(conn, doc_id, top_n=top_n, stop_doc_freq=stop), 1):
            tail = (uris.get(rel.doc_id) or "").rsplit("/", 1)[-1][:44]
            shared = getattr(rel, "shared", None) or getattr(rel, "shared_count", "?")
            print(f"   {i}. {tail:<46} shared={shared}")
        print()


for p in ("downside-risk-prediction", "tanker-flow"):
    show(p)

print("### candidate NOTE pairs — do the rates notes cluster? ###\n")
for p in ("swaps-b3f4d16b", "rates-foundations", "em-rates-trading", "swaps-momentum-strat"):
    show(p, top_n=5)

print("\n### reverse directions for candidate note pairs ###\n")
for p in ("em-ideas", "dashboard-70e000de", "jargon-sheet"):
    show(p, top_n=5)
