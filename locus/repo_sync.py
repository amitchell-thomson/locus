"""Incremental re-ingest of a tracked code repo — re-process only the files a commit changed.

A repo is ONE document whose `content_hash` is the git HEAD, so any commit moves it and the
naive path re-ingests the whole repo (re-extract + re-embed every file, holding the single
ingest slot for the repo's full size). But Locus already stores a manifest of every eligible
file's `blob_sha` at ingest (`{hash}.manifest.json`), so the changed-file set is a pure
content diff — no git invocation, and it works for non-git drops too.

This module re-prepares ONLY changed/added files, surgically swaps their sections inside the
existing document, drops deleted files' sections, re-runs the (cheap, one-call) doc synthesis,
and updates the manifest. Unchanged sections, chunks, vectors, and the FTS index are left
entirely untouched — the GPU only embeds what actually changed.

Falls back to the proven full `ingest_repo` when there is no prior document, no stored
manifest, or `force` is set (a pipeline upgrade must re-run every file).
"""

from __future__ import annotations

import json
import logging
from pathlib import Path

from locus.config import load
from locus.extract import code as code_extract
from locus.extract import pdf as pdf_extract  # title_is_suspect
from locus.ingest import embed, synthesis
from locus.ingest_pipeline import (
    IngestResult,
    _SummaryCache,
    _delete_section_rows,
    _insert_section,
    _prepare_section,
    _write_repo_manifest,
    ingest_repo,
    pass_profile,
)

log = logging.getLogger(__name__)

# Transient offset that lifts surviving sections' positions clear of the final 0..N range
# while they are repositioned, so no intermediate UPDATE trips UNIQUE(doc_id, position).
_PARK = 1_000_000


def _load_manifest_blobs(content_hash: str) -> dict[str, str] | None:
    """The {path: blob_sha} map a prior ingest stored, or None if no manifest exists."""
    path = load().paths.raw_store / f"{content_hash}.manifest.json"
    if not path.exists():
        return None
    try:
        data = json.loads(path.read_text())
        return {f["path"]: f["blob_sha"] for f in data.get("files", [])}
    except (json.JSONDecodeError, KeyError, TypeError):
        return None


def diff_blobs(old: dict[str, str], new: dict[str, str]) -> tuple[set[str], set[str]]:
    """(changed_or_added, deleted) eligible file paths between two blob-sha maps."""
    changed = {p for p, sha in new.items() if old.get(p) != sha}
    deleted = {p for p in old if p not in new}
    return changed, deleted


def reingest_repo_incremental(repo_path, conn, *, force: bool = False) -> IngestResult:
    """Re-ingest a tracked repo, touching only the sections whose files changed. Never raises."""
    repo = Path(repo_path).resolve()
    uri = str(repo)
    try:
        snap = code_extract.collect_repo(repo)
        existing = conn.execute(
            "SELECT id, content_hash, title FROM documents WHERE source_uri=? AND source_type='code'",
            (uri,),
        ).fetchone()
        if existing is None:
            return ingest_repo(repo, conn, force=force)  # first ingest — full path
        if existing["content_hash"] == snap.content_hash and not force:
            return IngestResult(str(repo), "skipped", doc_id=existing["id"])
        old_blobs = _load_manifest_blobs(existing["content_hash"])
        if old_blobs is None or force:
            # No diff base (pre-manifest doc / LocusDrop tarball) or a forced full re-run.
            return ingest_repo(repo, conn, force=force)

        doc_id = existing["id"]
        changed, deleted = diff_blobs(old_blobs, snap.blob_shas)

        # HEAD moved but no INGESTIBLE file changed (a docs/test/.gitignore-only commit): just
        # advance the diff base. No extraction, no synthesis, no API — the whole point.
        if not changed and not deleted:
            _advance_manifest(conn, doc_id, existing["content_hash"], snap, repo)
            log.info("incremental: %s @ %s — no eligible file changed", repo.name, snap.content_hash[:12])
            return IngestResult(str(repo), "skipped", doc_id=doc_id)

        doc = code_extract.extract_repo(repo, snap)  # full extract (cheap; no LLM/embed yet)
        sections_by_path = {s.file_path: s for s in doc.sections}
        # A changed file that no longer yields a section (e.g. became a trivial __init__.py)
        # is, for the document, a deletion of its old section.
        changed_present = {p for p in changed if p in sections_by_path}
        gone = deleted | (changed - changed_present)

        profile = pass_profile("code")
        cache = _SummaryCache(conn, snap.blob_shas)
        pass_gaps: list[str] = []
        prepared = [
            _prepare_section(sections_by_path[p], profile, cache, pass_gaps)
            for p in sorted(changed_present)
        ]
        embed_model = embed.embedding_model()
        canonical = [s.file_path for s in doc.sections]  # final section order

        with conn:  # one transaction: the doc survives intact unless every step commits
            # 1. drop sections for files being replaced or removed
            for fp in sorted(changed_present | gone):
                row = conn.execute(
                    "SELECT id FROM sections WHERE doc_id=? AND file_path=?", (doc_id, fp)
                ).fetchone()
                if row:
                    _delete_section_rows(conn, row["id"])
            # 2. park survivors out of the 0..N range so repositioning can't collide
            conn.execute("UPDATE sections SET position = position + ? WHERE doc_id=?", (_PARK, doc_id))
            # 3. insert freshly-prepared sections (temp positions; fixed in step 4)
            for i, ps in enumerate(prepared):
                ps.position = _PARK * 2 + i
                _insert_section(conn, doc_id, ps, embed_model)
            # 4. assign final positions by file_path, in canonical extract order
            for pos, fp in enumerate(canonical):
                conn.execute(
                    "UPDATE sections SET position=? WHERE doc_id=? AND file_path=?", (pos, doc_id, fp)
                )
            # 5. re-synthesise from ALL current summaries (cheap) and update the doc row
            summaries = [
                r["summary"]
                for r in conn.execute(
                    "SELECT summary FROM sections WHERE doc_id=? ORDER BY position", (doc_id,)
                )
            ]
            syn = synthesis.synthesize_document(
                existing["title"], summaries, code=True, source_name=repo.name
            )
            title = existing["title"]
            if pdf_extract.title_is_suspect(title) and (syn.title or "").strip():
                title = syn.title.strip()
            section_map = [
                {"position": p, "title": s.title, "page_start": s.page_start, "page_end": s.page_end}
                for p, s in enumerate(doc.sections)
            ]
            conn.execute(
                "UPDATE documents SET content_hash=?, title=?, thesis=?, method=?, result=?, "
                "limitations=?, section_map=? WHERE id=?",
                (snap.content_hash, title, syn.thesis, syn.method, syn.result,
                 syn.limitations, json.dumps(section_map), doc_id),
            )

        cache.flush()  # only after the document committed
        _swap_manifest(existing["content_hash"], snap, repo)
        totals = conn.execute(
            "SELECT (SELECT COUNT(*) FROM chunks WHERE doc_id=?) c, "
            "(SELECT COUNT(*) FROM entities WHERE doc_id=?) e", (doc_id, doc_id)
        ).fetchone()
        log.info(
            "incremental: %s @ %s — %d changed, %d removed (of %d files)",
            repo.name, snap.content_hash[:12], len(changed_present), len(gone), len(canonical),
        )
        return IngestResult(
            str(repo), "ingested", doc_id=doc_id, sections=len(canonical),
            chunks=totals["c"], entities=totals["e"],
        )
    except Exception as exc:  # quarantine this repo, keep any batch alive (§6)
        log.warning("Quarantined incremental repo %s: %s", repo, exc)
        return IngestResult(str(repo), "quarantined", error=str(exc))


def _advance_manifest(conn, doc_id: int, old_hash: str, snap, repo: Path) -> None:
    """HEAD moved with no eligible change: bump content_hash + manifest, nothing else."""
    with conn:
        conn.execute("UPDATE documents SET content_hash=? WHERE id=?", (snap.content_hash, doc_id))
    _swap_manifest(old_hash, snap, repo)


def _swap_manifest(old_hash: str, snap, repo: Path) -> None:
    """Write the new manifest and remove the superseded one."""
    _write_repo_manifest(snap, repo)
    if old_hash != snap.content_hash:
        old = load().paths.raw_store / f"{old_hash}.manifest.json"
        old.unlink(missing_ok=True)
