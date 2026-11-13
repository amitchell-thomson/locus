"""`locus status` — a one-screen operational health summary.

Recurring manual checks collected into one command, with explicit WARN lines for the conditions
that have actually bitten before:

  - **backup age** — the corpus is one SQLite file; "when did I last snapshot it" should not
    require remembering the backup path.
  - **build drift** — a long-lived server runs its start-time code; the stamp here is the one to
    compare against the server's startup log.

The alias-substrate block and the reading-discovery counters went with the old engine (Slice C,
C2): `locus link` no longer runs, so the block only reported a stale table.

Pure collection (collect_status) is separated from rendering (format_status) so the metrics
are unit-testable without a live process.
"""

from __future__ import annotations

import json
import sqlite3
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
    quarantine_count: int      # real ingest FAILURES — a bug to triage (§14)
    db_bytes: int
    last_backup: str | None       # snapshot dir name
    last_backup_age_days: float | None
    build_stamp: str
    warnings: list[str] = field(default_factory=list)
    # Not documents (images, lockfiles, empty files): set aside so they are not retried, and
    # deliberately NOT a warning — only `quarantine_count` means "a bug to triage".
    unsupported_count: int = 0


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

    def _set_aside(dirname: str) -> int:
        d = incoming / dirname
        if not d.exists():
            return 0
        return sum(1 for p in d.rglob("*") if p.is_file() and not p.name.startswith("."))

    qdir = incoming / ".quarantine"
    quarantine_count = _set_aside(".quarantine")
    # Counted apart from quarantine, and NOT a warning: a PNG or a `uv.lock` is not a document and
    # never will be. Filing the two together kept 8 permanent files in the triage pile and made
    # `locus status` warn every day about something that was never actionable.
    unsupported_count = _set_aside(".unsupported")

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
    if quarantine_count:
        warnings.append(
            f"{quarantine_count} quarantined file(s) under {qdir} — triage them (§14)"
        )
    # A tablet folder the registry does not know still captures under its area, but nothing
    # links it to a course, repo or application. Named here, from the last capture run, so a typo
    # or an unregistered course is fixed rather than filed under nothing (Slice 0.12).
    try:
        last_capture = conn.execute(
            "SELECT stats FROM agent_runs WHERE kind='capture' AND stats IS NOT NULL "
            "ORDER BY id DESC LIMIT 1"
        ).fetchone()
    except sqlite3.OperationalError:          # a database without the agent tables
        last_capture = None
    if last_capture:
        try:
            unmatched = json.loads(last_capture[0] or "{}").get("unmatched_folders") or []
        except ValueError:
            unmatched = []
        if unmatched:
            warnings.append(
                f"tablet folder(s) not in taxonomy/registry.toml: {', '.join(unmatched)} — "
                "add them or rename the folder"
            )
    if last_backup is None:
        warnings.append("no backup snapshot found — run `locus backup`")
    elif last_backup_age_days is not None and last_backup_age_days > 7:
        warnings.append(
            f"last backup is {last_backup_age_days:.0f} days old — run `locus backup`"
        )

    return StatusReport(
        doc_total=doc_total,
        by_category=by_category,
        by_source_type=by_source_type,
        last_ingest=last_ingest,
        unit_counts=unit_counts,
        vector_total=vector_total,
        quarantine_count=quarantine_count,
        unsupported_count=unsupported_count,
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
    lines.append("   compare with the running `locus serve` startup log; restart it if they differ")
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
    aside = f"  ({r.unsupported_count} unsupported, not documents)" if r.unsupported_count else ""
    lines.append(f"quarantine             : {r.quarantine_count} file(s){aside}")
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
