"""Loop B on the capture timer — reading annotations captured and transcribed automatically.

WHY THIS EXISTS (2026-08-06). The owner asked why only one of his margin questions on a paper
came back answered. The chain that produces an answer is: ink captured → handwriting TRANSCRIBED
into `note` → intent classified `not_understood` → `locus review --answer-marks` answers it →
the daily page prints it. Every link ran on a timer except the second: `transcribe_marks` only
ran inside the manual `locus annotate --transcribe`, so a paper he annotated but never ran the
command for had 105 marks of stored geometry and not one written word — questions the system
could not see, on a surface whose whole promise is that it sees them. A path that looks wired
and isn't (CLAUDE.md §3).

WHAT RUNS. `annotate_sync` rides the same half-hourly capture timer as Loop A, consuming the
same staging dir. Loop A takes the Notes folders; this takes the READING folders Loop A
excludes. Per changed document: fetch the cloud `.rmdoc`, extract mark geometry (free, local),
store marks, transcribe NEW ink (bounded, billed vision — the same metered SDK path as Loop A's
transcription), and let the nightly intent/answer passes do the rest on their own timers.

THE DEVICE→CORPUS MAPPING IS BY CONTENT HASH. Marks must key on the CORPUS document's
source_uri or the whole downstream (answers, connections, links) never sees them. The manual
command left that to a `--source-uri` flag, and the one time it was forgotten (2026-08-06, by
the agent) 98 marks landed under a device path joined to nothing. The `.rmdoc` bundle carries
the original PDF bytes and `documents.content_hash` is sha256 of exactly those bytes, so the
join is computed, not remembered: hash match → that document's source_uri; no match (not yet
ingested) → the device path, which `annotate --ingest`'s rekey step upgrades later. Verified
live before building: the parent-orders bundle's hash equals the ingested row's content_hash.

CHANGE DETECTION reuses Loop A's staged-render manifest with an `annotate:` key prefix — the
device only pushes documents whose content changed, and an unchanged staged render means no new
ink, so a no-op run costs zero rmapi downloads and zero model calls.
"""

from __future__ import annotations

import hashlib
import logging
import sqlite3
import tempfile
from dataclasses import dataclass, field
from pathlib import Path

log = logging.getLogger(__name__)

# Top-level device folders whose documents are READING material — the complement of Loop A's
# exclusions, listed explicitly rather than derived so adding a device folder never silently
# opts it into billed transcription. Pre-migration name kept like Loop A keeps its exclusions.
READING_FOLDERS: tuple[str, ...] = ("Reading", "reading_list")


@dataclass
class AnnotateOutcome:
    uuid: str
    name: str
    status: str                   # 'annotated' | 'unchanged' | 'failed'
    source_uri: str = ""
    hash_mapped: bool = False     # True when content-hash resolved a corpus document
    marks: int = 0
    transcribed: int = 0
    error: str | None = None


@dataclass
class AnnotateSyncResult:
    outcomes: list[AnnotateOutcome] = field(default_factory=list)

    @property
    def annotated(self) -> int:
        return sum(o.status == "annotated" for o in self.outcomes)


def reading_index(runner, *, reading_folders: tuple[str, ...] = READING_FOLDERS) -> dict:
    """uuid -> (full device path, name) for every document under a reading folder.

    `build_uuid_index` deliberately drops the full path (Loop A never needs it); Loop B fetches
    the `.rmdoc` by path, so this keeps it.
    """
    from locus.capture.remarkable import _find_file_paths, _stat

    index: dict[str, tuple[str, str]] = {}
    for device_path in _find_file_paths(runner):
        top = device_path.strip("/").split("/", 1)[0]
        if top not in reading_folders:
            continue
        meta = _stat(runner, device_path)
        if not meta or not meta.get("ID"):
            continue
        index[meta["ID"]] = (device_path, meta.get("Name") or Path(device_path).name)
    return index


def _corpus_uri_for(conn: sqlite3.Connection, pdf_bytes: bytes) -> str | None:
    """The ingested document these exact PDF bytes are, or None.

    `documents.content_hash` is sha256 of the ingested file's bytes and the `.rmdoc` bundle
    carries the original PDF unmodified, so equality here IS document identity — the same
    idempotency key ingest uses, pointed the other way.
    """
    h = hashlib.sha256(pdf_bytes).hexdigest()
    row = conn.execute(
        "SELECT source_uri FROM documents WHERE content_hash=?", (h,)
    ).fetchone()
    return row["source_uri"] if row else None


def annotate_sync(
    conn: sqlite3.Connection,
    *,
    staging_dir: str | Path | None = None,
    manifest: dict | None = None,
    index_fn=None,
    fetch_fn=None,
    read_fn=None,
    marks_fn=None,
    transcribe_fn=None,
    transcribe_limit: int | None = None,
) -> AnnotateSyncResult:
    """Capture + transcribe reading annotations for every staged, changed reading document.

    `manifest` maps staged-render identity to what was last processed; None loads and persists
    Loop A's manifest file, under `annotate:<uuid>` keys so the two loops share one file without
    colliding. All externals are injectable for model-free tests. Failure on one document never
    aborts the batch (ingest §7).
    """
    from locus.agent import journal
    from locus.capture.annotate import store_marks
    from locus.capture.loop_a import MANIFEST_NAME, _load_manifest, _save_manifest
    from locus.capture.mark_text import transcribe_marks
    from locus.capture.remarkable import _subprocess_runner
    from locus.capture.rmdoc import fetch_rmdoc, read_rmdoc
    from locus.config import load

    cfg = load()
    staging = Path(staging_dir) if staging_dir is not None else cfg.capture.staging_dir
    mpath = None
    if manifest is None:
        mpath = cfg.paths.raw_store / MANIFEST_NAME
        manifest = _load_manifest(mpath)
    limit = (
        transcribe_limit
        if transcribe_limit is not None
        else cfg.capture.annotate_max_transcribe
    )
    index_fn = index_fn or (
        lambda: reading_index(_subprocess_runner(cfg.capture.rmapi_binary))
    )
    fetch_fn = fetch_fn or (
        lambda path, dest: fetch_rmdoc(path, dest, rmapi_binary=cfg.capture.rmapi_binary)
    )
    read_fn = read_fn or read_rmdoc
    if marks_fn is None:
        from locus.capture.annotate import marks_for_document

        marks_fn = marks_for_document
    if transcribe_fn is None:

        def transcribe_fn(conn, marks, *, source_uri, limit):
            return transcribe_marks(
                conn, marks, source_uri=source_uri,
                model=cfg.capture.transcribe_model, limit=limit,
            )

    result = AnnotateSyncResult()
    staged = sorted(staging.glob("*.pdf")) if staging.exists() else []
    if not staged:
        return result
    try:
        index = index_fn()
    except Exception as exc:  # cloud unreachable: capture again next tick, nothing lost
        log.warning("annotate-sync: reading index failed: %s", exc)
        return result

    budget = max(0, int(limit))
    with journal.run(conn, "annotate") as run:
        for pdf in staged:
            uuid = pdf.stem
            if uuid not in index:
                continue
            device_path, name = index[uuid]
            key = f"annotate:{uuid}"
            h = hashlib.sha256(pdf.read_bytes()).hexdigest()
            if manifest.get(key) == h:
                result.outcomes.append(AnnotateOutcome(uuid, name, "unchanged"))
                continue
            try:
                with tempfile.TemporaryDirectory(prefix="locus-loopb-") as tmp:
                    doc = read_fn(fetch_fn(device_path, tmp))
                    marks = marks_fn(doc)
                    uri = _corpus_uri_for(conn, doc.pdf_bytes)
                    hash_mapped = uri is not None
                    uri = uri or device_path
                    stored = store_marks(
                        conn, marks, source_uri=uri, doc_uuid=doc.doc_uuid,
                        source_run=run.id,
                    )
                    transcribed = 0
                    if budget > 0 and cfg.capture.annotate_transcribe:
                        transcribed = transcribe_fn(
                            conn, marks, source_uri=uri, limit=budget
                        )
                        budget -= transcribed
                manifest[key] = h
                result.outcomes.append(
                    AnnotateOutcome(
                        uuid, name, "annotated", source_uri=uri, hash_mapped=hash_mapped,
                        marks=stored, transcribed=transcribed,
                    )
                )
                if not hash_mapped:
                    # Loud, not fatal: marks keyed by device path join to no corpus document, so
                    # answers/links cannot see them until the doc is ingested and re-keyed.
                    log.warning(
                        "annotate-sync: %s not in the corpus (no content-hash match) — "
                        "marks keyed by device path %r", name, device_path,
                    )
            except Exception as exc:  # one doc's failure never aborts the batch
                log.warning("annotate-sync: %s (%s) failed: %s", name, uuid[:8], exc)
                result.outcomes.append(AnnotateOutcome(uuid, name, "failed", error=str(exc)))
        run.stats = {
            "annotated": result.annotated,
            "unchanged": sum(o.status == "unchanged" for o in result.outcomes),
            "failed": sum(o.status == "failed" for o in result.outcomes),
            "marks": sum(o.marks for o in result.outcomes),
            "transcribed": sum(o.transcribed for o in result.outcomes),
            "unmapped_to_corpus": sum(
                1 for o in result.outcomes if o.status == "annotated" and not o.hash_mapped
            ),
        }
    if mpath is not None:
        _save_manifest(mpath, manifest)
    return result
