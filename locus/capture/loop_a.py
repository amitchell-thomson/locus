"""Loop A orchestrator — reMarkable handwriting → searchable note (agent-layer plan §8.1, §10).

Chains the tested pieces into one journaled, budget-metered, idempotent pass over the staging dir:

  identify  (capture/remarkable)  -> a named, categorised CaptureItem per <uuid>.pdf
    → transcribe (Sonnet VISION, metered SDK) -> Markdown + uncertainty flags; cost -> budget ledger
    → fill-in  (Haiku claude -p, text)        -> high-confidence gaps resolved, AI-marked ⟦…⟧
    → FILE     -> notes/<slug>-<uuid8>.md with frontmatter (title, category, maturity: rough,
                  remarkable_folder provenance, source raster) + the transcribed body
    → enrich   (in-process retrieval)         -> grounded `> [!ai] Related` owned block
    → INGEST   (notes_sync)                   -> down-weighted-rough, searchable corpus

Idempotent: a per-uuid manifest of the raster's content hash skips a doc whose PDF is unchanged, so
an expensive re-transcription only happens when the handwriting actually changed (the note then
replaces its prior version by path via notes_sync). The whole run is journaled (agent_runs) so a
crash is visible and wastes no spend beyond the doc in flight. Every model call is injectable, so
the orchestration is unit-testable without a device or the API.
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
    status: str  # 'captured' | 'unchanged' | 'failed'
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
    unmapped: list[str] = field(default_factory=list)
    ingest: NotesSyncResult | None = None
    cost_usd: float = 0.0

    @property
    def captured(self) -> int:
        return sum(o.status == "captured" for o in self.outcomes)


def _slug(name: str) -> str:
    s = re.sub(r"[^a-z0-9]+", "-", name.lower()).strip("-")
    return s or "note"


def _pdf_hash(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _frontmatter(item, *, now: str) -> str:
    # title/category/maturity are consumed by ingest (notes_sync reads category+maturity); the
    # remarkable_* fields are capture provenance (the folder is kept, not mirrored into category).
    fields = {
        "title": item.name,
        "category": item.category,
        "maturity": "rough",
        "remarkable_folder": item.folder,
        "remarkable_uuid": item.uuid,
        "source_pdf": str(item.pdf_path),
        "transcribed_by": "loop-a",
        "captured": now,
    }
    return "---\n" + "\n".join(f"{k}: {v}" for k, v in fields.items()) + "\n---\n"


def capture_sync(
    conn,
    *,
    staging_dir=None,
    notes_dir=None,
    manifest_path=None,
    ingest: bool = True,
    identify_fn=None,
    transcribe_fn=None,
    fillin_fn=None,
    enrich_fn=None,
    now: str | None = None,
) -> CaptureSyncResult:
    """Run Loop A over the staging dir. Model calls are injectable (defaults use the real
    transcribe/fill-in/enrich); `ingest=False` skips the final notes_sync (render-only)."""
    from locus.capture.remarkable import identify_staged
    from locus.capture.transcribe import transcribe_pdf
    from locus.capture.fillin import fill_gaps
    from locus.enrich.related import enrich_note

    cfg = load()
    staging = Path(staging_dir) if staging_dir is not None else cfg.capture.staging_dir
    ndir = Path(notes_dir) if notes_dir is not None else cfg.paths.notes
    identify_fn = identify_fn or (lambda s: identify_staged(s, rmapi_binary=cfg.capture.rmapi_binary))
    transcribe_fn = transcribe_fn or transcribe_pdf
    fillin_fn = fillin_fn or fill_gaps
    enrich_fn = enrich_fn or enrich_note
    now = now or datetime.now(timezone.utc).isoformat()

    mpath = Path(manifest_path) if manifest_path is not None else (cfg.paths.raw_store / MANIFEST_NAME)
    manifest = _load_manifest(mpath)
    ledger = BudgetLedger(cap_usd=cfg.agent.daily_cost_cap_usd)

    items, unmapped = identify_fn(staging)
    result = CaptureSyncResult(unmapped=list(unmapped))

    with journal.run(conn, "capture") as run:
        for item in items:
            try:
                h = _pdf_hash(item.pdf_path)
                if manifest.get(item.uuid) == h:
                    result.outcomes.append(CaptureOutcome(item.uuid, item.name, "unchanged"))
                    continue

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

                manifest[item.uuid] = h
                result.outcomes.append(CaptureOutcome(
                    item.uuid, item.name, "captured", note_path=str(note_path),
                    pages=len(transcript.pages), illegible=transcript.illegible_total,
                    uncertain=transcript.uncertain_total, filled=filled.filled,
                    related=enriched.related, cost_usd=transcript.cost_usd,
                ))
            except Exception as exc:  # one doc's failure never aborts the batch (§6)
                log.warning("capture: %s (%s) failed: %s", item.name, item.uuid[:8], exc)
                result.outcomes.append(CaptureOutcome(item.uuid, item.name, "failed", error=str(exc)))

        run.stats = {
            "captured": result.captured,
            "unchanged": sum(o.status == "unchanged" for o in result.outcomes),
            "failed": sum(o.status == "failed" for o in result.outcomes),
            "unmapped": len(unmapped),
            **ledger.stats(),
        }

    _save_manifest(mpath, manifest)
    result.cost_usd = ledger.spent_usd
    if ingest:
        result.ingest = sync_notes(conn, ndir)
    return result


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
