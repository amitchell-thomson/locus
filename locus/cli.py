"""Locus CLI — the product surface.

Phase-1 subcommands:
  locus ingest <paths...>     ingest files into the vault
  locus list                  list ingested documents
  locus inspect <doc>         show what was ingested for a document (Layer-0 quality check)

(`query` arrives in Stage 7.)
"""

from __future__ import annotations

import argparse
import json
import sys

from locus.config import load
from locus.db.connection import get_connection
from locus.ingest_pipeline import ingest_paths


def _open():
    return get_connection(load().paths.db)


def _resolve_doc(conn, ident: str):
    """Resolve a document by numeric id, or by a substring of its title / source path."""
    if ident.isdigit():
        return conn.execute("SELECT * FROM documents WHERE id=?", (int(ident),)).fetchone()
    rows = conn.execute(
        "SELECT * FROM documents WHERE title LIKE ? OR source_uri LIKE ? ORDER BY id",
        (f"%{ident}%", f"%{ident}%"),
    ).fetchall()
    if len(rows) == 1:
        return rows[0]
    if not rows:
        return None
    print("Multiple documents match — narrow it, or use the id:")
    for r in rows:
        print(f"  [{r['id']}] {r['title']}")
    return None


def cmd_ingest(args) -> None:
    for r in ingest_paths(args.paths, reingest=args.reingest):
        if r.status == "ingested":
            print(
                f"[ingested]  {r.path}  doc_id={r.doc_id} sections={r.sections} "
                f"chunks={r.chunks} props={r.propositions} entities={r.entities}"
            )
        elif r.status == "skipped":
            print(f"[skipped]   {r.path}  (already ingested, doc_id={r.doc_id})")
        else:
            print(f"[{r.status}]  {r.path}  -- {r.error}")


def cmd_list(args) -> None:
    conn = _open()
    rows = conn.execute(
        """
        SELECT d.id, d.title, d.source_type,
          (SELECT COUNT(*) FROM sections s     WHERE s.doc_id=d.id) AS secs,
          (SELECT COUNT(*) FROM chunks c       WHERE c.doc_id=d.id) AS chunks,
          (SELECT COUNT(*) FROM propositions p WHERE p.doc_id=d.id) AS props,
          (SELECT COUNT(*) FROM entities e     WHERE e.doc_id=d.id) AS ents
        FROM documents d ORDER BY d.id
        """
    ).fetchall()
    conn.close()
    if not rows:
        print("No documents ingested yet.")
        return
    for r in rows:
        print(
            f"[{r['id']}] {r['title']}  ({r['source_type']}; {r['secs']} sections, "
            f"{r['chunks']} chunks, {r['props']} props, {r['ents']} entities)"
        )


def cmd_inspect(args) -> None:
    conn = _open()
    doc = _resolve_doc(conn, args.doc)
    if not doc:
        print(f"No (unique) document matches {args.doc!r}. Try `locus list`.")
        conn.close()
        sys.exit(1)

    doc_id = doc["id"]
    bar = "=" * 88
    print(bar)
    print(f"[{doc_id}] {doc['title']}")
    print(f"  source : {doc['source_uri']}")
    print(f"  type   : {doc['source_type']} | ingested {doc['ingested_at']} | model {doc['ingest_model']}")
    print("-" * 88)
    print("SYNTHESIS")
    for field in ("thesis", "method", "result", "limitations"):
        print(f"  {field:<12}: {doc[field]}")
    flags = json.loads(doc["gap_flags"] or "[]")
    print(f"  gaps ({len(flags)}):")
    for g in flags:
        print(f"    - {g}")

    section_map = {m["position"]: m for m in json.loads(doc["section_map"] or "[]")}
    sections = conn.execute(
        "SELECT * FROM sections WHERE doc_id=? ORDER BY position", (doc_id,)
    ).fetchall()
    by_pos = {s["position"]: s for s in sections}
    positions = [args.section] if args.section is not None else sorted(by_pos)

    for pos in positions:
        s = by_pos.get(pos)
        if s is None:
            continue
        pm = section_map.get(pos, {})
        print(f"\n### Section {pos}: {s['title']!r}  (pp {pm.get('page_start','?')}-{pm.get('page_end','?')})")
        if args.source:
            chunks = conn.execute(
                "SELECT raw_text FROM chunks WHERE section_id=? ORDER BY position", (s["id"],)
            ).fetchall()
            src = "\n".join(c["raw_text"] for c in chunks)[: args.max_source]
            print("  --- SOURCE (reconstructed from chunks) ---")
            print("  " + src.replace("\n", "\n  "))
            print("  --- EXTRACTED ---")
        print("  SUMMARY:")
        print(f"    {s['summary']}")
        props = conn.execute(
            "SELECT text FROM propositions WHERE section_id=? ORDER BY position", (s["id"],)
        ).fetchall()
        print(f"  PROPOSITIONS ({len(props)}):")
        for p in props:
            print(f"    - {p['text']}")
        ents = conn.execute(
            "SELECT name, type FROM entities WHERE section_id=? ORDER BY type, name", (s["id"],)
        ).fetchall()
        print(f"  ENTITIES ({len(ents)}):")
        for e in ents:
            print(f"    - {e['name']} ({e['type']})")
    conn.close()


def cmd_audit(args) -> None:
    from locus.eval.metrics import corpus_metrics, doc_metrics, format_metrics

    conn = _open()
    docs = [doc_metrics(conn, int(args.doc))] if args.doc else corpus_metrics(conn)
    conn.close()
    print(format_metrics(docs))


def cmd_eval(args) -> None:
    from locus.config import Config
    from locus.eval.harness import evaluate

    try:
        Config.anthropic_api_key()  # fail early with a clear message if the key is missing
    except RuntimeError as exc:
        print(exc)
        sys.exit(1)

    models = args.models.split(",") if args.models else [None]
    conn = _open()
    try:
        for model in models:
            label = model or "existing (DB / qwen ingest)"
            print(f"\n=== Evaluating: {label}  (sample={args.sample}, seed={args.seed}) ===")
            judged, agg = evaluate(
                conn, sample=args.sample, seed=args.seed, doc_id=args.doc,
                model=model, judge_model=args.judge_model,
            )
            for k, v in agg.items():
                print(f"  {k:<32} {v:.2f}")
    finally:
        conn.close()


def main(argv=None) -> None:
    parser = argparse.ArgumentParser(prog="locus", description="Locus — query-driven knowledge vault")
    sub = parser.add_subparsers(dest="cmd", required=True)

    pi = sub.add_parser("ingest", help="ingest files into the vault")
    pi.add_argument("paths", nargs="+", help="files to ingest")
    pi.add_argument(
        "--reingest", action="store_true",
        help="rebuild documents already present (delete + re-ingest) instead of skipping",
    )
    pi.set_defaults(func=cmd_ingest)

    pl = sub.add_parser("list", help="list ingested documents")
    pl.set_defaults(func=cmd_list)

    pn = sub.add_parser("inspect", help="show what was ingested for a document")
    pn.add_argument("doc", help="document id, or a substring of its title/path")
    pn.add_argument("--section", type=int, default=None, help="only this section position")
    pn.add_argument("--source", action="store_true", help="also show source text (from chunks)")
    pn.add_argument("--max-source", type=int, default=1200, help="max source chars to show")
    pn.set_defaults(func=cmd_inspect)

    pa = sub.add_parser("audit", help="structural ingest-quality metrics (no API)")
    pa.add_argument("--doc", default=None, help="restrict to one document id")
    pa.set_defaults(func=cmd_audit)

    pe = sub.add_parser("eval", help="LLM-as-judge ingest-quality eval (needs ANTHROPIC_API_KEY)")
    pe.add_argument("--sample", type=int, default=8, help="number of sections to sample")
    pe.add_argument("--seed", type=int, default=0, help="sampling seed (reproducible)")
    pe.add_argument("--doc", type=int, default=None, help="restrict to one document id")
    pe.add_argument(
        "--models", default=None,
        help="comma-separated models to regenerate+compare (e.g. qwen2.5:7b,llama3.1:8b); "
        "omit to judge the existing DB outputs",
    )
    pe.add_argument("--judge-model", default=None, help="override the Claude judge model")
    pe.set_defaults(func=cmd_eval)

    args = parser.parse_args(argv)
    args.func(args)


if __name__ == "__main__":
    main()
