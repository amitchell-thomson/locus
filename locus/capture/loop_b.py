"""Loop B on the capture timer — reading annotations captured and transcribed automatically.

WHY THIS EXISTS (2026-08-06). The owner asked why only one of his margin questions on a paper
came back answered. The chain that produces an answer is: ink captured → handwriting TRANSCRIBED
into `note` → intent classified `not_understood` → `locus review --answer-marks` answers it →
the daily page prints it. Every link ran on a timer except the second: `transcribe_marks` only
ran inside the manual `locus annotate --transcribe`, so a paper he annotated but never ran the
command for had 105 marks of stored geometry and not one written word — questions the system
could not see, on a surface whose whole promise is that it sees them. A path that looks wired
and isn't (CLAUDE.md §3).

WHAT RUNS. `annotate_sync` rides the same half-hourly capture timer as Loop A, over the same
placed tablet. Loop A takes the notebooks; this takes the PDFs. Per changed document: fetch the cloud `.rmdoc`, extract mark geometry (free, local),
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

CHANGE DETECTION is the document's ink key from the cloud tree (capture/device.py), stored in
Loop A's manifest under `annotate:<uuid>`. Until Slice 0.12 it was the hash of a render the
tablet agent pushed, and when that agent stopped on or about 1 Aug this loop saw nothing for
twelve weeks without a word. A no-op run still costs zero downloads and zero model calls.

THREE KINDS OF PDF since Slice 0.12, decided by where it sits and whether Locus sent it:
  - in `Reading/`: everything above, unchanged.
  - a PDF Locus SENT, wherever he filed it: Claude's document. Never ingested; its marks keep
    the device path as their key, and his notes are promoted WITH the passage each was written
    against, because "notes on their own are often meaningless" (Alec, 23 Oct).
  - any other PDF in an area folder: a SOURCE. Copied into the drop folder under its area
    (`incoming/studies/...`), where `locus-watch` ingests it like anything he drops there; its
    marks wait until the library has it, then key on its `source_uri` and are promoted the same
    way. A copy that was quarantined is never re-queued, so a bad PDF cannot loop.
Marks follow a document by its uuid: when he moves one, its earlier marks are re-keyed onto the
current key rather than left under the old path.

RELATIONSHIP TO `reading/sweep.py` (which predates this and coined "Loop B"): sweep is the
hourly CLOUD-PULL pass over `reading_targets` — geometry only, papers the system delivered —
and survives the device push channel dying. This module is the primary: any reading-folder
document, content-hash mapping, transcription, chosen-book ingest, and note promotion. Both
store through the same idempotent upsert, so the overlap on delivered papers is harmless by
construction; do not "deduplicate" them into one path, the redundancy is the resilience.
"""

from __future__ import annotations

import hashlib
import logging
import sqlite3
from dataclasses import dataclass, field
from pathlib import Path

log = logging.getLogger(__name__)

# Top-level device folders whose documents are READING material — the complement of Loop A's
# exclusions, listed explicitly rather than derived so adding a device folder never silently
# opts it into billed transcription. Pre-migration name kept like Loop A keeps its exclusions.
READING_FOLDERS: tuple[str, ...] = ("Reading", "reading_list")


# Reading sub-folders whose documents the owner has CHOSEN — moving a paper here is his one
# manual reading gesture, and it is the ingest trigger for documents that never went through
# the proposal flow (a book he added by hand). `Proposed` deliberately never auto-ingests:
# most proposals get rejected, and ingesting them all would pour agent-selected material into
# the corpus ahead of his verdict.
_CHOSEN_SUBFOLDERS: tuple[str, ...] = ("In-Progress", "Finished")


@dataclass
class AnnotateOutcome:
    uuid: str
    name: str
    # 'annotated' | 'unchanged' | 'deferred' | 'failed', and for a source not yet in the
    # library 'queued' | 'waiting' | 'quarantined' (see `_queue_source`)
    status: str
    source_uri: str = ""
    hash_mapped: bool = False     # True when content-hash resolved a corpus document
    ingested: bool = False        # True when this run ingested the document itself
    marks: int = 0
    transcribed: int = 0
    promoted: str = ""            # promote_reading_notes status ('' = no notes to promote)
    error: str | None = None
    role: str = ""                # the placement it was read under ('reading' | 'sent' | 'source')


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

    A native notebook has NO PDF bytes, and the hash of nothing is not a document identity.
    """
    if not pdf_bytes:
        return None
    h = hashlib.sha256(pdf_bytes).hexdigest()
    row = conn.execute(
        "SELECT source_uri FROM documents WHERE content_hash=?", (h,)
    ).fetchone()
    return row["source_uri"] if row else None


def _chosen(device_path: str) -> bool:
    """Has the owner moved this document into a folder that says "I am reading this"?"""
    parts = [p for p in device_path.strip("/").split("/") if p]
    return any(p in _CHOSEN_SUBFOLDERS for p in parts[1:])


def _ingest_from_bundle(conn: sqlite3.Connection, doc, name: str) -> str | None:
    """Ingest a CHOSEN document straight from the bundle's PDF bytes; its source_uri, or None.

    The proposal flow's `reading/accept.ingest_accepted` covers papers the SYSTEM delivered; a
    book the owner dropped onto the device by hand was the one reading document with no ingest
    path that didn't need a command typed. Same spine, same flock, same idempotency — the bundle
    bytes hash to the same content_hash ingest will compute, so a concurrent accept-loop ingest
    of the same file is a no-op, and `IngestLockHeld` degrades to retry-next-tick rather than
    contending (one ingest at a time, §14).
    """
    from locus.config import load as _load
    from locus.ingest_lock import IngestLockHeld, ingest_lock
    from locus.ingest_pipeline import ingest_file

    dest = Path(_load().paths.incoming) / "paper" / f"{name}.pdf"
    dest.parent.mkdir(parents=True, exist_ok=True)
    if not dest.exists():
        dest.write_bytes(doc.pdf_bytes)
    try:
        with ingest_lock():
            result = ingest_file(dest, conn, category="paper")
    except IngestLockHeld:
        log.info("annotate-sync: ingest lock held; %s waits for the next tick", name)
        return None
    if result is None or result.status not in ("ingested", "skipped"):
        log.warning(
            "annotate-sync: ingest of %s did not land (%s)", name,
            getattr(result, "reason", "") or getattr(result, "status", "no result"),
        )
        return None
    row = conn.execute(
        "SELECT source_uri FROM documents WHERE id=?", (result.doc_id,)
    ).fetchone()
    return row["source_uri"] if row else None


def _queue_source(doc, where, pdf_bytes: bytes, *, incoming: Path) -> str:
    """Put a source PDF in the drop folder under its area for `locus-watch`. Returns a status.

    'queued' when written now, 'waiting' when an earlier copy has not been ingested yet, and
    'quarantined' when an earlier copy failed: re-queuing that one would quarantine it again
    every half hour, for ever.
    """
    name = doc.name.replace("/", "-")
    if not name.lower().endswith(".pdf"):
        name += ".pdf"
    area_dir = incoming / (where.area or "").lower()
    for set_aside in (".quarantine", ".unsupported"):
        held = incoming / set_aside
        if held.exists() and any(held.rglob(name)):
            return "quarantined"
    dest = area_dir / name
    if dest.exists():
        return "waiting"
    area_dir.mkdir(parents=True, exist_ok=True)
    tmp = dest.with_suffix(".part")
    tmp.write_bytes(pdf_bytes)
    tmp.replace(dest)
    return "queued"


def annotate_sync(
    conn: sqlite3.Connection,
    *,
    placed=None,
    manifest: dict | None = None,
    fetch_fn=None,
    read_fn=None,
    marks_fn=None,
    transcribe_fn=None,
    ingest_fn=None,
    promote_fn=None,
    queue_fn=None,
    transcribe_limit: int | None = None,
    max_fetch: int | None = None,
) -> AnnotateSyncResult:
    """Capture + transcribe the ink on every changed PDF in `placed` (see the module docstring).

    `placed` is `[(DeviceDoc, Placement)]` from capture/device.py (None reads the tablet).
    `manifest` maps `annotate:<uuid>` to the ink key last processed; None loads and persists
    Loop A's manifest file. All externals are injectable for model-free tests. Failure on one
    document never aborts the batch (ingest §7).
    """
    from locus.agent import journal
    from locus.capture.annotate import rekey_marks_by_uuid, store_marks
    from locus.capture.loop_a import (
        MANIFEST_NAME, _load_manifest, _save_manifest, default_fetch, placed_documents,
    )
    from locus.capture.mark_text import transcribe_marks
    from locus.capture.rmdoc import read_rmdoc
    from locus.config import load

    cfg = load()
    mpath = None
    if manifest is None:
        mpath = cfg.paths.raw_store / MANIFEST_NAME
        manifest = _load_manifest(mpath)
    limit = (
        transcribe_limit
        if transcribe_limit is not None
        else cfg.capture.annotate_max_transcribe
    )
    fetch_cap = cfg.capture.annotate_max_fetch if max_fetch is None else max_fetch
    fetch_fn = fetch_fn or default_fetch(cfg)
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
    ingest_fn = ingest_fn or _ingest_from_bundle
    if promote_fn is None:
        from locus.agent.promote import promote_reading_notes

        promote_fn = promote_reading_notes
    queue_fn = queue_fn or (
        lambda doc, where, pdf_bytes: _queue_source(
            doc, where, pdf_bytes, incoming=Path(cfg.paths.incoming)
        )
    )
    if placed is None:
        placed = placed_documents(conn, cfg)

    result = AnnotateSyncResult()
    budget = max(0, int(limit))
    fetched = 0
    with journal.run(conn, "annotate") as run:
        for doc, where in placed:
            if where.role not in ("reading", "sent", "source"):
                continue
            uuid, name, device_path = doc.uuid, doc.name, doc.path
            key = f"annotate:{uuid}"
            if manifest.get(key) == doc.ink_key:
                result.outcomes.append(AnnotateOutcome(uuid, name, "unchanged"))
                continue

            # A source's identity is its bytes, and the tree already has their hash: whether the
            # library holds it is known before anything is downloaded.
            library_uri = None
            if where.role == "source" and doc.pdf_hash:
                row = conn.execute(
                    "SELECT source_uri FROM documents WHERE content_hash=?", (doc.pdf_hash,)
                ).fetchone()
                library_uri = row["source_uri"] if row else None
            if where.role != "reading" and doc.ink_pages == 0 and (
                where.role == "sent" or library_uri is not None
            ):
                manifest[key] = doc.ink_key      # nothing written on it: nothing to read
                result.outcomes.append(AnnotateOutcome(uuid, name, "unchanged"))
                continue
            if fetched >= fetch_cap:
                result.outcomes.append(AnnotateOutcome(uuid, name, "deferred"))
                continue
            fetched += 1
            try:
                doc_read = read_fn(fetch_fn(doc))
                if where.role == "source" and library_uri is None:
                    # Not in the library yet: hand it to the drop folder, and come back to its
                    # ink once `locus-watch` has ingested it. The key is NOT recorded, so the
                    # next tick looks again.
                    status = queue_fn(doc, where, doc_read.pdf_bytes)
                    result.outcomes.append(AnnotateOutcome(uuid, name, status))
                    continue

                marks = marks_fn(doc_read)
                hash_mapped = ingested = False
                if where.role == "reading":
                    uri = _corpus_uri_for(conn, doc_read.pdf_bytes)
                    hash_mapped = uri is not None
                    if uri is None and _chosen(device_path) and doc_read.pdf_bytes:
                        # A document he CHOSE (moved to In-Progress/Finished) that no ingest
                        # path ever covered: a hand-added book. Never a native notebook: it has
                        # no PDF, and ingesting its empty bytes would file a zero-byte `.pdf`.
                        uri = ingest_fn(conn, doc_read, name)
                        ingested = uri is not None
                    uri = uri or device_path
                elif where.role == "source":
                    uri, hash_mapped = library_uri, True
                else:                                # sent: Claude's document, keyed on the device
                    uri = device_path

                moved = rekey_marks_by_uuid(conn, doc_uuid=doc_read.doc_uuid, new_uri=uri)
                if moved:
                    log.info("annotate-sync: re-keyed %d earlier mark(s) onto %s", moved, uri)
                stored = store_marks(
                    conn, marks, source_uri=uri, doc_uuid=doc_read.doc_uuid, source_run=run.id,
                )
                transcribed = 0
                if budget > 0 and cfg.capture.annotate_transcribe:
                    transcribed = transcribe_fn(conn, marks, source_uri=uri, limit=budget)
                    budget -= transcribed
                promoted = ""
                if uri != device_path or where.role == "sent":
                    # HIS margin notes become a notes document automatically, each with the
                    # passage it was written against: idempotent (stable path, content-compare),
                    # and `notes_sync` on the next capture tick ingests the file. For Claude's
                    # document the title is its device name, since the library has no row for it.
                    p = promote_fn(conn, uri, title=name) if where.role == "sent" else promote_fn(conn, uri)
                    promoted = p.status if p else ""
                chosen_but_missing = (
                    where.role == "reading" and uri == device_path and _chosen(device_path)
                    and bool(doc_read.pdf_bytes)
                )
                if not chosen_but_missing:
                    # A book he chose whose ingest did not land (the lock was held, or the ingest
                    # passes failed: the API credit ran out on 27 Oct) is NOT recorded as done,
                    # or it would never be tried again until he wrote on it.
                    manifest[key] = doc.ink_key
                result.outcomes.append(
                    AnnotateOutcome(
                        uuid, name, "annotated", source_uri=uri, hash_mapped=hash_mapped,
                        ingested=ingested, marks=stored, transcribed=transcribed,
                        promoted=promoted, role=where.role,
                    )
                )
                if chosen_but_missing:
                    log.warning(
                        "annotate-sync: %s is chosen but its ingest did not land: marks keyed by "
                        "device path %r, tried again next run", name, device_path,
                    )
                elif where.role == "reading" and uri == device_path:
                    # Loud, not fatal: a still-Proposed document he has marked up. Its marks
                    # join the corpus the moment he moves it out of Proposed (the ingest above
                    # + rekey), so nothing is lost; until then no downstream pass sees it.
                    log.warning(
                        "annotate-sync: %s not in the corpus (still in Proposed): "
                        "marks keyed by device path %r until he chooses it", name, device_path,
                    )
            except Exception as exc:  # one doc's failure never aborts the batch
                log.warning("annotate-sync: %s (%s) failed: %s", name, uuid[:8], exc)
                result.outcomes.append(AnnotateOutcome(uuid, name, "failed", error=str(exc)))
        run.stats = {
            "annotated": result.annotated,
            "unchanged": sum(o.status == "unchanged" for o in result.outcomes),
            "queued": sum(o.status in ("queued", "waiting") for o in result.outcomes),
            "quarantined": sum(o.status == "quarantined" for o in result.outcomes),
            "deferred": sum(o.status == "deferred" for o in result.outcomes),
            "failed": sum(o.status == "failed" for o in result.outcomes),
            "downloads": fetched,
            "marks": sum(o.marks for o in result.outcomes),
            "transcribed": sum(o.transcribed for o in result.outcomes),
            "ingested": sum(o.ingested for o in result.outcomes),
            "promoted": sum(bool(o.promoted and o.promoted != "unchanged")
                            for o in result.outcomes),
            # Reading documents only: a document Locus sent is keyed on the device by design.
            "unmapped_to_corpus": sum(
                1 for o in result.outcomes
                if o.status == "annotated" and o.role == "reading"
                and not o.hash_mapped and not o.ingested
            ),
        }
    if mpath is not None:
        _save_manifest(mpath, manifest)
    return result
