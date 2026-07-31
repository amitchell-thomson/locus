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


def cmd_watch_repo(args) -> None:
    """Watch tracked code repos and incrementally re-ingest moved HEADs (own process).

    Mutually exclusive with `locus watch` through the shared ingest lock — run them as two
    processes and neither interrupts the other's ingest.
    """
    import logging

    from locus.watcher import watch_repos

    logging.basicConfig(level=logging.INFO, format="%(message)s")
    try:
        watch_repos(once=args.once)
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

    r = retrieve(args.query, facets=_facets(args), include_excluded=args.include_excluded)
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


def _backup_root(args) -> Path:
    """Backup destination: --dest if given, else vault/backups (sibling of the DB)."""
    if getattr(args, "dest", None):
        return Path(args.dest)
    return load().paths.db.parent / "backups"


def _fmt_bytes(n: float) -> str:
    for unit in ("B", "KiB", "MiB", "GiB"):
        if n < 1024 or unit == "GiB":
            return f"{n:.0f} {unit}" if unit == "B" else f"{n:.1f} {unit}"
        n /= 1024
    return f"{n:.1f} GiB"


def cmd_backup(args) -> None:
    """Snapshot the durable corpus state (DB + raw store + authored notes)."""
    from locus import backup as bk

    cfg = load()
    root = _backup_root(args)

    if args.list:
        snaps = bk.list_snapshots(root)
        if not snaps:
            print(f"No snapshots under {root}")
            return
        for s in snaps:
            m = bk.read_manifest(s)
            if m:
                print(f"  {s.name}  ({m.doc_count} docs, db {_fmt_bytes(m.db_bytes)}, "
                      f"raw {_fmt_bytes(m.raw_bytes)} / {m.raw_files} files, sha {m.git_sha})")
            else:
                print(f"  {s.name}  (no manifest)")
        return

    print(f"Backing up to {root} ...")
    snapshot = bk.create_backup(
        db=cfg.paths.db, raw_store=cfg.paths.raw_store, notes=cfg.paths.notes,
        backup_root=root, log=print,
    )
    m = bk.read_manifest(snapshot)
    print(f"\nSnapshot: {snapshot}")
    if m:
        print(f"  {m.doc_count} docs | db {_fmt_bytes(m.db_bytes)} ({m.db_page_count} pages) "
              f"| raw {_fmt_bytes(m.raw_bytes)} ({m.raw_files} files)"
              + (f" | hardlinked vs {m.linked_from}" if m.linked_from else ""))


def cmd_restore(args) -> None:
    """Restore the corpus from a snapshot (DESTRUCTIVE — overwrites live DB/raw/notes)."""
    from locus import backup as bk
    from locus.ingest_lock import IngestLockHeld, ingest_lock

    cfg = load()
    snapshot = Path(args.snapshot)
    if not snapshot.is_absolute() and not snapshot.exists():
        snapshot = _backup_root(args) / snapshot  # accept a bare snapshot name
    if not snapshot.exists():
        print(f"No such snapshot: {snapshot}")
        sys.exit(1)

    m = bk.read_manifest(snapshot)
    print(f"Restore from {snapshot}")
    if m:
        print(f"  {m.doc_count} docs, db {_fmt_bytes(m.db_bytes)}, raw {m.raw_files} files, "
              f"taken {m.created_at} (code sha {m.git_sha})")
    if not args.yes:
        print("\nThis OVERWRITES the live DB, raw store, and notes. Re-run with --yes to proceed.")
        return
    # The ingest lock guarantees no writer is mid-transaction while we swap the DB file out.
    try:
        with ingest_lock():
            bk.restore_backup(
                snapshot, db=cfg.paths.db, raw_store=cfg.paths.raw_store,
                notes=cfg.paths.notes, log=print,
            )
    except IngestLockHeld as exc:
        print(exc)
        sys.exit(1)
    print("Restore complete. Restart any long-lived `locus mcp` server.")


def cmd_status(args) -> None:
    """One-screen operational health summary (no API; local only)."""
    from locus.config import PROJECT_ROOT
    from locus.status import collect_status, format_status

    cfg = load()
    conn = _open()
    try:
        report = collect_status(
            conn,
            db_path=cfg.paths.db,
            incoming=cfg.paths.incoming,
            backup_root=_backup_root(args),
            project_root=PROJECT_ROOT,
        )
    finally:
        conn.close()
    print(format_status(report))


def cmd_export_obsidian(args) -> None:
    """Render the corpus to a read-only Obsidian vault (§13; joins-only, no API).

    Manual-only, like `link`/`retitle`: it reads the alias substrate, so run it AFTER
    `locus link` (it warns and degrades to a docs-only graph otherwise). The output is
    regenerable and gitignored; view it from the Mac by rsync-pulling the tree — see
    docs/obsidian-projection-plan.md §10.
    """
    from locus.export import export_vault

    cfg = load()
    out_dir = Path(args.dest) if args.dest else cfg.obsidian.out_dir
    conn = _open()
    try:
        report = export_vault(
            conn,
            out_dir,
            top_related=args.top_related if args.top_related is not None else cfg.obsidian.top_related,
            include_excluded=args.include_excluded,
            min_cluster_docs=cfg.obsidian.min_cluster_docs,
            emit_entity_notes=cfg.obsidian.emit_entity_notes and not args.no_entity_notes,
            db_path=cfg.paths.db,
            log=print,
        )
    finally:
        conn.close()
    print(
        f"\nExported to {report.out_dir}\n"
        f"  {report.doc_notes} doc notes | {report.entity_notes} entity notes | "
        f"{report.related_edges} related edges | {report.pruned} pruned"
    )
    for w in report.warnings:
        print(f"  ! {w}")
    print(
        "\nView from the Mac (plan §10):\n"
        "  rsync -az --delete --exclude '.obsidian/' "
        f"<host>:{report.out_dir}/ ~/LocusVault/"
    )


def cmd_read(args) -> None:
    """Render markdown -> a device-tuned PDF and push it to the reMarkable (agent-layer §8.5).

    The server->device push channel (rmapi put) — proven here, relied on by the daily page and
    every reading loop. `<path>` is a markdown file or a directory (each *.md rendered + pushed).
    `--no-push` renders locally only (writes PDFs beside the source or to --out), which is also
    the offline path when the device is asleep.
    """
    from locus.reading.deliver_remarkable import deliver_pdf
    from locus.reading.md2pdf import PageGeometry, render_markdown_file

    cfg = load().reading
    geometry = PageGeometry(
        width_in=cfg.page_width_in,
        height_in=cfg.page_height_in,
        margin_in=cfg.margin_in,
        font_pt=cfg.font_pt,
    )
    folder = args.to or cfg.target_folder

    src = Path(args.path)
    if src.is_dir():
        md_files = sorted(src.glob("*.md"))
        if not md_files:
            print(f"no .md files under {src}")
            sys.exit(1)
    elif src.is_file():
        md_files = [src]
    else:
        print(f"not found: {src}")
        sys.exit(1)

    out_dir = Path(args.out) if args.out else None
    for md in md_files:
        out_pdf = (out_dir / f"{md.stem}.pdf") if out_dir else md.with_suffix(".pdf")
        render_markdown_file(md, out_pdf, geometry=geometry)
        if args.no_push:
            print(f"rendered {md.name} -> {out_pdf}")
            continue
        result = deliver_pdf(
            out_pdf,
            remote_folder=folder,
            rmapi_binary=cfg.rmapi_binary,
        )
        note = " (created folder)" if result.created_folder else ""
        print(f"delivered {result.filename} -> reMarkable:/{result.remote_folder}{note}")


def cmd_daily(args) -> None:
    """Compose the daily reMarkable page and push it as an annotatable PDF (agent-layer §9).

    Aggregates only — no model call, no proposal, no spend. The page renders whether or not
    last night's structure run succeeded, which is §9's "degrades silently if an agent didn't
    run" guardrail; an empty day is a valid, calm page, not an error.
    """
    from locus.agent import compose_daily as cd
    from locus.reading.deliver_remarkable import deliver_pdf
    from locus.reading.md2pdf import PageGeometry, render_markdown_file
    from locus.vault.writer import write_generated_note

    cfg = load()
    conn = _open()
    try:
        page = cd.compose(conn)
        body = cd.render(page)

        out_dir = Path(args.out) if args.out else Path(cfg.paths.notes) / "_generated"
        out_dir.mkdir(parents=True, exist_ok=True)
        md_path = out_dir / "_home.md"
        # `_generated/` is corpus-excluded (notes_sync) so the page never re-enters retrieval —
        # invariant 5, no feedback contamination from the agent's own output.
        write_generated_note(
            md_path, body, run_id=f"daily:{page.page_date}",
            extra={"title": f"Daily {page.page_date}"},
        )

        # DATED filename, one device document per day. Two reasons, both load-bearing:
        # yesterday's page must survive on the tablet until it has been pulled back (the
        # two-way loop reads the annotations off it), and rmapi REFUSES a same-named
        # re-upload rather than duplicating it — a fixed name works once and fails every
        # day after (caught by running the systemd unit at deploy, 2026-07-30).
        pdf_path = out_dir / f"daily-{page.page_date}.pdf"
        if not args.no_render:
            r = cfg.reading
            render_markdown_file(
                md_path, pdf_path,
                geometry=PageGeometry(
                    width_in=r.page_width_in, height_in=r.page_height_in,
                    margin_in=r.margin_in, font_pt=r.font_pt,
                ),
            )

        cd.persist(
            conn, page,
            md_path=str(md_path),
            pdf_path=str(pdf_path) if not args.no_render else None,
        )

        counts = (
            f"{len(page.connections)} connection(s) · {len(page.recalls)} recall · "
            f"{len(page.readings)} read-next · {len(page.blessings)} awaiting"
        )
        print(f"composed {page.page_date}: {counts}")
        print(f"  {md_path}")
        if args.no_render:
            return
        if args.no_push:
            print(f"  {pdf_path}")
            return
        res = deliver_pdf(
            pdf_path,
            remote_folder=args.to or cfg.reading.target_folder,
            rmapi_binary=cfg.reading.rmapi_binary,
            # Same-day rebuilds revise today's page; different days never collide.
            replace=True,
        )
        note = " (created folder)" if res.created_folder else ""
        print(f"  delivered {res.filename} -> reMarkable:/{res.remote_folder}{note}")
    finally:
        conn.close()


def cmd_daily_pull(args) -> None:
    """Pull an annotated daily page back and route what was written on it (agent-layer §9).

    Idempotent by (page date, anchor): re-running against the same scan revises each region
    rather than adding a second one, so a page read twice cannot double-grade a recall answer
    or bless an object twice. Billed — one vision call per page.
    """
    from locus.agent.promote import promote_all
    from locus.agent.pull_daily import pull_daily

    conn = _open()
    promoted = []
    try:
        result = pull_daily(conn, args.pdf, page_date=args.date, force=args.force)
        if result.status == "routed" and not args.no_promote:
            # Anything he DEVELOPED becomes a note, so the next `locus notes-sync` puts his own
            # thinking into the corpus. Without this the loop ends in a side table that no
            # retrieval arm can see (locus/agent/promote.py).
            promoted = [p for p in promote_all(conn) if p.wrote]
    finally:
        conn.close()

    if result.status == "not-on-device":
        # Not an error: the page is simply still on the tablet, or untouched and never re-pushed.
        print(f"{result.page_date}: not pushed back from the device yet — nothing to read.")
        return
    if result.status == "unchanged":
        print(f"{result.page_date}: unchanged since the last pull — skipped (no model call).")
        return

    for o in result.outcomes:
        print(f"  {o.anchor:<4} {o.kind:<11} {o.outcome:<18} {o.detail}")
    if result.unknown_anchors:
        # Never guessed at: an anchor we did not print is not something we can route.
        print(f"  ignored unknown anchors: {', '.join(sorted(result.unknown_anchors))}")
    print(f"{result.page_date}: {result.acted} region(s) acted on of {len(result.outcomes)}")
    for p in promoted:
        print(f"  promoted ({p.status}): {p.path.name}  [{p.entries} pass(es)]")
    if promoted:
        print("  run `locus notes-sync` to bring them into the corpus")


def cmd_promote(args) -> None:
    """Write developed threads out as notes so his own thinking enters the corpus (§15).

    Free and local — no model. Only OWNER-authored text is written: agent rationale must never
    re-enter the corpus as if it were his (locus/agent/promote.py).
    """
    from locus.agent.promote import promote_all, unpromoted_count

    conn = _open()
    try:
        if args.count:
            print(f"{unpromoted_count(conn)} thread(s) carry your development and are not notes yet")
            return
        results = promote_all(conn, notes_dir=args.out)
    finally:
        conn.close()

    wrote = [p for p in results if p.wrote]
    for p in wrote:
        print(f"  {p.status:<9} {p.path}  [{p.entries} pass(es)]")
    print(
        f"{len(wrote)} note(s) written, {len(results) - len(wrote)} already current"
        if results else "nothing to promote — no thread carries development yet"
    )
    if wrote:
        print("run `locus notes-sync` to ingest them")


def cmd_annotate(args) -> None:
    """Loop B: pull a PDF's reMarkable annotations and record which passage each one marks.

    Reads the CLOUD copy (`rmapi get` -> .rmdoc), so it works with the tablet powered off. The
    linking is geometry, not vision: strokes are mapped into PDF page coordinates and
    intersected with the text layer, so no model is called and nothing is billed.
    """
    import logging
    import tempfile

    # Progress logging ON. `--ingest` can run a 200-page book for tens of minutes, and the
    # pipeline already emits per-section progress ("section 12/87 done") — but this command
    # never configured logging, so every one of those lines was discarded. Three attempts at the
    # book ingest were debugged from CPU counters and socket tables because of it, when the log
    # would have said which section it stopped on. `ingest`/`sync` have always done this.
    logging.basicConfig(level=logging.INFO, format="%(message)s")

    from locus.capture.annotate import marks_for_document, store_marks
    from locus.capture.rmdoc import fetch_rmdoc, read_rmdoc

    if args.rmdoc:
        path = Path(args.rmdoc)
    else:
        tmp = tempfile.mkdtemp(prefix="locus-rmdoc-")
        print(f"fetching {args.device_path!r} from the reMarkable cloud ...")
        path = fetch_rmdoc(args.device_path, tmp)

    doc = read_rmdoc(path)
    marks = marks_for_document(doc)
    source_uri = args.source_uri or args.device_path or str(path)

    ingested = None
    if args.ingest:
        # The annotated PDF itself belongs in the corpus. Without this the marks key on a DEVICE
        # PATH that joins to no document, so the passages and his notes are invisible to
        # retrieval and to `locus link` — a book he read and annotated leaves no trace the rest
        # of the system can use. The source PDF is already inside the bundle we downloaded.
        from locus.config import load as _load
        from locus.ingest_pipeline import ingest_file

        name = (args.device_path or path.stem).rstrip("/").rsplit("/", 1)[-1]
        dest = Path(_load().paths.incoming) / args.category / f"{name}.pdf"
        dest.parent.mkdir(parents=True, exist_ok=True)
        if not dest.exists():
            dest.write_bytes(doc.pdf_bytes)
        print(f"ingesting {dest} (this is not quick — 200+ pages) ...")

    conn = _open()
    transcribed = 0
    reading = None
    try:
        if args.ingest:
            # UNDER THE INGEST LOCK. `ingest_file` is called directly here rather than through
            # `cmd_ingest`, and the first cut skipped the lock — so a 40-minute book ingest could
            # run concurrently with the half-hourly capture-sync timer, which also ingests.
            # Concurrent ingests contend on Ollama and produce spurious quarantines (§14), and
            # the failure would land on the OTHER job, making it hard to attribute.
            from locus.ingest_lock import IngestLockHeld, ingest_lock

            try:
                with ingest_lock():
                    ingested = ingest_file(dest, conn, category=args.category)
            except IngestLockHeld as exc:
                print(f"{exc}\nmarks stored; re-run with --ingest when the other ingest finishes")
                ingested = None
            if ingested is not None and ingested.status in ("ingested", "skipped"):
                row = conn.execute(
                    "SELECT source_uri FROM documents WHERE id=?", (ingested.doc_id,)
                ).fetchone()
                if row:
                    # Re-key anything captured earlier under the device path so he does not
                    # have to re-annotate the book to connect it up.
                    from locus.capture.annotate import rekey_marks

                    moved = rekey_marks(conn, old_uri=source_uri, new_uri=row["source_uri"])
                    source_uri = row["source_uri"]
                    print(f"ingested doc {ingested.doc_id}; re-keyed {moved} existing mark(s)")
            elif ingested is not None:
                print(f"ingest {ingested.status}: {ingested.reason or ''} — marks keyed by path")

        written = store_marks(conn, marks, source_uri=source_uri, doc_uuid=doc.doc_uuid)
        if args.transcribe:
            # The other half of Loop B: the geometry says WHICH passage he marked, this says
            # what he wrote beside it. Billed — one vision call per handwritten mark, and only
            # for ink that has never been read (locus/capture/mark_text.py).
            from locus.capture.mark_text import transcribe_marks

            transcribed = transcribe_marks(
                conn, marks, source_uri=source_uri, limit=args.max_transcribe
            )
        if args.notes:
            # His margin notes are HIS writing and belong in the corpus as such — the passages
            # they mark belong to the book. Emitted as a note, quoting each passage as evidence.
            from locus.agent.promote import promote_reading_notes

            reading = promote_reading_notes(conn, source_uri)
    finally:
        conn.close()

    from collections import Counter

    kinds = Counter(m.kind for m in marks)
    print(f"{len(doc.pages)} annotated page(s), {len(marks)} mark(s): {dict(kinds)}")
    for m in marks:
        text = (m.covered_text or m.line_text or "").strip()
        print(f"  p{m.pdf_page + 1:<5} {m.kind:<12} {text[:88]}")
    if args.transcribe:
        print(f"transcribed {transcribed} handwritten mark(s)")
    if reading is not None:
        print(f"reading notes ({reading.status}): {reading.path}  [{reading.entries} note(s)]")
        print("  run `locus notes-sync` to bring them into the corpus")
    else:
        from locus.capture.mark_text import has_ink

        pending = sum(1 for m in marks if has_ink(m))
        if pending:
            print(f"{pending} mark(s) carry handwriting — `--transcribe` to read them (billed)")
    print(f"stored {written} mark(s) against {source_uri}")


def cmd_capture_sync(args) -> None:
    """Loop A: transcribe + fill-in + enrich + ingest staged reMarkable handwriting renders.

    Each changed <uuid>.pdf in the staging dir becomes a `rough` note (Sonnet vision transcription,
    Haiku fill-in, grounded Related block), then note-sync ingests it. Idempotent — an unchanged
    render is skipped. Billed (metered vision + subscription text); the run is journaled.
    """
    from locus.capture.loop_a import capture_sync

    conn = _open()
    try:
        r = capture_sync(conn, staging_dir=args.staging, ingest=not args.no_ingest)
    finally:
        conn.close()
    for o in r.outcomes:
        if o.status == "captured":
            print(f"  captured  {o.name}  ({o.pages}pp · {o.illegible} illegible · {o.uncertain} [?] · "
                  f"{o.filled} filled · {o.related} related)  -> {o.note_path}")
        elif o.status == "failed":
            print(f"  FAILED    {o.name}: {o.error}")
    n_unchanged = sum(o.status == "unchanged" for o in r.outcomes)
    n_failed = sum(o.status == "failed" for o in r.outcomes)
    print(f"\ncapture: {r.captured} captured, {n_unchanged} unchanged, {n_failed} failed, "
          f"{len(r.unmapped)} unmapped   est cost ${r.cost_usd:.4f}")
    if r.ingest:
        print(f"ingest: {r.ingest.ingested} ingested, {r.ingest.skipped} unchanged, "
              f"{r.ingest.deleted} deleted, {r.ingest.failed} failed")


def cmd_structure(args) -> None:
    """Propose structured objects + belief positions from ingested documents (agent-layer §6.2-6.3).

    Objects land as `status=proposed`; the owner blesses them with `locus objects --bless <id>`.
    `--dry-run` runs every precision gate against the real corpus and writes NOTHING — the way to
    check the proposal bar before turning this loose corpus-wide.
    """
    from locus.structure.propose import structure_documents

    conn = _open()
    try:
        doc_ids = _resolve_structure_docs(conn, args)
        if not doc_ids:
            print("No documents match. Use --doc, --category, or --since.")
            return
        print(f"Structuring {len(doc_ids)} document(s){' (DRY RUN — no writes)' if args.dry_run else ''}…")
        out = structure_documents(conn, doc_ids, dry_run=args.dry_run)
        for res in out.per_doc:
            plan = res.plan
            if plan is None:
                continue
            header = f"[{res.doc_id}] {plan.doc_title}"
            items = plan.objects if args.dry_run else None
            if args.dry_run and (plan.objects or plan.positions or (args.verbose and plan.rejected)):
                print(f"\n{header}")
                for o in items or []:
                    print(f"  + {o.type:<9} {o.title}   ({len(o.links)} links)")
                    if o.why:
                        print(f"      why: {o.why}")
                for p in plan.positions:
                    print(f"  ~ position [{p.subject_kind}] {p.dated_at}: {p.stance[:110]}")
                if args.verbose:
                    for r in plan.rejected:
                        print(f"  - rejected {r.kind} {r.subject!r}: {r.reason}")
            elif not args.dry_run and (res.created or res.updated or res.positions):
                print(f"{header}: {len(res.created)} new, {len(res.updated)} updated, "
                      f"{res.positions} position(s)")
        verb = "would create/update" if args.dry_run else "created"
        print(f"\nstructure: {out.documents} docs · {out.created} {verb} · {out.updated} updated · "
              f"{out.positions} positions · {out.rejected} rejected · {out.degraded} degraded "
              f"· ${out.cost_usd:.4f}")
    finally:
        conn.close()


def _resolve_structure_docs(conn, args) -> list[int]:
    """Which documents to structure: explicit --doc ids, else a category/date/maturity selection."""
    if args.doc:
        return [int(d) for d in args.doc]
    clauses, params = [], []
    if args.category:
        clauses.append("category = ?")
        params.append(args.category)
    if args.since:
        clauses.append("COALESCE(source_date, ingested_at) >= ?")
        params.append(args.since)
    if getattr(args, "ingested_since", None):
        # Distinct from --since, which filters on the AUTHORED date. A scheduled run needs
        # "what has arrived since I last looked", and a handwritten note authored weeks ago but
        # ingested last night must be structured — --since would skip it. Without this a nightly
        # timer either re-bills every document or silently misses the new ones.
        clauses.append("ingested_at >= ?")
        params.append(args.ingested_since)
    if args.maturity:
        clauses.append("maturity = ?")
        params.append(args.maturity)
    where = ("WHERE " + " AND ".join(clauses)) if clauses else ""
    sql = f"SELECT id FROM documents {where} ORDER BY id"
    if args.limit:
        sql += f" LIMIT {int(args.limit)}"
    return [r["id"] for r in conn.execute(sql, params)]


def cmd_objects(args) -> None:
    """List / bless / archive proposed structured objects (the human half of propose-never-mutate).

    Blessing is the ONLY way an object becomes `active`; agents may re-propose into its body but
    never change its status.
    """
    from locus.agent import state

    conn = _open()
    try:
        if args.bless or args.archive:
            for raw in args.bless or []:
                ok = state.set_status(conn, int(raw), "active")
                state.log_acceptance(conn, surface="object", candidate_key=str(raw),
                                     verdict="kept" if ok else "rejected")
                print(f"{'blessed' if ok else 'no such object'} {raw}")
            for raw in args.archive or []:
                ok = state.set_status(conn, int(raw), "archived")
                state.log_acceptance(conn, surface="object", candidate_key=str(raw),
                                     verdict="rejected")
                print(f"{'archived' if ok else 'no such object'} {raw}")
            return
        objects = state.list_objects(conn, type_=args.type, status=args.status, limit=args.limit)
        if not objects:
            print("No objects match.")
            return
        for obj in objects:
            print(f"[{obj.id}] {obj.status:<8} {obj.type:<9} {obj.title}")
            for key in ("why", "approach", "mastery", "state"):
                if obj.body.get(key):
                    print(f"      {key}: {obj.body[key]}")
            for thread in obj.body.get("open_threads", []):
                print(f"      · {thread}")
            for link in state.links_for(conn, obj.id):
                print(f"      -> {link.relation} {link.target_kind}:{link.target_key}")
        print(f"\n{len(objects)} object(s)")
    finally:
        conn.close()


def cmd_review(args) -> None:
    """Spaced-repetition review over the owner's own propositions (agent-layer §3.6, SM-2).

    With no flags, shows what is due. `--grade <item_id>:<0-5>` records a recall grade and
    reschedules. `--add-object <id>` schedules an object's practice material.
    """
    from locus.learn import review as rv

    conn = _open()
    try:
        for spec in args.grade or []:
            ref, _, quality = spec.partition(":")
            if not quality.isdigit():
                print(f"bad --grade {spec!r}; expected <item_id>:<0-5>")
                continue
            item = rv.grade_item(conn, int(ref), int(quality))
            print(f"item {ref}: next due {item.due} (interval {item.interval}d, "
                  f"ease {item.ease:.2f})" if item else f"no review item {ref}")
        if args.enrol:
            added = rv.enrol_from_blessed_objects(
                conn,
                **({} if args.enrol_max is None else {'max_new': args.enrol_max}),
            )
            if not added:
                print("nothing new to enrol (every candidate is already scheduled)")
            for item in added:
                text, source = rv.resolve_prompt(conn, item)
                print(f"enrolled item {item.id} (due {item.due}): {text[:72]}")
            return
        for oid in args.add_object or []:
            item = rv.schedule_prompt(conn, prompt_kind="object", prompt_ref=str(oid))
            print(f"scheduled object {oid} as review item {item.id} (due {item.due})")
        if args.grade or args.add_object:
            return

        due = rv.due_items(conn, limit=args.limit)
        if not due:
            print("Nothing due. (A calm empty state is a valid one — §9.)")
            return
        for item in due:
            text, source = rv.resolve_prompt(conn, item)
            where = f"  — {source}" if source else ""
            print(f"[{item.id}] due {item.due} (reps {item.reps}, ease {item.ease:.2f}){where}")
            print(f"      {text}")
        print(f"\n{len(due)} item(s) due. Grade with `locus review --grade <id>:<0-5>`.")
    finally:
        conn.close()


def cmd_gaps(args) -> None:
    """Where the owner's grasp is thin, for a project/concept object (agent-layer §3.6).

    Deterministic — no model call. The strong signal is the EXPLANATION gap: a concept the
    project implements that he has never written about in his own words.
    """
    from locus.learn.gaps import gaps_for_object

    conn = _open()
    try:
        found = gaps_for_object(conn, args.object, limit=args.limit)
        if not found:
            print("No gaps detected (or no such object).")
            return
        for gap in found:
            print(f"  {gap.render()}")
        print(f"\n{len(found)} gap(s)")
    finally:
        conn.close()


def cmd_evolution(args) -> None:
    """Render the owner's dated position trajectory per concept/project (agent-layer §6.3).

    With no `--subject`, lists every subject that has a recorded chain. `--tensions` additionally
    runs the judged contradiction pass over the latest stance (billed, advisory).
    """
    from locus.evolve.trajectory import (
        all_trajectories, build_trajectory, render_trajectory, resolve_subject,
    )

    conn = _open()
    try:
        if not args.subject:
            trajectories = all_trajectories(conn, limit=args.limit)
            if not trajectories:
                print("No belief positions recorded yet. Run `locus structure` over your notes.")
                return
            for traj in trajectories:
                span = f"{traj.entries[0].dated_at} → {traj.entries[-1].dated_at}"
                print(f"  [{traj.subject_kind}] {traj.label}  ({len(traj.entries)} positions, {span})")
            print(f"\n{len(trajectories)} subject(s) with a trajectory")
            return

        kind, key = resolve_subject(conn, args.subject)
        if key is None:
            print(f"No trajectory subject matches {args.subject!r}.")
            return
        traj = build_trajectory(conn, kind, key, with_tensions=args.tensions)
        print(render_trajectory(traj))
    finally:
        conn.close()



def cmd_capture_conversation(args) -> None:
    """Loop C: save a Claude conversation as a rough note (agent-layer §8.3).

    `--jsonl` imports a Claude Code transcript from disk; otherwise reads markdown from `--file`
    or stdin (e.g. a decision-summary). Write-to-inbox: the note lands in notes/conversations/ and
    is ingested by note-sync (run with `--ingest` to sync now).
    """
    from locus.capture.conversations import capture_conversation, import_jsonl_transcript

    if args.jsonl:
        cap = import_jsonl_transcript(args.jsonl, title=args.title, project=args.project)
    else:
        content = Path(args.file).read_text(encoding="utf-8") if args.file else sys.stdin.read()
        if not content.strip():
            print("no content (pass --jsonl, --file, or pipe markdown on stdin)")
            sys.exit(1)
        cap = capture_conversation(
            content, title=args.title or "Captured conversation", project=args.project, source="cli"
        )
    print(f"captured: {cap.path}")
    if args.ingest:
        from locus.notes_sync import sync_notes

        conn = _open()
        try:
            r = sync_notes(conn)
        finally:
            conn.close()
        print(f"ingest: {r.ingested} ingested, {r.skipped} unchanged, {r.failed} failed")


def cmd_notes_sync(args) -> None:
    """Incrementally ingest the authoring notes directory (agent-layer §6.7).

    (Re)ingests only notes whose whitespace-normalised content changed, skips trivial re-saves,
    and deletes documents whose note file was removed. Maturity comes from each note's frontmatter
    (`maturity: rough|tidy`), else `--default-maturity`. The capture loops call this internally;
    run it by hand to ingest notes you authored or dropped into vault/notes/.
    """
    from locus.notes_sync import sync_notes

    conn = _open()
    try:
        r = sync_notes(conn, args.dir, default_maturity=args.default_maturity)
    finally:
        conn.close()
    print(
        f"notes-sync: {r.ingested} ingested, {r.skipped} unchanged, "
        f"{r.deleted} deleted, {r.failed} failed"
    )


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

    pwr = sub.add_parser(
        "watch-repo",
        help="watch tracked code repos; incrementally re-ingest moved HEADs (separate process from watch)",
    )
    pwr.add_argument("--once", action="store_true", help="sync all tracked repos once and exit")
    pwr.set_defaults(func=cmd_watch_repo)

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
    pr.add_argument(
        "--include-excluded", action="store_true",
        help="include docs in [retrieve].exclude_source_uris (e.g. the self-ingested locus repo)",
    )
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

    pst = sub.add_parser("status", help="one-screen operational health summary (no API)")
    pst.set_defaults(func=cmd_status)

    px = sub.add_parser(
        "export-obsidian",
        help="render the corpus to a read-only Obsidian vault (joins-only, no API; run after `locus link`)",
    )
    px.add_argument("--dest", default=None, help="output vault root (default: [obsidian].out_dir, vault/obsidian)")
    px.add_argument("--top-related", type=int, default=None, help="doc<->doc related edges per note (default: config)")
    px.add_argument("--no-entity-notes", action="store_true", help="docs-only graph (skip canonical entity notes)")
    px.add_argument(
        "--include-excluded", action="store_true",
        help="include docs in [retrieve].exclude_source_uris (e.g. the self-ingested locus repo)",
    )
    px.set_defaults(func=cmd_export_obsidian)

    prd = sub.add_parser(
        "read",
        help="render markdown -> a reMarkable-tuned PDF and push it to the tablet (rmapi)",
    )
    prd.add_argument("path", help="a markdown file, or a directory of *.md files")
    prd.add_argument("--to", default=None, help="device folder (default: [reading].target_folder)")
    prd.add_argument("--out", default=None, help="write PDFs to this dir (default: beside the source)")
    prd.add_argument("--no-push", action="store_true", help="render locally only; skip the rmapi push")
    prd.set_defaults(func=cmd_read)

    pdy = sub.add_parser(
        "daily",
        help="compose today's reMarkable page (aggregate-only, no spend) and push it as a PDF",
    )
    pdy.add_argument("--to", default=None, help="device folder (default: [reading].target_folder)")
    pdy.add_argument("--out", default=None, help="write _home.md here (default: <notes>/_generated)")
    pdy.add_argument("--no-push", action="store_true", help="render locally only; skip the rmapi push")
    pdy.add_argument("--no-render", action="store_true", help="write the markdown only; skip the PDF")
    pdy.set_defaults(func=cmd_daily)

    pdp = sub.add_parser(
        "daily-pull",
        help="read an annotated daily page and route the handwriting (billed: vision per page)",
    )
    pdp.add_argument(
        "pdf", nargs="?", default=None,
        help="the annotated page (default: fetch the annotated copy from the cloud)",
    )
    pdp.add_argument(
        "--no-promote", action="store_true",
        help="skip writing developed threads out as notes",
    )
    pdp.add_argument("--date", default=None, help="page date (default: today)")
    pdp.add_argument(
        "--force", action="store_true",
        help="re-read even if the file is unchanged since the last pull (spends a vision call)",
    )
    pdp.set_defaults(func=cmd_daily_pull)

    ppr = sub.add_parser(
        "promote",
        help="write developed threads out as notes so they enter the corpus (free, local)",
    )
    ppr.add_argument("--out", default=None, help="notes dir (default: [paths].notes)")
    ppr.add_argument(
        "--count", action="store_true", help="report how many threads are not yet notes"
    )
    ppr.set_defaults(func=cmd_promote)

    pan = sub.add_parser(
        "annotate",
        help="Loop B: read a PDF's reMarkable annotations and link them to the text they mark",
    )
    pan.add_argument(
        "device_path", nargs="?", default=None,
        help="device path, e.g. '/reading_list/Advanced Portfolio Management'",
    )
    pan.add_argument("--rmdoc", default=None, help="use an already-downloaded .rmdoc instead")
    pan.add_argument("--source-uri", default=None, help="stable key to store marks under")
    pan.add_argument(
        "--transcribe", action="store_true",
        help="also read the handwriting beside each mark (billed: one vision call per note)",
    )
    pan.add_argument(
        "--ingest", action="store_true",
        help="also ingest the annotated PDF itself, and key the marks to that document",
    )
    pan.add_argument(
        "--category", default="paper",
        help="category for --ingest (default: paper)",
    )
    pan.add_argument(
        "--notes", action="store_true",
        help="write your transcribed margin notes out as a note (free, local)",
    )
    pan.add_argument(
        "--max-transcribe", type=int, default=None,
        help="stop after this many transcriptions (spend cap for a heavily annotated book)",
    )
    pan.set_defaults(func=cmd_annotate)

    pcs = sub.add_parser(
        "capture-sync",
        help="Loop A: transcribe + enrich + ingest staged reMarkable handwriting renders (billed)",
    )
    pcs.add_argument("--staging", default=None, help="staging dir (default: [capture].staging_dir)")
    pcs.add_argument("--no-ingest", action="store_true", help="render notes only; skip the notes-sync ingest")
    pcs.set_defaults(func=cmd_capture_sync)

    pstr = sub.add_parser(
        "structure",
        help="propose structured objects + belief positions from ingested docs (billed; --dry-run is free of writes)",
    )
    pstr.add_argument("--doc", action="append", help="document id (repeatable); else use the filters")
    pstr.add_argument("--category", default=None, help="only documents in this category")
    pstr.add_argument("--since", default=None, help="only documents AUTHORED on or after (YYYY-MM-DD)")
    pstr.add_argument("--ingested-since", default=None,
                      help="only documents INGESTED on or after (YYYY-MM-DD) — for scheduled runs")
    pstr.add_argument("--maturity", choices=["rough", "tidy"], default=None, help="only documents of this maturity")
    pstr.add_argument("--limit", type=int, default=None, help="cap how many documents are processed")
    pstr.add_argument("--dry-run", action="store_true", help="run every gate and print the plan; write nothing")
    pstr.add_argument("--verbose", action="store_true", help="also show rejected candidates and why")
    pstr.set_defaults(func=cmd_structure)

    prv = sub.add_parser("review", help="spaced-repetition review over your own propositions (SM-2)")
    prv.add_argument("--grade", action="append", help="record a grade: <item_id>:<0-5> (repeatable)")
    prv.add_argument("--add-object", action="append", help="schedule an object for review (repeatable)")
    prv.add_argument("--limit", type=int, default=5, help="max items shown (daily-page cap)")
    prv.add_argument(
        "--enrol", action="store_true",
        help="add a few unscheduled propositions from blessed objects (deterministic, free)",
    )
    prv.add_argument(
        "--enrol-max", type=int, default=None,
        help="cap new items this run (default: a small, deliberately gradual number)",
    )
    prv.set_defaults(func=cmd_review)

    pgp = sub.add_parser("gaps", help="where your grasp is thin for a project/concept object (no API)")
    pgp.add_argument("object", type=int, help="object id (see `locus objects`)")
    pgp.add_argument("--limit", type=int, default=10)
    pgp.set_defaults(func=cmd_gaps)

    pev = sub.add_parser("evolution", help="show the dated belief trajectory for a concept/project")
    pev.add_argument("subject", nargs="?", default=None, help="concept name or project title (omit to list all)")
    pev.add_argument("--tensions", action="store_true", help="also run the judged contradiction pass (billed)")
    pev.add_argument("--limit", type=int, default=50, help="max subjects when listing")
    pev.set_defaults(func=cmd_evolution)

    pobj = sub.add_parser("objects", help="list / bless / archive structured objects")
    pobj.add_argument("--type", choices=["project", "concept", "question", "reading"], default=None)
    pobj.add_argument("--status", choices=["proposed", "active", "archived"], default=None)
    pobj.add_argument("--limit", type=int, default=50)
    pobj.add_argument("--bless", action="append", help="object id to mark active (repeatable)")
    pobj.add_argument("--archive", action="append", help="object id to archive (repeatable)")
    pobj.set_defaults(func=cmd_objects)

    pcc = sub.add_parser(
        "capture-conversation",
        help="Loop C: save a Claude conversation (or a .jsonl transcript) as a rough note",
    )
    pcc.add_argument("--jsonl", default=None, help="import a Claude Code .jsonl transcript from disk")
    pcc.add_argument("--file", default=None, help="read markdown content from a file (else stdin)")
    pcc.add_argument("--title", default=None, help="note title (default: first user line / stem)")
    pcc.add_argument("--project", default=None, help="project tag (provenance)")
    pcc.add_argument("--ingest", action="store_true", help="run note-sync after capturing")
    pcc.set_defaults(func=cmd_capture_conversation)

    pns = sub.add_parser(
        "notes-sync",
        help="incrementally ingest vault/notes/ (changed notes only; deletes removed ones)",
    )
    pns.add_argument("--dir", default=None, help="notes directory (default: [paths].notes)")
    pns.add_argument(
        "--default-maturity", choices=["rough", "tidy"], default="rough",
        help="maturity for notes without a frontmatter maturity (default: rough)",
    )
    pns.set_defaults(func=cmd_notes_sync)

    pb = sub.add_parser("backup", help="snapshot the corpus (DB + raw store + notes)")
    pb.add_argument("--dest", default=None, help="backup root (default: vault/backups)")
    pb.add_argument("--list", action="store_true", help="list existing snapshots and exit")
    pb.set_defaults(func=cmd_backup)

    prs = sub.add_parser("restore", help="restore the corpus from a snapshot (destructive)")
    prs.add_argument("snapshot", help="snapshot dir path, or bare name under the backup root")
    prs.add_argument("--dest", default=None, help="backup root for bare names (default: vault/backups)")
    prs.add_argument("--yes", action="store_true", help="actually overwrite live state (else dry-run)")
    prs.set_defaults(func=cmd_restore)

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
