"""Incremental ingest of the authoring notes directory (agent-layer plan §6.7).

The capture inbox (`vault/notes/` — transcribed handwriting, captured conversations, typed notes)
churns: a note is re-saved constantly, usually with trivial whitespace changes. Re-ingesting the
whole directory on every save (re-extract + re-embed everything) would be unusable, and raw
content-hash idempotency alone can't help — a trailing-newline change moves the byte hash and
looks like a real edit (the CLAUDE.md §11 whitespace-sensitivity limit).

So, on the `repo_sync.py` manifest-diff template, this keeps a manifest of each note's
WHITESPACE-NORMALISED hash. A sync ingests only notes whose normalised hash changed (a real edit),
skips whitespace-only re-saves for free, and deletes documents whose note file disappeared —
the two things per-file content-hash idempotency cannot do.

Each note is its own document (`category='note'`), so the actual ingest reuses `ingest_file`;
this module only decides WHAT to (re)ingest or delete. Maturity comes from the note's frontmatter
(`maturity: rough|tidy`), so promotion is just editing the frontmatter and re-syncing (§6.1).

Excluded from ingest (invariant 5 — no feedback-loop contamination): `_generated/` (agent output),
`_home.md` (the daily page), `*.locus.md` sidecars, and Syncthing conflict copies.
"""

from __future__ import annotations

import hashlib
import json
import logging
from dataclasses import dataclass
from pathlib import Path

from locus.config import load
from locus.extract.textdoc import _parse_frontmatter
from locus.ingest_pipeline import delete_document, ingest_file

log = logging.getLogger(__name__)

MANIFEST_NAME = "notes_sync.manifest.json"


@dataclass
class NotesSyncResult:
    ingested: int = 0
    skipped: int = 0   # whitespace-only re-save or unchanged — no work done
    deleted: int = 0   # note file removed -> document deleted
    failed: int = 0     # ingest quarantined/unsupported — left out of the manifest to retry


def normalized_hash(text: str) -> str:
    """SHA-256 of `text` after whitespace normalisation, so trivial re-saves hash identically:
    line endings unified, trailing per-line whitespace dropped, runs of blank lines collapsed to
    one, and leading/trailing blank lines trimmed. A real content edit still changes the hash."""
    unified = text.replace("\r\n", "\n").replace("\r", "\n")
    collapsed: list[str] = []
    for line in unified.split("\n"):
        stripped = line.rstrip()
        if stripped == "" and collapsed and collapsed[-1] == "":
            continue  # collapse consecutive blank lines
        collapsed.append(stripped)
    return hashlib.sha256(("\n".join(collapsed).strip() + "\n").encode("utf-8")).hexdigest()


def _is_ingestable_note(path: Path, notes_dir: Path) -> bool:
    if path.suffix != ".md" or not path.is_file():
        return False
    name = path.name
    if name.startswith(".") or name == "_home.md" or name.endswith(".locus.md"):
        return False
    if ".sync-conflict-" in name:
        return False
    return "_generated" not in path.relative_to(notes_dir).parts


def _iter_notes(notes_dir: Path) -> list[Path]:
    if not notes_dir.exists():
        return []
    return sorted(p for p in notes_dir.rglob("*.md") if _is_ingestable_note(p, notes_dir))


_VALID_CATEGORIES = ("paper", "coursework", "project", "career", "note")


def _read_frontmatter_field(path: Path, key: str) -> str | None:
    try:
        meta, _ = _parse_frontmatter(path.read_text(encoding="utf-8"))
    except OSError:
        return None
    return (meta.get(key) or "").strip().lower() or None


def _read_maturity(path: Path, default: str) -> str:
    """The note's `maturity` frontmatter value (rough|tidy), else `default`."""
    value = _read_frontmatter_field(path, "maturity")
    return value if value in ("rough", "tidy") else default


def _read_category(path: Path, default: str) -> str:
    """The note's `category` frontmatter value, else `default`. Lets Loop A file a note under its
    reMarkable-folder category (mostly `note`, some coursework/project); an authored note without
    the field stays `note`."""
    value = _read_frontmatter_field(path, "category")
    return value if value in _VALID_CATEGORIES else default


def _manifest_path(explicit: Path | None) -> Path:
    return explicit or (load().paths.raw_store / MANIFEST_NAME)


def _load_manifest(path: Path) -> dict[str, str]:
    if not path.exists():
        return {}
    try:
        data = json.loads(path.read_text())
        return dict(data.get("notes", {}))
    except (json.JSONDecodeError, TypeError):
        return {}


def _save_manifest(path: Path, notes: dict[str, str]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({"notes": notes}, indent=1, sort_keys=True))


def sync_notes(
    conn,
    notes_dir: str | Path | None = None,
    *,
    default_maturity: str = "rough",
    manifest_path: str | Path | None = None,
) -> NotesSyncResult:
    """Incrementally ingest `notes_dir` (default `[paths].notes`): (re)ingest changed notes, skip
    whitespace-only re-saves, delete documents for removed notes. Never raises per note — an
    ingest failure is counted and the note is left out of the manifest so the next sync retries.

    `default_maturity` tags notes whose frontmatter omits `maturity`; the capture inbox is
    provisional, so it defaults to `rough` (an authored note promotes by setting `maturity: tidy`)."""
    notes_dir = Path(notes_dir) if notes_dir is not None else load().paths.notes
    mpath = _manifest_path(Path(manifest_path) if manifest_path is not None else None)

    prior = _load_manifest(mpath)
    result = NotesSyncResult()
    new_manifest: dict[str, str] = {}

    for path in _iter_notes(notes_dir):
        key = str(path)
        h = normalized_hash(path.read_text(encoding="utf-8"))
        if prior.get(key) == h:  # unchanged (incl. whitespace-only re-save) — no work
            new_manifest[key] = h
            result.skipped += 1
            continue
        # replace_uri=key: a note's identity is its PATH — an edit REPLACES the prior document at
        # this path (not a new content-hash doc alongside it). category='note': notes live under
        # vault/notes/, outside the incoming/<cat>/ folder convention (§6.7).
        r = ingest_file(
            path, conn, reingest=True, maturity=_read_maturity(path, default_maturity),
            category=_read_category(path, "note"), replace_uri=key,
        )
        if r.status in ("ingested", "skipped"):
            new_manifest[key] = h
            result.ingested += 1 if r.status == "ingested" else 0
            result.skipped += 1 if r.status == "skipped" else 0
        else:  # quarantined/unsupported — leave out of the manifest so the next sync retries
            result.failed += 1
            log.warning("notes-sync: %s -> %s (%s)", path.name, r.status, r.error)

    # Deletions: a note in the prior manifest that is gone from disk -> delete its document.
    for key in prior:
        if key not in new_manifest and not Path(key).exists():
            row = conn.execute("SELECT id FROM documents WHERE source_uri=?", (key,)).fetchone()
            if row:
                delete_document(conn, row["id"])
                result.deleted += 1

    _save_manifest(mpath, new_manifest)
    return result
