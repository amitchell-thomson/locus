"""Locus CLI: the services (serve, heuris, capture, watch, backup), the laptop pushes, and the
manual library operations. `locus --help` lists them.

The old engine's commands (daily, decide, discover, link, structure, review, query and the rest)
were deleted in Slice C (C2, 13 Nov 2026).
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

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


# Command -> the `agent_runs` kind it is journalled under. Journalling happens at DISPATCH rather
# than inside each command: it is one place instead of nine, it cannot be forgotten when a command
# is added, and it records a crash that happens anywhere in the command rather than only inside a
# block someone remembered to wrap.
#
# Before this, ONLY `capture` opened a row — so `locus status` and the daily page's status line
# reported on one of nine nightly steps and called the rest healthy. A health check that can only
# see the thing already working is not a health check.
_JOURNAL_KINDS = {
    "backup": "backup",
    "notes-sync": "notes-sync",
}


def _journal_kind(args) -> str | None:
    """The run kind for this invocation, or None when it is an interactive read."""
    command = getattr(args, "cmd", None)
    return _JOURNAL_KINDS.get(command or "")


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


def cmd_serve(args) -> None:
    """The HTTP service (Slice 0.6): one process on the server, MCP at /mcp, bearer token from
    LOCUS_HTTP_TOKEN (an env file the unit reads, never committed)."""
    import os

    from locus.db.migrate import migrate

    cfg = load()
    migrate(cfg.paths.db)
    from locus.mcp_server import serve

    serve(args.host, args.port, os.environ.get("LOCUS_HTTP_TOKEN", ""),
          enable_query=args.enable_query or cfg.mcp.enable_query)


def cmd_serve_public(args) -> None:
    """The public listener for claude.ai and the phone (H1.10): OAuth with a passphrase, behind
    Tailscale Funnel. LOCUS_PUBLIC_PASSPHRASE and LOCUS_PUBLIC_URL come from the unit's env file."""
    import os

    from locus.mcp_server import serve_public

    cfg = load()
    phrase = os.environ.get("LOCUS_PUBLIC_PASSPHRASE", "")
    url = os.environ.get("LOCUS_PUBLIC_URL", "")
    if len(phrase) < 12 or not url.startswith("https://"):
        raise SystemExit("LOCUS_PUBLIC_PASSPHRASE (12+ characters) and LOCUS_PUBLIC_URL (https) are required")
    serve_public(args.port, phrase, url, cfg.paths.db.parent / "public-auth.db")


def cmd_transcripts_push(args) -> None:
    """Laptop transcripts, scrubbed, to the server (Slice 0.10). Run by launchd every 30 min."""
    from locus.transcripts_push import describe, push

    r = push(dry_run=args.dry_run)
    print(describe(r) if not args.dry_run else f"{r.scrubbed} would be scrubbed and sent, {r.unchanged} unchanged")
    if r.error:
        sys.exit(1)


def cmd_heuris(args) -> None:
    """heuris (H1): index the sources into heuris.db, or say how fresh each is."""
    from locus.heuris import db as hdb
    from locus.heuris import run as hrun

    conn = hdb.connect()
    try:
        if args.action == "derive":
            _heuris_derive(conn, args)
            return
        if args.action == "index":
            names = [n for n in args.sources if n != "all"] or None
            for o in hrun.run(names, conn=conn):
                st = o.stats
                did = f"+{st.added} added, {st.unchanged} unchanged, -{st.removed} removed, {st.chunks} chunks" if st else ""
                print(f"{o.source:9} {'ERROR ' + o.error if o.error else did}  ({o.seconds:.1f}s)")
            print()
        rows = hrun.status(conn)
        print(hrun.status_json(conn) if args.json else hrun.render(rows))
        if args.action == "index" and any(r["state"] == "error" for r in rows):
            sys.exit(1)
    finally:
        conn.close()


def cmd_facts(args) -> None:
    """heuris's facts ledger (H2): read it, or change it from the window (the Home chat's `y`,
    Approve), from the import of the applications file, or by hand."""
    import json as _json
    from contextlib import closing
    from dataclasses import asdict
    from pathlib import Path

    from locus.heuris import db as hdb
    from locus.heuris import facts as F
    from locus.heuris import tools as ht

    with closing(hdb.connect()) as conn:
        try:
            a = args.action
            if a == "show":
                if args.json:
                    fs = F.current(conn, args.subject, args.slot)
                    print(_json.dumps([asdict(f) for f in fs]))
                else:
                    print(ht.facts(conn, args.subject, args.slot, args.history))
            elif a == "remember":
                w = F.write(conn, args.subject, args.slot, args.value, route=args.route, as_of=args.as_of,
                            sources=args.source or [f"you said so in the locus window, {F._as_of(None)}"])
                print(w.describe())
            elif a == "forget":
                f, restored = F.forget(conn, args.id, route=args.route)
                print(f"Forgotten: {f.line()}." + (f" Restored: {restored.line()}." if restored else ""))
            elif a == "proposed":
                fs = F.proposed(conn)
                print(_json.dumps([asdict(f) for f in fs]) if args.json else
                      "\n".join(f"{f.line()}  why: {f.why}" for f in fs) or "Nothing proposed.")
            elif a == "approve":
                print(F.approve(conn, args.id, value=args.value_as).describe())
            elif a == "reject":
                print(f"Rejected: {F.reject(conn, args.id).line()}")
            elif a == "log":
                rows = F.automatic_log(conn, days=args.days)
                print(_json.dumps(rows) if args.json else "\n".join(
                    f"{r['at'][:16]}  {r['route']:8} {'UNDONE ' if r['undone'] else ''}{r['line']}" for r in rows)
                    or "No automatic facts.")
            elif a == "undo":
                f, restored = F.forget(conn, args.id, route="home", action="undo")
                print(f"Undone: {f.line()}." + (f" Restored: {restored.line()}." if restored else ""))
            elif a == "queue":
                # What the window reads once a minute (H2.5), in one call: the current facts
                # (Home's careers), the proposals (Approve) and the automatic writes (the log).
                print(_json.dumps({
                    "current": [asdict(f) for f in F.current(conn)],
                    "proposed": [asdict(f) for f in F.proposed(conn)],
                    "log": F.automatic_log(conn, days=args.days),
                }))
            elif a == "import-applications":
                ws = F.import_applications(conn, Path(args.path).expanduser())
                for w in ws:
                    print(w.describe())
        except F.FactError as e:
            print(e, file=sys.stderr)
            sys.exit(2)


def _heuris_derive(conn, args) -> None:
    """H2: facts from mail and the calendar, records, and briefs, batched on the credit."""
    import time as _time

    from locus.agent import usage
    from locus.config import load as _load
    from locus.db.connection import get_connection
    from locus.heuris import briefs, derive, records

    jobs = args.jobs or ["facts", "records", "briefs"]
    gate_conn = get_connection(_load().paths.db)
    try:
        def once(submit_new: bool) -> bool:
            did = False
            if "facts" in jobs:
                with usage.job("heuris-facts"):
                    r = derive.run_facts(conn, since=args.since, limit=args.limit, force=args.force,
                                         gate_conn=gate_conn, submit_new=submit_new)
                if submit_new or r.collected:
                    print(r.describe())
                did |= bool(r.collected)
            if "records" in jobs:
                with usage.job("heuris-records"):
                    r = records.run_records(conn, since=args.since, limit=args.limit, force=args.force,
                                            submit_new=submit_new)
                if submit_new or r.collected:
                    print(r.describe())
                did |= bool(r.collected)
            if "briefs" in jobs:
                with usage.job("heuris-briefs"):
                    r = briefs.run_briefs(conn, force=args.force, submit_new=submit_new)
                if submit_new or r.collected:
                    print(r.describe())
                did |= bool(r.collected)
            return did

        once(True)
        while args.wait and conn.execute("SELECT COUNT(*) FROM batches WHERE applied=0").fetchone()[0]:
            _time.sleep(30)
            once(False)
    finally:
        gate_conn.close()


def cmd_heuris_push(args) -> None:
    """Laptop: mail (kept by the H1.2 rule, scrubbed) with its attachments, project docs and
    commits, and career files (H1.12), to the server."""
    from locus.heuris import files_push, mail_push, repos_push

    failed = False
    for name, mod in (("mail", mail_push), ("repos", repos_push), ("files", files_push)):
        try:
            r = mod.push(send=not args.dry_run)
            print(f"{name}: {mod.describe(r)}")
            failed |= bool(r.error)
        except Exception as exc:
            print(f"{name}: FAILED {type(exc).__name__}: {exc}")
            failed = True
    if failed:
        sys.exit(1)


def cmd_mcp_log(args) -> None:
    """What the MCP server was actually asked to do — the answer to a bare "Tool execution failed".

    An MCP client reports every failed call as the same contentless string, whether the tool
    raised, the server was gone, or the client stopped waiting while the server was still
    working. On 2026-09-08 that ambiguity sent the debugging to Tailscale and the server while
    the delivery path was measurably fine. This reads the server-side record instead of
    guessing: a call with no `start` never arrived, a `start` with no `end` outlived the
    client's patience, and an `end` carrying an error names the exception.
    """
    from locus.observe import mcp_log

    path = mcp_log.log_path()
    if not path.exists():
        print(f"No MCP call log yet at {path}.")
        print("It is written by `locus mcp`; a server started before this build writes nothing.")
        return

    from locus.config import PROJECT_ROOT
    from locus.mcp_server import _build_stamp

    head = _build_stamp()
    live = mcp_log.live_servers()
    if live:
        print(f"live `locus mcp` server(s)   (checkout at {head}, {PROJECT_ROOT})")
        for server in live:
            build = server.get("build", "?")
            flag = "  <-- STALE, running old code" if build != head else ""
            print(f"  pid {server.get('pid')}  started {server.get('at')}  build {build}{flag}")
        print()

    pending = {(e.get("pid"), e.get("call")) for e in mcp_log.unanswered()}
    events = mcp_log.read_recent(args.number, errors_only=args.errors)
    if not events:
        print(f"No matching events in {path}.")
        return

    print(f"{path}  (last {len(events)} event(s))")
    print("=" * 78)
    for event in events:
        at = event.get("at", "?")
        kind = event.get("event", "?")
        pid = event.get("pid", "?")
        if kind in ("server_start", "server_stop"):
            print(f"{at}  pid {pid}  {kind}  build {event.get('build', '?')}")
            continue
        tool = event.get("tool", "?")
        if kind == "start":
            mark = "  <-- NEVER FINISHED" if (pid, event.get("call")) in pending else ""
            args_txt = json.dumps(event.get("args", {}), default=str)[:100]
            print(f"{at}  pid {pid}  ->  {tool}({args_txt}){mark}")
        else:
            ms = event.get("ms", "?")
            if event.get("ok"):
                print(f"{at}  pid {pid}  ok  {tool}  {ms}ms")
            else:
                print(f"{at}  pid {pid}  ERR {tool}  {ms}ms  "
                      f"{event.get('error', '?')}: {event.get('message', '')}")
                if args.traceback and event.get("traceback"):
                    for line in event["traceback"].rstrip().splitlines():
                        print(f"        {line}")

    if pending:
        print()
        print(f"{len(pending)} call(s) arrived and never finished. That is the client giving up "
              "while the server was still working — not a server that is down.")


def _backup_root(args) -> Path:
    """Backup destination: --dest if given, else data/backups (sibling of the DB)."""
    if getattr(args, "dest", None):
        return Path(args.dest)
    return load().paths.db.parent / "backups"


def _fmt_bytes(n: float) -> str:
    for unit in ("B", "KiB", "MiB", "GiB"):
        if n < 1024 or unit == "GiB":
            return f"{n:.0f} {unit}" if unit == "B" else f"{n:.1f} {unit}"
        n /= 1024
    return f"{n:.1f} GiB"


def cmd_migrate(args) -> None:
    """Upgrade the DB to head and print the revision before and after.

    `deploy/deploy.sh` calls this right after `locus backup` when a pull brings a new migration.
    Before it existed, the schema moved whenever the next ingest or `locus mcp` happened to call
    `migrate()`, so a deploy could not say what revision it left the DB at.
    """
    from locus.db.migrate import current_revision, head_revision, migrate

    db = load().paths.db
    before = current_revision(db)
    migrate(db)
    after = current_revision(db)
    note = "unchanged" if before == after else "migrated"
    print(f"schema {before} -> {after} (head {head_revision(db)}, {note})")


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


def cmd_marks(args) -> None:
    """Read back the annotations he made on the tablet (free, local — images cost a fetch).

    The text half is a database read. `--images` composites his ink onto the PDF pages from the
    cloud copy and writes PNGs, which is the only way to see what a mark over a FIGURE means —
    `covered_text` is empty for those by construction (CLAUDE.md §14).
    """
    from locus.capture import review

    conn = _open()
    try:
        if not args.document:
            known = review.documents_with_marks(conn)
            if not known:
                print("nothing annotated yet")
                return
            for _, title, n in known:
                print(f"  {n:4} marks — {title}")
            return

        targets = review.resolve_target(conn, args.document, rmapi_binary=load().capture.rmapi_binary)
        if not targets:
            print(f"nothing on the device or in the library matches {args.document!r}")
            sys.exit(1)
        if len(targets) > 1:
            print(f"{args.document!r} matches several — say which:")
            for t in targets:
                print(f"  {t.title}  [{t.device_path}]")
            sys.exit(1)

        target = targets[0]
        doc = (
            review.load(conn, target.source_uri, page=args.page, intent=args.intent)
            if target.source_uri else None
        )
        if doc is not None:
            print(doc.render(image_hint=not args.images))

        if not args.images:
            return
        # The SAME path the MCP `markups` tool takes — whole-device resolution and the
        # margin-preserving renderer. When the CLI and the tool disagreed about where a
        # document lives or how wide its page is, the CLI was the one that looked right and
        # was wrong.
        # max_bytes=None: this writes PNGs to a directory, and no 1MB tool-result ceiling
        # applies to a file. Handing back a silently downscaled image to a caller that asked
        # for files would be a quality regression with no visible cause.
        m = review.markups(
            conn, target, cfg=load(), pages=args.page and [args.page],
            refresh=args.refresh, max_images=args.images, margins=not args.no_margins,
            max_bytes=None,
        )
        if m.swept:
            print(f"(swept {m.swept} mark(s) from the device — geometric only, not transcribed)")
        out_dir = Path(args.out or ".")
        out_dir.mkdir(parents=True, exist_ok=True)
        for idx in sorted(m.pages):
            dest = out_dir / f"p{idx + 1:04d}.png"
            dest.write_bytes(m.pages[idx])
            print(f"  wrote {dest}")
        if m.omitted:
            print(f"  ({len(m.omitted)} inked page(s) not written: "
                  f"{', '.join(str(p + 1) for p in m.omitted)} — raise --images or pass --page)")
    finally:
        conn.close()


def cmd_gates(args) -> None:
    """What each threshold rejected over the window — the dead-gate check (no API; local only).

    The failure it exists for: `find_tensions` was inert for its whole life because
    `_MAX_DISTANCE` admitted only paraphrases, and nothing anywhere said so. A gate that admits
    nothing is indistinguishable, in every other surface, from a subject with nothing to say.
    """
    from locus.observe import gates

    conn = _open()
    try:
        print(gates.render(gates.report(conn, days=args.days), days=args.days))
    finally:
        conn.close()


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


def cmd_home_system(args) -> None:
    """Home's system section as one JSON object (home-live H7): read over ssh by the window."""
    import json

    from locus.home_system import collect

    cfg = load().agent
    conn = _open()
    try:
        print(json.dumps(collect(conn, cycle_day=cfg.credit_cycle_day, backend=cfg.backend)))
    finally:
        conn.close()


def cmd_warm_latex(args) -> None:
    """Compile a throwaway document so the engine downloads its packages NOW.

    WHY THIS IS A COMMAND. `tectonic` fetches the packages a document asks for on first use, and
    the device preamble loads tikz and pgfplots for every document, so the first compile on a
    machine downloads ~51MB of the pgf tree. Measured 2026-09-08: 201.5s and 149.7s on two cold
    runs, against ~1.2s warm.

    That cost is invisible until it lands on the worst possible caller. `to_remarkable` runs
    inside an MCP tool call, and the MCP CLIENT has its own patience: it gives up long before a
    cold fetch finishes and reports a bare "Tool execution failed" with no message, because the
    server is still working and has raised nothing to report. Raising the server's own ceiling
    (`[reading].latex_timeout_s`) cannot fix that — it only makes the server block longer after
    the client has stopped listening. The fix has to be to pay the download OUT of band, which is
    what this does. Run it once per machine, and after `rm -rf ~/.cache/tectonic`.
    """
    import tempfile
    import time

    from locus.reading.send import latex_geometry
    from locus.reading.tex2pdf import available_engine, render_latex_to_pdf

    engine = available_engine(load().reading.latex_engine)
    # The probe must exercise the SAME preamble a real send uses, or it warms the wrong packages
    # and reports success while the first real document still pays the download.
    probe = r"\section{Warm}Cache probe. \begin{tikzpicture}\draw (0,0)--(1,1);\end{tikzpicture}"
    with tempfile.TemporaryDirectory() as tmp:
        started = time.time()
        print(f"warming {engine} (first run downloads ~51MB; later runs are ~1s)...")
        render_latex_to_pdf(probe, Path(tmp) / "warm.pdf", geometry=latex_geometry(), title="Warm")
        elapsed = time.time() - started
    print(f"{engine} ready in {elapsed:.1f}s.")
    if elapsed > 20:
        print("That was the cold download; it is cached now, so sends will be fast.")


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
        if src.suffix.lower() == ".pdf":
            # A PDF is already the artifact. Falling through here fed it to the markdown
            # renderer, which does not fail — it typesets the raw bytes and pushes the result,
            # so the device got a document full of binary noise under the right name. Push it
            # unchanged instead; `send_pdf` also proves it really is a PDF first.
            from locus.reading.send import send_pdf

            # Check --no-push BEFORE sending, not after: `--no-push` means "render only", and
            # for a PDF there is nothing to render, so the honest answer is to do nothing.
            if args.no_push:
                print(f"{src} is already a PDF — nothing to render (drop --no-push to send it)")
                return
            sent = send_pdf(src, title=args.title, folder=folder)
            print(f"pushed {sent.filename} -> reMarkable:{sent.device_path}")
            return
        if src.suffix.lower() == ".tex":
            # A .tex file is LaTeX, and the markdown renderer would not fail on it — pandoc
            # would typeset the macros as literal prose and push a document of backslashes,
            # the same silent mis-render the .pdf branch above exists to prevent.
            from locus.reading.tex2pdf import render_latex_file

            # NOT the `geometry` built above: that one carries `font_pt`, which sizes the
            # markdown path. LaTeX reads at its own size and margin.
            from locus.reading.send import latex_geometry

            out_pdf = (Path(args.out) / f"{src.stem}.pdf") if args.out else src.with_suffix(".pdf")
            render_latex_file(out_pdf=out_pdf, tex_path=src, geometry=latex_geometry(),
                              engine=cfg.latex_engine)
            if args.no_push:
                print(f"rendered {src.name} -> {out_pdf}")
                return
            result = deliver_pdf(out_pdf, remote_folder=folder, rmapi_binary=cfg.rmapi_binary)
            note = " (created folder)" if result.created_folder else ""
            print(f"delivered {result.filename} -> reMarkable:/{result.remote_folder}{note}")
            return
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
        doc = read_rmdoc(path)
    else:
        # Downloaded into the VAULT CACHE, not `mkdtemp`. The old temp directory was never
        # cleaned up (1.3MB per bundle, left behind on every run), and — more usefully — the
        # bundle is exactly what `locus marks` and the MCP `markups` tool want next. Filed under
        # the document's own uuid, a second look at the same ink costs no fetch at all.
        from locus.capture.review import _cache_dir

        cache = _cache_dir(load())
        cache.mkdir(parents=True, exist_ok=True)
        print(f"fetching {args.device_path!r} from the reMarkable cloud ...")
        with tempfile.TemporaryDirectory(dir=cache) as staging:
            path = fetch_rmdoc(args.device_path, staging)
            doc = read_rmdoc(path)
            if doc.doc_uuid:
                kept = cache / f"{doc.doc_uuid}.rmdoc"
                kept.write_bytes(Path(path).read_bytes())
                path = kept
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

        if doc.is_notebook:
            print(
                "--ingest needs a PDF, and this is a native notebook (handwriting only, no PDF "
                "underneath). Read it with `locus marks <name> --images` instead.",
                file=sys.stderr,
            )
            return 1
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
    """Capture from the reMarkable cloud: notebooks (Loop A) and ink on PDFs (Loop B).

    One cloud call lists the tablet (capture/device.py). A changed notebook is rendered from its
    bundle, transcribed (Sonnet vision), filled in, enriched and filed as a `rough` note; a changed
    PDF has its marks stored and transcribed, and is either a source (ingested through the drop
    folder) or a document Locus sent (never ingested; his notes promoted with their passages).
    Idempotent on the ink. Billed (metered vision + subscription text); both loops are journaled.
    """
    from locus.capture.loop_a import capture_sync, placed_documents
    from locus.capture.loop_b import annotate_sync

    conn = _open()
    try:
        placed = placed_documents(conn)
        r = capture_sync(
            conn, placed=placed, staging_dir=args.staging, ingest=not args.no_ingest,
            recapture=args.recapture,
        )
        b = annotate_sync(conn, placed=placed)
    finally:
        conn.close()
    for o in r.outcomes:
        if o.status == "captured":
            print(f"  captured  {o.name}  ({o.pages}pp, {o.pages_read} read now · {o.illegible} illegible · {o.uncertain} [?] · "
                  f"{o.filled} filled · {o.related} related)  -> {o.note_path}")
        elif o.status == "failed":
            print(f"  FAILED    {o.name}: {o.error}")
    print(f"\ncapture: {len(placed)} documents placed · {r.captured} notebook(s) captured, "
          f"{r.count('unchanged')} unchanged, {r.count('seeded')} seeded, "
          f"{r.count('deferred')} deferred to the next run, {r.count('failed')} failed   "
          f"est cost ${r.cost_usd:.4f}")
    if r.unmatched_folders:
        print(f"  not in the registry: {', '.join(r.unmatched_folders)}")
    for o in b.outcomes:
        if o.status == "annotated":
            if o.role == "sent":
                key = "device (a document Locus sent)"
            elif o.hash_mapped:
                key = "library"
            elif o.ingested:
                key = "library (ingested this run)"
            else:
                key = "DEVICE PATH (awaiting his move out of Proposed)"
            extra = f" · notes {o.promoted}" if o.promoted and o.promoted != "unchanged" else ""
            print(f"  annotated {o.name}  ({o.marks} marks · {o.transcribed} transcribed · "
                  f"keyed to {key}{extra})")
        elif o.status in ("queued", "quarantined"):
            print(f"  source {o.status}: {o.name}")
        elif o.status == "failed":
            print(f"  ANNOTATE FAILED {o.name}: {o.error}")
    deferred = sum(o.status == "deferred" for o in b.outcomes)
    print(f"annotate: {b.annotated} PDF(s) with new ink"
          + (f", {deferred} deferred to the next run" if deferred else ""))
    print(f"cloud: 1 listing + {r.count('captured') + r.count('failed')} notebook and "
          f"{sum(o.status not in ('unchanged', 'deferred') for o in b.outcomes)} PDF download(s)")
    if r.ingest:
        print(f"ingest: {r.ingest.ingested} ingested, {r.ingest.skipped} unchanged, "
              f"{r.ingest.deleted} deleted, {r.ingest.failed} failed")


# A document Locus sent before migration 0038 recorded sends (Slice 0.12, one-off): every push
# names its file with a leading date ("2026-10-19 Locus plan v3"), the daily pages are
# `daily-<date>`, and a dossier carries its date at the end. Third-party PDFs on the tablet
# (lecture slides, papers, his own exports) carry none of these.
_SENT_NAME = re.compile(r"^\d{4}-\d{2}-\d{2} |\d{4}-\d{2}-\d{2}$|^daily-\d{4}-\d{2}-\d{2}")


def cmd_budget(args) -> None:
    """What the subscription was spent on: model calls by job, today and the last seven days.

    Every headless Claude call is one `model_calls` row (agent/usage.py). Tokens are the real
    measure; minutes say how long jobs held the queue; the notional $ is the envelope's
    API-equivalent figure, useful for weighing models, not money spent.
    """
    from locus.agent import claude, usage

    cfg = load().agent
    today, week = usage.windows()
    cycle = usage.cycle_start(cfg.credit_cycle_day)
    conn = _open()
    try:
        print(usage.format_totals(f"today (since {today[:16]} UTC)", usage.totals(conn, since=today)))
        print()
        print(usage.format_totals("last 7 days", usage.totals(conn, since=week)))
        used = usage.credit_used(conn, since=cycle.isoformat())
    finally:
        conn.close()
    route = "API credit first, subscription if it refuses" if cfg.backend == "api" else "subscription only"
    print(f"\nroute: {route}" + ("" if claude._api_open() or cfg.backend != "api"
                                  else " (credit refused recently: on the subscription for now)"))
    print(f"API credit this cycle (since {cycle:%d %b}): about ${used:.2f} of $100, estimated "
          "from [agent].api_prices; the Anthropic console is the exact figure")


def cmd_record(args) -> None:
    """Record what systemd saw — a unit that failed, or a multi-command unit that completed.

    `agent_runs` is opened and closed by the code, so it is blind to a unit whose import fails,
    whose binary is missing, or which the OOM killer takes: none of those reach Python, and "no
    row" is indistinguishable from "did not run tonight". `OnFailure=` calls this, so systemd's
    knowledge reaches the same place the health check reads.

    `--ok` is the other half, for `locus-maintain`: it is nine ExecStart lines rather than one
    command, so nothing else is in a position to say the UNIT finished.
    """
    from locus.agent import journal
    from locus import health as H

    conn = _open()
    try:
        if args.ok:
            run_id = journal.start_run(conn, args.unit)
            journal.finish_run(conn, run_id, "ok", stats={"via": "systemd"})
            print(f"recorded a successful run of {args.unit}")
        else:
            H.record_failure(conn, args.unit, args.detail or "")
            print(f"recorded a failure of {args.unit}")
    finally:
        conn.close()


def cmd_notes_sync(args) -> None:
    """Incrementally ingest the authoring notes directory (agent-layer §6.7).

    (Re)ingests only notes whose whitespace-normalised content changed, skips trivial re-saves,
    and deletes documents whose note file was removed. Maturity comes from each note's frontmatter
    (`maturity: rough|tidy`), else `--default-maturity`. The capture loops call this internally;
    run it by hand to ingest notes you authored or dropped into data/notes/.
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
    from locus.eval.metrics import corpus_metrics, doc_metrics, format_metrics

    conn = _open()
    docs = [doc_metrics(conn, int(args.doc))] if args.doc else corpus_metrics(conn)
    print(format_metrics(docs))
    conn.close()


def cmd_files(args) -> None:
    """`~/files` and the tablet keep one folder tree (files-and-chat M1; see locus/files_sync.py).
    `sync` and `questions` run on the laptop; `tablet`, `tablet-mkdir` and `tablet-rename` run on
    the server, which holds the rmapi login, and are what `sync` calls over ssh."""
    import json as _json
    import time as _time

    from locus import files_sync as fs

    def runner():
        from locus.capture.remarkable import _subprocess_runner
        from locus.config import load

        cfg = load()
        return _subprocess_runner(cfg.capture.rmapi_binary), cfg.capture.rmapi_tree_cache

    if args.files_cmd == "sync":
        p = fs.sync(dry_run=args.dry_run)
        verb = "would make" if args.dry_run else "made"
        for rel in p.to_laptop:
            print(f"{verb} on the laptop  {rel}")
        for rel in p.to_tablet:
            print(f"{verb} on the tablet  {rel}")
        for old, new in p.renames:
            print(f"rename pending     {old} -> {new}  (locus files tablet-rename --apply, on the server)")
        for q in p.questions:
            if q["path"] not in {o for o, _ in p.renames}:
                print(f"question           {fs.describe(q)}")
        if not (p.to_laptop or p.to_tablet or p.questions or p.renames):
            print("in step: the laptop and the tablet have the same folders")
    elif args.files_cmd == "questions":
        qs = fs.questions()
        for q in qs:
            print(fs.describe(q))
        if not qs:
            print("no folder questions")
    elif args.files_cmd == "tablet":
        run, cache = runner()
        print(_json.dumps(sorted(fs.tablet_now(run, tree_cache=cache))))
    elif args.files_cmd == "tablet-mkdir":
        from locus.reading.deliver_remarkable import _ensure_folder

        run, _ = runner()
        for rel in sorted(args.paths, key=lambda s: (s.count("/"), s)):
            if rel.split("/", 1)[0] not in fs.AREAS or ".." in rel.split("/"):
                raise SystemExit(f"refusing {rel!r}: tablet folders are made under the areas only")
            _ensure_folder(run, "/" + rel)
            print(f"made /{rel}")
            _time.sleep(args.pace)
    elif args.files_cmd == "tablet-rename":
        run, cache = runner()
        tablet = fs.tablet_now(run, tree_cache=cache)
        renames = fs.deep_rename_plan(tablet) if args.deep else fs.rename_plan(tablet)
        for old, new in renames:
            if args.apply:
                code, out, err = run(["mv", "/" + old, "/" + new])
                if code != 0:
                    raise SystemExit(f"rmapi mv /{old} failed: {(err or out).strip()[:300]}")
                print(f"renamed /{old} -> /{new}")
                _time.sleep(args.pace)
            else:
                print(f"would rename /{old} -> /{new}")
        if not renames:
            print("every tablet folder already has a folder name" if args.deep
                  else "every registered tablet folder already has its folder name")


def cmd_canvas(args) -> None:
    """Canvas, read only (spec P). `courses` lists every course and proposes the registry
    mapping as a diff; `--apply` writes it (only after he approves the diff)."""
    import dataclasses
    import difflib
    import json as _json

    from locus import canvas
    from locus.heuris import registry

    if args.canvas_cmd == "modules":
        print(_json.dumps(canvas.module_names()) if args.json else
              "\n".join(f"{k}: {', '.join(v)}" for k, v in canvas.module_names().items()))
        return
    if args.canvas_cmd == "syllabus":
        _canvas_syllabus(args)
        return
    cs = canvas.fetch_courses()
    if args.json:
        print(_json.dumps([dataclasses.asdict(c) for c in cs]))
        return
    m = canvas.plan(registry.entries(), cs)
    mapped = {c.id: rid for rid, c in m.mapped}
    new = {c.id for c in m.new}
    for c in cs:
        tag = mapped.get(c.id) or ("new: " + (c.code or "").lower() if c.id in new else "-")
        print(f"{c.id:>7}  {c.term[:7]:<8} {tag:<10} {c.name}")
    for rid, old, c in m.renamed:
        print(f"note: {rid} is {old!r} in the registry, {c.name!r} on Canvas (folder kept)")
    before = registry.PATH.read_text()
    after = canvas.apply_to_registry(before, m)
    if args.apply:
        registry.PATH.write_text(after)
        print(f"wrote {registry.PATH}")
    elif after != before:
        print()
        print("".join(difflib.unified_diff(before.splitlines(True), after.splitlines(True),
                                           "taxonomy/registry.toml", "taxonomy/registry.toml (proposed)")))


def _canvas_syllabus(args) -> None:
    """P8, first half: a year's syllabus, verbatim, as one syllabus.md per submodule folder."""
    from pathlib import Path

    import pymupdf

    from locus import canvas, syllabus
    from locus.files_sync import FILES_ROOT
    from locus.heuris import registry

    text = "\n".join(p.get_text() for p in pymupdf.open(args.pdf))
    secs = syllabus.sections(text)
    notes: list[tuple[str, str]] = []
    specs = list(args.note or [])
    if args.notes_file:
        specs += [ln.strip() for ln in Path(args.notes_file).read_text().splitlines() if ln.strip()]
    for spec in specs:
        key, _, ptr = spec.partition("=")
        notes.append((key, ptr))
    pl = syllabus.plan(secs, courses=registry.entries(), modules=canvas.module_names(),
                       edition=args.edition, source=args.source, root=Path(FILES_ROOT).expanduser(),
                       notes=notes)
    for code, title, module, folder in pl.rows:
        print(f"{code:<7} {title[:52]:<52}  {module[:44]:<44}  {folder}")
    print(f"{len(pl.rows)} sections into {len(pl.files)} files")
    for path, body in pl.files.items():
        if args.apply:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(body)
    print("written" if args.apply else "(dry run: --apply writes them)")


def main(argv=None) -> None:
    parser = argparse.ArgumentParser(prog="locus", description="Locus — query-driven personal library")
    sub = parser.add_subparsers(dest="cmd", required=True)

    pi = sub.add_parser("ingest", help="ingest files into the library")
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

    pw = sub.add_parser("watch", help="auto-ingest files dropped into data/incoming/")
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


    pml = sub.add_parser(
        "mcp-log",
        help="what the MCP server was asked to do, and what happened (no API; local only)",
    )
    pml.add_argument("-n", "--number", type=int, default=40, help="events to show (default 40)")
    pml.add_argument("--errors", action="store_true", help="only calls that failed")
    pml.add_argument("--traceback", action="store_true", help="print stored tracebacks in full")
    pml.set_defaults(func=cmd_mcp_log)


    pfs = sub.add_parser("files", help="keep ~/files and the tablet one folder tree")
    fsub = pfs.add_subparsers(dest="files_cmd", required=True)
    fss = fsub.add_parser("sync", help="laptop: compare both trees, make what is missing, list questions")
    fss.add_argument("--dry-run", action="store_true", help="make nothing; print what would be made")
    fsub.add_parser("questions", help="laptop: the folder questions from the last sync")
    fsub.add_parser("tablet", help="server: the tablet's folders as JSON (one rmapi call)")
    fsm = fsub.add_parser("tablet-mkdir", help="server: make tablet folders (Area/... paths)")
    fsm.add_argument("paths", nargs="+")
    fsm.add_argument("--pace", type=float, default=1.0, help="seconds between cloud calls (429s)")
    fsr = fsub.add_parser("tablet-rename", help="server: rename level-2 tablet folders to the registry's folder")
    fsr.add_argument("--apply", action="store_true", help="rename; without it, print the plan")
    fsr.add_argument("--deep", action="store_true",
                     help="rename the folders INSIDE a thing's folder to the same rule (spaces, capitals)")
    fsr.add_argument("--pace", type=float, default=2.0, help="seconds between cloud calls (429s)")
    pfs.set_defaults(func=cmd_files)

    pcv = sub.add_parser("canvas", help="Canvas, read only (spec P)")
    cvs = pcv.add_subparsers(dest="canvas_cmd", required=True)
    cvc = cvs.add_parser("courses", help="every course, and the registry mapping as a diff")
    cvc.add_argument("--json", action="store_true", help="the course list as JSON (what the laptop asks the server for)")
    cvc.add_argument("--apply", action="store_true", help="write the mapping into taxonomy/registry.toml")
    cvm = cvs.add_parser("modules", help="each mapped course's Canvas module names")
    cvm.add_argument("--json", action="store_true")
    cvy = cvs.add_parser("syllabus", help="a year's syllabus PDF as syllabus.md per submodule (P8)")
    cvy.add_argument("pdf")
    cvy.add_argument("--edition", required=True, help='e.g. "1st Year Syllabus 2024-25"')
    cvy.add_argument("--source", required=True, help="where the PDF came from (a pointer)")
    cvy.add_argument("--note", action="append", help="code=pointer to his note for that submodule")
    cvy.add_argument("--notes-file", help="a file of code=pointer lines (code|title where a code repeats)")
    cvy.add_argument("--apply", action="store_true", help="write the files; without it, print the plan")
    pcv.set_defaults(func=cmd_canvas)

    pst = sub.add_parser("status", help="one-screen operational health summary (no API)")
    pst.set_defaults(func=cmd_status)

    psv = sub.add_parser("serve", help="the HTTP service: MCP at /mcp behind a bearer token (Slice 0.6)")
    psv.add_argument("--host", default="127.0.0.1", help="address to bind (the unit passes the Tailscale one)")
    psv.add_argument("--port", type=int, default=7422)
    psv.add_argument("--enable-query", action="store_true", help="also expose the billable query tool")
    psv.set_defaults(func=cmd_serve)

    psp = sub.add_parser("serve-public", help="the public listener for claude.ai and the phone: OAuth, behind Funnel (H1.10)")
    psp.add_argument("--port", type=int, default=7423)
    psp.set_defaults(func=cmd_serve_public)

    ptp = sub.add_parser("transcripts-push", help="laptop Claude transcripts, scrubbed, to the server (Slice 0.10)")
    ptp.add_argument("--dry-run", action="store_true", help="say how many would go; scrub and send nothing")
    ptp.set_defaults(func=cmd_transcripts_push)

    phe = sub.add_parser("heuris", help="heuris (H1): index mail, calendar, chats and repos; or their freshness")
    phe.add_argument("action", choices=["index", "status", "derive"])
    phe.add_argument("--since", default="2026-06-01", help="derive: read mail from this date (D9)")
    phe.add_argument("--limit", type=int, default=None, help="derive: at most this many threads per batch")
    phe.add_argument("--force", action="store_true", help="derive: past the day's credit pace (a backfill)")
    phe.add_argument("--wait", action="store_true", help="derive: wait for the batches and apply them")
    phe.add_argument("--jobs", nargs="*", choices=["facts", "records", "briefs"], help="derive: which (default all)")
    phe.add_argument("sources", nargs="*", default=[], help="mail calendar chats repos (default: all)")
    phe.add_argument("--json", action="store_true", help="status as JSON")
    phe.set_defaults(func=cmd_heuris)

    pfa = sub.add_parser("facts", help="heuris (H2): the facts ledger: show, remember, forget, approve, the auto log")
    pfa.add_argument("action", choices=["show", "remember", "forget", "proposed", "approve", "reject", "log", "undo",
                                        "queue", "import-applications"])
    pfa.add_argument("subject", nargs="?", help="show/remember: me, a registry id, person:<name>")
    pfa.add_argument("slot", nargs="?", help="show/remember: stage, offer:pay, note:<name>, ...")
    pfa.add_argument("value", nargs="?", help="remember: the value")
    pfa.add_argument("--id", type=int, help="forget/approve/reject/undo: the fact's id")
    pfa.add_argument("--source", action="append", help="remember: a pointer or where it was said (repeatable)")
    pfa.add_argument("--as-of", dest="as_of", help="remember: YYYY-MM-DD (default today)")
    pfa.add_argument("--route", default="home", choices=["chat", "home", "import"], help="who is writing")
    pfa.add_argument("--path", default="~/files/Careers/applications.toml", help="import-applications: the file")
    pfa.add_argument("--history", action="store_true", help="show: every value the slot has held")
    pfa.add_argument("--value", dest="value_as", help="approve: the value as corrected")
    pfa.add_argument("--days", type=int, default=14, help="log: how far back")
    pfa.add_argument("--json", action="store_true")
    pfa.set_defaults(func=cmd_facts)

    php = sub.add_parser("heuris-push", help="laptop: kept mail and project docs/commits, scrubbed, to the server")
    php.add_argument("--dry-run", action="store_true", help="build the staging tree; send nothing")
    php.set_defaults(func=cmd_heuris_push)

    phs = sub.add_parser("home-system", help="Home's system section as JSON (read by the window)")
    phs.set_defaults(func=cmd_home_system)


    pgt = sub.add_parser(
        "gates", help="what each threshold rejected over the last N days (no API; dead-gate check)"
    )
    pgt.add_argument("--days", type=int, default=7, help="window in days (default: 7)")
    pgt.set_defaults(func=cmd_gates)


    prd = sub.add_parser(
        "read",
        help="push to the tablet: markdown rendered to a device-tuned PDF, or a PDF as-is (rmapi)",
    )
    prd.add_argument("path", help="a markdown file, a directory of *.md files, or a PDF to push as-is")
    prd.add_argument("--title", default=None,
                     help="device document name for a PDF push (default: the filename)")
    prd.add_argument("--to", default=None, help="device folder (default: [reading].target_folder)")
    prd.add_argument("--out", default=None, help="write PDFs to this dir (default: beside the source)")
    prd.add_argument("--no-push", action="store_true", help="render locally only; skip the rmapi push")
    prd.set_defaults(func=cmd_read)

    pwl = sub.add_parser(
        "warm-latex",
        help="pay the one-off LaTeX package download now, so the first send to the tablet is fast",
    )
    pwl.set_defaults(func=cmd_warm_latex)

    pmk = sub.add_parser(
        "marks",
        help="read back what you highlighted and wrote on the tablet (free; --images fetches)",
    )
    pmk.add_argument("document", nargs="?", default=None,
                     help="title fragment or source_uri (omit to list what you have annotated)")
    pmk.add_argument("--page", type=int, default=None, help="restrict to one 1-based page")
    pmk.add_argument("--intent", default=None,
                     choices=("important", "not_understood", "idea"), help="filter by intent")
    pmk.add_argument("--images", type=int, nargs="?", const=4, default=0, metavar="N",
                     help="also write the N most-marked pages as PNGs with your ink on them")
    pmk.add_argument("--out", default=None, help="directory for --images (default: cwd)")
    pmk.add_argument("--refresh", action="store_true",
                     help="re-fetch from the device and re-sweep before rendering")
    pmk.add_argument("--no-margins", action="store_true",
                     help="render on the page rect only, clipping margin ink (the old behaviour)")
    pmk.set_defaults(func=cmd_marks)


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
        help="capture from the reMarkable cloud: notebooks transcribed, ink on PDFs read (billed)",
    )
    pcs.add_argument("--staging", default=None, help="staging dir (default: [capture].staging_dir)")
    pcs.add_argument("--no-ingest", action="store_true", help="render notes only; skip the notes-sync ingest")
    pcs.add_argument("--recapture", action="store_true",
                     help="transcribe notebooks the old tablet agent already captured, instead of seeding them")
    pcs.set_defaults(func=cmd_capture_sync)

    pbu = sub.add_parser(
        "budget", help="model calls on the subscription by job: today and the last 7 days",
    )
    pbu.set_defaults(func=cmd_budget)


    prc = sub.add_parser(
        "record", help="record a unit failure (systemd OnFailure) or a completed multi-step unit"
    )
    prc.add_argument("unit", help="the systemd unit, or the run kind for --ok")
    prc.add_argument("--ok", action="store_true", help="record a SUCCESSFUL run of this kind")
    prc.add_argument("--detail", default=None, help="exit status / cause, for triage")
    prc.set_defaults(func=cmd_record)


    pns = sub.add_parser(
        "notes-sync",
        help="incrementally ingest data/notes/ (changed notes only; deletes removed ones)",
    )
    pns.add_argument("--dir", default=None, help="notes directory (default: [paths].notes)")
    pns.add_argument(
        "--default-maturity", choices=["rough", "tidy"], default="rough",
        help="maturity for notes without a frontmatter maturity (default: rough)",
    )
    pns.set_defaults(func=cmd_notes_sync)

    pb = sub.add_parser("backup", help="snapshot the corpus (DB + raw store + notes)")
    pb.add_argument("--dest", default=None, help="backup root (default: data/backups)")
    pb.add_argument("--list", action="store_true", help="list existing snapshots and exit")
    pb.set_defaults(func=cmd_backup)

    pmg = sub.add_parser("migrate", help="upgrade the DB schema to head; print before/after")
    pmg.set_defaults(func=cmd_migrate)

    prs = sub.add_parser("restore", help="restore the corpus from a snapshot (destructive)")
    prs.add_argument("snapshot", help="snapshot dir path, or bare name under the backup root")
    prs.add_argument("--dest", default=None, help="backup root for bare names (default: data/backups)")
    prs.add_argument("--yes", action="store_true", help="actually overwrite live state (else dry-run)")
    prs.set_defaults(func=cmd_restore)

    pa = sub.add_parser("audit", help="structural ingest-quality metrics (no API)")
    pa.add_argument("--doc", default=None, help="restrict to one document id")
    pa.set_defaults(func=cmd_audit)


    args = parser.parse_args(argv)
    from locus.agent import usage

    usage.set_default_job(args.cmd if getattr(args, "cmd", None) else "cli")
    kind = _journal_kind(args)
    if kind is None:
        args.func(args)
        return
    # Journalled: the row is opened BEFORE the work and committed immediately, so a crash leaves a
    # visible orphan rather than no evidence at all (journal.run's crash-safety contract).
    from locus.agent import journal

    conn = _open()
    try:
        with journal.run(conn, kind):
            args.func(args)
    finally:
        conn.close()


if __name__ == "__main__":
    main()
