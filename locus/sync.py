"""Commit-triggered sync of tracked code repos (PLAN.md step 10).

The repos in config `[repos]` are working directories under active development. Each
sync pass compares every repo's `git rev-parse HEAD` against the stored document's
`content_hash` (located by `source_uri`, which is stable across commits) and re-ingests
only repos with new commits — so the hourly check inside `locus watch` is ~free, and an
actual re-ingest happens only after the owner commits. The pass-output cache
(ingest_pipeline._SummaryCache) makes that re-ingest proportional to the files the
commit touched, not the repo size.

Errors are per-repo: a missing path or non-git dir is reported and the batch continues.
"""

from __future__ import annotations

import logging
from pathlib import Path

from locus.config import load
from locus.extract.code import repo_head
from locus.ingest_pipeline import IngestResult, ingest_repo

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
            results.append(IngestResult(str(repo), "skipped", doc_id=existing["id"]))
            continue
        log.info("sync: %s @ %s -> ingesting", repo.name, head[:12])
        results.append(ingest_repo(repo, conn, force=force))
    return results
