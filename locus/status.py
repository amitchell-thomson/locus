"""`locus status` — a one-screen operational health summary.

Production-hardening turned several recurring manual checks into things the owner has to
remember to run (is the alias substrate stale after an ingest? is a backup recent? does the
running MCP server predate the last deploy?). This collects them into one command so the
state of the system is legible at a glance, with explicit WARN lines for the conditions that
have actually bitten before:

  - **alias staleness** — `locus link` builds a TOTAL (name,type)->canonical mapping, so any
    entity surface absent from entity_aliases means docs were ingested since the last link
    and the substrate no longer covers the corpus (related-docs/entity-arm silently degrade).
  - **backup age** — the corpus is one SQLite file; "when did I last snapshot it" should not
    require remembering the backup path.
  - **MCP build drift** — a long-lived stdio-over-SSH server runs its start-time code; the
    stamp here is the one to compare against the server's startup log (§2 / round-7).

Pure collection (collect_status) is separated from rendering (format_status) so the metrics
are unit-testable without a live process.
"""

from __future__ import annotations

import subprocess
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path


@dataclass
class StatusReport:
    doc_total: int
    by_category: dict[str, int]
    by_source_type: dict[str, int]
    last_ingest: str | None
    unit_counts: dict[str, int]  # chunks/propositions/entities/figures/sections
    vector_total: int            # sum across the four vec0 tables (KNN scale signal)
    aliases_built: bool
    alias_cross_doc_canonicals: int
    alias_uncovered_surfaces: int  # entity (name,type) not yet in entity_aliases -> link stale
    quarantine_count: int
    db_bytes: int
    last_backup: str | None       # snapshot dir name
    last_backup_age_days: float | None
    build_stamp: str
    warnings: list[str] = field(default_factory=list)
    # Reading discovery (Phase 4). Optional: a database predating migration 0017 has none of
    # these tables, and `locus status` must stay useful on one rather than raising.
    reading: dict[str, int] | None = None


def _build_stamp(project_root: Path) -> str:
    """git commit + authored date + dirty flag — the same shape the MCP server logs at start,
    so `locus status` and the server's log line are directly comparable. 'unknown' if no git."""
    def _git(*args: str) -> str:
        return subprocess.run(
            ["git", "-C", str(project_root), *args],
            capture_output=True, text=True, timeout=5,
        ).stdout.strip()

    try:
        commit = _git("rev-parse", "--short", "HEAD") or "unknown"
        when = _git("show", "-s", "--format=%cs", "HEAD")
        dirty = "+dirty" if _git("status", "--porcelain") else ""
        return f"{commit}{dirty} ({when})" if when else f"{commit}{dirty}"
    except Exception:
        return "unknown"


def _reading_health(conn) -> "dict[str, int] | None":
    """Reading-discovery counters, or None on a database predating migration 0017."""
    try:
        one = lambda sql: conn.execute(sql).fetchone()[0]
        return {
            "proposed": one("SELECT COUNT(*) FROM reading_proposals WHERE status='proposed'"),
            "queued": one("SELECT COUNT(*) FROM reading_proposals WHERE status='candidate'"),
            "pool": one("SELECT COUNT(*) FROM discovery_candidates"),
            "accepted": one(
                "SELECT COUNT(*) FROM reading_proposals WHERE status IN ('accepted','ingested')"),
            "rejected": one("SELECT COUNT(*) FROM reading_proposals WHERE status='rejected'"),
            "marks": one("SELECT COALESCE(SUM(marks), 0) FROM reading_targets"),
        }
    except Exception:
        return None


def _count(conn, table: str) -> int:
    return conn.execute(f"SELECT COUNT(*) FROM {table}").fetchone()[0]


def collect_status(
    conn,
    *,
    db_path: Path,
    incoming: Path,
    backup_root: Path,
    project_root: Path,
    now: datetime | None = None,
) -> StatusReport:
    now = now or datetime.now(timezone.utc)

    doc_total = _count(conn, "documents")
    by_category = {
        r["category"] or "(none)": r["n"]
        for r in conn.execute(
            "SELECT category, COUNT(*) n FROM documents GROUP BY category ORDER BY n DESC"
        )
    }
    by_source_type = {
        r["source_type"]: r["n"]
        for r in conn.execute(
            "SELECT source_type, COUNT(*) n FROM documents GROUP BY source_type ORDER BY n DESC"
        )
    }
    last_ingest = conn.execute("SELECT MAX(ingested_at) FROM documents").fetchone()[0]

    unit_counts = {
        t: _count(conn, t) for t in ("sections", "chunks", "propositions", "entities", "figures")
    }
    vector_total = sum(
        _count(conn, t)
        for t in ("chunk_vectors", "section_vectors", "proposition_vectors", "figure_vectors")
    )

    from locus.link.related import aliases_built

    built = aliases_built(conn)
    cross_doc = 0
    uncovered = 0
    if built:
        from locus.eval.metrics import alias_qc

        qc = alias_qc(conn)
        cross_doc = qc.cross_doc_canonicals if qc else 0
        # Total mapping => every entity surface should appear as a variant. Any that don't
        # were ingested after the last `locus link` and aren't covered by the substrate.
        uncovered = conn.execute(
            """
            SELECT COUNT(*) FROM (
              SELECT DISTINCT e.name, e.type FROM entities e
              LEFT JOIN entity_aliases a
                ON a.variant_name = e.name AND a.variant_type = e.type
              WHERE a.variant_name IS NULL
            )
            """
        ).fetchone()[0]

    quarantine_count = 0
    qdir = incoming / ".quarantine"
    if qdir.exists():
        quarantine_count = sum(
            1 for p in qdir.rglob("*") if p.is_file() and not p.name.startswith(".")
        )

    db_bytes = db_path.stat().st_size if db_path.exists() else 0

    from locus import backup as bk

    snaps = bk.list_snapshots(backup_root)
    last_backup = None
    last_backup_age_days = None
    if snaps:
        last = snaps[-1]
        last_backup = last.name
        m = bk.read_manifest(last)
        if m and m.created_at:
            try:
                created = datetime.fromisoformat(m.created_at)
                if created.tzinfo is None:
                    created = created.replace(tzinfo=timezone.utc)
                last_backup_age_days = (now - created).total_seconds() / 86400
            except ValueError:
                pass

    warnings: list[str] = []
    if not built:
        warnings.append("entity-alias substrate not built — run `locus link`")
    elif uncovered:
        warnings.append(
            f"{uncovered} entity surface(s) not in the alias substrate — "
            "ingest happened since the last `locus link`; rerun it"
        )
    if quarantine_count:
        warnings.append(
            f"{quarantine_count} quarantined file(s) under {qdir} — triage them (§14)"
        )
    if last_backup is None:
        warnings.append("no backup snapshot found — run `locus backup`")
    elif last_backup_age_days is not None and last_backup_age_days > 7:
        warnings.append(
            f"last backup is {last_backup_age_days:.0f} days old — run `locus backup`"
        )

    return StatusReport(
        reading=_reading_health(conn),
        doc_total=doc_total,
        by_category=by_category,
        by_source_type=by_source_type,
        last_ingest=last_ingest,
        unit_counts=unit_counts,
        vector_total=vector_total,
        aliases_built=built,
        alias_cross_doc_canonicals=cross_doc,
        alias_uncovered_surfaces=uncovered,
        quarantine_count=quarantine_count,
        db_bytes=db_bytes,
        last_backup=last_backup,
        last_backup_age_days=last_backup_age_days,
        build_stamp=_build_stamp(project_root),
        warnings=warnings,
    )


def _fmt_bytes(n: float) -> str:
    for unit in ("B", "KiB", "MiB", "GiB"):
        if n < 1024 or unit == "GiB":
            return f"{n:.0f} {unit}" if unit == "B" else f"{n:.1f} {unit}"
        n /= 1024
    return f"{n:.1f} GiB"


def format_status(r: StatusReport) -> str:
    lines = ["LOCUS STATUS", "=" * 60]
    lines.append(f"build (this checkout)  : {r.build_stamp}")
    lines.append("   compare with the running `locus mcp` startup log; restart it if they differ")
    lines.append("")
    lines.append(f"documents              : {r.doc_total}")
    cats = "  ".join(f"{k}:{v}" for k, v in r.by_category.items())
    lines.append(f"   by category         : {cats}")
    types = "  ".join(f"{k}:{v}" for k, v in r.by_source_type.items())
    lines.append(f"   by source_type      : {types}")
    lines.append(f"   last ingest         : {r.last_ingest or '(none)'}")
    lines.append("")
    units = "  ".join(f"{k}:{v}" for k, v in r.unit_counts.items())
    lines.append(f"units                  : {units}")
    lines.append(f"vectors (vec0 total)   : {r.vector_total}  [brute-force KNN]")
    lines.append("")
    if r.aliases_built:
        lines.append(
            f"alias substrate        : built  | cross-doc canonicals {r.alias_cross_doc_canonicals}"
            f"  | uncovered surfaces {r.alias_uncovered_surfaces}"
        )
    else:
        lines.append("alias substrate        : NOT BUILT")
    if r.reading is not None:
        d = r.reading
        lines.append(
            f"reading proposed       : {d['proposed']} on the tablet | {d['queued']} queued "
            f"| pool {d['pool']}"
        )
        lines.append(
            f"reading judged         : {d['accepted']} accepted / {d['rejected']} rejected "
            f"| {d['marks']} mark(s) read back"
        )
    lines.append(f"quarantine             : {r.quarantine_count} file(s)")
    lines.append(f"database               : {_fmt_bytes(r.db_bytes)}")
    if r.last_backup:
        age = f"{r.last_backup_age_days:.1f} days ago" if r.last_backup_age_days is not None else "age unknown"
        lines.append(f"last backup            : {r.last_backup}  ({age})")
    else:
        lines.append("last backup            : (none)")
    lines.append("")
    lines.extend(_activity_block(r))
    lines.append("")
    if r.warnings:
        lines.append("WARNINGS")
        lines.extend(f"  ! {w}" for w in r.warnings)
    else:
        lines.append("no warnings — system healthy")
    return "\n".join(lines)


def _activity_block(r: "StatusReport") -> list[str]:
    """Since yesterday: what ran, what it cost, and what is broken.

    Reads `locus.health`, which is the same source the daily page's status line uses — one
    definition of "healthy", so the page and the terminal cannot disagree. Spend is broken out per
    kind because a single total answers "how much" but never "what for", and the answer to the
    second is what actually changes a decision about it.
    """
    from locus import health as H
    from locus.config import load
    from locus.db.connection import get_connection

    try:
        conn = get_connection(load().paths.db)
    except Exception:                                   # status must never fail on its own report
        return []
    try:
        checked = H.check(conn)
        spend = H.spend_by_kind(conn)
    finally:
        conn.close()

    lines = ["SINCE YESTERDAY"]
    if checked.ran:
        ran = " · ".join(f"{k} x{n}" if n > 1 else k for k, n in sorted(checked.ran.items()))
        lines.append(f"  ran                  : {ran}")
    else:
        lines.append("  ran                  : (nothing)")

    cap = getattr(load().agent, "daily_cost_cap_usd", None)
    against = f" of ${cap:.2f} cap" if cap else ""
    lines.append(f"  spend                : ${checked.cost_usd:.4f}{against}  "
                 f"({checked.calls} model call(s))")
    for kind, cost, calls in spend[:5]:
        if cost:
            lines.append(f"      {kind:<18} ${cost:.4f}  ({calls} call(s))")

    if checked.problems:
        lines.append(f"  PROBLEMS             : {len(checked.problems)}")
        lines.extend(f"      ! {p.render()}" for p in checked.problems)
    else:
        lines.append("  problems             : none — every timer is on schedule")
    # Failures a later run of the same kind superseded. Off the daily page (which reports the
    # present tense) but printed here, because "it broke and recovered" is exactly what a
    # flapping job looks like, and it is only visible if something says so.
    if checked.recovered:
        lines.append(f"  recovered             : {len(checked.recovered)}")
        lines.extend(f"      ~ {p.render()}" for p in checked.recovered)
    return lines
