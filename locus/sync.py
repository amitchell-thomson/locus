"""Commit-triggered sync of tracked code repos (build step 10).

The repos in config `[repos]` are working directories under active development. Each
sync pass compares every repo's `git rev-parse HEAD` against the stored document's
`content_hash` (located by `source_uri`, which is stable across commits) and re-ingests
only repos with new commits — so the hourly check inside `locus watch-repo` is ~free, and
an actual re-ingest happens only after the owner commits. That re-ingest is INCREMENTAL
(locus/repo_sync.py): a blob-sha diff against the stored manifest re-prepares only the
files the commit changed, leaving every unchanged section/chunk/vector untouched.

Errors are per-repo: a missing path or non-git dir is reported and the batch continues.
"""

from __future__ import annotations

import logging
from pathlib import Path

from locus.config import load
from locus.extract.code import repo_head
from locus.ingest_pipeline import IngestResult
from locus.repo_sync import reingest_repo_incremental

log = logging.getLogger(__name__)


def sync_repos(conn, repos: list[Path] | None = None, *, force: bool = False) -> list[IngestResult]:
    """One sync pass over the tracked repos (default: config `[repos].paths`).

    `force=True` re-ingests even when HEAD is unchanged — e.g. after a Locus pipeline
    upgrade (a summarize PROMPT_VERSION bump also invalidates the pass cache, so forced
    runs genuinely re-run the passes).
    """
    if repos is None:
        repos = [Path(p) for p in load().repos.paths]
    results: list[IngestResult] = []
    for repo in repos:
        repo = Path(repo).resolve()
        if not repo.is_dir():
            log.warning("sync: %s is not a directory; skipping", repo)
            results.append(IngestResult(str(repo), "quarantined", error="not a directory"))
            continue
        head = repo_head(repo)
        if head is None:
            log.warning("sync: %s is not a git repository; skipping (tracked repos must be git)", repo)
            results.append(IngestResult(str(repo), "quarantined", error="not a git repository"))
            continue
        existing = conn.execute(
            "SELECT id, content_hash FROM documents WHERE source_uri=? AND source_type='code'",
            (str(repo),),
        ).fetchone()
        if existing and existing["content_hash"] == head and not force:
            # Cheap unchanged-HEAD skip BEFORE the blob hashing in reingest — keeps the
            # hourly poll ~free (a `git rev-parse`, no file reads).
            results.append(IngestResult(str(repo), "skipped", doc_id=existing["id"]))
            continue
        log.info("sync: %s @ %s -> ingesting (incremental)", repo.name, head[:12])
        results.append(reingest_repo_incremental(repo, conn, force=force))
    return results
