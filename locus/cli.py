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
from pathlib import Path

from locus.config import load
from locus.db.connection import get_connection
from locus.ingest_pipeline import ingest_paths


def _open():
    return get_connection(load().paths.db)


def _facets(args):
    """Build a retrieval Facets from --since/--until/--category, validating date format.

    Returns None when no facet is set (the common case) so retrieval stays unrestricted.
    """
    from datetime import date

    from locus.retrieve import Facets

    for field in ("since", "until"):
        value = getattr(args, field, None)
        if value is not None:
            try:
                date.fromisoformat(value)
            except ValueError:
                print(f"--{field} must be an ISO date (YYYY-MM-DD); got {value!r}")
                sys.exit(1)
    facets = Facets(
        since=getattr(args, "since", None),
        until=getattr(args, "until", None),
        category=getattr(args, "category", None),
    )
    return facets if facets.active() else None


def _add_facet_args(parser) -> None:
    """Attach the shared --since/--until/--category retrieval facets to a subparser."""
    parser.add_argument("--since", default=None, help="only documents dated on/after this ISO date (YYYY-MM-DD)")
    parser.add_argument("--until", default=None, help="only documents dated on/before this ISO date (YYYY-MM-DD)")
    parser.add_argument("--category", default=None, help="only documents in this category (e.g. paper, project, note)")


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


def _print_results(results) -> None:
    for r in results:
        if r.status == "ingested":
            print(
                f"[ingested]  {r.path}  doc_id={r.doc_id} sections={r.sections} "
                f"chunks={r.chunks} props={r.propositions} entities={r.entities} "
                f"figures={r.figures}"
            )
        elif r.status == "skipped":
            print(f"[skipped]   {r.path}  (already ingested, doc_id={r.doc_id})")
        else:
            print(f"[{r.status}]  {r.path}  -- {r.error}")


def cmd_ingest(args) -> None:
    from locus.ingest_lock import IngestLockHeld, ingest_lock

    try:
        with ingest_lock():
            _print_results(ingest_paths(args.paths, reingest=args.reingest))
    except IngestLockHeld as exc:
        print(exc)
        sys.exit(1)


def cmd_sync(args) -> None:
    """Sync tracked code repos: re-ingest those whose git HEAD moved (or all with --force)."""
    import logging

    from locus.ingest_lock import IngestLockHeld, ingest_lock
    from locus.sync import sync_repos

    logging.basicConfig(level=logging.INFO, format="%(message)s")
    repos = [Path(p) for p in args.paths] if args.paths else None  # None -> config [repos]
    conn = _open()
    try:
        with ingest_lock():
            _print_results(sync_repos(conn, repos=repos, force=args.force))
    except IngestLockHeld as exc:
        print(exc)
        sys.exit(1)
    finally:
        conn.close()


def cmd_link(args) -> None:
    """Rebuild the entity-alias substrate (entity_aliases) over the stored corpus.

    Manual-only by design: the fuzzy tier makes billed Claude API calls (only for
    new/changed clusters — verdicts are cached), so it never rides the watcher. Run after
    ingest batches; a long-lived MCP server should be restarted afterwards (it caches the
    alias map per process).
    """
    from locus.link.aliases import build_aliases

    conn = _open()
    try:
        # build_aliases logs progress + the final report through `log`.
        build_aliases(
            conn,
            use_llm=False if args.no_llm else None,  # None -> config [alias].use_llm
            use_cache=not args.no_cache,
            log=print,
        )
    finally:
        conn.close()


def cmd_retitle(args) -> None:
    """Recompute distinctive '[Module — ][Seq: ]Topic' titles across the corpus.

    Manual-only, like `locus link`: needs the global view to break same-title collisions,
    and distils topics via billed Claude API calls (cached by content hash — re-runs after
    new ingests only call for unseen docs). Writes only documents.title (no re-ingest). Run
    after the pour; a long-lived MCP server need not restart (list/inspect read title live).
    --dry-run previews every change without writing; --rollback restores the prior titles.
    """
    from locus import retitle

    conn = _open()
    try:
        if args.rollback:
            retitle.rollback(conn, log=print)
            return
        report = retitle.build_titles(
            conn,
            use_llm=False if args.no_llm else None,  # None -> config [retitle].use_llm
            use_cache=not args.no_cache,
            dry_run=args.dry_run,
            log=print,
        )
        for doc_id, old, new in report.proposals:
            print(f"  [{doc_id}] {old!r}\n        -> {new!r}")
        if args.dry_run:
            print("(dry run — no titles written)")
    finally:
        conn.close()


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
    # Knowledge gaps only — pipeline audit-trail lines (OCR fallbacks, degraded passes)
    # belong to `locus audit`, not the content surface (round-5 audit finding).
    from locus.eval.metrics import semantic_gaps

    flags = semantic_gaps(json.loads(doc["gap_flags"] or "[]"))
    print(f"  gaps ({len(flags)}):")
    for g in flags:
        print(f"    - {g}")

    from locus.link.related import format_related

    print("-" * 88)
    for line in format_related(conn, doc_id):
        print(line)

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


def cmd_watch(args) -> None:
    import logging

    from locus.watcher import watch

    logging.basicConfig(level=logging.INFO, format="%(message)s")
    try:
        watch(interval=args.interval, once=args.once)
    except KeyboardInterrupt:
        print("\nstopped.")


def cmd_query(args) -> None:
    from locus.config import Config
    from locus.query import QUERY_MODES, answer

    try:
        Config.anthropic_api_key()  # fail early with a clear message if the key is missing
    except RuntimeError as exc:
        print(exc)
        sys.exit(1)
    if args.mode not in QUERY_MODES:
        print(f"unknown mode {args.mode!r}; choose from {sorted(QUERY_MODES)}")
        sys.exit(1)

    result = answer(args.query, mode=args.mode, facets=_facets(args))
    if result.low_confidence:
        from locus.retrieve.pipeline import confidence_banner
        print(confidence_banner(result.confidence_band) + "\n")
    print(result.answer)
    if result.figures_attached:
        print(f"\n[{result.figures_attached} figure image(s) were attached to the Claude call]")
    if result.citations:
        print("\n--- sources ---")
        for c in result.citations:
            print(f"  * {c}")


def cmd_retrieve(args) -> None:
    from locus.retrieve import retrieve

    r = retrieve(args.query, facets=_facets(args))
    print(f"query: {r.query}\n")
    if r.low_confidence:
        from locus.retrieve.pipeline import confidence_banner
        print(confidence_banner(r.confidence_band) + "\n")
    print("=== reranked survivors ===")
    for c in r.survivors:
        rr = f"{c.rerank_score:+.2f}" if c.rerank_score is not None else "n/a"
        print(f"  {c.kind:<11} doc{c.doc_id} rr={rr} src={sorted(c.sources)} | {c.text[:72]!r}")
    print("\n=== citations ===")
    for cit in r.citations:
        print("  *", cit)
    if r.figures:
        from locus.config import load as _load

        raw = _load().paths.raw_store
        print("\n=== figures (images attach at generation) ===")
        for f in r.figures:
            unit = "slide" if f.kind == "slide" else f"p.{f.page}"
            print(f"  * [{unit}] \"{f.doc_title}\" — {raw / f.raw_path}")
    if args.context:
        print("\n=== assembled context ===")
        print(r.context)


def cmd_mcp(args) -> None:
    from locus.db.migrate import migrate

    cfg = load()
    migrate(cfg.paths.db)  # ensure the schema is current before serving
    try:
        from locus.mcp_server import run
    except ImportError:
        print("The MCP server needs the 'mcp' package. Install it with: uv add mcp")
        sys.exit(1)
    # The billable `query` tool is exposed only if config or the flag opts in (default off).
    run(enable_query=args.enable_query or cfg.mcp.enable_query)


def cmd_audit(args) -> None:
    from locus.eval.metrics import (
        alias_qc, corpus_metrics, doc_metrics, format_alias_qc, format_metrics,
    )

    conn = _open()
    docs = [doc_metrics(conn, int(args.doc))] if args.doc else corpus_metrics(conn)
    print(format_metrics(docs))
    if not args.doc:  # alias substrate is corpus-level, not per-doc
        print()
        print(format_alias_qc(alias_qc(conn)))
    conn.close()


def cmd_eval(args) -> None:
    from locus.config import Config

    suites = ["judge", "math", "retrieval"] if args.suite == "full" else [args.suite]
    if suites != ["retrieval"]:  # retrieval is local-only; the judged suites need the API
        try:
            Config.anthropic_api_key()  # fail early with a clear message if the key is missing
        except RuntimeError as exc:
            print(exc)
            sys.exit(1)

    conn = _open()
    try:
        if "judge" in suites:
            from locus.eval.harness import evaluate

            models = args.models.split(",") if args.models else [None]
            for model in models:
                label = model or "existing (DB / qwen ingest)"
                print(f"\n=== Ingest-quality judge: {label}  (sample={args.sample}, seed={args.seed}) ===")
                judged, agg = evaluate(
                    conn, sample=args.sample, seed=args.seed, doc_id=args.doc,
                    model=model, judge_model=args.judge_model,
                )
                for k, v in agg.items():
                    print(f"  {k:<32} {v:.2f}")
        if "math" in suites:
            from locus.eval import math_eval

            print(f"\n=== Math fidelity (gate metric)  (sample={args.sample}, seed={args.seed}) ===")
            results, agg = math_eval.evaluate_math_fidelity(
                conn, sample=args.sample, seed=args.seed, judge_model=args.judge_model
            )
            print(math_eval.format_results(results, agg))
        if "retrieval" in suites:
            from locus.eval import retrieval_eval

            print("\n=== Labelled retrieval (recall@k / MRR) ===")
            results, agg = retrieval_eval.evaluate_retrieval(conn)
            print(retrieval_eval.format_results(results, agg))
            print("\n=== Link substrate (related-document pairs) ===")
            lines, lagg = retrieval_eval.score_links(conn)
            for line in lines:
                print(line)
            for k, v in lagg.items():
                print(f"  {k:<24} {v:.3f}")
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

    pw = sub.add_parser("watch", help="auto-ingest files dropped into vault/incoming/")
    pw.add_argument("--interval", type=float, default=5.0, help="poll interval seconds")
    pw.add_argument("--once", action="store_true", help="process the backlog once and exit")
    pw.set_defaults(func=cmd_watch)

    ps = sub.add_parser("sync", help="re-ingest tracked code repos whose git HEAD moved")
    ps.add_argument("paths", nargs="*", help="repo paths (default: config [repos].paths)")
    ps.add_argument("--force", action="store_true", help="re-ingest even if HEAD is unchanged")
    ps.set_defaults(func=cmd_sync)

    pq = sub.add_parser("query", help="ask the vault a question (retrieve + Claude); needs ANTHROPIC_API_KEY")
    pq.add_argument("query", help="the question")
    pq.add_argument(
        "--mode", default="standard",
        help="standard | gap | synthesis | code | framing | project",
    )
    _add_facet_args(pq)
    pq.set_defaults(func=cmd_query)

    pr = sub.add_parser("retrieve", help="run the retrieval pipeline and show what it returns")
    pr.add_argument("query", help="the query text")
    pr.add_argument("--context", action="store_true", help="also print the assembled context")
    _add_facet_args(pr)
    pr.set_defaults(func=cmd_retrieve)

    pm = sub.add_parser("mcp", help="run the MCP server over stdio (for stdio-over-SSH clients)")
    pm.add_argument(
        "--enable-query", action="store_true",
        help="also expose the server-side `query` tool (makes billed Claude API calls)",
    )
    pm.set_defaults(func=cmd_mcp)

    pk = sub.add_parser(
        "link",
        help="rebuild the cross-document entity-alias substrate (Claude API for fuzzy clusters)",
    )
    pk.add_argument(
        "--no-llm", action="store_true",
        help="deterministic tiers only (no API; fuzzy clusters stay unmerged)",
    )
    pk.add_argument(
        "--no-cache", action="store_true",
        help="re-adjudicate every fuzzy cluster, ignoring cached verdicts",
    )
    pk.set_defaults(func=cmd_link)

    pr = sub.add_parser(
        "retitle",
        help="recompute distinctive document titles corpus-wide (Claude API for topics)",
    )
    pr.add_argument(
        "--no-llm", action="store_true",
        help="deterministic thesis-clause topics only (no API)",
    )
    pr.add_argument(
        "--no-cache", action="store_true",
        help="re-distil every doc's topic, ignoring cached topics",
    )
    pr.add_argument(
        "--dry-run", action="store_true",
        help="print every proposed title change without writing",
    )
    pr.add_argument(
        "--rollback", action="store_true",
        help="restore titles from the most recent pre-run snapshot",
    )
    pr.set_defaults(func=cmd_retitle)

    pa = sub.add_parser("audit", help="structural ingest-quality metrics (no API)")
    pa.add_argument("--doc", default=None, help="restrict to one document id")
    pa.set_defaults(func=cmd_audit)

    pe = sub.add_parser("eval", help="quality evals: judge / math fidelity / labelled retrieval")
    pe.add_argument(
        "--suite", choices=["judge", "math", "retrieval", "full"], default="judge",
        help="judge = ingest-quality LLM judge; math = page-image math-fidelity gate; "
        "retrieval = labelled recall@k/MRR (local-only); full = all three",
    )
    pe.add_argument("--sample", type=int, default=8, help="number of sections/pages to sample")
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
