"""Loop A orchestrator — reMarkable handwriting → searchable note (agent-layer plan §8.1, §10).

Chains the tested pieces into one journaled, budget-metered, idempotent pass over his notebooks:

  find      (capture/device)    -> every notebook on the tablet, its area and its ink key, from
                                   the cloud tree in one call
    → render (capture/rmdoc)    -> the notebook's pages drawn from its cloud bundle, <uuid>.pdf
    → transcribe (Sonnet VISION, metered SDK) -> Markdown + uncertainty flags; cost -> budget ledger
    → fill-in  (Haiku claude -p, text)        -> high-confidence gaps resolved, AI-marked ⟦…⟧
    → FILE     -> notes/<slug>-<uuid8>.md with frontmatter (title, category from the area,
                  maturity: rough, device path, registry thing) + the transcribed body
    → enrich   (in-process retrieval)         -> grounded `> [!ai] Related` owned block
    → INGEST   (notes_sync)                   -> down-weighted-rough, searchable corpus

WHERE THE PAGES COME FROM (Slice 0.12). Until 1 Aug 2026 an agent on the tablet rendered each
changed notebook and pushed it here; it stopped, and for twelve weeks every run reported
"0 captured" and exited 0. The render is now made on the server from the cloud bundle
(`rmdoc.composite_pdf`, blank canvas for a notebook), so nothing on the device has to work.

Idempotent on the INK, not on the render: `cloud:<uuid>` in the manifest holds the notebook's ink
key from the cloud tree, which changes when a stroke layer does and not when he moves or opens
the notebook. A render is not byte-reproducible (pymupdf stamps each save), so keying on it would
re-pay a billed transcription for nothing. The per-uuid render hash is still written, and a
notebook the old tablet agent rendered and Loop A already transcribed is SEEDED as current on
first sight rather than re-transcribed (`recapture=True` overrides); a notebook with no ink is
recorded and skipped. At most `max_notebooks_per_run` are rendered per run, so a backlog spreads
over ticks. The whole run is journaled (agent_runs) so a crash is visible and wastes no spend
beyond the doc in flight. Every model call is injectable, so the orchestration is unit-testable
without a device or the API.
"""

from __future__ import annotations

import hashlib
import logging
import re
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path

from locus.agent import journal
from locus.agent.budget import BudgetLedger
from locus.agent.claude import ClaudeResult
from locus.config import load
from locus.notes_sync import NotesSyncResult, sync_notes
from locus.vault.writer import _atomic_write

log = logging.getLogger(__name__)

MANIFEST_NAME = "capture.manifest.json"


@dataclass
class CaptureOutcome:
    uuid: str
    name: str
    # 'captured' | 'unchanged' | 'seeded' (current since the tablet agent) | 'deferred' (over the
    # per-run cap) | 'failed'
    status: str
    note_path: str | None = None
    pages: int = 0
    illegible: int = 0
    uncertain: int = 0
    filled: int = 0
    related: int = 0
    cost_usd: float = 0.0
    error: str | None = None


@dataclass
class CaptureSyncResult:
    outcomes: list[CaptureOutcome] = field(default_factory=list)
    unmatched_folders: list[str] = field(default_factory=list)
    ingest: NotesSyncResult | None = None
    cost_usd: float = 0.0

    @property
    def captured(self) -> int:
        return sum(o.status == "captured" for o in self.outcomes)

    def count(self, status: str) -> int:
        return sum(o.status == status for o in self.outcomes)


def _slug(name: str) -> str:
    s = re.sub(r"[^a-z0-9]+", "-", name.lower()).strip("-")
    return s or "note"


def _pdf_hash(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _authored_date(item, *, fallback: str) -> str:
    """The note's `date:` — the device's ModifiedClient (when the owner last WROTE in the
    notebook), falling back to the capture time only when the device reported none.

    This is what `extract/textdoc` turns into documents.source_date and therefore what
    `belief_positions.dated_at` records. Dating handwriting by capture time makes every note
    captured in one batch share a date, which flattens the understanding-evolution trajectory
    (§6.3) into a single point — the failure observed on the first 12-document capture."""
    stamp = (getattr(item, "modified", None) or "").strip()
    return (stamp or fallback)[:10]


def _frontmatter(item, *, now: str) -> str:
    # title/category/maturity are consumed by ingest (notes_sync reads category+maturity); the
    # remarkable_* fields are capture provenance (the folder is kept, not mirrored into category).
    fields = {
        "title": item.name,
        # `date` is read by extract/textdoc into source_date -> belief_positions.dated_at.
        "date": _authored_date(item, fallback=now),
        "category": item.category,
        "maturity": "rough",
        "remarkable_folder": item.folder,
        "remarkable_uuid": item.uuid,
        "area": item.area or "",
        "thing": item.thing or "",
        "source_pdf": str(item.pdf_path),
        "transcribed_by": "loop-a",
        "captured": now,
    }
    return "---\n" + "\n".join(f"{k}: {v}" for k, v in fields.items() if v != "") + "\n---\n"


def capture_sync(
    conn,
    *,
    placed=None,
    staging_dir=None,
    notes_dir=None,
    manifest_path=None,
    ingest: bool = True,
    fetch_fn=None,
    render_fn=None,
    transcribe_fn=None,
    fillin_fn=None,
    enrich_fn=None,
    max_notebooks: int | None = None,
    recapture: bool = False,
    now: str | None = None,
) -> CaptureSyncResult:
    """Run Loop A over the notebooks in `placed` (`[(DeviceDoc, Placement)]`, see
    capture/device.py; None reads the tablet). Model calls and the cloud fetch are injectable;
    `ingest=False` skips the final notes_sync (render-only)."""
    from locus.capture.device import unmatched_folders
    from locus.capture.fillin import fill_gaps
    from locus.capture.remarkable import CaptureItem
    from locus.capture.rmdoc import composite_pdf, read_rmdoc
    from locus.capture.transcribe import transcribe_pdf
    from locus.enrich.related import enrich_note

    cfg = load()
    staging = Path(staging_dir) if staging_dir is not None else cfg.capture.staging_dir
    ndir = Path(notes_dir) if notes_dir is not None else cfg.paths.notes
    if placed is None:
        placed = placed_documents(conn, cfg)
    fetch_fn = fetch_fn or default_fetch(cfg)
    render_fn = render_fn or (lambda bundle, out: composite_pdf(read_rmdoc(bundle), out))
    transcribe_fn = transcribe_fn or transcribe_pdf
    fillin_fn = fillin_fn or fill_gaps
    enrich_fn = enrich_fn or enrich_note
    cap = cfg.capture.max_notebooks_per_run if max_notebooks is None else max_notebooks
    now = now or datetime.now(timezone.utc).isoformat()

    mpath = Path(manifest_path) if manifest_path is not None else (cfg.paths.raw_store / MANIFEST_NAME)
    manifest = _load_manifest(mpath)
    ledger = BudgetLedger(cap_usd=cfg.agent.daily_cost_cap_usd)
    result = CaptureSyncResult(unmatched_folders=unmatched_folders(placed))
    rendered = 0

    with journal.run(conn, "capture") as run:
        for doc, where in placed:
            if where.role != "notebook":
                continue
            key = f"cloud:{doc.uuid}"
            if manifest.get(key) == doc.ink_key:
                result.outcomes.append(CaptureOutcome(doc.uuid, doc.name, "unchanged"))
                continue
            if doc.ink_pages == 0:
                manifest[key] = doc.ink_key          # an empty notebook: nothing to read yet
                result.outcomes.append(CaptureOutcome(doc.uuid, doc.name, "unchanged"))
                continue
            if (
                not recapture and key not in manifest and doc.uuid in manifest
                and (staging / f"{doc.uuid}.pdf").exists()
            ):
                # Transcribed from the tablet agent's render before it stopped. Re-transcribing
                # it to learn whether the ink moved since would bill every old notebook once.
                manifest[key] = doc.ink_key
                result.outcomes.append(CaptureOutcome(doc.uuid, doc.name, "seeded"))
                continue
            if rendered >= cap:
                result.outcomes.append(CaptureOutcome(doc.uuid, doc.name, "deferred"))
                continue
            rendered += 1
            try:
                bundle = fetch_fn(doc)
                pdf_path = render_fn(bundle, staging / f"{doc.uuid}.pdf")
                item = CaptureItem(
                    uuid=doc.uuid, pdf_path=Path(pdf_path), name=doc.name,
                    folder="/" + "/".join(doc.folders), category=where.category,
                    modified=doc.modified, area=where.area, thing=where.thing,
                )
                transcript = transcribe_fn(item.pdf_path)
                ledger.record(ClaudeResult(
                    text="", cost_usd=transcript.cost_usd,
                    usage={"input_tokens": transcript.input_tokens,
                           "output_tokens": transcript.output_tokens},
                ))
                filled = fillin_fn(transcript.markdown)

                note_path = ndir / f"{_slug(item.name)}-{item.uuid[:8]}.md"
                _atomic_write(note_path, _frontmatter(item, now=now) + "\n" + filled.markdown.strip() + "\n")
                enriched = enrich_fn(note_path, filled.markdown, conn=conn, run_id=str(run.id))

                manifest[item.uuid] = _pdf_hash(item.pdf_path)
                manifest[key] = doc.ink_key
                result.outcomes.append(CaptureOutcome(
                    item.uuid, item.name, "captured", note_path=str(note_path),
                    pages=len(transcript.pages), illegible=transcript.illegible_total,
                    uncertain=transcript.uncertain_total, filled=filled.filled,
                    related=enriched.related, cost_usd=transcript.cost_usd,
                ))
            except Exception as exc:  # one doc's failure never aborts the batch (§6)
                log.warning("capture: %s (%s) failed: %s", doc.name, doc.uuid[:8], exc)
                result.outcomes.append(CaptureOutcome(doc.uuid, doc.name, "failed", error=str(exc)))

        run.stats = {
            "captured": result.captured,
            "unchanged": result.count("unchanged"),
            "seeded": result.count("seeded"),
            "deferred": result.count("deferred"),
            "failed": result.count("failed"),
            # The tablet listing is one call per tick (placed_documents); each render is one
            # download. Recorded so the cloud cost of a run is a number, not a guess (§14, 429s).
            "downloads": rendered,
            "unmatched_folders": result.unmatched_folders,
            **ledger.stats(),
        }

    _save_manifest(mpath, manifest)
    result.cost_usd = ledger.spent_usd
    if ingest:
        result.ingest = sync_notes(conn, ndir)
    return result


def placed_documents(conn, cfg=None):
    """Read the tablet once and place every document: `[(DeviceDoc, Placement)]`.

    One cloud call (capture/device.read_device). Shared by Loop A and Loop B in a capture run so
    the tablet is listed once per tick, not once per loop.
    """
    from locus.capture.device import load_registry, place, read_device
    from locus.capture.remarkable import _subprocess_runner
    from locus.capture.sent import sent_hashes

    cfg = cfg or load()
    docs = read_device(
        _subprocess_runner(cfg.capture.rmapi_binary), tree_cache=cfg.capture.rmapi_tree_cache
    )
    sent = sent_hashes(conn)
    registry = load_registry()
    out = []
    for doc in docs:
        where = place(doc, sent_hashes=sent, registry=registry)
        if where is not None:
            out.append((doc, where))
    return out


def default_fetch(cfg):
    """Fetch a document's bundle from the cloud into the rmdoc cache; returns the cached path.

    The cache (`data/cache/rmdoc/<uuid>.rmdoc`) is the one `locus marks` reads, so a capture
    leaves the reader current as a side effect. Fetched by device path, which the tree gave us
    seconds ago.
    """
    import shutil
    import tempfile

    from locus.capture.rmdoc import fetch_rmdoc

    cache = Path(cfg.paths.db).parent / "cache" / "rmdoc"

    def fetch(doc) -> Path:
        with tempfile.TemporaryDirectory(prefix="locus-capture-") as tmp:
            got = fetch_rmdoc(doc.path, tmp, rmapi_binary=cfg.capture.rmapi_binary, timeout=600)
            cache.mkdir(parents=True, exist_ok=True)
            dest = cache / f"{doc.uuid}.rmdoc"
            shutil.copyfile(got, dest)
        return dest

    return fetch


def _load_manifest(path: Path) -> dict[str, str]:
    if not path.exists():
        return {}
    try:
        import json

        return dict(json.loads(path.read_text()).get("captures", {}))
    except (OSError, ValueError):
        return {}


def _save_manifest(path: Path, captures: dict[str, str]) -> None:
    import json

    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({"captures": captures}, indent=1, sort_keys=True))
